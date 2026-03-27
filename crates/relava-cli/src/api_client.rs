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
            Self::ServerNotRunning(url) => {
                write!(
                    f,
                    "Registry server not running at {url}. Start it with `relava server start`."
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

    /// Build the full URL for an API path.
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Send a request builder, mapping connection errors appropriately.
    fn send(
        &self,
        builder: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::Response, ApiError> {
        builder.send().map_err(|e| self.map_send_error(e))
    }

    /// Check that a response is successful, or convert it to an `ApiError`.
    fn check_response(
        &self,
        response: reqwest::blocking::Response,
    ) -> Result<reqwest::blocking::Response, ApiError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let msg = response
            .json::<ErrorBody>()
            .ok()
            .map(|b| b.error)
            .unwrap_or_else(|| format!("server returned status {status}"));

        Err(match status {
            reqwest::StatusCode::NOT_FOUND => ApiError::NotFound(msg),
            reqwest::StatusCode::CONFLICT => ApiError::AlreadyExists(msg),
            reqwest::StatusCode::UNPROCESSABLE_ENTITY => ApiError::ValidationError(msg),
            _ => ApiError::Http(msg),
        })
    }

    /// Send a GET request, check for success, and parse the JSON body.
    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let response = self.send(self.client.get(self.url(path)))?;
        self.check_response(response)?
            .json()
            .map_err(|e| ApiError::Http(format!("failed to parse response: {e}")))
    }

    /// Check that the server is reachable.
    pub fn health_check(&self) -> Result<(), ApiError> {
        let resp = self.send(self.client.get(self.url("/api/v1/health")))?;
        self.check_response(resp)?;
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
        self.get_json(&path)
    }

    /// Get a single resource by type and name.
    pub fn get_resource(
        &self,
        resource_type: &str,
        name: &str,
    ) -> Result<ResourceResponse, ApiError> {
        self.get_json(&format!("/api/v1/resources/{resource_type}/{name}"))
    }

    /// Create a resource in the registry.
    #[allow(dead_code)]
    pub fn create_resource(
        &self,
        resource_type: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<ResourceResponse, ApiError> {
        let url = self.url(&format!("/api/v1/resources/{resource_type}/{name}"));
        let builder = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "description": description }));
        let response = self.send(builder)?;
        self.check_response(response)?
            .json()
            .map_err(|e| ApiError::Http(format!("failed to parse response: {e}")))
    }

    /// Delete a resource from the registry.
    pub fn delete_resource(&self, resource_type: &str, name: &str) -> Result<(), ApiError> {
        let url = self.url(&format!("/api/v1/resources/{resource_type}/{name}"));
        let response = self.send(self.client.delete(&url))?;
        self.check_response(response)?;
        Ok(())
    }

    /// List versions for a resource.
    #[allow(dead_code)]
    pub fn list_versions(
        &self,
        resource_type: &str,
        name: &str,
    ) -> Result<Vec<VersionResponse>, ApiError> {
        self.get_json(&format!(
            "/api/v1/resources/{resource_type}/{name}/versions"
        ))
    }

    /// Get a specific version of a resource.
    #[allow(dead_code)]
    pub fn get_version(
        &self,
        resource_type: &str,
        name: &str,
        version: &str,
    ) -> Result<VersionResponse, ApiError> {
        self.get_json(&format!(
            "/api/v1/resources/{resource_type}/{name}/versions/{version}"
        ))
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
