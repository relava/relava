//! Cache management commands: `relava cache clean` and `relava cache status`.
//!
//! Operates on the download cache at `~/.relava/cache/`. Never touches the
//! store directory or installed resource locations.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::output::Tag;

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Options for `relava cache clean`.
pub struct CacheCleanOpts<'a> {
    pub cache_dir: &'a Path,
    pub older_than: Option<Duration>,
    pub json: bool,
}

/// Options for `relava cache status`.
pub struct CacheStatusOpts<'a> {
    pub cache_dir: &'a Path,
    pub json: bool,
}

/// Options for automatic eviction.
#[allow(dead_code)]
pub struct EvictionOpts<'a> {
    pub cache_dir: &'a Path,
    pub max_bytes: u64,
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of `relava cache clean`.
#[derive(Debug, serde::Serialize)]
pub struct CacheCleanResult {
    /// Number of entries (version directories) removed.
    pub removed: usize,
    /// Total bytes freed.
    pub bytes_freed: u64,
    /// Human-readable size freed.
    pub size_freed: String,
    /// Entries that were removed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<RemovedEntry>,
}

/// A single removed cache entry.
#[derive(Debug, serde::Serialize)]
pub struct RemovedEntry {
    pub resource_type: String,
    pub name: String,
    pub version: String,
}

/// Result of `relava cache status`.
#[derive(Debug, serde::Serialize)]
pub struct CacheStatusResult {
    /// Total size of the cache in bytes.
    pub total_bytes: u64,
    /// Human-readable total size.
    pub total_size: String,
    /// Number of cached entries (version directories).
    pub entry_count: usize,
    /// Oldest entry age as human-readable string, or null if empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_entry: Option<String>,
    /// Per-entry breakdown.
    pub entries: Vec<CacheEntry>,
}

/// A single cache entry for status reporting.
#[derive(Debug, serde::Serialize)]
pub struct CacheEntry {
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub bytes: u64,
    pub size: String,
    pub age: String,
}

/// Result of automatic eviction.
#[derive(Debug, serde::Serialize)]
#[allow(dead_code)]
pub struct EvictionResult {
    pub evicted: usize,
    pub bytes_freed: u64,
}

// ---------------------------------------------------------------------------
// Internal: cache entry with metadata
// ---------------------------------------------------------------------------

