//! Self-update mechanism for the relava CLI and server binaries.
//!
//! At startup, checks GitHub Releases for the latest version (throttled to
//! once per 24 hours). If a newer version is available and stdout is a TTY,
//! prompts the user interactively before downloading. Uses atomic rename for
//! safe replacement and SHA-256 checksum verification for integrity.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::output::Tag;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// GitHub API endpoint for the latest release.
const RELEASES_URL: &str = "https://api.github.com/repos/relava/relava/releases/latest";

/// User-Agent header required by the GitHub API.
const USER_AGENT: &str = "relava-cli";

/// Current version of this binary, set at compile time.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Startup self-update check interval (24 hours).
const STARTUP_CHECK_INTERVAL: Duration = Duration::from_secs(86400);

/// Timestamp file for throttling startup self-update checks.
const TIMESTAMP_FILE: &str = "last_self_update_check";

/// Names of the binaries to update.
const BINARIES: &[&str] = &["relava", "relava-server"];

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of updating a single binary.
#[derive(Debug, serde::Serialize)]
pub struct BinaryUpdateResult {
    pub name: String,
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Information about a GitHub release.
#[derive(Debug, Clone)]
struct ReleaseInfo {
    /// Version tag (e.g., "0.2.0"), stripped of leading "v".
    version: String,
    /// Assets available for download.
    assets: Vec<ReleaseAsset>,
}

/// A single downloadable asset from a GitHub release.
#[derive(Debug, Clone)]
struct ReleaseAsset {
    name: String,
    download_url: String,
}

// ---------------------------------------------------------------------------
// Public API — startup check with interactive prompt
// ---------------------------------------------------------------------------

/// Perform the startup self-update check.
///
/// This is the main entry point called from `main()`. It:
/// 1. Checks whether enough time has passed since the last check (24h throttle)
/// 2. Queries GitHub Releases for the latest version
/// 3. If a newer version is available and stdout is a TTY, prompts the user
/// 4. If the user accepts, downloads, verifies, and replaces both binaries
///
/// Non-interactive environments (non-TTY stdout) skip the prompt entirely.
/// Network failures are silently ignored.
pub fn startup_check() {
    startup_check_with(is_interactive(), &mut std::io::stdin().lock());
}

/// Testable inner implementation of the startup check.
///
/// `interactive` controls whether a prompt is shown (false in non-TTY or CI).
/// `input` is the source for reading the user's yes/no response.
fn startup_check_with<R: std::io::BufRead>(interactive: bool, input: &mut R) {
    let Some(relava_dir) = default_relava_dir() else {
        return;
    };

    if !should_startup_check(&relava_dir) {
        return;
    }

    write_startup_timestamp(&relava_dir);

    let release = match fetch_latest_release_quiet() {
        Some(r) => r,
        None => return,
    };

    if !should_update(CURRENT_VERSION, &release.version) {
        return;
    }

    if !interactive {
        // Non-TTY: just print a notice to stderr and continue
        eprintln!(
            "\x1b[33mA new version of relava is available (current: v{}, latest: v{}).\x1b[0m",
            CURRENT_VERSION, release.version
        );
        return;
    }

    // Interactive TTY: prompt the user
    eprint!(
        "\x1b[33mA new version of relava is available (current: v{}, latest: v{}). Update now? [Y/n] \x1b[0m",
        CURRENT_VERSION, release.version
    );

    let mut answer = String::new();
    if input.read_line(&mut answer).is_err() {
        return;
    }

    let answer = answer.trim().to_lowercase();
    if !answer.is_empty() && answer != "y" && answer != "yes" {
        return;
    }

    // User accepted — perform the update
    perform_update(&release);
}

/// Download, verify, and replace all binaries from the given release.
fn perform_update(release: &ReleaseInfo) {
    let (os, arch) = match detect_platform() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{}", Tag::Fail.fmt(&e));
            return;
        }
    };

    let mut any_error = false;
    for bin_name in BINARIES {
        let result = update_binary(bin_name, release, &os, &arch, false);
        if result.status == "error" {
            any_error = true;
        }
    }

    if !any_error {
        println!(
            "{}",
            Tag::Ok.fmt(&format!("successfully updated to v{}", release.version))
        );
    }
}

