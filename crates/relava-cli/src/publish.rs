//! `relava publish <type> <name>` — validate, hash, and upload a resource.
//!
//! Reads resource files from the default location (or `--path`), runs
//! client-side validation, computes SHA-256 per file, and uploads via
//! multipart POST to the server.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::api_client::ApiClient;
use crate::output::Tag;
use crate::validate;
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
}

/// Result of a successful publish.
#[derive(Debug, Serialize)]
pub struct PublishResult {
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub files: usize,
    pub total_bytes: u64,
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

    // 3. Build metadata JSON
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

    // 4. Build multipart form and upload
    if !opts.json {
        println!();
        println!(
            "Uploading {} file{}...",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }

    let client = ApiClient::new(opts.server_url);
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
fn collect_files(root: &Path) -> Result<Vec<FileEntry>, String> {
    let paths = collect_file_paths(root)?;
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

        // Skip hidden files/dirs
        if name_str.starts_with('.') {
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
}
