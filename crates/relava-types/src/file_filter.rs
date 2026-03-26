//! Binary file detection and text-only enforcement for resource types.
//!
//! Provides a null-byte probe to classify files as binary or text, and
//! enforcement rules that restrict certain resource types to text-only files.

use std::path::Path;

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
    I: IntoIterator<Item = (std::path::PathBuf, String)>,
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
}
