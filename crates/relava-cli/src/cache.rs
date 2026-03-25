use std::path::{Component, Path, PathBuf};

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::registry::DownloadResponse;
use relava_types::validate::ResourceType;
use relava_types::version::Version;

/// Manages a download cache at `~/.relava/cache/`.
///
/// Cache layout:
/// ```text
/// cache/
///   skills/denden/1.0.0/
///     SKILL.md
///     templates/foo.md
///   agents/debugger/0.5.0/
///     debugger.md
/// ```
pub struct DownloadCache {
    root: PathBuf,
}

impl DownloadCache {
    /// Create a cache rooted at the given directory.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Path to a cached resource version directory.
    pub fn version_dir(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
    ) -> PathBuf {
        self.root
            .join(resource_type.store_dir_name())
            .join(name)
            .join(version.to_string())
    }

    /// Check if a resource version is already cached.
    pub fn is_cached(&self, resource_type: ResourceType, name: &str, version: &Version) -> bool {
        self.version_dir(resource_type, name, version).is_dir()
    }

    /// Store a download response in the cache.
    ///
    /// Returns the list of relative file paths that were cached.
    /// Rejects paths that are absolute or contain `..` components (path traversal).
    pub fn store(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
        response: &DownloadResponse,
    ) -> Result<Vec<String>, CacheError> {
        if response.files.is_empty() {
            return Err(CacheError::Decode("download contains no files".to_string()));
        }

        let dir = self.version_dir(resource_type, name, version);
        std::fs::create_dir_all(&dir).map_err(CacheError::Io)?;

        let mut paths = Vec::new();
        for file in &response.files {
            validate_relative_path(&file.path)?;

            let dest = dir.join(&file.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(CacheError::Io)?;
            }

            let content = base64::engine::general_purpose::STANDARD
                .decode(&file.content)
                .map_err(|e| {
                    CacheError::Decode(format!("failed to decode {}: {}", file.path, e))
                })?;

            std::fs::write(&dest, &content).map_err(CacheError::Io)?;
            paths.push(file.path.clone());
        }
        Ok(paths)
    }

    /// List files in a cached resource version directory.
    ///
    /// Returns paths relative to the version directory.
    pub fn list_files(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
    ) -> Result<Vec<String>, CacheError> {
        let dir = self.version_dir(resource_type, name, version);
        if !dir.is_dir() {
            return Err(CacheError::NotCached(format!(
                "{} {}@{} not in cache",
                resource_type, name, version
            )));
        }
        collect_relative_paths(&dir, &dir)
    }

    /// Read a cached file.
    pub fn read_file(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
        relative_path: &str,
    ) -> Result<Vec<u8>, CacheError> {
        let file_path = self
            .version_dir(resource_type, name, version)
            .join(relative_path);
        std::fs::read(&file_path).map_err(CacheError::Io)
    }
}

/// Validate that a file path is relative and contains no parent directory components.
/// Prevents path traversal attacks from malicious registry responses.
fn validate_relative_path(path: &str) -> Result<(), CacheError> {
    let p = Path::new(path);
    // Path::is_absolute() is platform-specific — on Windows it doesn't reject
    // Unix-style absolute paths like "/etc/passwd". Check for leading `/` explicitly.
    if p.is_absolute() || path.starts_with('/') {
        return Err(CacheError::Decode(format!("unsafe file path: {path}")));
    }
    if p.components().any(|c| c == Component::ParentDir) {
        return Err(CacheError::Decode(format!("unsafe file path: {path}")));
    }
    Ok(())
}

/// Collect all file paths relative to `base` by walking `dir`.
fn collect_relative_paths(dir: &Path, base: &Path) -> Result<Vec<String>, CacheError> {
    let mut result = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(CacheError::Io)?;
    for entry in entries {
        let entry = entry.map_err(CacheError::Io)?;
        let path = entry.path();
        if path.is_dir() {
            result.extend(collect_relative_paths(&path, base)?);
        } else {
            let relative = path
                .strip_prefix(base)
                .map_err(|e| CacheError::Decode(e.to_string()))?;
            // Normalize to forward slashes for cross-platform consistency.
            // On Windows, strip_prefix produces paths with `\` separators.
            let s = relative.to_string_lossy().replace('\\', "/");
            result.push(s);
        }
    }
    result.sort();
    Ok(result)
}

