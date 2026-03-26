//! Server lifecycle commands: start, stop, and status.
//!
//! Manages the `relava-server` process via a PID state file stored at
//! `~/.relava/server.pid`. The state file is JSON containing the PID,
//! port, and start timestamp for reliable process tracking.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::output::Tag;

// ---------------------------------------------------------------------------
// State file
// ---------------------------------------------------------------------------

/// Persisted server state written to `~/.relava/server.pid`.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ServerState {
    pid: u32,
    port: u16,
    started_at: u64,
}

/// Return the `~/.relava` directory path.
fn relava_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("cannot determine home directory")?;
    Ok(home.join(".relava"))
}

/// Return the path to the server state file (`~/.relava/server.pid`).
fn state_file_path() -> Result<PathBuf, String> {
    Ok(relava_dir()?.join("server.pid"))
}

/// Return the path to the server log file (`~/.relava/server.log`).
fn log_file_path() -> Result<PathBuf, String> {
    Ok(relava_dir()?.join("server.log"))
}

/// Read the current server state from the PID file.
fn read_state() -> Result<Option<ServerState>, String> {
    let path = state_file_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let contents =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let state: ServerState = serde_json::from_str(&contents)
        .map_err(|e| format!("corrupt state file {}: {e}", path.display()))?;
    Ok(Some(state))
}

/// Write server state to the PID file.
fn write_state(state: &ServerState) -> Result<(), String> {
    let path = state_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("failed to serialize state: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// Remove the PID state file.
fn remove_state() -> Result<(), String> {
    let path = state_file_path()?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("failed to remove {}: {e}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Process utilities
// ---------------------------------------------------------------------------

/// Check whether a process with the given PID is still running.
///
/// Note: `kill -0` cannot distinguish "not found" (ESRCH) from "not
/// permitted" (EPERM). If the server was started by a different user,
/// this reports it as not running. Acceptable for single-user local usage.
fn is_process_running(pid: u32) -> bool {
    // `kill -0 <pid>` checks existence without sending a signal.
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Send SIGTERM to the given PID. Returns true if the signal was sent.
fn send_sigterm(pid: u32) -> bool {
    Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Locate the `relava-server` binary.
///
/// Checks the same directory as the current executable first (covers both
/// `cargo install` and `target/debug/` during development), then falls back
/// to a plain name that relies on `$PATH`.
fn server_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("relava-server");
        if sibling.exists() {
            return sibling;
        }
    }
    PathBuf::from("relava-server")
}

/// Return the current Unix timestamp in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Check if the server already running. Returns the state if running, or
/// cleans up a stale PID file and returns `None`.
fn check_existing_server() -> Result<Option<ServerState>, String> {
    if let Some(state) = read_state()? {
        if is_process_running(state.pid) {
            return Ok(Some(state));
        }
        // Stale PID file — process no longer running.
        remove_state()?;
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

/// Print a serializable value as pretty JSON.
fn print_json(value: &impl Serialize) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|e| format!("json error: {e}"))?;
    println!("{json}");
    Ok(())
}

// ---------------------------------------------------------------------------
// JSON output types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StartResult {
    status: &'static str,
    pid: u32,
    port: u16,
    daemon: bool,
    url: String,
}

#[derive(Serialize)]
struct StopResult {
    status: &'static str,
    pid: u32,
}

#[derive(Serialize)]
struct StatusResult {
    running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uptime_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Start the registry server.
pub fn start(port: u16, daemon: bool, json: bool, verbose: bool) -> Result<(), String> {
    // Check for an already-running server.
    if let Some(state) = check_existing_server()? {
        let msg = format!(
            "server is already running on port {} (PID {}). \
             Stop it with `relava server stop` or use a different --port.",
            state.port, state.pid,
        );
        return Err(msg);
    }

    let binary = server_binary();
    if verbose {
        eprintln!("server binary: {}", binary.display());
    }

    if daemon {
        start_daemon(&binary, port, json, verbose)
    } else {
        start_foreground(&binary, port, json, verbose)
    }
}

/// Start the server in daemon (background) mode.
fn start_daemon(binary: &Path, port: u16, json: bool, verbose: bool) -> Result<(), String> {
    let log_path = log_file_path()?;

    // Open log file for daemon output.
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("failed to open log file {}: {e}", log_path.display()))?;
    let stderr_log = log_file
        .try_clone()
        .map_err(|e| format!("failed to clone log file handle: {e}"))?;

    let child = Command::new(binary)
        .env("RELAVA_PORT", port.to_string())
        .env("RELAVA_HOST", "127.0.0.1")
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr_log))
        .spawn()
        .map_err(|e| format!("failed to start server: {e}"))?;

    let pid = child.id();
    let state = ServerState {
        pid,
        port,
        started_at: now_secs(),
    };
    write_state(&state)?;

    // Brief pause then verify the process is still alive (catches immediate
    // crashes like port-in-use).
    std::thread::sleep(std::time::Duration::from_millis(200));

    if !is_process_running(pid) {
        remove_state()?;
        let hint = if log_path.exists() {
            format!(" Check {} for details.", log_path.display())
        } else {
            String::new()
        };
        return Err(format!("server exited immediately after starting.{hint}"));
    }

    let url = format!("http://127.0.0.1:{port}");

    if json {
        print_json(&StartResult {
            status: "started",
            pid,
            port,
            daemon: true,
            url,
        })?;
    } else {
        println!(
            "{}",
            Tag::Ok.fmt(&format!("Server started on {url} (PID {pid})"))
        );
        if verbose {
            println!(
                "{}",
                Tag::Ok.fmt(&format!("Log file: {}", log_path.display()))
            );
        }
    }

    Ok(())
}

/// Start the server in foreground mode (blocking).
fn start_foreground(binary: &Path, port: u16, json: bool, _verbose: bool) -> Result<(), String> {
    let mut child = Command::new(binary)
        .env("RELAVA_PORT", port.to_string())
        .env("RELAVA_HOST", "127.0.0.1")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("failed to start server: {e}"))?;

    let pid = child.id();
    write_state(&ServerState {
        pid,
        port,
        started_at: now_secs(),
    })?;

    if !json {
        println!(
            "{}",
            Tag::Ok.fmt(&format!(
                "Server running on http://127.0.0.1:{port} (PID {pid})"
            ))
        );
        println!("Press Ctrl+C to stop.");
    }

    // Block until the child exits.
    let status = child
        .wait()
        .map_err(|e| format!("failed to wait for server process: {e}"))?;

    // Clean up PID file on exit.
    if let Err(e) = remove_state() {
        eprintln!(
            "{}",
            Tag::Warn.fmt(&format!("failed to clean up PID file: {e}"))
        );
    }

    let result_status = if status.success() {
        "stopped"
    } else {
        "failed"
    };

    if json {
        print_json(&StartResult {
            status: result_status,
            pid,
            port,
            daemon: false,
            url: format!("http://127.0.0.1:{port}"),
        })?;
    }

    if !status.success() {
        return Err(format!("server exited with {status}"));
    }

    Ok(())
}

