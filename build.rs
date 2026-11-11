//! Compiles the Blueprint UI (src/ui/window.blp) into GtkBuilder XML in OUT_DIR.
//!
//! The compiler is `blueprint-compiler` from PATH, or the program named by the `BLUEPRINT_COMPILER` variable.
//! When it is not available (typically on Windows), the pre-generated `src/ui/window.ui` is used instead; refresh
//! it with `scripts/generate_ui.sh` after every change of `window.blp`.
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    // Windows: icon, version info and the PerMonitorV2 DPI manifest go into the executable.
    println!("cargo:rerun-if-changed=assets/windows");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/windows/titan_control.rc", embed_resource::NONE)
            .manifest_required()
            .expect("cannot embed the Windows resources (needs windres or rc.exe)");
    }

    let source = "src/ui/window.blp";
    let prebuilt = "src/ui/window.ui";
    println!("cargo:rerun-if-changed={source}");
    println!("cargo:rerun-if-changed={prebuilt}");
    println!("cargo:rerun-if-env-changed=BLUEPRINT_COMPILER");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("window.ui");
    let compiler = env::var("BLUEPRINT_COMPILER").unwrap_or_else(|_| "blueprint-compiler".into());

    match Command::new(&compiler).env("PYTHONUTF8", "1").args(["compile", source, "--output"]).arg(&out).status() {
        Ok(status) => assert!(status.success(), "{compiler} failed for {source}"),
        Err(error) => {
            if fs::metadata(prebuilt).is_ok() {
                println!("cargo:warning={compiler} not found ({error}); using the pre-generated {prebuilt}");
                fs::copy(prebuilt, &out).unwrap_or_else(|e| panic!("cannot copy {prebuilt}: {e}"));
            } else {
                panic!(
                    "blueprint-compiler is required to build the UI ({error}); install the `blueprint-compiler` \
                     package, set BLUEPRINT_COMPILER, or provide {prebuilt} (scripts/generate_ui.sh)"
                );
            }
        }
    }
}
