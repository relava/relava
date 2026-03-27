//! File filtering for resource directories.
//!
//! Provides:
//! - **Binary detection**: null-byte probe to classify files as binary or text,
//!   with enforcement rules per resource type.
//! - **Ignore patterns**: `.relavaignore` parsing with gitignore-style glob
//!   matching for excluding files from publish and sync operations.

use std::path::{Path, PathBuf};

use crate::validate::ResourceType;

/// Number of bytes to read when probing a file for binary content.
const BINARY_PROBE_SIZE: usize = 8_000;

/// Check if a file is binary by scanning the first 8,000 bytes for null bytes.
///
/// Returns `Ok(true)` if a null byte is found, `Ok(false)` otherwise.
pub fn is_binary(path: &Path) -> Result<bool, std::io::Error> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; BINARY_PROBE_SIZE];
    let bytes_read = file.read(&mut buffer)?;
    Ok(buffer[..bytes_read].contains(&0))
}

/// Whether a resource type requires text-only files (no binary content).
///
/// Skills, commands, and rules must be text-only. Agents allow any file types.
pub fn requires_text_only(resource_type: ResourceType) -> bool {
    matches!(
        resource_type,
        ResourceType::Skill | ResourceType::Command | ResourceType::Rule
    )
}

/// Result of scanning a directory for binary files.
#[derive(Debug, Default)]
pub struct BinaryScanResult {
    /// Paths of files detected as binary (relative to the scanned directory).
    pub binary_files: Vec<String>,
}

impl BinaryScanResult {
    /// Returns `true` if no binary files were found.
    pub fn is_clean(&self) -> bool {
        self.binary_files.is_empty()
    }
}

/// Scan files for binary content, returning any binary file paths found.
///
/// Only checks files when the resource type requires text-only content.
/// For resource types that allow binary files (e.g., agents), returns an
/// empty result immediately.
///
/// The `files` iterator should yield `(absolute_path, display_path)` pairs
/// where `display_path` is the human-readable relative path for error messages.
pub fn scan_for_binary_files<I>(resource_type: ResourceType, files: I) -> BinaryScanResult
where
    I: IntoIterator<Item = (PathBuf, String)>,
{
    if !requires_text_only(resource_type) {
        return BinaryScanResult::default();
    }

    let binary_files = files
        .into_iter()
        .filter(|(path, _)| matches!(is_binary(path), Ok(true)))
        .map(|(_, display_path)| display_path)
        .collect();

    BinaryScanResult { binary_files }
}

// ---------------------------------------------------------------------------
// .relavaignore support
// ---------------------------------------------------------------------------

/// The filename for ignore patterns in a resource directory.
pub const RELAVAIGNORE_FILE: &str = ".relavaignore";

/// Parsed ignore patterns from a `.relavaignore` file.
///
/// Supports gitignore-style patterns:
/// - `*.ext` — match files by extension
/// - `dirname/` — match directories by name
/// - `specific-file` — match a specific file name or path
/// - `#` lines and blank lines are ignored (comments / whitespace)
#[derive(Debug, Default)]
pub struct IgnorePatterns {
    matcher: Option<globset::GlobSet>,
}

