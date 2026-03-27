//! `relava publish <type> <name>` — validate, hash, and upload a resource.
//!
//! Reads resource files from the default location (or `--path`), runs
//! client-side validation, computes SHA-256 per file, compares against the
//! latest published version for change detection, and uploads via multipart
//! POST to the server.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::api_client::{ApiClient, ApiError};
use crate::output::Tag;
use crate::validate;
use relava_types::file_filter::{self, IgnorePatterns, RELAVAIGNORE_FILE};
use relava_types::validate::ResourceType;

// ---------------------------------------------------------------------------
// Constants (shared with validate.rs)
// ---------------------------------------------------------------------------

const MAX_FILE_COUNT: usize = 100;
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const MAX_TOTAL_SIZE: u64 = 50 * 1024 * 1024; // 50 MB

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Options for the publish command.
pub struct PublishOpts<'a> {
    pub server_url: &'a str,
    pub resource_type: ResourceType,
    pub name: &'a str,
    pub path: Option<&'a Path>,
    pub json: bool,
    pub verbose: bool,
    /// Skip change detection and publish regardless.
    pub force: bool,
    /// Auto-confirm the publish prompt (non-interactive).
    pub yes: bool,
}

/// Result of a successful publish.
#[derive(Debug, Serialize)]
pub struct PublishResult {
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub files: usize,
    pub total_bytes: u64,
    /// `true` if publish was skipped because no changes were detected.
    pub skipped: bool,
}

// ---------------------------------------------------------------------------
// File metadata
// ---------------------------------------------------------------------------

/// Metadata for a single file being published.
#[derive(Debug, Serialize)]
pub struct FileEntry {
    /// Path relative to the resource root (e.g. "SKILL.md" or "lib/utils.md").
    pub relative_path: String,
    /// SHA-256 hex digest of the file contents.
    pub sha256: String,
    /// File size in bytes.
    pub size: u64,
}

// ---------------------------------------------------------------------------
// Change detection
// ---------------------------------------------------------------------------

/// Category of change for a file between local and registry versions.
///
/// Variant declaration order matches the desired display order:
/// added → modified → removed. The derived `Ord` relies on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ChangeKind {
    Added,
    Modified,
    Removed,
}

impl std::fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeKind::Added => write!(f, "added"),
            ChangeKind::Modified => write!(f, "modified"),
            ChangeKind::Removed => write!(f, "removed"),
        }
    }
}

/// A single file change detected between local and registry.
#[derive(Debug)]
struct FileChange {
    path: String,
    kind: ChangeKind,
}

/// Compare local files against registry checksums.
///
/// Returns a list of changes (added, modified, removed).
fn detect_changes(
    local_files: &[FileEntry],
    registry_checksums: &HashMap<String, String>,
) -> Vec<FileChange> {
    let mut changes = Vec::new();

    // Check local files against registry
    for entry in local_files {
        match registry_checksums.get(&entry.relative_path) {
            None => changes.push(FileChange {
                path: entry.relative_path.clone(),
                kind: ChangeKind::Added,
            }),
            Some(registry_sha) if registry_sha != &entry.sha256 => {
                changes.push(FileChange {
                    path: entry.relative_path.clone(),
                    kind: ChangeKind::Modified,
                });
            }
            _ => {} // unchanged
        }
    }

    // Check for removed files (in registry but not local)
    let local_paths: std::collections::HashSet<&str> = local_files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    for registry_path in registry_checksums.keys() {
        if !local_paths.contains(registry_path.as_str()) {
            changes.push(FileChange {
                path: registry_path.clone(),
                kind: ChangeKind::Removed,
            });
        }
    }

    // Sort for deterministic output: added, modified, removed, then by path.
    // ChangeKind's derived Ord matches this order.
    changes.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.path.cmp(&b.path)));

    changes
}

/// Print a diff summary to stdout.
fn print_diff_summary(changes: &[FileChange]) {
    for change in changes {
        println!("  [{:<8}] {}", change.kind, change.path);
    }
}

