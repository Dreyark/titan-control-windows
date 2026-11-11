//! ddc_probe: READ-ONLY DDC/CI probe of Titan Army P275MV PLUS monitors through the Windows Monitor
//! Configuration API (Dxva2). See docs/windows-port.md.
//!
//! It sends only VCP GET requests (`GetVCPFeatureAndVCPFeatureReply`). There is no write path in this crate.

mod ddc;
mod edid;
mod log;
mod transport;
#[cfg(windows)]
mod windows_ddc;

#[cfg(windows)]
pub use windows_ddc::win_error_name;

#[cfg(not(windows))]
pub fn win_error_name(raw: u32) -> String {
    format!("error 0x{raw:08X}")
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ddc_probe works on Windows only.");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    std::process::exit(app::run());
}

#[cfg(windows)]
mod app {
    use crate::ddc::{Access, Ddc, PROFILE_TABLE_LEN, READ_TRIES, TableRead, VCP_FIRMWARE, VCP_VERSION};
    use crate::edid::{self, EdidIdentity};
    use crate::windows_ddc::{DisplayInfo, PhysicalMonitors, enumerate_displays};
    use crate::{log, transport::FrameError};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    use windows::Win32::System::SystemInformation::GetLocalTime;

    const DEFAULT_REPS: usize = 10;
    /// Pause between table read attempts (as the app's confirmation loop).
    const TABLE_PAUSE: Duration = Duration::from_millis(500);
    /// Stop repeating after this many consecutive unrecoverable failures (the monitor is not answering).
    const MAX_CONSECUTIVE_WEDGED: usize = 3;

    struct Args {
        list_only: bool,
        monitor: Option<String>,
        out_dir: PathBuf,
        reps: usize,
        yes: bool,
        /// `--scan FILE`: read every register once (read-only) and save the values.
        scan: Option<PathBuf>,
        /// `--diff A B`: compare two scans (no monitor traffic).
        diff: Option<(PathBuf, PathBuf)>,
    }

