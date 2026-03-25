use super::models::{Resource, Version};

/// Errors from store operations.
#[derive(Debug)]
pub enum StoreError {
    /// Database or I/O error.
    Io(String),
    /// Resource not found.
    NotFound(String),
    /// Duplicate resource or version.
    AlreadyExists(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "store I/O error: {msg}"),
            Self::NotFound(msg) => write!(f, "not found: {msg}"),
            Self::AlreadyExists(msg) => write!(f, "already exists: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Metadata store for resources and versions (backed by SQLite in MVP).
pub trait ResourceStore {
    fn get_resource(
        &self,
        scope: Option<&str>,
        name: &str,
        resource_type: &str,
    ) -> Result<Resource, StoreError>;

    fn list_versions(&self, resource_id: i64) -> Result<Vec<Version>, StoreError>;

    fn publish(&self, resource: &Resource, version: &Version) -> Result<(), StoreError>;

    fn search(
        &self,
        query: &str,
        resource_type: Option<&str>,
    ) -> Result<Vec<Resource>, StoreError>;
}

/// Blob store for resource files (backed by local filesystem in MVP).
pub trait BlobStore {
    fn store(&self, path: &str, data: &[u8]) -> Result<(), StoreError>;

    fn fetch(&self, path: &str) -> Result<Vec<u8>, StoreError>;

    fn delete(&self, path: &str) -> Result<(), StoreError>;

    fn exists(&self, path: &str) -> Result<bool, StoreError>;
}
