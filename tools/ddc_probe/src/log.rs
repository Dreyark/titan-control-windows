//! Tiny logger: `say` prints to the console and the log file, `file` only to the log file.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static LOG: OnceLock<(Mutex<File>, Instant)> = OnceLock::new();

pub fn init(file: File) {
    let _ = LOG.set((Mutex::new(file), Instant::now()));
}

fn write(line: &str) {
    if let Some((file, start)) = LOG.get()
        && let Ok(mut file) = file.lock()
    {
        let _ = writeln!(file, "[{:>9.3}] {line}", start.elapsed().as_secs_f64());
    }
}

pub fn say(line: &str) {
    println!("{line}");
    write(line);
}

pub fn file(line: &str) {
    write(line);
}
