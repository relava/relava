use serde::Deserialize;

use crate::validate::ResourceType;
use crate::version::{Version, VersionConstraint, VersionError};

/// Errors from registry operations.
#[derive(Debug)]
pub enum RegistryError {
    /// Server is unreachable.
    ServerUnreachable(String),
    /// Resource not found in registry.
    ResourceNotFound { resource_type: String, name: String },
    /// Requested version not found.
    VersionNotFound { name: String, version: String },
    /// Version resolution failed.
    VersionResolution(VersionError),
    /// HTTP or deserialization error.
    Http(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerUnreachable(url) => {
                write!(
                    f,
                    "server not reachable at {url}. Run 'relava server start' first."
                )
            }
            Self::ResourceNotFound {
                resource_type,
                name,
            } => {
                write!(f, "{resource_type} '{name}' not found in registry")
            }
            Self::VersionNotFound { name, version } => {
                write!(f, "version {version} of '{name}' not found in registry")
            }
            Self::VersionResolution(e) => write!(f, "version resolution failed: {e}"),
            Self::Http(msg) => write!(f, "HTTP error: {msg}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Response from GET /resources/:type/:name/versions
#[derive(Debug, Deserialize)]
pub struct VersionListResponse {
    pub versions: Vec<VersionEntry>,
}

/// A single version entry from the versions list endpoint.
#[derive(Debug, Deserialize)]
pub struct VersionEntry {
    pub version: String,
}

/// A file entry in the download response.
#[derive(Debug, Deserialize)]
pub struct DownloadFile {
    /// Relative path within the resource (e.g. "SKILL.md", "templates/foo.md")
    pub path: String,
    /// Base64-encoded file content
    pub content: String,
}

/// Response from GET /resources/:type/:name/versions/:version/download
#[derive(Debug, Deserialize)]
pub struct DownloadResponse {
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub files: Vec<DownloadFile>,
}

/// HTTP client for the Relava registry server.
pub struct RegistryClient {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl RegistryClient {
    pub fn new(base_url: &str) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            base_url,
            client: reqwest::blocking::Client::new(),
        }
    }

    /// Check that the server is reachable.
    pub fn health_check(&self) -> Result<(), RegistryError> {
        let url = format!("{}/api/v1/health", self.base_url);
        self.client
            .get(&url)
            .send()
            .map_err(|_| RegistryError::ServerUnreachable(self.base_url.clone()))?;
        Ok(())
    }

    /// List available versions for a resource.
    pub fn list_versions(
        &self,
        resource_type: ResourceType,
        name: &str,
    ) -> Result<Vec<Version>, RegistryError> {
        let url = format!(
            "{}/api/v1/resources/{}/{}/versions",
            self.base_url, resource_type, name
        );

        let response = self.client.get(&url).send().map_err(|e| {
            if e.is_connect() {
                RegistryError::ServerUnreachable(self.base_url.clone())
            } else {
                RegistryError::Http(e.to_string())
            }
        })?;

        if response.status().as_u16() == 404 {
            return Err(RegistryError::ResourceNotFound {
                resource_type: resource_type.to_string(),
                name: name.to_string(),
            });
        }

        if !response.status().is_success() {
            return Err(RegistryError::Http(format!(
                "server returned status {}",
                response.status()
            )));
        }

        let body: VersionListResponse = response
            .json()
            .map_err(|e| RegistryError::Http(e.to_string()))?;

        let mut versions = Vec::new();
        for entry in &body.versions {
            let v = Version::parse(&entry.version).map_err(RegistryError::VersionResolution)?;
            versions.push(v);
        }
        Ok(versions)
    }

    /// Resolve a version constraint against the registry.
    ///
    /// If `version_pin` is `Some`, it is treated as an exact pin.
    /// If `None`, resolves to the latest available version.
    pub fn resolve_version(
        &self,
        resource_type: ResourceType,
        name: &str,
        version_pin: Option<&str>,
    ) -> Result<Version, RegistryError> {
        let constraint = match version_pin {
            Some(v) => VersionConstraint::parse(v).map_err(RegistryError::VersionResolution)?,
            None => VersionConstraint::Latest,
        };

        let available = self.list_versions(resource_type, name)?;
        constraint.resolve(&available).map_err(|e| match e {
            VersionError::VersionNotFound(v) => RegistryError::VersionNotFound {
                name: name.to_string(),
                version: v,
            },
            other => RegistryError::VersionResolution(other),
        })
    }

    /// Download resource files for a specific version.
    pub fn download(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
    ) -> Result<DownloadResponse, RegistryError> {
        let url = format!(
            "{}/api/v1/resources/{}/{}/versions/{}/download",
            self.base_url, resource_type, name, version
        );

        let response = self.client.get(&url).send().map_err(|e| {
            if e.is_connect() {
                RegistryError::ServerUnreachable(self.base_url.clone())
            } else {
                RegistryError::Http(e.to_string())
            }
        })?;

        if response.status().as_u16() == 404 {
            return Err(RegistryError::VersionNotFound {
                name: name.to_string(),
                version: version.to_string(),
            });
        }

        if !response.status().is_success() {
            return Err(RegistryError::Http(format!(
                "server returned status {}",
                response.status()
            )));
        }

        response
            .json()
            .map_err(|e| RegistryError::Http(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_error_display_server_unreachable() {
        let err = RegistryError::ServerUnreachable("http://localhost:7420".to_string());
        let msg = err.to_string();
        assert!(msg.contains("not reachable"));
        assert!(msg.contains("localhost:7420"));
    }

    #[test]
    fn registry_error_display_resource_not_found() {
        let err = RegistryError::ResourceNotFound {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
        };
        assert!(err.to_string().contains("denden"));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn registry_error_display_version_not_found() {
        let err = RegistryError::VersionNotFound {
            name: "denden".to_string(),
            version: "1.0.0".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("1.0.0"));
        assert!(msg.contains("denden"));
    }

    #[test]
    fn client_trims_trailing_slash() {
        let client = RegistryClient::new("http://localhost:7420/");
        assert_eq!(client.base_url, "http://localhost:7420");
    }
}
