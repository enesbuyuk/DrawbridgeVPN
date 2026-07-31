use std::fs::{self, OpenOptions, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use anyhow::{Context, Result};
fn log_file_path() -> Result<PathBuf> {
    let mut dir = dirs::data_dir().context("Failed to resolve the application data directory")?;
    dir.push("DrawbridgeVPN");
    fs::create_dir_all(&dir).context("Failed to create the application data directory")?;
    Ok(dir.join("drawbridgevpn.log"))
}
static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();
fn file_handle() -> Result<&'static Mutex<File>> {
    if let Some(handle) = LOG_FILE.get() {
        return Ok(handle);
    }
    let path = log_file_path()?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open the log file at {}", path.display()))?;
    Ok(LOG_FILE.get_or_init(|| Mutex::new(file)))
}
static START: OnceLock<Instant> = OnceLock::new();
fn elapsed_stamp() -> String {
    let elapsed = START.get_or_init(Instant::now).elapsed();
    format!("[{:7.3}s]", elapsed.as_secs_f64())
}
pub fn append_line(line: &str) {
    let Ok(handle) = file_handle() else {
        return;
    };
    let Ok(mut file) = handle.lock() else {
        return;
    };
    let stamp = elapsed_stamp();
    for segment in line.split('\n') {
        let _ = writeln!(file, "{stamp} {segment}");
    }
    let _ = file.flush();
}
pub fn path() -> Option<PathBuf> {
    log_file_path().ok()
}