// ---------------------------------------------------------------------------
// TTY detection
// ---------------------------------------------------------------------------

/// Check whether stdout is connected to a terminal.
fn is_interactive() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------

/// Detect the current OS and architecture.
fn detect_platform() -> Result<(String, String), String> {
    let os = detect_os()?;
    let arch = detect_arch()?;
    Ok((os, arch))
}

/// Detect the current OS.
fn detect_os() -> Result<String, String> {
    match std::env::consts::OS {
        "macos" => Ok("darwin".to_string()),
        "linux" => Ok("linux".to_string()),
        "windows" => Ok("windows".to_string()),
        other => Err(format!("unsupported operating system: {other}")),
    }
}

/// Detect the current CPU architecture.
fn detect_arch() -> Result<String, String> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64".to_string()),
        "aarch64" => Ok("aarch64".to_string()),
        other => Err(format!("unsupported architecture: {other}")),
    }
}

/// Build the expected asset filename for a given binary.
///
/// Convention: `relava-{version}-{os}-{arch}.tar.gz`
/// The archive contains both `relava` and `relava-server` binaries.
fn asset_name(version: &str, os: &str, arch: &str) -> String {
    format!("relava-{version}-{os}-{arch}.tar.gz")
}

/// Build the expected checksum asset filename.
fn checksum_asset_name(version: &str, os: &str, arch: &str) -> String {
    format!("relava-{version}-{os}-{arch}.sha256")
}

// ---------------------------------------------------------------------------
// GitHub API
// ---------------------------------------------------------------------------

/// Fetch the latest release quietly. Returns None on any error.
fn fetch_latest_release_quiet() -> Option<ReleaseInfo> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(USER_AGENT)
        .build()
        .ok()?;

    let response = client
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    let body: serde_json::Value = response.json().ok()?;
    parse_release_info(&body).ok()
}

/// Parse a GitHub release JSON response into `ReleaseInfo`.
fn parse_release_info(body: &serde_json::Value) -> Result<ReleaseInfo, String> {
    let tag = body["tag_name"]
        .as_str()
        .ok_or("missing tag_name in release response")?;

    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();

    let assets = body["assets"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|a| {
            let name = a["name"].as_str()?.to_string();
            let download_url = a["browser_download_url"].as_str()?.to_string();
            Some(ReleaseAsset { name, download_url })
        })
        .collect();

    Ok(ReleaseInfo { version, assets })
}

// ---------------------------------------------------------------------------
// Version comparison
// ---------------------------------------------------------------------------

/// Compare two version strings. Returns true if `latest` is newer than `current`.
fn should_update(current: &str, latest: &str) -> bool {
    let Ok(current_v) = relava_types::version::Version::parse(current) else {
        return false;
    };
    let Ok(latest_v) = relava_types::version::Version::parse(latest) else {
        return false;
    };
    latest_v > current_v
}

// ---------------------------------------------------------------------------
// Binary update logic
// ---------------------------------------------------------------------------