/// Prompt the user for confirmation. Returns `true` if confirmed.
///
/// Propagates I/O errors instead of silently treating them as "no".
fn prompt_confirm(message: &str) -> Result<bool, String> {
    print!("{message} ");
    io::stdout()
        .flush()
        .map_err(|e| format!("failed to write to stdout: {e}"))?;
    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("failed to read from stdin: {e}"))?;
    let answer = line.trim().to_lowercase();
    Ok(answer.is_empty() || answer == "y" || answer == "yes")
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run `relava publish <type> <name>`.
pub fn run(opts: &PublishOpts) -> Result<PublishResult, String> {
    let resource_dir = resolve_resource_dir(opts)?;

    // 1. Client-side validation (reuse validate module)
    if !opts.json {
        println!("Validating {} {}...", opts.resource_type, opts.name);
    }

    let validate_opts = validate::ValidateOpts {
        server_url: opts.server_url,
        resource_type: opts.resource_type,
        path: &resource_dir,
        json: opts.json,
        _verbose: opts.verbose,
    };

    let validation = validate::run(&validate_opts)?;
    if !validation.is_valid() {
        return Err(format!(
            "Validation failed with {} error{}. Fix issues before publishing.",
            validation.failures,
            if validation.failures == 1 { "" } else { "s" }
        ));
    }

    // 2. Collect files with hashes and enforce limits
    if !opts.json {
        println!();
        println!("Preparing upload...");
    }

    let files = collect_files(&resource_dir)?;
    enforce_limits(&files)?;

    // 3. Change detection (unless --force)
    let client = ApiClient::new(opts.server_url);

    if !opts.force
        && let Some(skipped) = run_change_detection(&client, opts, &files)?
    {
        return Ok(skipped);
    }

    // 4. Print file listing
    if !opts.json {
        for entry in &files {
            println!(
                "{}",
                Tag::Ok.fmt(&format!(
                    "{} ({})",
                    entry.relative_path,
                    format_size(entry.size)
                ))
            );
        }
    }

    // 5. Build metadata JSON
    let checksums: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "path": f.relative_path,
                "sha256": f.sha256,
                "size": f.size,
            })
        })
        .collect();

    let metadata = serde_json::json!({
        "files": checksums,
    });

    // 6. Build multipart form and upload
    if !opts.json {
        println!();
        println!(
            "Uploading {} file{}...",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }

    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    let response = client
        .publish(
            opts.resource_type,
            opts.name,
            &resource_dir,
            &files,
            &metadata,
        )
        .map_err(|e| e.to_string())?;

    let result = PublishResult {
        resource_type: opts.resource_type.to_string(),
        name: opts.name.to_string(),
        version: response.version,
        files: files.len(),
        total_bytes,
        skipped: false,
    };

    if !opts.json {
        println!();
        println!(
            "{}",
            Tag::Ok.fmt(&format!(
                "Published {} {} v{}",
                result.resource_type, result.name, result.version
            ))
        );
    }

    Ok(result)
}

