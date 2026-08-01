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
// Root-only directory (mode 711: only root can create/replace entries in it, so a
// non-root local user cannot pre-plant a symlink at the log path before we open it).
// The log file itself is chmod 644 so the unprivileged app process can read it back.
const OPENFORTIVPN_LOG_DIR: &str = "/var/log/drawbridgevpn";
const OPENFORTIVPN_LOG_PATH: &str = "/var/log/drawbridgevpn/openfortivpn.log";
// `do shell script ... with administrator privileges` runs with the system default PATH
// (/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin), which does not include Homebrew's
// Apple Silicon prefix. Resolve the real binary path ourselves instead of relying on PATH.
const OPENFORTIVPN_CANDIDATE_PATHS: &[&str] = &[
    "/opt/homebrew/bin/openfortivpn",
    "/usr/local/bin/openfortivpn",
    "/usr/bin/openfortivpn",
];
fn find_openfortivpn_executable() -> String {
    OPENFORTIVPN_CANDIDATE_PATHS
        .iter()
        .find(|path| std::path::Path::new(path).is_file())
        .copied()
        .unwrap_or("openfortivpn")
        .to_string()
}
fn build_connect_shell_command(profile: &Profile, cookie: &str, watch_pid: u32) -> String {
    let host_port = format!("{}:{}", profile.vpn_host, profile.vpn_port);
    let cookie_arg = format!("--cookie=SVPNCOOKIE={cookie}");
    let cert_arg = format!("--trusted-cert={}", profile.cert_digest);
    let openfortivpn_bin = find_openfortivpn_executable();
    format!(
        "mkdir -p -m 711 {log_dir} && : > {log_path} && chmod 644 {log_path}; \
         {} {} {} {} >> {log_path} 2>&1 & \
         VPNPID=$!; \
         while kill -0 $VPNPID 2>/dev/null && kill -0 {watch_pid} 2>/dev/null; do sleep 1; done; \
         kill -TERM $VPNPID 2>/dev/null; \
         wait $VPNPID 2>/dev/null; \
         pkill -x pppd 2>/dev/null; \
         true",
        shell_single_quote(&openfortivpn_bin),
        shell_single_quote(&host_port),
        shell_single_quote(&cookie_arg),
        shell_single_quote(&cert_arg),
        log_dir = shell_single_quote(OPENFORTIVPN_LOG_DIR),
        log_path = shell_single_quote(OPENFORTIVPN_LOG_PATH)
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
fn ppp_interfaces_with_ip() -> HashSet<String> {
    let Ok(output) = Command::new("ifconfig").output() else {
        return HashSet::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = HashSet::new();
    let mut current: Option<String> = None;
    let mut current_has_ip = false;
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            if let Some(name) = current.take() {
                if current_has_ip {
                    result.insert(name);
                }
            }
            let name = line.split(':').next().unwrap_or("");
            current = name.starts_with("ppp").then(|| name.to_string());
            current_has_ip = false;
        } else if current.is_some() && line.trim_start().starts_with("inet ") {
            current_has_ip = true;
        }
    }
    if let Some(name) = current {
        if current_has_ip {
            result.insert(name);
        }
    }
    result
}
fn read_openfortivpn_log() -> String {
    std::fs::read_to_string(OPENFORTIVPN_LOG_PATH)
        .unwrap_or_else(|e| format!("(could not read {OPENFORTIVPN_LOG_PATH}: {e})"))
}
pub fn connect(profile: &Profile, cookie: &str, log_tx: Sender<LogEvent>) -> Result<u32> {
    let baseline = ppp_interfaces_with_ip();
    let inner_command = build_connect_shell_command(profile, cookie, std::process::id());
    let mut child = run_privileged_shell_command(&inner_command)
        .context("Failed to start openfortivpn through osascript")?;
    let pid = child.id();
    stream_output(&mut child, log_tx);
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if ppp_interfaces_with_ip().difference(&baseline).next().is_some() {
            return Ok(pid);
        }
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!(
                "openfortivpn exited before the tunnel came up (status {status}): {}",
                read_openfortivpn_log()
            );
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "Timed out waiting for the ppp tunnel interface to come up: {}",
                read_openfortivpn_log()
            );
        }
        thread::sleep(Duration::from_millis(500));
    }
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
        if !name.starts_with("ppp") || !seen.insert(*name) {
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
fn openfortivpn_or_pppd_running() -> bool {
    for name in ["openfortivpn", "pppd"] {
        if let Ok(output) = Command::new("pgrep").args(["-x", name]).output() {
            if !output.stdout.is_empty() {
                return true;
            }
        }
    }
    false
}
pub fn disconnect(log_tx: Sender<LogEvent>) -> Result<()> {
    let escaped = applescript_double_quote_escape(
        "before=$(pgrep -x openfortivpn | wc -l | tr -d ' '); \
         pkill -x openfortivpn; K1=$?; \
         pkill -x pppd; K2=$?; \
         sleep 3; \
         mid=$(pgrep -x openfortivpn | wc -l | tr -d ' '); \
         if [ \"$mid\" != \"0\" ]; then \
             pkill -9 -x openfortivpn; \
             pkill -9 -x pppd; \
             sleep 1; \
         fi; \
         after=$(pgrep -x openfortivpn | wc -l | tr -d ' '); \
         echo 'pkill_diag' before=$before k1=$K1 k2=$K2 mid=$mid after=$after; \
         true",
    );
    let applescript = format!("do shell script \"{escaped}\" with administrator privileges");
    let output = Command::new("osascript")
        .arg("-e")
        .arg(applescript)
        .output()
        .context("Failed to spawn osascript to stop openfortivpn")?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stdout.is_empty() || !stderr.is_empty() {
        let _ = log_tx.send(LogEvent::Log(format!(
            "pkill output: stdout={stdout:?} stderr={stderr:?}"
        )));
    }
    if !output.status.success() {
        anyhow::bail!("pkill exited with status {}: {stderr}", output.status);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && openfortivpn_or_pppd_running() {
        thread::sleep(Duration::from_millis(300));
    }
    if openfortivpn_or_pppd_running() {
        anyhow::bail!(
            "pkill reported success but openfortivpn/pppd is still running \
             (a different process may own it, or the tunnel is under a different auth session)"
        );
    }
    Ok(())
}
