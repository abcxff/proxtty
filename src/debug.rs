//! Opt-in diagnostic logging.
//!
//! Enabled by setting `PROXTTY_DEBUG=/path/to/log`. Since stderr is on the
//! alternate screen while running, diagnostics go to a file instead. Used to
//! capture the raw input stream and scroll reactions when chasing input bugs.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Option<Mutex<File>>> = OnceLock::new();

fn log_file() -> &'static Option<Mutex<File>> {
    LOG.get_or_init(|| {
        let path = std::env::var("PROXTTY_DEBUG").ok()?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new)
    })
}

/// A single free-form diagnostic line.
pub fn msg(text: &str) {
    if let Some(lock) = log_file() {
        if let Ok(mut file) = lock.lock() {
            let _ = writeln!(file, "{text}");
        }
    }
}

/// Log a chunk of raw input bytes, both as hex and as a readable escape string.
pub fn raw(bytes: &[u8]) {
    if log_file().is_none() {
        return;
    }
    let hex = bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let readable = String::from_utf8_lossy(bytes)
        .escape_default()
        .to_string();
    msg(&format!("raw[{}]: {hex}  |  {readable}", bytes.len()));
}