/// Run change detection against the latest published version.
///
/// Returns `Some(PublishResult)` if publish should be skipped (no changes),
/// or `None` if changes exist and the user confirmed (or `--yes` was set).
/// Returns `Err` if the user declined or an error occurred.
fn run_change_detection(
    client: &ApiClient,
    opts: &PublishOpts,
    files: &[FileEntry],
) -> Result<Option<PublishResult>, String> {
    let type_str = opts.resource_type.to_string();

    // Look up the resource to get latest_version
    let resource = match client.get_resource(&type_str, opts.name) {
        Ok(r) => r,
        Err(ApiError::NotFound(_)) => {
            // First publish — no previous version to compare against
            if !opts.json {
                println!("First publish — no previous version to compare.");
            }
            return Ok(None);
        }
        Err(e) => return Err(e.to_string()),
    };

    let latest_version = match resource.latest_version {
        Some(v) => v,
        None => {
            // Resource exists but has no published versions
            if !opts.json {
                println!("No published versions found. Publishing as new.");
            }
            return Ok(None);
        }
    };

    if !opts.json {
        println!(
            "Comparing {} {} against registry version {}...",
            opts.resource_type, opts.name, latest_version
        );
    }

    // Fetch per-file checksums from the registry
    let checksums_response =
        match client.get_version_checksums(&type_str, opts.name, &latest_version) {
            Ok(r) => r,
            Err(ApiError::NotFound(_)) => {
                // 404 means either: (a) legacy version without checksums, or
                // (b) version was deleted between listing and checksum fetch (TOCTOU).
                // Either way, we cannot compare — proceed with publish.
                if !opts.json {
                    println!(
                        "No checksums available for v{latest_version} \
                         (legacy version or version no longer exists). \
                         Proceeding with publish."
                    );
                }
                return Ok(None);
            }
            Err(e) => return Err(e.to_string()),
        };

    // Build registry checksum map
    let registry_checksums: HashMap<String, String> = checksums_response
        .files
        .into_iter()
        .map(|f| (f.path, f.sha256))
        .collect();

    // Empty checksums response means legacy version without per-file checksums
    if registry_checksums.is_empty() {
        if !opts.json {
            println!("No checksums available for comparison. Proceeding with publish.");
        }
        return Ok(None);
    }

    let changes = detect_changes(files, &registry_checksums);

    if changes.is_empty() {
        // No changes detected — skip publish
        if !opts.json {
            println!("No changes detected. Nothing to publish.");
        }
        return Ok(Some(PublishResult {
            resource_type: type_str,
            name: opts.name.to_string(),
            version: latest_version,
            files: files.len(),
            total_bytes: files.iter().map(|f| f.size).sum(),
            skipped: true,
        }));
    }

    // Changes detected — show diff summary
    if !opts.json {
        print_diff_summary(&changes);
    }

    // Prompt for confirmation (unless --yes or --json)
    if !opts.yes && !opts.json && !prompt_confirm("Publish? [Y/n]")? {
        return Err("Publish cancelled.".to_string());
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the resource directory from `--path` or the default location.
fn resolve_resource_dir(opts: &PublishOpts) -> Result<PathBuf, String> {
    if let Some(path) = opts.path {
        return path
            .canonicalize()
            .map_err(|e| format!("cannot access '{}': {e}", path.display()));
    }

    // Default: .claude/<type_dir>/<name> relative to cwd
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot determine current directory: {e}"))?;
    let type_dir = match opts.resource_type {
        ResourceType::Skill => "skills",
        ResourceType::Agent => "agents",
        ResourceType::Command => "commands",
        ResourceType::Rule => "rules",
    };
    let path = cwd.join(".claude").join(type_dir).join(opts.name);
    if !path.exists() {
        return Err(format!(
            "Resource directory not found: {}\nUse --path to specify a custom location.",
            path.display()
        ));
    }
    path.canonicalize()
        .map_err(|e| format!("cannot access '{}': {e}", path.display()))
}

/// Collect all files under the resource directory, computing SHA-256 hashes.
///
/// Loads `.relavaignore` from the resource root (if present) and excludes
/// matching files. The `.relavaignore` file itself is always included.
fn collect_files(root: &Path) -> Result<Vec<FileEntry>, String> {
    let ignore = IgnorePatterns::load(root)?;
    let paths = collect_file_paths(root)?;
    let paths = file_filter::filter_ignored(root, paths, &ignore);
    let mut entries = Vec::new();

    for path in paths {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        let data =
            std::fs::read(&path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;

        let sha256 = format!("{:x}", Sha256::digest(&data));

        entries.push(FileEntry {
            relative_path: relative,
            sha256,
            size: data.len() as u64,
        });
    }

    Ok(entries)
}

/// Recursively collect all file paths under a directory, skipping hidden files.
fn collect_file_paths(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut files = Vec::new();
    collect_recursive(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read directory '{}': {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("directory read error: {e}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden files/dirs, but allow .relavaignore through
        if name_str.starts_with('.') && name_str != RELAVAIGNORE_FILE {
            continue;
        }

        if path.is_dir() {
            collect_recursive(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// Enforce file limits: count, per-file size, total size.
fn enforce_limits(files: &[FileEntry]) -> Result<(), String> {
    if files.is_empty() {
        return Err("no files found to publish".to_string());
    }

    if files.len() > MAX_FILE_COUNT {
        return Err(format!(
            "too many files: {} (max {})",
            files.len(),
            MAX_FILE_COUNT
        ));
    }

    for f in files {
        if f.size > MAX_FILE_SIZE {
            return Err(format!(
                "file '{}' exceeds 10 MB limit ({})",
                f.relative_path,
                format_size(f.size)
            ));
        }
    }

    let total: u64 = files.iter().map(|f| f.size).sum();
    if total > MAX_TOTAL_SIZE {
        return Err(format!(
            "total size {} exceeds 50 MB limit",
            format_size(total)
        ));
    }

    Ok(())
}

/// Format a byte count for display.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("relava-publish-test-{}-{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collect_files_computes_sha256() {
        let dir = temp_dir();
        fs::write(dir.join("SKILL.md"), "hello world").unwrap();

        let files = collect_files(&dir).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "SKILL.md");
        assert!(!files[0].sha256.is_empty());
        assert_eq!(files[0].size, 11);

        // Verify SHA-256 is correct
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(files[0].sha256, expected);
    }

    #[test]
    fn collect_files_skips_hidden() {
        let dir = temp_dir();
        fs::write(dir.join("SKILL.md"), "visible").unwrap();
        fs::write(dir.join(".hidden"), "hidden").unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git/config"), "git stuff").unwrap();

        let files = collect_files(&dir).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "SKILL.md");
    }

    #[test]
    fn collect_files_recursive() {
        let dir = temp_dir();
        fs::write(dir.join("SKILL.md"), "root").unwrap();
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("lib/utils.md"), "util").unwrap();

        let files = collect_files(&dir).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn enforce_limits_empty_files() {
        let files: Vec<FileEntry> = vec![];
        let result = enforce_limits(&files);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no files found"));
    }

    #[test]
    fn enforce_limits_too_many_files() {
        let files: Vec<FileEntry> = (0..101)
            .map(|i| FileEntry {
                relative_path: format!("file{i}.md"),
                sha256: "abc".to_string(),
                size: 100,
            })
            .collect();
        let result = enforce_limits(&files);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too many files"));
    }

    #[test]
    fn enforce_limits_file_too_large() {
        let files = vec![FileEntry {
            relative_path: "huge.md".to_string(),
            sha256: "abc".to_string(),
            size: MAX_FILE_SIZE + 1,
        }];
        let result = enforce_limits(&files);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds 10 MB"));
    }

    #[test]
    fn enforce_limits_total_too_large() {
        // 6 files × 9 MB each = 54 MB > 50 MB
        let files: Vec<FileEntry> = (0..6)
            .map(|i| FileEntry {
                relative_path: format!("file{i}.md"),
                sha256: "abc".to_string(),
                size: 9 * 1024 * 1024,
            })
            .collect();
        let result = enforce_limits(&files);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds 50 MB"));
    }

    #[test]
    fn enforce_limits_valid() {
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc".to_string(),
            size: 1024,
        }];
        assert!(enforce_limits(&files).is_ok());
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(500), "500 B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(2048), "2.0 KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }

    // -- .relavaignore tests --

    #[test]
    fn collect_files_applies_relavaignore() {
        let dir = temp_dir();
        fs::write(dir.join("SKILL.md"), "skill content").unwrap();
        fs::write(dir.join("notes.tmp"), "temp file").unwrap();
        fs::write(dir.join(".relavaignore"), "*.tmp\n").unwrap();

        let files = collect_files(&dir).unwrap();
        let names: Vec<_> = files.iter().map(|f| f.relative_path.as_str()).collect();
        assert!(names.contains(&"SKILL.md"));
        assert!(!names.contains(&"notes.tmp"));
    }

    #[test]
    fn collect_files_includes_relavaignore_itself() {
        let dir = temp_dir();
        fs::write(dir.join("SKILL.md"), "content").unwrap();
        fs::write(dir.join(".relavaignore"), "*.tmp\n").unwrap();

        let files = collect_files(&dir).unwrap();
        let names: Vec<_> = files.iter().map(|f| f.relative_path.as_str()).collect();
        assert!(names.contains(&".relavaignore"));
    }

    #[test]
    fn collect_files_no_relavaignore_includes_all() {
        let dir = temp_dir();
        fs::write(dir.join("SKILL.md"), "content").unwrap();
        fs::write(dir.join("extra.txt"), "extra").unwrap();

        let files = collect_files(&dir).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn collect_files_relavaignore_excludes_directory() {
        let dir = temp_dir();
        fs::write(dir.join("SKILL.md"), "content").unwrap();
        fs::create_dir_all(dir.join("build")).unwrap();
        fs::write(dir.join("build/output.bin"), "binary").unwrap();
        fs::write(dir.join(".relavaignore"), "build/\n").unwrap();

        let files = collect_files(&dir).unwrap();
        let names: Vec<_> = files.iter().map(|f| f.relative_path.as_str()).collect();
        assert!(names.contains(&"SKILL.md"));
        assert!(names.contains(&".relavaignore"));
        assert!(!names.contains(&"build/output.bin"));
    }

    // -- Change detection tests --

    #[test]
    fn detect_changes_no_changes() {
        let files = vec![
            FileEntry {
                relative_path: "SKILL.md".to_string(),
                sha256: "abc123".to_string(),
                size: 100,
            },
            FileEntry {
                relative_path: "lib/utils.md".to_string(),
                sha256: "def456".to_string(),
                size: 50,
            },
        ];
        let registry: HashMap<String, String> = [
            ("SKILL.md".to_string(), "abc123".to_string()),
            ("lib/utils.md".to_string(), "def456".to_string()),
        ]
        .into();

        let changes = detect_changes(&files, &registry);
        assert!(changes.is_empty());
    }

    #[test]
    fn detect_changes_added_file() {
        let files = vec![
            FileEntry {
                relative_path: "SKILL.md".to_string(),
                sha256: "abc123".to_string(),
                size: 100,
            },
            FileEntry {
                relative_path: "new-file.md".to_string(),
                sha256: "new456".to_string(),
                size: 50,
            },
        ];
        let registry: HashMap<String, String> =
            [("SKILL.md".to_string(), "abc123".to_string())].into();

        let changes = detect_changes(&files, &registry);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "new-file.md");
        assert_eq!(changes[0].kind, ChangeKind::Added);
    }

    #[test]
    fn detect_changes_modified_file() {
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "new-hash".to_string(),
            size: 200,
        }];
        let registry: HashMap<String, String> =
            [("SKILL.md".to_string(), "old-hash".to_string())].into();

        let changes = detect_changes(&files, &registry);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "SKILL.md");
        assert_eq!(changes[0].kind, ChangeKind::Modified);
    }

    #[test]
    fn detect_changes_removed_file() {
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc123".to_string(),
            size: 100,
        }];
        let registry: HashMap<String, String> = [
            ("SKILL.md".to_string(), "abc123".to_string()),
            ("old-file.md".to_string(), "old456".to_string()),
        ]
        .into();

        let changes = detect_changes(&files, &registry);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "old-file.md");
        assert_eq!(changes[0].kind, ChangeKind::Removed);
    }

    #[test]
    fn detect_changes_mixed() {
        let files = vec![
            FileEntry {
                relative_path: "SKILL.md".to_string(),
                sha256: "modified-hash".to_string(),
                size: 200,
            },
            FileEntry {
                relative_path: "new.md".to_string(),
                sha256: "new-hash".to_string(),
                size: 50,
            },
        ];
        let registry: HashMap<String, String> = [
            ("SKILL.md".to_string(), "old-hash".to_string()),
            ("removed.md".to_string(), "removed-hash".to_string()),
        ]
        .into();

        let changes = detect_changes(&files, &registry);
        assert_eq!(changes.len(), 3);
        // Sorted order: added, modified, removed
        assert_eq!(changes[0].kind, ChangeKind::Added);
        assert_eq!(changes[0].path, "new.md");
        assert_eq!(changes[1].kind, ChangeKind::Modified);
        assert_eq!(changes[1].path, "SKILL.md");
        assert_eq!(changes[2].kind, ChangeKind::Removed);
        assert_eq!(changes[2].path, "removed.md");
    }

    #[test]
    fn detect_changes_empty_local_all_removed() {
        let files: Vec<FileEntry> = vec![];
        let registry: HashMap<String, String> =
            [("SKILL.md".to_string(), "abc123".to_string())].into();

        let changes = detect_changes(&files, &registry);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Removed);
    }

    #[test]
    fn detect_changes_empty_registry_all_added() {
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc123".to_string(),
            size: 100,
        }];
        let registry: HashMap<String, String> = HashMap::new();

        let changes = detect_changes(&files, &registry);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Added);
    }

    #[test]
    fn detect_changes_sorts_by_path_within_same_kind() {
        let files = vec![
            FileEntry {
                relative_path: "z-file.md".to_string(),
                sha256: "new1".to_string(),
                size: 10,
            },
            FileEntry {
                relative_path: "a-file.md".to_string(),
                sha256: "new2".to_string(),
                size: 10,
            },
        ];
        let registry: HashMap<String, String> = HashMap::new();

        let changes = detect_changes(&files, &registry);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, "a-file.md");
        assert_eq!(changes[1].path, "z-file.md");
    }

    // -- run_change_detection integration tests (mockito) --

    fn make_opts(server_url: &str, force: bool, yes: bool) -> PublishOpts<'_> {
        PublishOpts {
            server_url,
            resource_type: ResourceType::Skill,
            name: "denden",
            path: None,
            json: true, // json mode avoids stdout noise and skips prompt
            verbose: false,
            force,
            yes,
        }
    }

    #[test]
    fn change_detection_first_publish_returns_none() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/api/v1/resources/skill/denden")
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();

        let url = server.url();
        let client = crate::api_client::ApiClient::new(&url);
        let opts = make_opts(&url, false, false);
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc".to_string(),
            size: 100,
        }];

        let result = run_change_detection(&client, &opts, &files).unwrap();
        assert!(result.is_none(), "first publish should proceed (None)");
    }

    #[test]
    fn change_detection_resource_exists_no_versions() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/api/v1/resources/skill/denden")
            .with_status(200)
            .with_body(r#"{"name":"denden","type":"skill"}"#)
            .create();

        let url = server.url();
        let client = crate::api_client::ApiClient::new(&url);
        let opts = make_opts(&url, false, false);
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc".to_string(),
            size: 100,
        }];

        let result = run_change_detection(&client, &opts, &files).unwrap();
        assert!(result.is_none(), "no versions should proceed (None)");
    }

    #[test]
    fn change_detection_legacy_version_no_checksums_endpoint() {
        let mut server = mockito::Server::new();
        let _mock_resource = server
            .mock("GET", "/api/v1/resources/skill/denden")
            .with_status(200)
            .with_body(r#"{"name":"denden","type":"skill","latest_version":"0.1.0"}"#)
            .create();
        let _mock_checksums = server
            .mock(
                "GET",
                "/api/v1/resources/skill/denden/versions/0.1.0/checksums",
            )
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();

        let url = server.url();
        let client = crate::api_client::ApiClient::new(&url);
        let opts = make_opts(&url, false, false);
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc".to_string(),
            size: 100,
        }];

        let result = run_change_detection(&client, &opts, &files).unwrap();
        assert!(result.is_none(), "legacy version should proceed (None)");
    }

    #[test]
    fn change_detection_empty_checksums_proceeds() {
        let mut server = mockito::Server::new();
        let _mock_resource = server
            .mock("GET", "/api/v1/resources/skill/denden")
            .with_status(200)
            .with_body(r#"{"name":"denden","type":"skill","latest_version":"0.1.0"}"#)
            .create();
        let _mock_checksums = server
            .mock(
                "GET",
                "/api/v1/resources/skill/denden/versions/0.1.0/checksums",
            )
            .with_status(200)
            .with_body(r#"{"version":"0.1.0","files":[]}"#)
            .create();

        let url = server.url();
        let client = crate::api_client::ApiClient::new(&url);
        let opts = make_opts(&url, false, false);
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc".to_string(),
            size: 100,
        }];

        let result = run_change_detection(&client, &opts, &files).unwrap();
        assert!(result.is_none(), "empty checksums should proceed (None)");
    }

    #[test]
    fn change_detection_no_changes_returns_skipped() {
        let mut server = mockito::Server::new();
        let _mock_resource = server
            .mock("GET", "/api/v1/resources/skill/denden")
            .with_status(200)
            .with_body(r#"{"name":"denden","type":"skill","latest_version":"1.0.0"}"#)
            .create();
        let _mock_checksums = server
            .mock(
                "GET",
                "/api/v1/resources/skill/denden/versions/1.0.0/checksums",
            )
            .with_status(200)
            .with_body(r#"{"version":"1.0.0","files":[{"path":"SKILL.md","sha256":"abc123"}]}"#)
            .create();

        let url = server.url();
        let client = crate::api_client::ApiClient::new(&url);
        let opts = make_opts(&url, false, false);
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc123".to_string(),
            size: 100,
        }];

        let result = run_change_detection(&client, &opts, &files).unwrap();
        let skipped = result.expect("no changes should return Some(skipped)");
        assert!(skipped.skipped, "skipped flag should be true");
        assert_eq!(skipped.version, "1.0.0");
        assert_eq!(skipped.files, 1);
    }

    #[test]
    fn change_detection_with_changes_and_json_mode_proceeds() {
        // json mode skips prompt, so changes should proceed
        let mut server = mockito::Server::new();
        let _mock_resource = server
            .mock("GET", "/api/v1/resources/skill/denden")
            .with_status(200)
            .with_body(r#"{"name":"denden","type":"skill","latest_version":"1.0.0"}"#)
            .create();
        let _mock_checksums = server
            .mock(
                "GET",
                "/api/v1/resources/skill/denden/versions/1.0.0/checksums",
            )
            .with_status(200)
            .with_body(r#"{"version":"1.0.0","files":[{"path":"SKILL.md","sha256":"old-hash"}]}"#)
            .create();

        let url = server.url();
        let client = crate::api_client::ApiClient::new(&url);
        let opts = make_opts(&url, false, false); // json=true in make_opts
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "new-hash".to_string(),
            size: 200,
        }];

        let result = run_change_detection(&client, &opts, &files).unwrap();
        assert!(
            result.is_none(),
            "changes in json mode should proceed (None)"
        );
    }

    #[test]
    fn change_detection_yes_flag_auto_confirms() {
        // --yes flag (with json=false) should auto-confirm without blocking on stdin
        let mut server = mockito::Server::new();
        let _mock_resource = server
            .mock("GET", "/api/v1/resources/skill/denden")
            .with_status(200)
            .with_body(r#"{"name":"denden","type":"skill","latest_version":"1.0.0"}"#)
            .create();
        let _mock_checksums = server
            .mock(
                "GET",
                "/api/v1/resources/skill/denden/versions/1.0.0/checksums",
            )
            .with_status(200)
            .with_body(r#"{"version":"1.0.0","files":[{"path":"SKILL.md","sha256":"old-hash"}]}"#)
            .create();

        let url = server.url();
        let client = crate::api_client::ApiClient::new(&url);
        let opts = PublishOpts {
            server_url: &url,
            resource_type: ResourceType::Skill,
            name: "denden",
            path: None,
            json: false, // not json mode — exercises the --yes path specifically
            verbose: false,
            force: false,
            yes: true, // --yes flag auto-confirms
        };
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "new-hash".to_string(),
            size: 200,
        }];

        let result = run_change_detection(&client, &opts, &files).unwrap();
        assert!(
            result.is_none(),
            "--yes flag should auto-confirm and proceed (None)"
        );
    }

    #[test]
    fn change_detection_server_error_propagates() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/api/v1/resources/skill/denden")
            .with_status(500)
            .with_body(r#"{"error":"internal server error"}"#)
            .create();

        let url = server.url();
        let client = crate::api_client::ApiClient::new(&url);
        let opts = make_opts(&url, false, false);
        let files = vec![FileEntry {
            relative_path: "SKILL.md".to_string(),
            sha256: "abc".to_string(),
            size: 100,
        }];

        let result = run_change_detection(&client, &opts, &files);
        assert!(result.is_err(), "server error should propagate");
    }
}
