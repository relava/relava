//! Automatic update notification check.
//!
//! On commands like `relava list` and `relava info`, silently checks whether
//! any installed resources have newer versions available on the server.
//! The check is throttled to at most once per hour via a timestamp file
//! stored at `~/.relava/last_update_check`.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use relava_types::manifest::ProjectManifest;
use relava_types::validate::ResourceType;

use crate::api_client::{ApiClient, UpdateAvailableResponse, UpdateCheckEntry};

/// Current time as seconds since the Unix epoch.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Minimum interval between automatic update checks (1 hour).
const CHECK_INTERVAL: Duration = Duration::from_secs(3600);

/// Name of the timestamp file under `~/.relava/`.
const TIMESTAMP_FILE: &str = "last_update_check";

/// Result of an update check.
#[derive(Debug, Default, serde::Serialize)]
pub struct UpdateCheckResult {
    /// Resources that have newer versions available.
    pub available: Vec<AvailableUpdate>,
    /// Whether the check was actually performed (false if throttled).
    pub checked: bool,
}

/// A single resource with an available update.
///
/// This is a type alias for the API response type — the wire format and
/// domain representation are identical.
pub type AvailableUpdate = UpdateAvailableResponse;

/// Run the update check if enough time has passed since the last check.
///
/// Returns `None` if the check was skipped (throttled or no manifest).
/// Silently returns an empty result on network/server errors to avoid
/// disrupting the primary command output.
pub fn check_if_due(
    server_url: &str,
    project_dir: &Path,
    relava_dir: Option<&Path>,
) -> UpdateCheckResult {
    let Some(relava_dir) = relava_dir
        .map(|p| p.to_path_buf())
        .or_else(default_relava_dir)
    else {
        return UpdateCheckResult::default();
    };

    if !should_check(&relava_dir) {
        return UpdateCheckResult::default();
    }

    let available = perform_check(server_url, project_dir);

    // Always update the timestamp, even on error, to avoid retrying every command
    write_timestamp(&relava_dir);

    UpdateCheckResult {
        available,
        checked: true,
    }
}

/// Check whether enough time has passed since the last check.
fn should_check(relava_dir: &Path) -> bool {
    let path = relava_dir.join(TIMESTAMP_FILE);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return true, // No file → first run → check
    };

    let last_check: u64 = match content.trim().parse() {
        Ok(ts) => ts,
        Err(_) => return true, // Malformed → check
    };

    now_secs().saturating_sub(last_check) >= CHECK_INTERVAL.as_secs()
}

/// Write the current timestamp to the check file.
fn write_timestamp(relava_dir: &Path) {
    let path = relava_dir.join(TIMESTAMP_FILE);
    let _ = std::fs::create_dir_all(relava_dir);
    if let Err(e) = std::fs::write(&path, now_secs().to_string()) {
        eprintln!("[warn] could not save update check timestamp: {e}");
    }
}

/// Get the default `~/.relava/` directory.
fn default_relava_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".relava"))
}

/// Compare manifest versions with the latest versions on the server,
/// returning any resources that have newer versions available.
///
/// Uses the batch `POST /api/v1/updates/check` endpoint — a single HTTP
/// request regardless of how many resources are in the manifest.
fn perform_check(server_url: &str, project_dir: &Path) -> Vec<AvailableUpdate> {
    let manifest = match load_manifest(project_dir) {
        Some(m) => m,
        None => return Vec::new(),
    };

    let client = ApiClient::new(server_url);

    let sections = [
        (ResourceType::Skill, &manifest.skills),
        (ResourceType::Agent, &manifest.agents),
        (ResourceType::Command, &manifest.commands),
        (ResourceType::Rule, &manifest.rules),
    ];

    let entries: Vec<UpdateCheckEntry> = sections
        .iter()
        .flat_map(|&(resource_type, section)| {
            section
                .iter()
                .filter(|(_, version_str)| *version_str != "*")
                .map(move |(name, version_str)| UpdateCheckEntry {
                    resource_type: resource_type.to_string(),
                    name: name.clone(),
                    version: version_str.clone(),
                })
        })
        .collect();

    if entries.is_empty() {
        return Vec::new();
    }

    // Silent failure: don't disrupt primary command output
    let Ok(response) = client.check_updates(&entries) else {
        return Vec::new();
    };

    response.available
}

/// Load the project manifest, returning None if not found or unparseable.
fn load_manifest(project_dir: &Path) -> Option<ProjectManifest> {
    let path = project_dir.join("relava.toml");
    ProjectManifest::from_file(&path).ok()
}