/// Compute SHA-256 hex digest of data.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[derive(Debug)]
pub enum CacheError {
    Io(std::io::Error),
    Decode(String),
    NotCached(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cache I/O error: {e}"),
            Self::Decode(msg) => write!(f, "cache decode error: {msg}"),
            Self::NotCached(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CacheError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{DownloadFile, DownloadResponse};

    fn test_cache() -> (std::path::PathBuf, DownloadCache) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("relava-cache-test-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let cache = DownloadCache::new(root.clone());
        (root, cache)
    }

    fn encode_base64(data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    #[test]
    fn cache_not_cached_initially() {
        let (_root, cache) = test_cache();
        let v = Version::parse("1.0.0").unwrap();
        assert!(!cache.is_cached(ResourceType::Skill, "denden", &v));
    }

    #[test]
    fn cache_store_and_retrieve() {
        let (_root, cache) = test_cache();
        let v = Version::parse("1.0.0").unwrap();
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            version: "1.0.0".to_string(),
            files: vec![
                DownloadFile {
                    path: "SKILL.md".to_string(),
                    content: encode_base64(b"# Denden Skill"),
                },
                DownloadFile {
                    path: "templates/foo.md".to_string(),
                    content: encode_base64(b"template content"),
                },
            ],
        };

        let paths = cache
            .store(ResourceType::Skill, "denden", &v, &response)
            .unwrap();
        assert_eq!(paths, vec!["SKILL.md", "templates/foo.md"]);
        assert!(cache.is_cached(ResourceType::Skill, "denden", &v));

        let files = cache.list_files(ResourceType::Skill, "denden", &v).unwrap();
        assert_eq!(files, vec!["SKILL.md", "templates/foo.md"]);

        let content = cache
            .read_file(ResourceType::Skill, "denden", &v, "SKILL.md")
            .unwrap();
        assert_eq!(content, b"# Denden Skill");
    }

    #[test]
    fn cache_list_not_cached() {
        let (_root, cache) = test_cache();
        let v = Version::parse("1.0.0").unwrap();
        let result = cache.list_files(ResourceType::Skill, "denden", &v);
        assert!(result.is_err());
    }

    #[test]
    fn sha256_hex_known_value() {
        // SHA-256 of empty string
        let hash = sha256_hex(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn version_dir_path() {
        let cache = DownloadCache::new(PathBuf::from("/tmp/cache"));
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(
            cache.version_dir(ResourceType::Skill, "denden", &v),
            PathBuf::from("/tmp/cache/skills/denden/1.2.3")
        );
        assert_eq!(
            cache.version_dir(ResourceType::Agent, "debugger", &v),
            PathBuf::from("/tmp/cache/agents/debugger/1.2.3")
        );
    }

    #[test]
    fn rejects_path_traversal_parent_dir() {
        let (_root, cache) = test_cache();
        let v = Version::parse("1.0.0").unwrap();
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "evil".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "../../.bashrc".to_string(),
                content: encode_base64(b"malicious"),
            }],
        };
        let result = cache.store(ResourceType::Skill, "evil", &v, &response);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unsafe file path"));
    }

    #[test]
    fn rejects_absolute_path() {
        let (_root, cache) = test_cache();
        let v = Version::parse("1.0.0").unwrap();
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "evil".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "/etc/passwd".to_string(),
                content: encode_base64(b"malicious"),
            }],
        };
        let result = cache.store(ResourceType::Skill, "evil", &v, &response);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unsafe file path"));
    }

    #[test]
    fn rejects_empty_file_list() {
        let (_root, cache) = test_cache();
        let v = Version::parse("1.0.0").unwrap();
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "empty".to_string(),
            version: "1.0.0".to_string(),
            files: vec![],
        };
        let result = cache.store(ResourceType::Skill, "empty", &v, &response);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no files"));
    }
}