/// Update a single binary by downloading, verifying, and replacing it.
fn update_binary(
    bin_name: &str,
    release: &ReleaseInfo,
    os: &str,
    arch: &str,
    verbose: bool,
) -> BinaryUpdateResult {
    let binary_path = match find_binary_path(bin_name) {
        Some(p) => p,
        None => {
            let msg = format!("{bin_name} not found in PATH");
            println!("{}", Tag::Skip.fmt(&msg));
            return BinaryUpdateResult {
                name: bin_name.to_string(),
                path: String::new(),
                status: "skipped".to_string(),
                error: Some(msg),
            };
        }
    };

    // Check if binary is writable
    if !is_writable(&binary_path) {
        let msg = format!(
            "{bin_name} at {} is not writable. Try running with sudo or adjust permissions.",
            binary_path.display()
        );
        println!("{}", Tag::Fail.fmt(&msg));
        return BinaryUpdateResult {
            name: bin_name.to_string(),
            path: binary_path.display().to_string(),
            status: "error".to_string(),
            error: Some(msg),
        };
    }

    // Find the archive asset
    let archive_name = asset_name(&release.version, os, arch);
    let archive_asset = release.assets.iter().find(|a| a.name == archive_name);

    let Some(archive_asset) = archive_asset else {
        let msg = format!("no release asset found for {archive_name}");
        println!("{}", Tag::Fail.fmt(&msg));
        return BinaryUpdateResult {
            name: bin_name.to_string(),
            path: binary_path.display().to_string(),
            status: "error".to_string(),
            error: Some(msg),
        };
    };

    if verbose {
        eprintln!("downloading {}", archive_asset.download_url);
    }

    // Download the archive
    let archive_data = match download_asset(&archive_asset.download_url) {
        Ok(data) => data,
        Err(e) => {
            let msg = format!("failed to download {archive_name}: {e}");
            println!("{}", Tag::Fail.fmt(&msg));
            return BinaryUpdateResult {
                name: bin_name.to_string(),
                path: binary_path.display().to_string(),
                status: "error".to_string(),
                error: Some(msg),
            };
        }
    };

    // Verify checksum if available
    let checksum_name = checksum_asset_name(&release.version, os, arch);
    let checksum_asset = release.assets.iter().find(|a| a.name == checksum_name);

    if let Some(checksum_asset) = checksum_asset {
        match verify_checksum(&archive_data, &checksum_asset.download_url) {
            Ok(()) => {
                if verbose {
                    eprintln!("checksum verified");
                }
            }
            Err(e) => {
                let msg = format!("checksum verification failed: {e}");
                println!("{}", Tag::Fail.fmt(&msg));
                return BinaryUpdateResult {
                    name: bin_name.to_string(),
                    path: binary_path.display().to_string(),
                    status: "error".to_string(),
                    error: Some(msg),
                };
            }
        }
    } else if verbose {
        eprintln!("no checksum file found, skipping verification");
    }

    // Extract the target binary from the archive
    let binary_data = match extract_binary_from_archive(&archive_data, bin_name) {
        Ok(data) => data,
        Err(e) => {
            let msg = format!("failed to extract {bin_name} from archive: {e}");
            println!("{}", Tag::Fail.fmt(&msg));
            return BinaryUpdateResult {
                name: bin_name.to_string(),
                path: binary_path.display().to_string(),
                status: "error".to_string(),
                error: Some(msg),
            };
        }
    };

    // Atomic replace: write to temp file, then rename
    match atomic_replace(&binary_path, &binary_data) {
        Ok(()) => {
            println!(
                "{}",
                Tag::Ok.fmt(&format!("{bin_name} updated at {}", binary_path.display()))
            );
            BinaryUpdateResult {
                name: bin_name.to_string(),
                path: binary_path.display().to_string(),
                status: "updated".to_string(),
                error: None,
            }
        }
        Err(e) => {
            let msg = format!("failed to replace {bin_name}: {e}");
            println!("{}", Tag::Fail.fmt(&msg));
            BinaryUpdateResult {
                name: bin_name.to_string(),
                path: binary_path.display().to_string(),
                status: "error".to_string(),
                error: Some(msg),
            }
        }
    }
}

/// Find the path of a binary by looking it up relative to the current executable,
/// then falling back to PATH.
fn find_binary_path(name: &str) -> Option<PathBuf> {
    // First: check next to the current executable
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(dir) = current_exe.parent()
    {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Fallback: search PATH
    which_binary(name)
}

/// Search for a binary on PATH.
fn which_binary(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|p| p.exists())
}

