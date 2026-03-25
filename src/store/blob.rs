use std::path::PathBuf;

use super::traits::{BlobStore, StoreError};

/// Local filesystem implementation of `BlobStore`.
///
/// All paths are relative to a root directory (typically `~/.relava/store/`).
pub struct LocalBlobStore {
    root: PathBuf,
}

impl LocalBlobStore {
    /// Create a new blob store rooted at the given directory.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve a relative path against the store root.
    fn resolve(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

impl BlobStore for LocalBlobStore {
    fn store(&self, path: &str, data: &[u8]) -> Result<(), StoreError> {
        let full = self.resolve(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError::Io(format!("failed to create directory: {e}")))?;
        }
        std::fs::write(&full, data)
            .map_err(|e| StoreError::Io(format!("failed to write {}: {e}", full.display())))
    }

    fn fetch(&self, path: &str) -> Result<Vec<u8>, StoreError> {
        let full = self.resolve(path);
        std::fs::read(&full).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                StoreError::NotFound(format!("blob not found: {path}"))
            }
            _ => StoreError::Io(format!("failed to read {}: {e}", full.display())),
        })
    }

    fn delete(&self, path: &str) -> Result<(), StoreError> {
        let full = self.resolve(path);
        if full.is_dir() {
            std::fs::remove_dir_all(&full)
                .map_err(|e| StoreError::Io(format!("failed to delete {}: {e}", full.display())))
        } else if full.exists() {
            std::fs::remove_file(&full)
                .map_err(|e| StoreError::Io(format!("failed to delete {}: {e}", full.display())))
        } else {
            Ok(()) // already gone
        }
    }

    fn exists(&self, path: &str) -> Result<bool, StoreError> {
        Ok(self.resolve(path).exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (PathBuf, LocalBlobStore) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "relava-blob-test-{}-{}",
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = LocalBlobStore::new(root.clone());
        (root, store)
    }

    #[test]
    fn store_and_fetch() {
        let (_root, store) = test_store();
        store.store("skills/denden/1.0.0/SKILL.md", b"hello").unwrap();
        let data = store.fetch("skills/denden/1.0.0/SKILL.md").unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn store_creates_parent_dirs() {
        let (root, store) = test_store();
        store.store("a/b/c/file.txt", b"data").unwrap();
        assert!(root.join("a/b/c/file.txt").exists());
    }

    #[test]
    fn fetch_not_found() {
        let (_root, store) = test_store();
        let result = store.fetch("nonexistent");
        assert!(matches!(result, Err(StoreError::NotFound(_))));
    }

    #[test]
    fn exists_check() {
        let (_root, store) = test_store();
        assert!(!store.exists("foo.txt").unwrap());
        store.store("foo.txt", b"bar").unwrap();
        assert!(store.exists("foo.txt").unwrap());
    }

    #[test]
    fn delete_file() {
        let (_root, store) = test_store();
        store.store("foo.txt", b"bar").unwrap();
        assert!(store.exists("foo.txt").unwrap());
        store.delete("foo.txt").unwrap();
        assert!(!store.exists("foo.txt").unwrap());
    }

    #[test]
    fn delete_directory() {
        let (_root, store) = test_store();
        store.store("dir/a.txt", b"a").unwrap();
        store.store("dir/b.txt", b"b").unwrap();
        store.delete("dir").unwrap();
        assert!(!store.exists("dir").unwrap());
    }

    #[test]
    fn delete_nonexistent_is_ok() {
        let (_root, store) = test_store();
        store.delete("nonexistent").unwrap();
    }
}
