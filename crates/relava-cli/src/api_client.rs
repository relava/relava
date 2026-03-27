//! HTTP client for the Relava registry server REST API.
//!
//! Wraps all `/api/v1` endpoints with proper error handling and the
//! standard "server not running" message when the server is unreachable.
//! Every CLI command that needs registry data should go through this module.

use serde::Deserialize;

/// Errors from API client operations.
#[derive(Debug)]
pub enum ApiError {
    /// Server is not running or unreachable.
    ServerNotRunning(String),
    /// Resource not found (404).
    NotFound(String),
    /// Resource already exists (409).
    AlreadyExists(String),
    /// Validation error (422).
    ValidationError(String),
    /// Other HTTP or deserialization error.
    Http(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerNotRunning(_url) => {
                write!(
                    f,
                    "Registry server not running. Start it with `relava server start`."
                )
            }
            Self::NotFound(msg) => write!(f, "{msg}"),
            Self::AlreadyExists(msg) => write!(f, "{msg}"),
            Self::ValidationError(msg) => write!(f, "validation error: {msg}"),
            Self::Http(msg) => write!(f, "HTTP error: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// A resource entry returned by the server.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct ResourceResponse {
    pub name: String,
    #[serde(rename = "type")]
    pub resource_type: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub latest_version: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A version entry returned by the server.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct VersionResponse {
    pub version: String,
    #[serde(default)]
    pub checksum: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
}

/// JSON error body returned by the server.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: String,
}

/// HTTP client for the Relava server REST API.
pub struct ApiClient {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl ApiClient {
    /// Create a new API client pointing at the given server URL.
    pub fn new(server_url: &str) -> Self {
        let base_url = server_url.trim_end_matches('/').to_string();
        Self {
            base_url,
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Map a reqwest send error to the appropriate `ApiError`.
    fn map_send_error(&self, e: reqwest::Error) -> ApiError {
        if e.is_connect() || e.is_timeout() {
            ApiError::ServerNotRunning(self.base_url.clone())
        } else {
            ApiError::Http(e.to_string())
        }
    }

    /// Send a GET request and return the raw response.
    fn get(&self, path: &str) -> Result<reqwest::blocking::Response, ApiError> {
        let url = format!("{}{path}", self.base_url);
        self.client
            .get(&url)
            .send()
            .map_err(|e| self.map_send_error(e))
    }

    /// Parse a non-success response into an `ApiError`.
    fn error_from_response(
        &self,
        status: reqwest::StatusCode,
        response: reqwest::blocking::Response,
    ) -> ApiError {
        let body: Option<ErrorBody> = response.json().ok();
        let msg = body
            .map(|b| b.error)
            .unwrap_or_else(|| format!("server returned status {status}"));

        if status == reqwest::StatusCode::NOT_FOUND {
            ApiError::NotFound(msg)
        } else if status == reqwest::StatusCode::CONFLICT {
            ApiError::AlreadyExists(msg)
        } else if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            ApiError::ValidationError(msg)
        } else {
            ApiError::Http(msg)
        }
    }

    /// Check that the server is reachable.
    pub fn health_check(&self) -> Result<(), ApiError> {
        self.get("/api/v1/health")?;
        Ok(())
    }

    /// List resources, optionally filtered by type.
    pub fn list_resources(
        &self,
        type_filter: Option<&str>,
    ) -> Result<Vec<ResourceResponse>, ApiError> {
        let path = match type_filter {
            Some(t) => format!("/api/v1/resources?type={t}"),
            None => "/api/v1/resources".to_string(),
        };
        let response = self.get(&path)?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.error_from_response(status, response));
        }
        response
            .json()
            .map_err(|e| ApiError::Http(format!("failed to parse response: {e}")))
    }

    /// Get a single resource by type and name.
    pub fn get_resource(
        &self,
        resource_type: &str,
        name: &str,
    ) -> Result<ResourceResponse, ApiError> {
        let response = self.get(&format!("/api/v1/resources/{resource_type}/{name}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.error_from_response(status, response));
        }
        response
            .json()
            .map_err(|e| ApiError::Http(format!("failed to parse response: {e}")))
    }

    /// Create a resource in the registry.
    #[allow(dead_code)]
    pub fn create_resource(
        &self,
        resource_type: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<ResourceResponse, ApiError> {
        let url = format!(
            "{}/api/v1/resources/{resource_type}/{name}",
            self.base_url
        );
        let body = serde_json::json!({ "description": description });
        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| self.map_send_error(e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.error_from_response(status, response));
        }
        response
            .json()
            .map_err(|e| ApiError::Http(format!("failed to parse response: {e}")))
    }

    /// Delete a resource from the registry.
    pub fn delete_resource(&self, resource_type: &str, name: &str) -> Result<(), ApiError> {
        let url = format!(
            "{}/api/v1/resources/{resource_type}/{name}",
            self.base_url
        );
        let response = self
            .client
            .delete(&url)
            .send()
            .map_err(|e| self.map_send_error(e))?;
        let status = response.status();
        if status == reqwest::StatusCode::NO_CONTENT || status.is_success() {
            Ok(())
        } else {
            Err(self.error_from_response(status, response))
        }
    }

    /// List versions for a resource.
    #[allow(dead_code)]
    pub fn list_versions(
        &self,
        resource_type: &str,
        name: &str,
    ) -> Result<Vec<VersionResponse>, ApiError> {
        let response = self.get(&format!(
            "/api/v1/resources/{resource_type}/{name}/versions"
        ))?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.error_from_response(status, response));
        }
        response
            .json()
            .map_err(|e| ApiError::Http(format!("failed to parse response: {e}")))
    }

    /// Get a specific version of a resource.
    #[allow(dead_code)]
    pub fn get_version(
        &self,
        resource_type: &str,
        name: &str,
        version: &str,
    ) -> Result<VersionResponse, ApiError> {
        let response = self.get(&format!(
            "/api/v1/resources/{resource_type}/{name}/versions/{version}"
        ))?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.error_from_response(status, response));
        }
        response
            .json()
            .map_err(|e| ApiError::Http(format!("failed to parse response: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_server_not_running_message() {
        let err = ApiError::ServerNotRunning("http://localhost:7420".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Registry server not running"));
        assert!(msg.contains("relava server start"));
    }

    #[test]
    fn api_error_not_found_message() {
        let err = ApiError::NotFound("skill 'denden' not found".to_string());
        assert_eq!(err.to_string(), "skill 'denden' not found");
    }

    #[test]
    fn api_error_already_exists_message() {
        let err = ApiError::AlreadyExists("skill 'denden' already exists".to_string());
        assert_eq!(err.to_string(), "skill 'denden' already exists");
    }

    #[test]
    fn api_error_validation_message() {
        let err = ApiError::ValidationError("invalid slug".to_string());
        assert!(err.to_string().contains("invalid slug"));
    }

    #[test]
    fn client_trims_trailing_slash() {
        let client = ApiClient::new("http://localhost:7420/");
        assert_eq!(client.base_url, "http://localhost:7420");
    }

    #[test]
    fn client_preserves_url_without_trailing_slash() {
        let client = ApiClient::new("http://localhost:7420");
        assert_eq!(client.base_url, "http://localhost:7420");
    }

    #[test]
    fn list_resources_server_unreachable() {
        // Use a port that's very unlikely to be listening
        let client = ApiClient::new("http://127.0.0.1:19999");
        let result = client.list_resources(None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Registry server not running"),
            "got: {}",
            err
        );
    }

    #[test]
    fn get_resource_server_unreachable() {
        let client = ApiClient::new("http://127.0.0.1:19999");
        let result = client.get_resource("skill", "denden");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Registry server not running"));
    }

    #[test]
    fn health_check_server_unreachable() {
        let client = ApiClient::new("http://127.0.0.1:19999");
        let result = client.health_check();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Registry server not running"));
    }

    #[test]
    fn delete_resource_server_unreachable() {
        let client = ApiClient::new("http://127.0.0.1:19999");
        let result = client.delete_resource("skill", "denden");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Registry server not running"));
    }

    #[test]
    fn resource_response_deserializes() {
        let json = r#"{"name":"denden","type":"skill","description":"A skill"}"#;
        let resp: ResourceResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.name, "denden");
        assert_eq!(resp.resource_type, "skill");
        assert_eq!(resp.description.as_deref(), Some("A skill"));
        assert!(resp.latest_version.is_none());
    }

    #[test]
    fn resource_response_deserializes_minimal() {
        let json = r#"{"name":"denden","type":"skill"}"#;
        let resp: ResourceResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.name, "denden");
        assert!(resp.description.is_none());
    }

    #[test]
    fn version_response_deserializes() {
        let json = r#"{"version":"1.0.0","checksum":"abc123"}"#;
        let resp: VersionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.version, "1.0.0");
        assert_eq!(resp.checksum.as_deref(), Some("abc123"));
        assert!(resp.published_at.is_none());
    }
}
