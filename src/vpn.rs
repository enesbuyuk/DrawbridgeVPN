use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use anyhow::{Context, Result};
use crate::events::LogEvent;
use crate::profile::Profile;
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn applescript_double_quote_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
fn build_connect_shell_command(profile: &Profile, cookie: &str, watch_pid: u32) -> String {
    let host_port = format!("{}:{}", profile.vpn_host, profile.vpn_port);
    let cookie_arg = format!("--cookie=SVPNCOOKIE={cookie}");
    let cert_arg = format!("--trusted-cert={}", profile.cert_digest);
    format!(
        "openfortivpn {} {} {} & \
         VPNPID=$!; \
         while kill -0 $VPNPID 2>/dev/null && kill -0 {watch_pid} 2>/dev/null; do sleep 1; done; \
         kill -TERM $VPNPID 2>/dev/null; \
         wait $VPNPID 2>/dev/null",
        shell_single_quote(&host_port),
        shell_single_quote(&cookie_arg),
        shell_single_quote(&cert_arg)
    )
}
fn run_privileged_shell_command(inner_shell_command: &str) -> Result<Child> {
    let escaped = applescript_double_quote_escape(inner_shell_command);
    let applescript = format!("do shell script \"{escaped}\" with administrator privileges");
    Command::new("osascript")
        .arg("-e")
        .arg(applescript)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn osascript for a privileged shell command")
}
fn stream_output(child: &mut Child, log_tx: Sender<LogEvent>) {
    if let Some(stdout) = child.stdout.take() {
        let tx = log_tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(std::io::Result::ok) {
                if tx.send(LogEvent::Log(line)).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(std::io::Result::ok) {
                if log_tx.send(LogEvent::Log(line)).is_err() {
                    break;
                }
            }
        });
    }
}
pub fn connect(profile: &Profile, cookie: &str, log_tx: Sender<LogEvent>) -> Result<u32> {
    let inner_command = build_connect_shell_command(profile, cookie, std::process::id());
    let mut child = run_privileged_shell_command(&inner_command)
        .context("Failed to start openfortivpn through osascript")?;
    let pid = child.id();
    stream_output(&mut child, log_tx);
    Ok(pid)
}
fn tunnel_interface_bytes() -> Result<(u64, u64)> {
    let output = Command::new("netstat")
        .arg("-ib")
        .output()
        .context("Failed to run netstat")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut seen = HashSet::new();
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = cols.first() else { continue };
        if !name.starts_with("utun") || !seen.insert(*name) {
            continue;
        }
        let ibytes: u64 = cols.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
        let obytes: u64 = cols.get(9).and_then(|s| s.parse().ok()).unwrap_or(0);
        rx_total += ibytes;
        tx_total += obytes;
    }
    Ok((rx_total, tx_total))
}
pub fn spawn_speed_monitor(log_tx: Sender<LogEvent>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut last = tunnel_interface_bytes().ok().map(|b| (b, Instant::now()));
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(1000));
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok((rx, tx)) = tunnel_interface_bytes() else {
                continue;
            };
            let now = Instant::now();
            if let Some(((prev_rx, prev_tx), prev_time)) = last {
                let dt = now.duration_since(prev_time).as_secs_f64().max(0.001);
                let down_bps = rx.saturating_sub(prev_rx) as f64 / dt;
                let up_bps = tx.saturating_sub(prev_tx) as f64 / dt;
                if log_tx
                    .send(LogEvent::NetSpeed { down_bps, up_bps })
                    .is_err()
                {
                    break;
                }
            }
            last = Some(((rx, tx), now));
        }
    });
}
pub fn disconnect() -> Result<()> {
    let escaped = applescript_double_quote_escape("pkill -x openfortivpn || true");
    let applescript = format!("do shell script \"{escaped}\" with administrator privileges");
    let status = Command::new("osascript")
        .arg("-e")
        .arg(applescript)
        .status()
        .context("Failed to spawn osascript to stop openfortivpn")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("pkill exited with status {status}")
    }
}