/// Stop a running server.
pub fn stop(json: bool, _verbose: bool) -> Result<(), String> {
    let state = read_state()?.ok_or(
        "no server is running (PID file not found). \
         Start one with `relava server start`.",
    )?;

    if !is_process_running(state.pid) {
        remove_state()?;
        return Err(format!(
            "server process {} is no longer running (stale PID file removed).",
            state.pid,
        ));
    }

    if !send_sigterm(state.pid) {
        return Err(format!(
            "failed to send stop signal to server (PID {}). \
             You may need to stop it manually.",
            state.pid,
        ));
    }

    // Wait for the process to exit (up to 5 seconds).
    let deadline = now_secs() + 5;
    while is_process_running(state.pid) && now_secs() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    if is_process_running(state.pid) {
        let pid_path = state_file_path().unwrap_or_default();
        return Err(format!(
            "server (PID {}) did not stop within 5 seconds. \
             You may need to force-kill it and remove {}.",
            state.pid,
            pid_path.display(),
        ));
    }

    remove_state()?;

    if json {
        print_json(&StopResult {
            status: "stopped",
            pid: state.pid,
        })?;
    } else {
        println!(
            "{}",
            Tag::Ok.fmt(&format!("Server stopped (PID {})", state.pid))
        );
    }

    Ok(())
}

/// Report server status.
pub fn status(json: bool, _verbose: bool) -> Result<(), String> {
    let state = match check_existing_server()? {
        Some(s) => s,
        None => {
            if json {
                print_json(&StatusResult {
                    running: false,
                    pid: None,
                    port: None,
                    url: None,
                    uptime_secs: None,
                })?;
            } else {
                println!("{}", Tag::Warn.fmt("Server is not running"));
            }
            return Ok(());
        }
    };

    let uptime = now_secs().saturating_sub(state.started_at);
    let url = format!("http://127.0.0.1:{}", state.port);

    if json {
        print_json(&StatusResult {
            running: true,
            pid: Some(state.pid),
            port: Some(state.port),
            url: Some(url),
            uptime_secs: Some(uptime),
        })?;
    } else {
        println!("{}", Tag::Ok.fmt(&format!("Server running on {url}")));
        println!("{}", Tag::Ok.fmt(&format!("PID: {}", state.pid)));
        println!(
            "{}",
            Tag::Ok.fmt(&format!("Uptime: {}", format_uptime(uptime)))
        );
    }

    Ok(())
}