/// A discovered cache entry with its path and metadata.
struct EntryInfo {
    resource_type: String,
    name: String,
    version: String,
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run `relava cache status`.
pub fn status(opts: &CacheStatusOpts) -> CacheStatusResult {
    let entries = discover_entries(opts.cache_dir);
    let now = SystemTime::now();

    let total_bytes: u64 = entries.iter().map(|e| e.bytes).sum();
    let entry_count = entries.len();

    let oldest_entry = entries
        .iter()
        .map(|e| e.modified)
        .min()
        .and_then(|oldest| now.duration_since(oldest).ok())
        .map(format_duration);

    let mut display_entries: Vec<CacheEntry> = entries
        .iter()
        .map(|e| {
            let age = now.duration_since(e.modified).unwrap_or_default();
            CacheEntry {
                resource_type: e.resource_type.clone(),
                name: e.name.clone(),
                version: e.version.clone(),
                bytes: e.bytes,
                size: format_bytes(e.bytes),
                age: format_duration(age),
            }
        })
        .collect();

    // Sort by resource_type, then name, then version for stable output
    display_entries.sort_by(|a, b| {
        a.resource_type
            .cmp(&b.resource_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.version.cmp(&b.version))
    });

    let result = CacheStatusResult {
        total_bytes,
        total_size: format_bytes(total_bytes),
        entry_count,
        oldest_entry,
        entries: display_entries,
    };

    if !opts.json {
        print_status(&result);
    }

    result
}

/// Run `relava cache clean`.
pub fn clean(opts: &CacheCleanOpts) -> CacheCleanResult {
    let entries = discover_entries(opts.cache_dir);
    let now = SystemTime::now();

    let to_remove: Vec<&EntryInfo> = entries
        .iter()
        .filter(|e| match opts.older_than {
            Some(max_age) => {
                let age = now.duration_since(e.modified).unwrap_or_default();
                age > max_age
            }
            None => true, // remove all
        })
        .collect();

    let mut removed = 0;
    let mut bytes_freed = 0u64;
    let mut removed_entries = Vec::new();

    for entry in &to_remove {
        let size = entry.bytes;
        if std::fs::remove_dir_all(&entry.path).is_ok() {
            removed += 1;
            bytes_freed += size;

            if !opts.json {
                println!(
                    "{}",
                    Tag::Ok.fmt(&format!(
                        "removed {}/{} v{} ({})",
                        entry.resource_type,
                        entry.name,
                        entry.version,
                        format_bytes(size),
                    ))
                );
            }

            removed_entries.push(RemovedEntry {
                resource_type: entry.resource_type.clone(),
                name: entry.name.clone(),
                version: entry.version.clone(),
            });

            // Clean up empty parent directories (name dir, then type dir)
            cleanup_empty_parents(&entry.path, opts.cache_dir);
        }
    }

    if !opts.json {
        if removed == 0 {
            println!("{}", Tag::Ok.fmt("cache is already clean"));
        } else {
            println!(
                "\nRemoved {} {} ({})",
                removed,
                pluralize("entry", "entries", removed),
                format_bytes(bytes_freed),
            );
        }
    }

    CacheCleanResult {
        removed,
        bytes_freed,
        size_freed: format_bytes(bytes_freed),
        entries: removed_entries,
    }
}

/// Run automatic eviction: remove oldest entries until cache is under `max_bytes`.
///
/// Returns the number of entries evicted and bytes freed. This is designed
/// to be called as a post-install hook — errors are non-fatal.
#[allow(dead_code)]
pub fn evict(opts: &EvictionOpts) -> EvictionResult {
    let mut entries = discover_entries(opts.cache_dir);
    let total: u64 = entries.iter().map(|e| e.bytes).sum();

    if total <= opts.max_bytes {
        return EvictionResult {
            evicted: 0,
            bytes_freed: 0,
        };
    }

    // Sort oldest first
    entries.sort_by_key(|e| e.modified);

    let mut freed = 0u64;
    let mut evicted = 0;
    let mut remaining = total;

    for entry in &entries {
        if remaining <= opts.max_bytes {
            break;
        }
        if std::fs::remove_dir_all(&entry.path).is_ok() {
            freed += entry.bytes;
            remaining -= entry.bytes;
            evicted += 1;
            cleanup_empty_parents(&entry.path, opts.cache_dir);
        }
    }

    EvictionResult {
        evicted,
        bytes_freed: freed,
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Walk the cache directory and discover all version-level entries.
///
/// Cache layout: `cache/<type_plural>/<name>/<version>/`
fn discover_entries(cache_dir: &Path) -> Vec<EntryInfo> {
    let mut entries = Vec::new();

    let type_dirs = match std::fs::read_dir(cache_dir) {
        Ok(rd) => rd,
        Err(_) => return entries, // cache dir doesn't exist yet — empty
    };

    for type_entry in type_dirs.flatten() {
        let type_path = type_entry.path();
        if !type_path.is_dir() {
            continue;
        }
        let resource_type = type_entry.file_name().to_string_lossy().to_string();

        let name_dirs = match std::fs::read_dir(&type_path) {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        for name_entry in name_dirs.flatten() {
            let name_path = name_entry.path();
            if !name_path.is_dir() {
                continue;
            }
            let name = name_entry.file_name().to_string_lossy().to_string();

            let version_dirs = match std::fs::read_dir(&name_path) {
                Ok(rd) => rd,
                Err(_) => continue,
            };

            for version_entry in version_dirs.flatten() {
                let version_path = version_entry.path();
                if !version_path.is_dir() {
                    continue;
                }
                let version = version_entry.file_name().to_string_lossy().to_string();

                let bytes = dir_size(&version_path);
                let modified = dir_newest_mtime(&version_path);

                entries.push(EntryInfo {
                    resource_type: resource_type.clone(),
                    name: name.clone(),
                    version,
                    path: version_path,
                    bytes,
                    modified,
                });
            }
        }
    }

    entries
}

/// Calculate total size of a directory recursively.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(path) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(meta) = p.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Find the newest modification time in a directory tree.
///
/// Falls back to `UNIX_EPOCH` if the directory is empty or metadata is
/// unavailable.
fn dir_newest_mtime(path: &Path) -> SystemTime {
    let mut newest = SystemTime::UNIX_EPOCH;
    walk_mtime(path, &mut newest);
    newest
}

fn walk_mtime(path: &Path, newest: &mut SystemTime) {
    if let Ok(rd) = std::fs::read_dir(path) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_mtime(&p, newest);
            } else if let Ok(meta) = p.metadata()
                && let Ok(mtime) = meta.modified()
                && mtime > *newest
            {
                *newest = mtime;
            }
        }
    }
}

/// Remove empty parent directories up to (but not including) `stop_at`.
fn cleanup_empty_parents(path: &Path, stop_at: &Path) {
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent == stop_at {
            break;
        }
        // Try to remove; rmdir fails if non-empty, which is what we want
        if std::fs::remove_dir(parent).is_err() {
            break;
        }
        current = parent.parent();
    }
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Format bytes as a human-readable string.
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Format a duration as a human-readable string.
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// Parse a duration string like "30d", "12h", "45m", "3600s".
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration string".to_string());
    }

    let (num_part, unit) = match s.as_bytes().last() {
        Some(b'd') => (&s[..s.len() - 1], 86400u64),
        Some(b'h') => (&s[..s.len() - 1], 3600u64),
        Some(b'm') => (&s[..s.len() - 1], 60u64),
        Some(b's') => (&s[..s.len() - 1], 1u64),
        _ => {
            return Err(format!(
                "invalid duration '{s}': expected suffix d, h, m, or s"
            ));
        }
    };

    let n: u64 = num_part
        .parse()
        .map_err(|_| format!("invalid duration '{s}': not a valid number"))?;

    Ok(Duration::from_secs(n * unit))
}

