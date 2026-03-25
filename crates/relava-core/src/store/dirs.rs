use std::path::{Path, PathBuf};

use crate::validate::ResourceType;

/// Manages the `~/.relava/` directory structure.
///
/// ```text
/// ~/.relava/
///   config.toml
///   db.sqlite
///   store/
///     skills/<name>/<version>/
///     agents/<name>/<version>/
///     commands/<name>/<version>/
///     rules/<name>/<version>/
///   cache/
///   logs/
/// ```
pub struct RelavaDir {
    root: PathBuf,
}

impl RelavaDir {
    /// Create a `RelavaDir` rooted at the given path.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Create a `RelavaDir` at the default location (`~/.relava/`).
    /// Returns `None` if the home directory cannot be determined.
    pub fn default_location() -> Option<Self> {
        dirs::home_dir().map(|home| Self::new(home.join(".relava")))
    }

    /// Root directory path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to `db.sqlite`.
    pub fn db_path(&self) -> PathBuf {
        self.root.join("db.sqlite")
    }

    /// Path to `config.toml`.
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Path to the `store/` directory.
    pub fn store_dir(&self) -> PathBuf {
        self.root.join("store")
    }

    /// Path to the `cache/` directory.
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Path to the `logs/` directory.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Path to a resource type directory, e.g. `store/skills/`.
    pub fn type_dir(&self, resource_type: ResourceType) -> PathBuf {
        self.store_dir().join(resource_type.store_dir_name())
    }

    /// Path to a specific resource version directory,
    /// e.g. `store/skills/denden/1.0.0/`.
    pub fn version_dir(&self, resource_type: ResourceType, name: &str, version: &str) -> PathBuf {
        self.type_dir(resource_type).join(name).join(version)
    }

    /// Create the full directory structure. Idempotent — safe to call
    /// multiple times without error.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.store_dir())?;
        for rt in ResourceType::ALL {
            std::fs::create_dir_all(self.type_dir(rt))?;
        }
        std::fs::create_dir_all(self.cache_dir())?;
        std::fs::create_dir_all(self.logs_dir())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("relava-dir-test-{}-{}", std::process::id(), id))
    }

    #[test]
    fn paths_are_correct() {
        let root = test_root();
        let rd = RelavaDir::new(root.clone());

        assert_eq!(rd.root(), &root);
        assert_eq!(rd.db_path(), root.join("db.sqlite"));
        assert_eq!(rd.config_path(), root.join("config.toml"));
        assert_eq!(rd.store_dir(), root.join("store"));
        assert_eq!(rd.cache_dir(), root.join("cache"));
        assert_eq!(rd.logs_dir(), root.join("logs"));
        assert_eq!(rd.type_dir(ResourceType::Skill), root.join("store/skills"));
        assert_eq!(
            rd.version_dir(ResourceType::Skill, "denden", "1.0.0"),
            root.join("store/skills/denden/1.0.0")
        );
    }

    #[test]
    fn ensure_dirs_creates_structure() {
        let root = test_root();
        let rd = RelavaDir::new(root.clone());
        rd.ensure_dirs().unwrap();

        assert!(root.join("store/skills").is_dir());
        assert!(root.join("store/agents").is_dir());
        assert!(root.join("store/commands").is_dir());
        assert!(root.join("store/rules").is_dir());
        assert!(root.join("cache").is_dir());
        assert!(root.join("logs").is_dir());
    }

    #[test]
    fn ensure_dirs_is_idempotent() {
        let root = test_root();
        let rd = RelavaDir::new(root);
        rd.ensure_dirs().unwrap();
        rd.ensure_dirs().unwrap(); // second call should not error
    }

    #[test]
    fn default_location_returns_some() {
        // In most environments, home_dir is available
        assert!(RelavaDir::default_location().is_some());
    }
}