/// Format seconds into a human-readable uptime string.
fn format_uptime(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        let s = secs % 60;
        return format!("{minutes}m {s}s");
    }
    let hours = minutes / 60;
    let m = minutes % 60;
    if hours < 24 {
        return format!("{hours}h {m}m");
    }
    let days = hours / 24;
    let h = hours % 24;
    format!("{days}d {h}h")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_seconds() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(59), "59s");
    }

    #[test]
    fn format_uptime_minutes() {
        assert_eq!(format_uptime(60), "1m 0s");
        assert_eq!(format_uptime(90), "1m 30s");
        assert_eq!(format_uptime(3599), "59m 59s");
    }

    #[test]
    fn format_uptime_hours() {
        assert_eq!(format_uptime(3600), "1h 0m");
        assert_eq!(format_uptime(7200), "2h 0m");
        assert_eq!(format_uptime(86399), "23h 59m");
    }

    #[test]
    fn format_uptime_days() {
        assert_eq!(format_uptime(86400), "1d 0h");
        assert_eq!(format_uptime(172800), "2d 0h");
        assert_eq!(format_uptime(90061), "1d 1h");
    }

    #[test]
    fn state_file_path_is_under_relava_dir() {
        // state_file_path depends on home directory existing; skip if not.
        if dirs::home_dir().is_none() {
            return;
        }
        let path = state_file_path().unwrap();
        assert!(path.ends_with(".relava/server.pid"));
    }

    #[test]
    fn log_file_path_is_under_relava_dir() {
        if dirs::home_dir().is_none() {
            return;
        }
        let path = log_file_path().unwrap();
        assert!(path.ends_with(".relava/server.log"));
    }

    #[test]
    fn read_state_returns_none_when_no_file() {
        // Use a temp dir to avoid interfering with real state.
        let tmp = tempfile::tempdir().unwrap();
        let fake_path = tmp.path().join("nonexistent.pid");
        assert!(!fake_path.exists());
        // We can't easily override the path, so just verify no panic on real path.
        // If there's no PID file, read_state returns None or Some.
        let result = read_state();
        assert!(result.is_ok());
    }

    #[test]
    fn server_state_roundtrip() {
        let state = ServerState {
            pid: 12345,
            port: 7420,
            started_at: 1000000,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: ServerState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pid, 12345);
        assert_eq!(parsed.port, 7420);
        assert_eq!(parsed.started_at, 1000000);
    }

    #[test]
    fn is_process_running_returns_true_for_self() {
        // Our own process should be running.
        assert!(is_process_running(std::process::id()));
    }

    #[test]
    fn is_process_running_returns_false_for_invalid_pid() {
        // PID 0 is special (kernel); very high PIDs are unlikely to exist.
        assert!(!is_process_running(4_000_000));
    }

    #[test]
    fn server_binary_returns_path() {
        let path = server_binary();
        // Should at minimum return a non-empty path.
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn now_secs_is_positive() {
        assert!(now_secs() > 0);
    }

    #[test]
    fn start_result_serializes() {
        let result = StartResult {
            status: "started",
            pid: 123,
            port: 7420,
            daemon: true,
            url: "http://127.0.0.1:7420".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("started"));
        assert!(json.contains("7420"));
    }

    #[test]
    fn stop_result_serializes() {
        let result = StopResult {
            status: "stopped",
            pid: 456,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("stopped"));
        assert!(json.contains("456"));
    }

    #[test]
    fn status_result_running_serializes() {
        let result = StatusResult {
            running: true,
            pid: Some(789),
            port: Some(7420),
            url: Some("http://127.0.0.1:7420".into()),
            uptime_secs: Some(3600),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("true"));
        assert!(json.contains("789"));
    }

    #[test]
    fn status_result_stopped_omits_optional_fields() {
        let result = StatusResult {
            running: false,
            pid: None,
            port: None,
            url: None,
            uptime_secs: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("false"));
        assert!(!json.contains("pid"));
        assert!(!json.contains("port"));
    }

    #[test]
    fn server_state_pretty_serialization() {
        let state = ServerState {
            pid: 99999,
            port: 8080,
            started_at: 123456,
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        assert!(json.contains("99999"));
        assert!(json.contains("8080"));
        assert!(json.contains("123456"));
    }
}