impl IgnorePatterns {
    /// Load ignore patterns from a `.relavaignore` file in the given directory.
    ///
    /// Returns an empty (no-op) pattern set if the file does not exist.
    /// Returns an error only if the file exists but cannot be read or parsed.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(RELAVAIGNORE_FILE);
        match std::fs::read_to_string(&path) {
            Ok(content) => Self::parse(&content),
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::NotADirectory =>
            {
                Ok(Self::default())
            }
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// Parse ignore patterns from a string (the contents of a `.relavaignore`).
    pub fn parse(content: &str) -> Result<Self, String> {
        let mut builder = globset::GlobSetBuilder::new();
        let mut has_patterns = false;

        for line in content.lines() {
            let trimmed = line.trim();

            // Skip blank lines and comments
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // Directory patterns: "dirname/" should match "dirname/**"
            // Bare names (no slash) match anywhere in the tree: "**/{name}"
            let pattern = if let Some(dir) = trimmed.strip_suffix('/') {
                format!("{dir}/**")
            } else if !trimmed.contains('/') {
                format!("**/{trimmed}")
            } else {
                trimmed.to_string()
            };

            let glob = globset::GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
                .map_err(|e| format!("invalid pattern '{}': {e}", trimmed))?;
            builder.add(glob);
            has_patterns = true;
        }

        if !has_patterns {
            return Ok(Self::default());
        }

        let matcher = builder
            .build()
            .map_err(|e| format!("cannot build ignore patterns: {e}"))?;
        Ok(Self {
            matcher: Some(matcher),
        })
    }

    /// Returns `true` if the given relative path matches any ignore pattern.
    pub fn is_ignored(&self, relative_path: &str) -> bool {
        self.matcher
            .as_ref()
            .is_some_and(|set| set.is_match(relative_path))
    }

    /// Returns `true` if no patterns are loaded (no-op filter).
    pub fn is_empty(&self) -> bool {
        self.matcher.is_none()
    }
}

/// Filter a list of paths using ignore patterns, returning only non-ignored paths.
///
/// Paths are matched as relative paths from the resource root. The `.relavaignore`
/// file itself is never filtered out (it is always included in published resources).
pub fn filter_ignored(root: &Path, files: Vec<PathBuf>, patterns: &IgnorePatterns) -> Vec<PathBuf> {
    if patterns.is_empty() {
        return files;
    }

    files
        .into_iter()
        .filter(|path| {
            let relative = path.strip_prefix(root).unwrap_or(path);

            // Non-UTF-8 paths can't match patterns; include them
            let Some(relative_str) = relative.to_str() else {
                return true;
            };

            // Never filter out .relavaignore itself
            if relative_str == RELAVAIGNORE_FILE {
                return true;
            }

            !patterns.is_ignored(relative_str)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "relava-file-filter-test-{}-{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_null_bytes_as_binary() {
        let dir = temp_dir();
        let file = dir.join("binary.bin");
        fs::write(&file, [0u8, 1, 2, 3]).unwrap();
        assert!(is_binary(&file).unwrap());
    }

    #[test]
    fn classifies_text_file_as_not_binary() {
        let dir = temp_dir();
        let file = dir.join("text.txt");
        fs::write(&file, "Hello, world!").unwrap();
        assert!(!is_binary(&file).unwrap());
    }

    #[test]
    fn detects_null_byte_beyond_start() {
        let dir = temp_dir();
        let file = dir.join("late-null.bin");
        let mut content = vec![b'A'; 5000];
        content[4999] = 0;
        fs::write(&file, &content).unwrap();
        assert!(is_binary(&file).unwrap());
    }

    #[test]
    fn classifies_empty_file_as_text() {
        let dir = temp_dir();
        let file = dir.join("empty.txt");
        fs::write(&file, "").unwrap();
        assert!(!is_binary(&file).unwrap());
    }

    #[test]
    fn null_byte_beyond_probe_size_not_detected() {
        let dir = temp_dir();
        let file = dir.join("far-null.bin");
        let mut content = vec![b'A'; 9000];
        content[8500] = 0; // beyond 8,000-byte probe
        fs::write(&file, &content).unwrap();
        assert!(!is_binary(&file).unwrap());
    }

    #[test]
    fn nonexistent_file_returns_error() {
        let dir = temp_dir();
        assert!(is_binary(&dir.join("nonexistent")).is_err());
    }

    // -- requires_text_only tests --

    #[test]
    fn skill_requires_text_only() {
        assert!(requires_text_only(ResourceType::Skill));
    }

    #[test]
    fn command_requires_text_only() {
        assert!(requires_text_only(ResourceType::Command));
    }

    #[test]
    fn rule_requires_text_only() {
        assert!(requires_text_only(ResourceType::Rule));
    }

    #[test]
    fn agent_allows_binary() {
        assert!(!requires_text_only(ResourceType::Agent));
    }

    // -- scan_for_binary_files tests --

    #[test]
    fn scan_finds_binary_in_text_only_resource() {
        let dir = temp_dir();
        let bin_file = dir.join("data.bin");
        let text_file = dir.join("readme.md");
        fs::write(&bin_file, [0u8; 50]).unwrap();
        fs::write(&text_file, "text content").unwrap();

        let files = vec![
            (bin_file, "data.bin".to_string()),
            (text_file, "readme.md".to_string()),
        ];

        let result = scan_for_binary_files(ResourceType::Skill, files);
        assert!(!result.is_clean());
        assert_eq!(result.binary_files, vec!["data.bin"]);
    }

    #[test]
    fn scan_skips_check_for_agents() {
        let dir = temp_dir();
        let bin_file = dir.join("data.bin");
        fs::write(&bin_file, [0u8; 50]).unwrap();

        let files = vec![(bin_file, "data.bin".to_string())];

        let result = scan_for_binary_files(ResourceType::Agent, files);
        assert!(result.is_clean());
    }

    #[test]
    fn scan_clean_when_all_text() {
        let dir = temp_dir();
        let text_file = dir.join("readme.md");
        fs::write(&text_file, "text content").unwrap();

        let files = vec![(text_file, "readme.md".to_string())];

        let result = scan_for_binary_files(ResourceType::Command, files);
        assert!(result.is_clean());
    }

    // -- IgnorePatterns tests --

    #[test]
    fn parse_empty_content() {
        let patterns = IgnorePatterns::parse("").unwrap();
        assert!(patterns.is_empty());
        assert!(!patterns.is_ignored("anything.md"));
    }

    #[test]
    fn parse_comments_and_blanks_only() {
        let patterns = IgnorePatterns::parse("# comment\n\n  \n# another comment\n").unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn match_extension_pattern() {
        let patterns = IgnorePatterns::parse("*.so\n*.dylib\n").unwrap();
        assert!(!patterns.is_empty());
        assert!(patterns.is_ignored("libfoo.so"));
        assert!(patterns.is_ignored("nested/dir/libbar.dylib"));
        assert!(!patterns.is_ignored("SKILL.md"));
    }

    #[test]
    fn match_directory_pattern() {
        let patterns = IgnorePatterns::parse("bin/\nnode_modules/\n").unwrap();
        assert!(patterns.is_ignored("bin/helper"));
        assert!(patterns.is_ignored("bin/sub/deep"));
        assert!(patterns.is_ignored("node_modules/pkg/index.js"));
        assert!(!patterns.is_ignored("SKILL.md"));
    }

    #[test]
    fn match_specific_file() {
        let patterns = IgnorePatterns::parse(".DS_Store\n").unwrap();
        assert!(patterns.is_ignored(".DS_Store"));
        assert!(patterns.is_ignored("sub/.DS_Store"));
        assert!(!patterns.is_ignored("readme.md"));
    }

    #[test]
    fn match_path_with_slash() {
        let patterns = IgnorePatterns::parse("build/output.bin\n").unwrap();
        assert!(patterns.is_ignored("build/output.bin"));
        assert!(!patterns.is_ignored("other/output.bin"));
    }

    #[test]
    fn invalid_pattern_returns_error() {
        let result = IgnorePatterns::parse("[invalid");
        assert!(result.is_err());
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = temp_dir();
        let patterns = IgnorePatterns::load(&dir).unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn load_from_file() {
        let dir = temp_dir();
        fs::write(dir.join(".relavaignore"), "*.tmp\nbuild/\n").unwrap();
        let patterns = IgnorePatterns::load(&dir).unwrap();
        assert!(patterns.is_ignored("foo.tmp"));
        assert!(patterns.is_ignored("build/out.bin"));
        assert!(!patterns.is_ignored("SKILL.md"));
    }

    #[test]
    fn filter_ignored_preserves_relavaignore_file() {
        let dir = temp_dir();
        fs::write(dir.join(".relavaignore"), "*.tmp\n").unwrap();
        fs::write(dir.join("keep.md"), "keep").unwrap();
        fs::write(dir.join("drop.tmp"), "drop").unwrap();

        let patterns = IgnorePatterns::load(&dir).unwrap();
        let files = vec![
            dir.join(".relavaignore"),
            dir.join("keep.md"),
            dir.join("drop.tmp"),
        ];

        let result = filter_ignored(&dir, files, &patterns);
        let names: Vec<_> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&".relavaignore".to_string()));
        assert!(names.contains(&"keep.md".to_string()));
        assert!(!names.contains(&"drop.tmp".to_string()));
    }

    #[test]
    fn filter_ignored_noop_when_empty() {
        let dir = temp_dir();
        let patterns = IgnorePatterns::default();
        let files = vec![dir.join("a.md"), dir.join("b.md")];
        let result = filter_ignored(&dir, files.clone(), &patterns);
        assert_eq!(result.len(), files.len());
    }

    #[test]
    fn patterns_with_mixed_content() {
        let content = "# Ignore build artifacts\n*.o\n*.so\n\n# Directories\nbuild/\ntmp/\n\n# Specific files\n.DS_Store\n";
        let patterns = IgnorePatterns::parse(content).unwrap();
        assert!(patterns.is_ignored("main.o"));
        assert!(patterns.is_ignored("lib/foo.so"));
        assert!(patterns.is_ignored("build/output"));
        assert!(patterns.is_ignored("tmp/cache"));
        assert!(patterns.is_ignored(".DS_Store"));
        assert!(!patterns.is_ignored("SKILL.md"));
    }
}