    fn parse_args() -> Result<Args, String> {
        let mut args = Args { list_only: false, monitor: None, out_dir: PathBuf::from("."), reps: DEFAULT_REPS, yes: false, scan: None, diff: None };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--list" => args.list_only = true,
                "--yes" => args.yes = true,
                "--scan" => args.scan = Some(PathBuf::from(it.next().ok_or("--scan needs a file name")?)),
                "--diff" => {
                    let a = it.next().ok_or("--diff needs two files")?;
                    let b = it.next().ok_or("--diff needs two files")?;
                    args.diff = Some((PathBuf::from(a), PathBuf::from(b)));
                }
                "--monitor" => args.monitor = Some(it.next().ok_or("--monitor needs a value, e.g. DISPLAY2")?),
                "--out" => args.out_dir = PathBuf::from(it.next().ok_or("--out needs a directory")?),
                "--reps" => {
                    args.reps = it
                        .next()
                        .and_then(|v| v.parse().ok())
                        .filter(|n| (1..=50).contains(n))
                        .ok_or("--reps needs a number 1..50")?
                }
                "-h" | "--help" => return Err(HELP.into()),
                other => return Err(format!("unknown argument '{other}'\n{HELP}")),
            }
        }
        Ok(args)
    }

    const HELP: &str = "ddc_probe - READ-ONLY DDC/CI probe (Dxva2) for P275MV PLUS monitors\n\
        \n  --list            only list displays and EDID identity (no DDC traffic at all)\
        \n  --monitor DISPLAYn  probe only this output (default: every P275MV)\
        \n  --reps N          profile table reads per monitor (default 10, max 50)\
        \n  --out DIR         directory for ddc_probe_result.json and ddc_probe.log (default: current)\
        \n  --scan FILE       read EVERY register once (GET only) and save them; needs --monitor\
        \n  --diff A B        compare two scans (no monitor traffic): the registers that changed\
        \n  --yes             do not wait for Enter before talking to the monitors";

    /// Keeps the system timer at 1 ms while alive (REPLY_DELAY/REQUEST_GAP accuracy).
    struct TimerResolution;

    impl TimerResolution {
        fn new() -> Self {
            // SAFETY: plain call; released in Drop.
            unsafe { timeBeginPeriod(1) };
            TimerResolution
        }
    }

    impl Drop for TimerResolution {
        fn drop(&mut self) {
            // SAFETY: balances timeBeginPeriod(1).
            unsafe { timeEndPeriod(1) };
        }
    }

    fn local_time() -> String {
        // SAFETY: no arguments, returns a plain struct.
        let t = unsafe { GetLocalTime() };
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }

    fn is_target(identity: &EdidIdentity) -> bool {
        identity.name.to_uppercase().contains("P275MV")
    }

    struct Entry {
        info: DisplayInfo,
        identity: Option<EdidIdentity>,
        /// `edid_id:serial:instance` - see docs/windows-port.md.
        monitor_id: Option<String>,
    }

    fn describe_displays(displays: Vec<DisplayInfo>) -> Vec<Entry> {
        displays
            .into_iter()
            .map(|info| {
                let identity = info.edid.as_deref().and_then(edid::identity);
                let monitor_id = identity.as_ref().map(|id| {
                    let instance = info.registry_key.clone().unwrap_or_default();
                    format!("{}:{}:{}", id.edid_id, if id.serial.is_empty() { "-" } else { &id.serial }, instance)
                });
                Entry { info, identity, monitor_id }
            })
            .collect()
    }

    fn entry_json(entry: &Entry) -> Value {
        let info = &entry.info;
        json!({
            "device": info.device,
            "primary": info.primary,
            "rect": [info.rect.0, info.rect.1, info.rect.2, info.rect.3],
            "interface_path": info.interface_path,
            "registry_key": info.registry_key,
            "edid_error": info.edid_error,
            "edid_len": info.edid.as_ref().map(|e| e.len()),
            "name": entry.identity.as_ref().map(|i| i.name.clone()),
            "edid_id": entry.identity.as_ref().map(|i| i.edid_id.clone()),
            "serial": entry.identity.as_ref().map(|i| i.serial.clone()),
            "manufactured": entry.identity.as_ref().map(|i| format!("week {} of {}", i.week, i.year)),
            "monitor_id": entry.monitor_id,
            "is_target_p275mv": entry.identity.as_ref().map(is_target).unwrap_or(false),
        })
    }

    fn hex(value: u16) -> String {
        format!("0x{value:04X}")
    }

    fn read_json<T: crate::transport::VcpTransport>(ddc: &mut Ddc<T>, label: &str, code: u8, access: Access) -> Value {
        let frames_before = ddc.frames;
        let began = Instant::now();
        let result = ddc.read_register(code, access, READ_TRIES);
        let ms = began.elapsed().as_millis();
        let frames = ddc.frames - frames_before;
        match result {
            Ok(reply) => {
                log::say(&format!("  GET {label}: max={} cur={} ({frames} frames, {ms} ms)", hex(reply.maximum), hex(reply.current)));
                json!({"label": label, "code": format!("0x{code:02X}"), "access": format!("{access:?}"), "ok": true,
                       "maximum": hex(reply.maximum), "current": hex(reply.current), "frames": frames, "ms": ms})
            }
            Err(error) => {
                log::say(&format!("  GET {label}: FAILED {} ({frames} frames, {ms} ms)", error.describe()));
                json!({"label": label, "code": format!("0x{code:02X}"), "access": format!("{access:?}"), "ok": false,
                       "error": error.describe(), "error_kind": error.kind(), "frames": frames, "ms": ms})
            }
        }
    }

    fn probe_monitor(entry: &Entry, reps: usize) -> Value {
        let device = &entry.info.device;
        let monitor_id = entry.monitor_id.clone().unwrap_or_default();
        log::say(&format!("\n=== {device}  {monitor_id} ==="));

        let physical = match PhysicalMonitors::open(entry.info.hmonitor) {
            Ok(p) => p,
            Err(error) => {
                log::say(&format!("  cannot open physical monitor: {error}"));
                return json!({"device": device, "monitor_id": monitor_id, "skipped": error});
            }
        };
        if physical.len() != 1 {
            let message = format!("{} physical monitors on one output (clone mode?): ambiguous, skipped", physical.len());
            log::say(&format!("  {message}"));
            return json!({"device": device, "monitor_id": monitor_id, "skipped": message});
        }

        let mut ddc = Ddc::new(physical.transport(0));
        log::say(&format!("  transport: {}", ddc.describe()));
        let started = Instant::now();

        let settled = ddc.settle();
        log::say(&format!("  initial settle (GET DF x2): {}", if settled { "ok" } else { "FAILED" }));

        // 1. Plain registers.
        log::say("  -- plain registers --");
        let plain: Vec<Value> = [
            ("DF vcp version", 0xDFu8),
            ("FE firmware", 0xFE),
            ("F3 usage time", 0xF3),
            ("AE refresh", 0xAE),
            ("C8 controller", 0xC8),
        ]
        .into_iter()
        .map(|(label, code)| read_json(&mut ddc, label, code, Access::Plain))
        .collect();
        let fe = plain.iter().find(|v| v["code"] == "0xFE");
        let fe_ok = fe.map(|v| v["ok"] == true && v["maximum"] == "0x0000" && v["current"] == "0x5101").unwrap_or(false);

        // 2. Manufacturer registers: GET 99 -> GET code.
        log::say("  -- vendor registers (GET 99 -> GET code) --");
        let vendor: Vec<Value> = [
            ("F6 volume", 0xF6u8, Access::Vendor),
            ("E1 DCR", 0xE1, Access::Vendor),
            ("E2 adaptive-sync (plain, as app)", 0xE2, Access::Plain),
            ("E2 adaptive-sync (via 99)", 0xE2, Access::Vendor),
            ("47 local dimming", 0x47, Access::Vendor),
        ]
        .into_iter()
        .map(|(label, code, access)| read_json(&mut ddc, label, code, access))
        .collect();

        // 3. Profile table, repeated.
        log::say(&format!("  -- profile table x{reps} --"));
        let mut attempts: Vec<Value> = Vec::new();
        let mut table_ok = 0usize;
        let mut default_ok = 0usize;
        let mut failed = 0usize;
        let mut wedged = 0usize;
        let mut consecutive_wedged = 0usize;
        let mut settle_recovered = 0usize;
        let mut error_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut tables: Vec<(u8, [u8; 40])> = Vec::new();
        let mut kinds: Vec<String> = Vec::new();

        for index in 1..=reps {
            let frames_before = ddc.frames;
            let resends_before = ddc.resends;
            let began = Instant::now();
            let outcome = ddc.read_profile_table_once();
            let ms = began.elapsed().as_millis();
            let frames = ddc.frames - frames_before;
            let resends = ddc.resends - resends_before;
            let mut record = json!({"index": index, "ms": ms, "frames": frames, "resends": resends});
            match outcome {
                Ok(TableRead::Table(scene, data)) => {
                    table_ok += 1;
                    consecutive_wedged = 0;
                    tables.push((scene, data));
                    kinds.push("custom table".into());
                    let hex_bytes: String = data.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
                    log::say(&format!("  #{index}: table of profile {scene} ok ({frames} frames, {resends} resends, {ms} ms)"));
                    record["outcome"] = json!("table");
                    record["scene_id"] = json!(scene);
                    record["bytes_hex"] = json!(hex_bytes);
                }
                Ok(TableRead::Default(scene)) => {
                    default_ok += 1;
                    consecutive_wedged = 0;
                    kinds.push("default profile".into());
                    log::say(&format!("  #{index}: profile {scene} is Default (odd): sequence ok, no table to read ({ms} ms)"));
                    record["outcome"] = json!("default");
                    record["scene_id"] = json!(scene);
                }
                Err(error) => {
                    failed += 1;
                    *error_counts.entry(error.clone()).or_insert(0) += 1;
                    log::say(&format!("  #{index}: FAILED: {error} ({frames} frames, {resends} resends, {ms} ms)"));
                    record["outcome"] = json!("error");
                    record["error"] = json!(error);
                    // Does the plain-GET settle bring the monitor back from a failed stream?
                    let settle_frames = ddc.frames;
                    let recovered = ddc.settle();
                    record["settle_recovered"] = json!(recovered);
                    record["settle_frames"] = json!(ddc.frames - settle_frames);
                    if recovered {
                        settle_recovered += 1;
                        consecutive_wedged = 0;
                    } else {
                        wedged += 1;
                        consecutive_wedged += 1;
                    }
                    log::say(&format!("      settle after error: {}", if recovered { "recovered" } else { "NOT recovered" }));
                }
            }
            attempts.push(record);
            if consecutive_wedged >= MAX_CONSECUTIVE_WEDGED {
                log::say(&format!("  {MAX_CONSECUTIVE_WEDGED} unrecoverable failures in a row: stopping repetitions"));
                break;
            }
            thread::sleep(TABLE_PAUSE);
        }

        let distinct_tables = {
            let mut unique: Vec<&(u8, [u8; 40])> = Vec::new();
            for table in &tables {
                if !unique.contains(&table) {
                    unique.push(table);
                }
            }
            unique.len()
        };
        // Hints for comparison with the OSD (layout of ProfileTable in monitor.rs: byte 2 brightness, 3 contrast).
        let decoded = tables.first().map(|(scene, data)| {
            let mut bytes = [0u8; PROFILE_TABLE_LEN];
            for (slot, value) in bytes.iter_mut().skip(2).zip(data.iter()) {
                *slot = *value;
            }
            json!({"scene_id": scene, "brightness": bytes[2], "contrast": bytes[3], "sharpness": bytes[4]})
        });

        let done = attempts.len();
        let sequence_ok = table_ok + default_ok;
        // Leave the monitor in a clean state.
        let final_settle = ddc.settle();
        log::say(&format!("  final settle: {}", if final_settle { "ok" } else { "FAILED" }));

        let table_threshold = (reps * 9).div_ceil(10);
        let all_custom = default_ok == 0 && done > 0;
        let gate = json!({
            "fe_is_0000_5101": fe_ok,
            "table_reads_attempted": done,
            "table_reads_ok": table_ok,
            "default_profile_reads": default_ok,
            "required_ok": table_threshold,
            "table_gate_passed": all_custom && table_ok >= table_threshold && wedged == 0,
            "table_not_exercised": default_ok > 0,
            "note": "Automatic hint only. A human must still compare brightness/profile with the OSD (test A).",
        });
        log::say(&format!(
            "  SUMMARY {device}: FE {}; table ok {table_ok}/{done}, default-profile sequences {default_ok}, failed {failed}, wedged {wedged}, settle recovered {settle_recovered}, distinct tables {distinct_tables}",
            if fe_ok { "= 5.1.1 (0000/5101)" } else { "NOT 0000/5101" },
        ));
        if default_ok > 0 {
            log::say("  NOTE: the active profile is a Default (odd) one, so no table was read. Select a Custom profile in the OSD and run again.");
        }

        let trace: Vec<Value> = ddc
            .trace
            .iter()
            .map(|t| json!({"t_ms": t.t_ms, "code": format!("0x{:02X}", t.code), "result": t.result, "ms": t.dur_ms}))
            .collect();

        json!({
            "device": device,
            "monitor_id": monitor_id,
            "transport": ddc.describe(),
            "elapsed_ms": started.elapsed().as_millis(),
            "initial_settle_ok": settled,
            "plain_reads": plain,
            "vendor_reads": vendor,
            "table": {
                "attempts": attempts,
                "ok": table_ok,
                "default_profile": default_ok,
                "failed": failed,
                "sequence_ok": sequence_ok,
                "wedged_after_error": wedged,
                "settle_recovered": settle_recovered,
                "resends_total": ddc.resends,
                "error_counts": error_counts,
                "kinds": kinds,
                "distinct_tables": distinct_tables,
                "decoded_hint": decoded,
            },
            "frame_errors": ddc.errors,
            "frames_total": ddc.frames,
            "gate_auto": gate,
            "final_settle_ok": final_settle,
            "trace": trace,
        })
    }


    /// Codes that must not be read after `GET 99`: the handshake itself, the profile register (it starts a table
    /// stream) and the stream codes.
    const NO_VENDOR_READ: [u8; 4] = [0x22, 0x99, 0xFE, 0xFF];
    /// Attempts per register: a reply lost to the monitor's outages must not look like "no such register".
    const SCAN_TRIES: usize = 3;

    /// Reads every register once, plain and (from 0x1F up, where no table stream exists) after `GET 99`.
    /// GET only. Saves `code -> "MAX/CUR"` or the error, for `--diff`.
    fn scan_monitor(entry: &Entry, path: &std::path::Path) -> Result<(), String> {
        let physical = PhysicalMonitors::open(entry.info.hmonitor)?;
        if physical.len() != 1 {
            return Err("more than one physical monitor on this output".into());
        }
        let mut ddc = Ddc::new(physical.transport(0));
        ddc.settle();
        let mut plain = serde_json::Map::new();
        let mut vendor = serde_json::Map::new();
        let started = Instant::now();
        for code in 0..=255u8 {
            let text = match ddc.read_register(code, Access::Plain, SCAN_TRIES) {
                Ok(reply) => format!("{:04X}/{:04X}", reply.maximum, reply.current),
                Err(error) => format!("ERR {}", error.kind()),
            };
            plain.insert(format!("0x{code:02X}"), json!(text));
            if code >= 0x1F && !NO_VENDOR_READ.contains(&code) {
                let text = match ddc.read_register(code, Access::Vendor, SCAN_TRIES) {
                    Ok(reply) => format!("{:04X}/{:04X}", reply.maximum, reply.current),
                    Err(error) => format!("ERR {}", error.kind()),
                };
                vendor.insert(format!("0x{code:02X}"), json!(text));
            }
            if code % 32 == 31 {
                log::say(&format!("  scanned up to 0x{code:02X} ({} s)", started.elapsed().as_secs()));
            }
        }
        // Leave the monitor in a clean state.
        ddc.settle();
        let result = json!({
            "device": entry.info.device,
            "monitor_id": entry.monitor_id,
            "scanned_at": local_time(),
            "plain": plain,
            "vendor": vendor,
        });
        std::fs::write(path, serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    /// Compares two scans (files). Only registers with a valid reply in BOTH scans count as changed: an error on one
    /// side is more often a lost reply than a real difference. No monitor traffic.
    fn diff_scans(a: &std::path::Path, b: &std::path::Path) -> Result<(), String> {
        let load = |path: &std::path::Path| -> Result<Value, String> {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
        };
        let (first, second) = (load(a)?, load(b)?);
        let valid = |v: Option<&Value>| v.and_then(|v| v.as_str()).filter(|s| !s.starts_with("ERR")).map(str::to_string);
        let mut changed = 0;
        let mut one_sided = 0;
        for section in ["plain", "vendor"] {
            let (Some(left), Some(right)) = (first[section].as_object(), second[section].as_object()) else { continue };
            for (code, value) in left {
                match (valid(Some(value)), valid(right.get(code))) {
                    (Some(x), Some(y)) if x != y => {
                        changed += 1;
                        println!("{section:>6} {code}: {x} -> {y}");
                    }
                    (Some(_), None) | (None, Some(_)) => one_sided += 1,
                    _ => {}
                }
            }
        }
        println!("\n{changed} register(s) changed between two valid replies.");
        println!("{one_sided} more answered in only one of the scans (usually a lost reply, not a change).");
        println!("Expected noise: 0B (jumps), F3 (usage time), 02/52 (OSD change flags), AC/AE/69 (the signal itself).");
        Ok(())
    }

    pub fn run() -> i32 {
        let args = match parse_args() {
            Ok(args) => args,
            Err(message) => {
                eprintln!("{message}");
                return 2;
            }
        };
        if let Some((a, b)) = &args.diff {
            return match diff_scans(a, b) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            };
        }
        if let Err(error) = std::fs::create_dir_all(&args.out_dir) {
            eprintln!("cannot create {}: {error}", args.out_dir.display());
            return 2;
        }
        let log_path = args.out_dir.join("ddc_probe.log");
        match std::fs::File::create(&log_path) {
            Ok(file) => log::init(file),
            Err(error) => {
                eprintln!("cannot create {}: {error}", log_path.display());
                return 2;
            }
        }

        log::say(&format!("ddc_probe {} - READ-ONLY (GET only) - {}", env!("CARGO_PKG_VERSION"), local_time()));
        let _timer = TimerResolution::new();

        let displays = match enumerate_displays() {
            Ok(d) => describe_displays(d),
            Err(error) => {
                log::say(&format!("enumeration failed: {error}"));
                return 1;
            }
        };
        log::say(&format!("\nDisplays found: {}", displays.len()));
        for entry in &displays {
            let target = entry.identity.as_ref().map(is_target).unwrap_or(false);
            log::say(&format!(
                "  {} {}  name={}  id={}  rect={:?}{}",
                entry.info.device,
                if entry.info.primary { "(primary)" } else { "" },
                entry.identity.as_ref().map(|i| i.name.as_str()).unwrap_or("?"),
                entry.monitor_id.as_deref().unwrap_or("?"),
                entry.info.rect,
                if target { "  [P275MV target]" } else { "  [not probed]" },
            ));
            if let Some(error) = &entry.info.edid_error {
                log::say(&format!("      EDID problem: {error}"));
            }
        }

        let selected: Vec<&Entry> = displays
            .iter()
            .filter(|e| e.identity.as_ref().map(is_target).unwrap_or(false))
            .filter(|e| match &args.monitor {
                Some(want) => {
                    let want = want.trim_start_matches("\\\\.\\").to_uppercase();
                    e.info.device.to_uppercase().ends_with(&want)
                }
                None => true,
            })
            .collect();

        let ids: Vec<&str> = selected.iter().filter_map(|e| e.monitor_id.as_deref()).collect();
        let ids_distinct = {
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            sorted.dedup();
            sorted.len() == ids.len()
        };
        let serials_distinct = {
            let mut serials: Vec<&str> =
                selected.iter().filter_map(|e| e.identity.as_ref().map(|i| i.serial.as_str())).collect();
            let total = serials.len();
            serials.sort_unstable();
            serials.dedup();
            serials.len() == total && !serials.contains(&"")
        };
        log::say(&format!(
            "P275MV targets: {}; MonitorIds distinct: {}; EDID serials distinct and non-empty: {}",
            selected.len(),
            ids_distinct,
            serials_distinct
        ));

        let mut result = json!({
            "tool": "ddc_probe",
            "version": env!("CARGO_PKG_VERSION"),
            "read_only": true,
            "started_at": local_time(),
            "displays": displays.iter().map(entry_json).collect::<Vec<_>>(),
            "targets": selected.len(),
            "monitor_ids_distinct": ids_distinct,
            "edid_serials_distinct": serials_distinct,
            "monitors": [],
        });

        if let Some(path) = &args.scan {
            if selected.len() != 1 {
                log::say("\n--scan needs exactly one monitor: add --monitor DISPLAYn");
                return 2;
            }
            if !args.yes {
                println!("\nClose every other DDC program. The scan sends only GET requests and takes a few minutes.\nPress Enter to start...");
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
            }
            return match scan_monitor(selected[0], path) {
                Ok(()) => {
                    log::say(&format!("Scan saved: {}", path.display()));
                    0
                }
                Err(error) => {
                    log::say(&format!("Scan failed: {error}"));
                    1
                }
            };
        }
        if args.list_only {
            log::say("\n--list: no DDC traffic was sent.");
        } else if selected.is_empty() {
            log::say("\nNo P275MV target found: nothing to probe.");
        } else {
            if !args.yes {
                println!(
                    "\nClose every other DDC program (VIEW MORE WIDGET, Twinkle Tray, ...). Only GET requests will be sent.\nPress Enter to start..."
                );
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
            }
            let monitors: Vec<Value> = selected.iter().map(|e| probe_monitor(e, args.reps)).collect();
            result["monitors"] = Value::Array(monitors);
        }
        result["finished_at"] = json!(local_time());

        let json_path = args.out_dir.join("ddc_probe_result.json");
        match serde_json::to_string_pretty(&result)
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(&json_path, text).map_err(|e| e.to_string()))
        {
            Ok(()) => log::say(&format!("\nWritten: {}\n         {}", json_path.display(), log_path.display())),
            Err(error) => {
                log::say(&format!("cannot write {}: {error}", json_path.display()));
                return 1;
            }
        }
        // Silence "unused" for items the main app uses.
        let _ = (VCP_FIRMWARE, VCP_VERSION, FrameError::Null);
        0
    }
}