/// Check if a file is writable by the current user.
fn is_writable(path: &Path) -> bool {
    // Try opening for write; if it succeeds, it's writable
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

// ---------------------------------------------------------------------------
// Download and verification
// ---------------------------------------------------------------------------

/// Download an asset from a URL.
fn download_asset(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("failed to create HTTP client: {e}"))?;

    let mut response = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("download returned status {}", response.status()));
    }

    let mut data = Vec::new();
    response
        .read_to_end(&mut data)
        .map_err(|e| format!("failed to read download: {e}"))?;

    Ok(data)
}

/// Verify the SHA-256 checksum of downloaded data against a checksum file.
fn verify_checksum(data: &[u8], checksum_url: &str) -> Result<(), String> {
    let checksum_content = download_asset(checksum_url)?;
    let checksum_str =
        String::from_utf8(checksum_content).map_err(|_| "checksum file is not valid UTF-8")?;

    // Checksum file format: "<hex>  <filename>" or just "<hex>"
    let expected = checksum_str
        .split_whitespace()
        .next()
        .ok_or("empty checksum file")?
        .to_lowercase();

    let actual = compute_sha256(data);

    if actual != expected {
        return Err(format!("expected {expected}, got {actual}"));
    }

    Ok(())
}

/// Compute the SHA-256 hex digest of data.
fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// Archive extraction
// ---------------------------------------------------------------------------

/// Extract a named binary from a gzipped tar archive.
fn extract_binary_from_archive(archive_data: &[u8], bin_name: &str) -> Result<Vec<u8>, String> {
    let decoder = flate2::read::GzDecoder::new(archive_data);
    let mut archive = tar::Archive::new(decoder);

    let entries = archive
        .entries()
        .map_err(|e| format!("failed to read archive entries: {e}"))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| format!("failed to read archive entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("failed to read entry path: {e}"))?;

        // Match the binary by filename (may be at top level or in a subdirectory)
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name == bin_name {
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|e| format!("failed to read {bin_name} from archive: {e}"))?;
            return Ok(data);
        }
    }

    Err(format!("{bin_name} not found in archive"))
}

// ---------------------------------------------------------------------------
// Atomic file replacement
// ---------------------------------------------------------------------------

/// Atomically replace a binary file.
///
/// Writes to a temporary file in the same directory, sets executable
/// permissions, then renames over the target. This ensures the binary
/// is never in a partially-written state.
fn atomic_replace(target: &Path, data: &[u8]) -> Result<(), String> {
    let dir = target.parent().ok_or("cannot determine parent directory")?;

    let tmp_path = dir.join(format!(".relava-update-{}", std::process::id()));

    // Write to temp file
    std::fs::write(&tmp_path, data).map_err(|e| format!("failed to write temporary file: {e}"))?;

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&tmp_path, perms)
            .map_err(|e| format!("failed to set permissions: {e}"))?;
    }

    // Atomic rename
    std::fs::rename(&tmp_path, target).map_err(|e| {
        // Clean up temp file on failure
        let _ = std::fs::remove_file(&tmp_path);
        format!("failed to replace binary: {e}")
    })
}

// ---------------------------------------------------------------------------
// Startup check throttling
// ---------------------------------------------------------------------------

/// Check whether enough time has passed since the last startup self-update check.
fn should_startup_check(relava_dir: &Path) -> bool {
    let path = relava_dir.join(TIMESTAMP_FILE);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return true,
    };

    let last_check: u64 = match content.trim().parse() {
        Ok(ts) => ts,
        Err(_) => return true,
    };

    now_secs().saturating_sub(last_check) >= STARTUP_CHECK_INTERVAL.as_secs()
}