fn pluralize<'a>(singular: &'a str, plural: &'a str, count: usize) -> &'a str {
    if count == 1 { singular } else { plural }
}

// ---------------------------------------------------------------------------
// Human-readable output
// ---------------------------------------------------------------------------

fn print_status(result: &CacheStatusResult) {
    println!("Cache: {}", result.total_size);
    println!("Entries: {}", result.entry_count,);
    if let Some(ref oldest) = result.oldest_entry {
        println!("Oldest: {oldest} ago");
    }

    if !result.entries.is_empty() {
        println!();
        let rows: Vec<Vec<String>> = result
            .entries
            .iter()
            .map(|e| {
                vec![
                    format!("{}/{}", e.resource_type, e.name),
                    e.version.clone(),
                    e.size.clone(),
                    format!("{} ago", e.age),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::output::table(&["Resource", "Version", "Size", "Age"], &rows)
        );
    }
}

// ---------------------------------------------------------------------------
// Default cache directory
// ---------------------------------------------------------------------------

/// Resolve the default cache directory (`~/.relava/cache/`).
pub fn default_cache_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|h| h.join(".relava").join("cache"))
        .ok_or_else(|| "cannot determine home directory".to_string())
}

/// Default maximum cache size for automatic eviction (500 MB).
#[allow(dead_code)]
pub const DEFAULT_MAX_CACHE_BYTES: u64 = 500 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_cache() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache_dir = tmp.path().join("cache");
        fs::create_dir_all(&cache_dir).unwrap();
        (tmp, cache_dir)
    }

    /// Create a fake cache entry with the given content size.
    fn create_entry(cache_dir: &Path, rtype: &str, name: &str, version: &str, content: &[u8]) {
        let dir = cache_dir.join(rtype).join(name).join(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("file.md"), content).unwrap();
    }

    /// Create a fake cache entry with a specific modification time.
    fn create_entry_with_age(
        cache_dir: &Path,
        rtype: &str,
        name: &str,
        version: &str,
        content: &[u8],
        age: Duration,
    ) {
        let dir = cache_dir.join(rtype).join(name).join(version);
        fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("file.md");
        fs::write(&file_path, content).unwrap();

        // Set modification time to `age` ago
        let mtime = SystemTime::now() - age;
        let mtime_filetime = filetime::FileTime::from_system_time(mtime);
        filetime::set_file_mtime(&file_path, mtime_filetime).unwrap();
    }

    // -- format_bytes tests --

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn format_bytes_small() {
        assert_eq!(format_bytes(512), "512 B");
    }

    #[test]
    fn format_bytes_kilobytes() {
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
    }

    #[test]
    fn format_bytes_megabytes() {
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
    }

    #[test]
    fn format_bytes_gigabytes() {
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
    }

    // -- format_duration tests --

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(120)), "2m");
    }

    #[test]
    fn format_duration_hours() {
        assert_eq!(format_duration(Duration::from_secs(7200)), "2h");
    }

    #[test]
    fn format_duration_days() {
        assert_eq!(format_duration(Duration::from_secs(172800)), "2d");
    }

    // -- parse_duration tests --

    #[test]
    fn parse_duration_days() {
        assert_eq!(
            parse_duration("30d").unwrap(),
            Duration::from_secs(30 * 86400)
        );
    }

    #[test]
    fn parse_duration_hours() {
        assert_eq!(
            parse_duration("12h").unwrap(),
            Duration::from_secs(12 * 3600)
        );
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration("45m").unwrap(), Duration::from_secs(45 * 60));
    }

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(parse_duration("3600s").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn parse_duration_invalid_suffix() {
        assert!(parse_duration("30x").is_err());
    }

    #[test]
    fn parse_duration_empty() {
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn parse_duration_not_a_number() {
        assert!(parse_duration("abcd").is_err());
    }

    // -- status tests --

    #[test]
    fn status_empty_cache() {
        let (_tmp, cache_dir) = temp_cache();
        let opts = CacheStatusOpts {
            cache_dir: &cache_dir,
            json: true,
        };
        let result = status(&opts);
        assert_eq!(result.entry_count, 0);
        assert_eq!(result.total_bytes, 0);
        assert!(result.oldest_entry.is_none());
        assert!(result.entries.is_empty());
    }

    #[test]
    fn status_nonexistent_cache_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache_dir = tmp.path().join("nonexistent");
        let opts = CacheStatusOpts {
            cache_dir: &cache_dir,
            json: true,
        };
        let result = status(&opts);
        assert_eq!(result.entry_count, 0);
        assert_eq!(result.total_bytes, 0);
    }

    #[test]
    fn status_with_entries() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry(&cache_dir, "skills", "denden", "1.0.0", b"hello world");
        create_entry(
            &cache_dir,
            "agents",
            "debugger",
            "0.5.0",
            b"agent content here",
        );

        let opts = CacheStatusOpts {
            cache_dir: &cache_dir,
            json: true,
        };
        let result = status(&opts);

        assert_eq!(result.entry_count, 2);
        assert!(result.total_bytes > 0);
        assert!(result.oldest_entry.is_some());
        assert_eq!(result.entries.len(), 2);

        // Entries are sorted: agents before skills
        assert_eq!(result.entries[0].resource_type, "agents");
        assert_eq!(result.entries[0].name, "debugger");
        assert_eq!(result.entries[1].resource_type, "skills");
        assert_eq!(result.entries[1].name, "denden");
    }

    #[test]
    fn status_multiple_versions() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry(&cache_dir, "skills", "denden", "1.0.0", b"v1");
        create_entry(&cache_dir, "skills", "denden", "2.0.0", b"v2 content");

        let opts = CacheStatusOpts {
            cache_dir: &cache_dir,
            json: true,
        };
        let result = status(&opts);

        assert_eq!(result.entry_count, 2);
        assert_eq!(result.entries.len(), 2);
    }

    #[test]
    fn status_total_bytes_sums_all_entries() {
        let (_tmp, cache_dir) = temp_cache();
        let content_a = vec![0u8; 1000];
        let content_b = vec![0u8; 2000];
        create_entry(&cache_dir, "skills", "a", "1.0.0", &content_a);
        create_entry(&cache_dir, "skills", "b", "1.0.0", &content_b);

        let opts = CacheStatusOpts {
            cache_dir: &cache_dir,
            json: true,
        };
        let result = status(&opts);

        assert_eq!(result.total_bytes, 3000);
    }

    // -- clean tests --

    #[test]
    fn clean_empty_cache() {
        let (_tmp, cache_dir) = temp_cache();
        let opts = CacheCleanOpts {
            cache_dir: &cache_dir,
            older_than: None,
            json: true,
        };
        let result = clean(&opts);
        assert_eq!(result.removed, 0);
        assert_eq!(result.bytes_freed, 0);
    }

    #[test]
    fn clean_removes_all_entries() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry(&cache_dir, "skills", "denden", "1.0.0", b"hello");
        create_entry(&cache_dir, "agents", "debugger", "0.5.0", b"world");

        let opts = CacheCleanOpts {
            cache_dir: &cache_dir,
            older_than: None,
            json: true,
        };
        let result = clean(&opts);

        assert_eq!(result.removed, 2);
        assert!(result.bytes_freed > 0);
        assert_eq!(result.entries.len(), 2);

        // Cache dir should now be empty (parent dirs cleaned up)
        let remaining = discover_entries(&cache_dir);
        assert!(remaining.is_empty());
    }

    #[test]
    fn clean_older_than_only_removes_old_entries() {
        let (_tmp, cache_dir) = temp_cache();

        // Create an "old" entry (40 days) and a "new" entry (just created)
        create_entry_with_age(
            &cache_dir,
            "skills",
            "old-skill",
            "1.0.0",
            b"old content",
            Duration::from_secs(40 * 86400),
        );
        create_entry(&cache_dir, "skills", "new-skill", "1.0.0", b"new content");

        let opts = CacheCleanOpts {
            cache_dir: &cache_dir,
            older_than: Some(Duration::from_secs(30 * 86400)),
            json: true,
        };
        let result = clean(&opts);

        assert_eq!(result.removed, 1);
        assert_eq!(result.entries[0].name, "old-skill");

        // New entry should still exist
        let remaining = discover_entries(&cache_dir);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "new-skill");
    }

    #[test]
    fn clean_older_than_keeps_all_when_none_old() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry(&cache_dir, "skills", "fresh", "1.0.0", b"content");

        let opts = CacheCleanOpts {
            cache_dir: &cache_dir,
            older_than: Some(Duration::from_secs(30 * 86400)),
            json: true,
        };
        let result = clean(&opts);

        assert_eq!(result.removed, 0);
        let remaining = discover_entries(&cache_dir);
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn clean_cleans_up_empty_parent_dirs() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry(&cache_dir, "skills", "denden", "1.0.0", b"content");

        let opts = CacheCleanOpts {
            cache_dir: &cache_dir,
            older_than: None,
            json: true,
        };
        clean(&opts);

        // The type dir "skills" and name dir "denden" should be removed
        assert!(!cache_dir.join("skills").join("denden").exists());
        assert!(!cache_dir.join("skills").exists());
    }

    #[test]
    fn clean_preserves_sibling_entries() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry_with_age(
            &cache_dir,
            "skills",
            "denden",
            "1.0.0",
            b"old",
            Duration::from_secs(40 * 86400),
        );
        create_entry(&cache_dir, "skills", "denden", "2.0.0", b"new");

        let opts = CacheCleanOpts {
            cache_dir: &cache_dir,
            older_than: Some(Duration::from_secs(30 * 86400)),
            json: true,
        };
        let result = clean(&opts);

        assert_eq!(result.removed, 1);
        // The name dir "denden" should still exist (v2.0.0 remains)
        assert!(cache_dir.join("skills").join("denden").exists());
        assert!(
            cache_dir
                .join("skills")
                .join("denden")
                .join("2.0.0")
                .exists()
        );
    }

    // -- eviction tests --

    #[test]
    fn evict_under_limit_does_nothing() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry(&cache_dir, "skills", "small", "1.0.0", b"tiny");

        let opts = EvictionOpts {
            cache_dir: &cache_dir,
            max_bytes: 1_000_000,
        };
        let result = evict(&opts);

        assert_eq!(result.evicted, 0);
        assert_eq!(result.bytes_freed, 0);
    }

    #[test]
    fn evict_removes_oldest_first() {
        let (_tmp, cache_dir) = temp_cache();
        let big_content = vec![0u8; 500];

        // Create entries with different ages
        create_entry_with_age(
            &cache_dir,
            "skills",
            "oldest",
            "1.0.0",
            &big_content,
            Duration::from_secs(3 * 86400),
        );
        create_entry_with_age(
            &cache_dir,
            "skills",
            "middle",
            "1.0.0",
            &big_content,
            Duration::from_secs(2 * 86400),
        );
        create_entry(&cache_dir, "skills", "newest", "1.0.0", &big_content);

        // Set max to only allow ~1 entry worth of space
        let opts = EvictionOpts {
            cache_dir: &cache_dir,
            max_bytes: 600,
        };
        let result = evict(&opts);

        assert!(result.evicted >= 1);
        assert!(result.bytes_freed > 0);

        // Newest should survive
        let remaining = discover_entries(&cache_dir);
        let names: Vec<&str> = remaining.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"newest"));
    }

    #[test]
    fn evict_empty_cache() {
        let (_tmp, cache_dir) = temp_cache();
        let opts = EvictionOpts {
            cache_dir: &cache_dir,
            max_bytes: 100,
        };
        let result = evict(&opts);
        assert_eq!(result.evicted, 0);
    }

    // -- discover_entries tests --

    #[test]
    fn discover_ignores_non_directory_files() {
        let (_tmp, cache_dir) = temp_cache();
        // Create a stray file at type level
        fs::write(cache_dir.join("stray_file.txt"), "stray").unwrap();
        // Create a valid entry
        create_entry(&cache_dir, "skills", "valid", "1.0.0", b"content");

        let entries = discover_entries(&cache_dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "valid");
    }

    #[test]
    fn discover_handles_nested_files() {
        let (_tmp, cache_dir) = temp_cache();
        let dir = cache_dir.join("skills").join("multi").join("1.0.0");
        fs::create_dir_all(dir.join("templates")).unwrap();
        fs::write(dir.join("SKILL.md"), "# Skill").unwrap();
        fs::write(dir.join("templates").join("foo.md"), "template").unwrap();

        let entries = discover_entries(&cache_dir);
        assert_eq!(entries.len(), 1);
        // Size should include both files
        assert!(entries[0].bytes > 0);
    }

    // -- dir_size tests --

    #[test]
    fn dir_size_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(dir_size(tmp.path()), 0);
    }

    #[test]
    fn dir_size_with_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        fs::write(tmp.path().join("b.txt"), "world!").unwrap();
        assert_eq!(dir_size(tmp.path()), 11); // 5 + 6
    }

    #[test]
    fn dir_size_recursive() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("a.txt"), "abc").unwrap();
        fs::write(tmp.path().join("sub").join("b.txt"), "defgh").unwrap();
        assert_eq!(dir_size(tmp.path()), 8); // 3 + 5
    }

    // -- pluralize tests --

    #[test]
    fn pluralize_singular() {
        assert_eq!(pluralize("entry", "entries", 1), "entry");
    }

    #[test]
    fn pluralize_plural() {
        assert_eq!(pluralize("entry", "entries", 0), "entries");
        assert_eq!(pluralize("entry", "entries", 2), "entries");
    }

    // -- JSON serialization tests --

    #[test]
    fn clean_result_serializes() {
        let result = CacheCleanResult {
            removed: 2,
            bytes_freed: 1024,
            size_freed: "1.0 KB".to_string(),
            entries: vec![RemovedEntry {
                resource_type: "skills".to_string(),
                name: "denden".to_string(),
                version: "1.0.0".to_string(),
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"removed\":2"));
        assert!(json.contains("\"bytes_freed\":1024"));
    }

    #[test]
    fn status_result_serializes() {
        let result = CacheStatusResult {
            total_bytes: 2048,
            total_size: "2.0 KB".to_string(),
            entry_count: 1,
            oldest_entry: Some("5d".to_string()),
            entries: vec![CacheEntry {
                resource_type: "skills".to_string(),
                name: "denden".to_string(),
                version: "1.0.0".to_string(),
                bytes: 2048,
                size: "2.0 KB".to_string(),
                age: "5d".to_string(),
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"entry_count\":1"));
        assert!(json.contains("\"total_bytes\":2048"));
    }

    #[test]
    fn eviction_result_serializes() {
        let result = EvictionResult {
            evicted: 3,
            bytes_freed: 5000,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"evicted\":3"));
    }

    // -- human-readable output tests --

    #[test]
    fn status_prints_human_output() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry(&cache_dir, "skills", "denden", "1.0.0", b"hello world");

        // json: false triggers print output (we just check it doesn't panic)
        let opts = CacheStatusOpts {
            cache_dir: &cache_dir,
            json: false,
        };
        let result = status(&opts);
        assert_eq!(result.entry_count, 1);
    }

    #[test]
    fn clean_prints_human_output() {
        let (_tmp, cache_dir) = temp_cache();
        create_entry(&cache_dir, "skills", "denden", "1.0.0", b"content");

        let opts = CacheCleanOpts {
            cache_dir: &cache_dir,
            older_than: None,
            json: false,
        };
        let result = clean(&opts);
        assert_eq!(result.removed, 1);
    }

    #[test]
    fn clean_empty_cache_prints_already_clean() {
        let (_tmp, cache_dir) = temp_cache();

        // json: false triggers the "already clean" message
        let opts = CacheCleanOpts {
            cache_dir: &cache_dir,
            older_than: None,
            json: false,
        };
        let result = clean(&opts);
        assert_eq!(result.removed, 0);
    }
}