/// Print a non-intrusive update notification to stderr.
pub fn print_notification(result: &UpdateCheckResult) {
    let count = result.available.len();
    if count == 0 {
        return;
    }

    eprintln!();
    if count == 1 {
        let u = &result.available[0];
        eprintln!(
            "\x1b[33m{}/{} {} → {} available.\x1b[0m",
            u.resource_type, u.name, u.installed_version, u.latest_version
        );
    } else {
        eprintln!("\x1b[33m{} resources have updates available.\x1b[0m", count);
    }
    eprintln!("Run `relava update --all` to update.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    // --- should_check ---

    #[test]
    fn should_check_returns_true_when_no_file() {
        let dir = temp_dir();
        assert!(should_check(dir.path()));
    }

    #[test]
    fn should_check_returns_true_when_malformed() {
        let dir = temp_dir();
        fs::write(dir.path().join(TIMESTAMP_FILE), "not a number").unwrap();
        assert!(should_check(dir.path()));
    }

    #[test]
    fn should_check_returns_false_when_recent() {
        let dir = temp_dir();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        fs::write(dir.path().join(TIMESTAMP_FILE), now.to_string()).unwrap();
        assert!(!should_check(dir.path()));
    }

    #[test]
    fn should_check_returns_true_when_old() {
        let dir = temp_dir();
        let old = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 7200; // 2 hours ago
        fs::write(dir.path().join(TIMESTAMP_FILE), old.to_string()).unwrap();
        assert!(should_check(dir.path()));
    }

    #[test]
    fn should_check_returns_true_at_exact_interval() {
        let dir = temp_dir();
        let boundary = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - CHECK_INTERVAL.as_secs();
        fs::write(dir.path().join(TIMESTAMP_FILE), boundary.to_string()).unwrap();
        assert!(should_check(dir.path()));
    }

    // --- write_timestamp ---

    #[test]
    fn write_timestamp_creates_file() {
        let dir = temp_dir();
        write_timestamp(dir.path());
        let content = fs::read_to_string(dir.path().join(TIMESTAMP_FILE)).unwrap();
        let ts: u64 = content.trim().parse().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(now - ts < 5, "timestamp should be recent");
    }

    #[test]
    fn write_timestamp_creates_parent_dirs() {
        let dir = temp_dir();
        let nested = dir.path().join("nested").join("dir");
        write_timestamp(&nested);
        assert!(nested.join(TIMESTAMP_FILE).exists());
    }

    // --- perform_check (batch endpoint) ---

    #[test]
    fn perform_check_returns_empty_on_server_error() {
        // Unreachable server → silent empty result
        let dir = temp_dir();
        fs::write(
            dir.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n",
        )
        .unwrap();
        let available = perform_check("http://127.0.0.1:19999", dir.path());
        assert!(available.is_empty());
    }

    #[test]
    fn perform_check_batch_finds_updates() {
        let dir = temp_dir();
        fs::write(
            dir.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n",
        )
        .unwrap();

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("POST", "/api/v1/updates/check")
            .with_status(200)
            .with_body(
                r#"{"available":[{"type":"skill","name":"denden","installed_version":"1.0.0","latest_version":"2.0.0"}]}"#,
            )
            .create();

        let available = perform_check(&server.url(), dir.path());
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].name, "denden");
        assert_eq!(available[0].installed_version, "1.0.0");
        assert_eq!(available[0].latest_version, "2.0.0");
    }

    #[test]
    fn perform_check_batch_up_to_date() {
        let dir = temp_dir();
        fs::write(
            dir.path().join("relava.toml"),
            "[skills]\ndenden = \"2.0.0\"\n",
        )
        .unwrap();

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("POST", "/api/v1/updates/check")
            .with_status(200)
            .with_body(r#"{"available":[]}"#)
            .create();

        let available = perform_check(&server.url(), dir.path());
        assert!(available.is_empty());
    }

    #[test]
    fn perform_check_batch_multiple_resources() {
        let dir = temp_dir();
        fs::write(
            dir.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n\n[agents]\ndebugger = \"0.5.0\"\n",
        )
        .unwrap();

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("POST", "/api/v1/updates/check")
            .with_status(200)
            .with_body(
                r#"{"available":[{"type":"skill","name":"denden","installed_version":"1.0.0","latest_version":"2.0.0"},{"type":"agent","name":"debugger","installed_version":"0.5.0","latest_version":"1.0.0"}]}"#,
            )
            .create();

        let available = perform_check(&server.url(), dir.path());
        assert_eq!(available.len(), 2);
    }

    #[test]
    fn perform_check_returns_empty_without_manifest() {
        let dir = temp_dir();
        let available = perform_check("http://127.0.0.1:19999", dir.path());
        assert!(available.is_empty());
    }

    #[test]
    fn perform_check_skips_wildcard_versions() {
        let dir = temp_dir();
        fs::write(dir.path().join("relava.toml"), "[skills]\ndenden = \"*\"\n").unwrap();

        // No HTTP request should be made (all wildcards → empty entries)
        let available = perform_check("http://127.0.0.1:19999", dir.path());
        assert!(available.is_empty());
    }

    // --- check_if_due ---

    #[test]
    fn check_if_due_throttles_after_check() {
        let dir = temp_dir();
        let relava_dir = dir.path().join(".relava");
        fs::create_dir_all(&relava_dir).unwrap();

        // First call: no timestamp file → performs check
        let result = check_if_due("http://127.0.0.1:19999", dir.path(), Some(&relava_dir));
        assert!(result.checked);

        // Second call: recent timestamp → throttled
        let result = check_if_due("http://127.0.0.1:19999", dir.path(), Some(&relava_dir));
        assert!(!result.checked);
    }

    // --- print_notification ---

    #[test]
    fn print_notification_no_updates_is_silent() {
        // Just verify no panic
        let result = UpdateCheckResult::default();
        print_notification(&result);
    }

    #[test]
    fn print_notification_single_update() {
        let result = UpdateCheckResult {
            available: vec![AvailableUpdate {
                resource_type: "skill".to_string(),
                name: "denden".to_string(),
                installed_version: "1.0.0".to_string(),
                latest_version: "2.0.0".to_string(),
            }],
            checked: true,
        };
        // Just verify no panic
        print_notification(&result);
    }

    #[test]
    fn print_notification_multiple_updates() {
        let result = UpdateCheckResult {
            available: vec![
                AvailableUpdate {
                    resource_type: "skill".to_string(),
                    name: "denden".to_string(),
                    installed_version: "1.0.0".to_string(),
                    latest_version: "2.0.0".to_string(),
                },
                AvailableUpdate {
                    resource_type: "agent".to_string(),
                    name: "debugger".to_string(),
                    installed_version: "0.5.0".to_string(),
                    latest_version: "1.0.0".to_string(),
                },
            ],
            checked: true,
        };
        print_notification(&result);
    }

    // --- UpdateCheckResult serialization ---

    #[test]
    fn update_check_result_serializes() {
        let result = UpdateCheckResult {
            available: vec![AvailableUpdate {
                resource_type: "skill".to_string(),
                name: "denden".to_string(),
                installed_version: "1.0.0".to_string(),
                latest_version: "2.0.0".to_string(),
            }],
            checked: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("denden"));
        assert!(json.contains("2.0.0"));
    }

    // --- perform_check: additional coverage ---

    #[test]
    fn perform_check_sends_only_pinned_versions_from_mixed_manifest() {
        let dir = temp_dir();
        fs::write(
            dir.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\nwild = \"*\"\n\n[agents]\ndebugger = \"0.5.0\"\n",
        )
        .unwrap();

        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/api/v1/updates/check")
            .with_status(200)
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"resources":[{"type":"skill","name":"denden","version":"1.0.0"},{"type":"agent","name":"debugger","version":"0.5.0"}]}"#.to_string(),
            ))
            .with_body(r#"{"available":[]}"#)
            .create();

        let _ = perform_check(&server.url(), dir.path());
        mock.assert(); // wildcard "wild" must NOT appear in request
    }

    #[test]
    fn perform_check_includes_all_resource_types() {
        let dir = temp_dir();
        fs::write(
            dir.path().join("relava.toml"),
            "[skills]\ns1 = \"1.0.0\"\n\n[agents]\na1 = \"1.0.0\"\n\n[commands]\nc1 = \"1.0.0\"\n\n[rules]\nr1 = \"1.0.0\"\n",
        )
        .unwrap();

        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/api/v1/updates/check")
            .with_status(200)
            .match_body(mockito::Matcher::JsonString(
                r#"{"resources":[
                    {"type":"skill","name":"s1","version":"1.0.0"},
                    {"type":"agent","name":"a1","version":"1.0.0"},
                    {"type":"command","name":"c1","version":"1.0.0"},
                    {"type":"rule","name":"r1","version":"1.0.0"}
                ]}"#.to_string(),
            ))
            .with_body(r#"{"available":[]}"#)
            .create();

        let _ = perform_check(&server.url(), dir.path());
        mock.assert();
    }

    #[test]
    fn perform_check_returns_empty_on_server_500() {
        let dir = temp_dir();
        fs::write(
            dir.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n",
        )
        .unwrap();

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("POST", "/api/v1/updates/check")
            .with_status(500)
            .with_body(r#"{"error":"internal server error"}"#)
            .create();

        let available = perform_check(&server.url(), dir.path());
        assert!(available.is_empty());
    }

    // --- load_manifest ---

    #[test]
    fn load_manifest_returns_none_when_missing() {
        let dir = temp_dir();
        assert!(load_manifest(dir.path()).is_none());
    }

    #[test]
    fn load_manifest_returns_none_for_invalid() {
        let dir = temp_dir();
        fs::write(dir.path().join("relava.toml"), "not valid {{{").unwrap();
        assert!(load_manifest(dir.path()).is_none());
    }

    #[test]
    fn load_manifest_parses_valid() {
        let dir = temp_dir();
        fs::write(
            dir.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n",
        )
        .unwrap();
        let m = load_manifest(dir.path()).unwrap();
        assert_eq!(m.skills["denden"], "1.0.0");
    }
}