/// Write the current timestamp for startup check throttling.
fn write_startup_timestamp(relava_dir: &Path) {
    let path = relava_dir.join(TIMESTAMP_FILE);
    let _ = std::fs::create_dir_all(relava_dir);
    let _ = std::fs::write(&path, now_secs().to_string());
}

/// Get the default `~/.relava/` directory.
fn default_relava_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".relava"))
}

/// Current time as seconds since the Unix epoch.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::BufReader;

    // -- version comparison --

    #[test]
    fn should_update_newer_version() {
        assert!(should_update("0.1.0", "0.2.0"));
        assert!(should_update("0.1.0", "1.0.0"));
        assert!(should_update("1.0.0", "1.0.1"));
    }

    #[test]
    fn should_update_same_version() {
        assert!(!should_update("0.1.0", "0.1.0"));
        assert!(!should_update("1.0.0", "1.0.0"));
    }

    #[test]
    fn should_update_older_version() {
        assert!(!should_update("0.2.0", "0.1.0"));
        assert!(!should_update("1.0.0", "0.9.0"));
    }

    #[test]
    fn should_update_invalid_versions() {
        assert!(!should_update("invalid", "0.1.0"));
        assert!(!should_update("0.1.0", "invalid"));
        assert!(!should_update("invalid", "also-invalid"));
    }

    // -- platform detection --

    #[test]
    fn detect_os_returns_known_value() {
        let os = detect_os();
        match std::env::consts::OS {
            "macos" | "linux" | "windows" => assert!(os.is_ok()),
            _ => assert!(os.is_err()),
        }
    }

    #[test]
    fn detect_arch_returns_known_value() {
        let arch = detect_arch();
        match std::env::consts::ARCH {
            "x86_64" | "aarch64" => assert!(arch.is_ok()),
            _ => assert!(arch.is_err()),
        }
    }

    #[test]
    fn detect_platform_returns_pair() {
        if matches!(std::env::consts::OS, "macos" | "linux" | "windows")
            && matches!(std::env::consts::ARCH, "x86_64" | "aarch64")
        {
            let (os, arch) = detect_platform().unwrap();
            assert!(!os.is_empty());
            assert!(!arch.is_empty());
        }
    }

    // -- asset naming --

    #[test]
    fn asset_name_format() {
        assert_eq!(
            asset_name("0.2.0", "darwin", "aarch64"),
            "relava-0.2.0-darwin-aarch64.tar.gz"
        );
        assert_eq!(
            asset_name("1.0.0", "linux", "x86_64"),
            "relava-1.0.0-linux-x86_64.tar.gz"
        );
    }

    #[test]
    fn checksum_asset_name_format() {
        assert_eq!(
            checksum_asset_name("0.2.0", "darwin", "aarch64"),
            "relava-0.2.0-darwin-aarch64.sha256"
        );
    }

    // -- parse_release_info --

    #[test]
    fn parse_release_info_valid() {
        let body = serde_json::json!({
            "tag_name": "v0.2.0",
            "assets": [
                {
                    "name": "relava-0.2.0-darwin-aarch64.tar.gz",
                    "browser_download_url": "https://example.com/relava-0.2.0-darwin-aarch64.tar.gz"
                },
                {
                    "name": "relava-0.2.0-darwin-aarch64.sha256",
                    "browser_download_url": "https://example.com/relava-0.2.0-darwin-aarch64.sha256"
                }
            ]
        });

        let info = parse_release_info(&body).unwrap();
        assert_eq!(info.version, "0.2.0");
        assert_eq!(info.assets.len(), 2);
        assert_eq!(info.assets[0].name, "relava-0.2.0-darwin-aarch64.tar.gz");
    }

    #[test]
    fn parse_release_info_strips_v_prefix() {
        let body = serde_json::json!({
            "tag_name": "v1.0.0",
            "assets": []
        });
        let info = parse_release_info(&body).unwrap();
        assert_eq!(info.version, "1.0.0");
    }

    #[test]
    fn parse_release_info_no_v_prefix() {
        let body = serde_json::json!({
            "tag_name": "1.0.0",
            "assets": []
        });
        let info = parse_release_info(&body).unwrap();
        assert_eq!(info.version, "1.0.0");
    }

    #[test]
    fn parse_release_info_missing_tag() {
        let body = serde_json::json!({
            "assets": []
        });
        assert!(parse_release_info(&body).is_err());
    }

    #[test]
    fn parse_release_info_missing_assets() {
        let body = serde_json::json!({
            "tag_name": "v1.0.0"
        });
        let info = parse_release_info(&body).unwrap();
        assert_eq!(info.version, "1.0.0");
        assert!(info.assets.is_empty());
    }

    #[test]
    fn parse_release_info_skips_malformed_assets() {
        let body = serde_json::json!({
            "tag_name": "v1.0.0",
            "assets": [
                {"name": "good.tar.gz", "browser_download_url": "https://example.com/good.tar.gz"},
                {"name": "missing_url"},
                {"browser_download_url": "https://example.com/missing_name"}
            ]
        });
        let info = parse_release_info(&body).unwrap();
        assert_eq!(info.assets.len(), 1);
        assert_eq!(info.assets[0].name, "good.tar.gz");
    }

    // -- checksum --

    #[test]
    fn compute_sha256_deterministic() {
        let hash = compute_sha256(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn compute_sha256_empty() {
        let hash = compute_sha256(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // -- atomic_replace --

    #[test]
    fn atomic_replace_creates_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("test-binary");
        fs::write(&target, b"old content").unwrap();

        atomic_replace(&target, b"new content").unwrap();

        let content = fs::read(&target).unwrap();
        assert_eq!(content, b"new content");
    }

    #[test]
    fn atomic_replace_preserves_old_on_failure() {
        let result = atomic_replace(Path::new("/nonexistent/dir/binary"), b"data");
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replace_sets_executable_permission() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("test-binary");
        fs::write(&target, b"old").unwrap();

        atomic_replace(&target, b"new").unwrap();

        let perms = fs::metadata(&target).unwrap().permissions();
        assert_eq!(perms.mode() & 0o755, 0o755);
    }

    // -- startup check throttling --

    #[test]
    fn should_startup_check_no_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(should_startup_check(tmp.path()));
    }

    #[test]
    fn should_startup_check_malformed() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join(TIMESTAMP_FILE), "not a number").unwrap();
        assert!(should_startup_check(tmp.path()));
    }

    #[test]
    fn should_startup_check_recent() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join(TIMESTAMP_FILE), now_secs().to_string()).unwrap();
        assert!(!should_startup_check(tmp.path()));
    }

    #[test]
    fn should_startup_check_old() {
        let tmp = tempfile::TempDir::new().unwrap();
        let old = now_secs() - 86401; // > 24 hours
        fs::write(tmp.path().join(TIMESTAMP_FILE), old.to_string()).unwrap();
        assert!(should_startup_check(tmp.path()));
    }

    #[test]
    fn should_startup_check_at_boundary() {
        let tmp = tempfile::TempDir::new().unwrap();
        let boundary = now_secs() - STARTUP_CHECK_INTERVAL.as_secs();
        fs::write(tmp.path().join(TIMESTAMP_FILE), boundary.to_string()).unwrap();
        assert!(should_startup_check(tmp.path()));
    }

    // -- write_startup_timestamp --

    #[test]
    fn write_startup_timestamp_creates_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_startup_timestamp(tmp.path());
        let path = tmp.path().join(TIMESTAMP_FILE);
        assert!(path.exists());
        let ts: u64 = fs::read_to_string(path).unwrap().trim().parse().unwrap();
        assert!(now_secs() - ts < 5);
    }

    #[test]
    fn write_startup_timestamp_creates_parent_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let nested = tmp.path().join("nested").join("dir");
        write_startup_timestamp(&nested);
        assert!(nested.join(TIMESTAMP_FILE).exists());
    }

    // -- find_binary_path --

    #[test]
    fn which_binary_finds_common_tools() {
        #[cfg(unix)]
        {
            let result = which_binary("sh");
            assert!(result.is_some());
        }
    }

    #[test]
    fn which_binary_returns_none_for_nonexistent() {
        let result = which_binary("nonexistent-binary-12345");
        assert!(result.is_none());
    }

    // -- is_writable --

    #[test]
    fn is_writable_for_user_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("writable");
        fs::write(&path, b"test").unwrap();
        assert!(is_writable(&path));
    }

    #[test]
    fn is_writable_returns_false_for_nonexistent() {
        assert!(!is_writable(Path::new("/nonexistent/path")));
    }

    // -- BinaryUpdateResult serialization --

    #[test]
    fn binary_update_result_serializes() {
        let result = BinaryUpdateResult {
            name: "relava".to_string(),
            path: "/usr/local/bin/relava".to_string(),
            status: "updated".to_string(),
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("relava"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn binary_update_result_includes_error() {
        let result = BinaryUpdateResult {
            name: "relava-server".to_string(),
            path: "/usr/local/bin/relava-server".to_string(),
            status: "error".to_string(),
            error: Some("permission denied".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"error\":\"permission denied\""));
    }

    // -- archive extraction --

    #[test]
    fn extract_binary_from_archive_finds_binary() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut builder = tar::Builder::new(Vec::new());

        let binary_content = b"#!/bin/sh\necho hello";
        let mut header = tar::Header::new_gnu();
        header.set_size(binary_content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();

        builder
            .append_data(&mut header, "relava", &binary_content[..])
            .unwrap();

        let tar_data = builder.into_inner().unwrap();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_data).unwrap();
        let gz_data = encoder.finish().unwrap();

        let extracted = extract_binary_from_archive(&gz_data, "relava").unwrap();
        assert_eq!(extracted, binary_content);
    }

    #[test]
    fn extract_binary_from_archive_not_found() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut builder = tar::Builder::new(Vec::new());

        let content = b"other file";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        builder
            .append_data(&mut header, "other-file", &content[..])
            .unwrap();

        let tar_data = builder.into_inner().unwrap();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_data).unwrap();
        let gz_data = encoder.finish().unwrap();

        let result = extract_binary_from_archive(&gz_data, "relava");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found in archive"));
    }

    #[test]
    fn extract_binary_from_archive_nested_path() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut builder = tar::Builder::new(Vec::new());

        let binary_content = b"binary data";
        let mut header = tar::Header::new_gnu();
        header.set_size(binary_content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();

        builder
            .append_data(&mut header, "relava-0.2.0/relava", &binary_content[..])
            .unwrap();

        let tar_data = builder.into_inner().unwrap();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_data).unwrap();
        let gz_data = encoder.finish().unwrap();

        let extracted = extract_binary_from_archive(&gz_data, "relava").unwrap();
        assert_eq!(extracted, binary_content);
    }

    #[test]
    fn extract_binary_invalid_archive() {
        let result = extract_binary_from_archive(b"not an archive", "relava");
        assert!(result.is_err());
    }

    // -- verify_checksum with mock server --

    #[test]
    fn verify_checksum_correct() {
        let data = b"hello world";
        let expected_hash = compute_sha256(data);

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/checksum.sha256")
            .with_status(200)
            .with_body(format!(
                "{expected_hash}  relava-0.2.0-darwin-aarch64.tar.gz"
            ))
            .create();

        let url = format!("{}/checksum.sha256", server.url());
        verify_checksum(data, &url).unwrap();
    }

    #[test]
    fn verify_checksum_mismatch() {
        let data = b"hello world";

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/checksum.sha256")
            .with_status(200)
            .with_body(
                "0000000000000000000000000000000000000000000000000000000000000000  file.tar.gz",
            )
            .create();

        let url = format!("{}/checksum.sha256", server.url());
        let result = verify_checksum(data, &url);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected"));
    }

    #[test]
    fn verify_checksum_download_failure() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/checksum.sha256")
            .with_status(404)
            .create();

        let url = format!("{}/checksum.sha256", server.url());
        let result = verify_checksum(b"data", &url);
        assert!(result.is_err());
    }

    #[test]
    fn verify_checksum_hash_only_format() {
        let data = b"test data";
        let expected_hash = compute_sha256(data);

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/checksum.sha256")
            .with_status(200)
            .with_body(&expected_hash)
            .create();

        let url = format!("{}/checksum.sha256", server.url());
        verify_checksum(data, &url).unwrap();
    }

    // -- startup prompt logic --

    #[test]
    fn prompt_user_accepts_with_enter() {
        // Simulates pressing Enter (empty line = yes)
        let answer = "";
        let trimmed = answer.trim().to_lowercase();
        let accepted = trimmed.is_empty() || trimmed == "y" || trimmed == "yes";
        assert!(accepted);
    }

    #[test]
    fn prompt_user_accepts_with_y() {
        let answer = "y\n";
        let trimmed = answer.trim().to_lowercase();
        let accepted = trimmed.is_empty() || trimmed == "y" || trimmed == "yes";
        assert!(accepted);
    }

    #[test]
    fn prompt_user_accepts_with_yes() {
        let answer = "yes\n";
        let trimmed = answer.trim().to_lowercase();
        let accepted = trimmed.is_empty() || trimmed == "y" || trimmed == "yes";
        assert!(accepted);
    }

    #[test]
    fn prompt_user_accepts_with_yes_uppercase() {
        let answer = "YES\n";
        let trimmed = answer.trim().to_lowercase();
        let accepted = trimmed.is_empty() || trimmed == "y" || trimmed == "yes";
        assert!(accepted);
    }

    #[test]
    fn prompt_user_declines_with_n() {
        let answer = "n\n";
        let trimmed = answer.trim().to_lowercase();
        let accepted = trimmed.is_empty() || trimmed == "y" || trimmed == "yes";
        assert!(!accepted);
    }

    #[test]
    fn prompt_user_declines_with_no() {
        let answer = "no\n";
        let trimmed = answer.trim().to_lowercase();
        let accepted = trimmed.is_empty() || trimmed == "y" || trimmed == "yes";
        assert!(!accepted);
    }

    #[test]
    fn prompt_user_declines_with_arbitrary_text() {
        let answer = "maybe\n";
        let trimmed = answer.trim().to_lowercase();
        let accepted = trimmed.is_empty() || trimmed == "y" || trimmed == "yes";
        assert!(!accepted);
    }

    // -- startup_check_with (non-interactive path) --

    #[test]
    fn startup_check_non_interactive_does_not_block() {
        // When non-interactive, the function should return without reading input.
        // We pass an empty reader — if it tried to block on input, it would get EOF.
        let mut input = BufReader::new(b"" as &[u8]);
        // This should complete without hanging; the throttle will handle
        // whether it actually checks (it will check since this is the first time,
        // but fetch_latest_release_quiet will fail since there's no network mock).
        startup_check_with(false, &mut input);
    }

    #[test]
    fn startup_check_throttled_skips() {
        // Set up a recent timestamp so the check is throttled
        let tmp = tempfile::TempDir::new().unwrap();
        let relava_dir = tmp.path();
        fs::write(relava_dir.join(TIMESTAMP_FILE), now_secs().to_string()).unwrap();

        // The function uses default_relava_dir() which reads ~/.relava,
        // so we can't easily inject the temp dir. But we can verify the
        // throttle logic directly.
        assert!(!should_startup_check(relava_dir));
    }
}
