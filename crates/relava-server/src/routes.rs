use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::resolve;
use crate::store::db::SqliteResourceStore;
use crate::store::models::{Resource, Version};
use crate::store::traits::{BlobStore, ResourceStore, StoreError};
use relava_types::validate::{ResourceType, validate_slug, validate_version};
use relava_types::version::Version as SemVer;

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

/// Consistent JSON error response returned by all endpoints.
#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

impl ApiError {
    fn new(msg: impl Into<String>) -> Self {
        Self { error: msg.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(self)).into_response()
    }
}

/// Convert a `StoreError` into an HTTP response with the correct status code.
///
/// Internal errors (Database, Io) are logged but not exposed to clients.
fn store_err(e: StoreError) -> Response {
    let (status, msg) = match &e {
        StoreError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
        StoreError::AlreadyExists(msg) => (StatusCode::CONFLICT, msg.clone()),
        StoreError::Database(msg) => {
            eprintln!("[relava-server] database error: {msg}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
            )
        }
        StoreError::Io(err) => {
            eprintln!("[relava-server] I/O error: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
            )
        }
    };
    (status, Json(ApiError::new(msg))).into_response()
}

/// Acquire the store lock, returning 500 if the Mutex is poisoned.
#[allow(clippy::result_large_err)]
fn acquire_store(
    state: &AppState,
) -> Result<std::sync::MutexGuard<'_, SqliteResourceStore>, Response> {
    state.store.lock().map_err(|_| {
        eprintln!("[relava-server] store mutex poisoned");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError::new("internal server error")),
        )
            .into_response()
    })
}

/// Return a 422 Unprocessable Entity with a validation message.
fn validation_err(msg: impl Into<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(ApiError::new(msg.into())),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Path / query helpers
// ---------------------------------------------------------------------------

/// Parse and validate a resource type from the URL path segment.
#[allow(clippy::result_large_err)]
fn parse_resource_type(s: &str) -> Result<ResourceType, Response> {
    ResourceType::from_str(s).map_err(|_| {
        validation_err(format!(
            "invalid resource type '{s}': must be skill, agent, command, or rule"
        ))
    })
}

/// Parse and validate a resource name (slug) from the URL path segment.
#[allow(clippy::result_large_err)]
fn parse_name(s: &str) -> Result<(), Response> {
    validate_slug(s).map_err(|e| validation_err(e.to_string()))
}

/// Parse and validate a semver version string from the URL path segment.
#[allow(clippy::result_large_err)]
fn parse_version(s: &str) -> Result<(), Response> {
    validate_version(s)
        .map(|_| ())
        .map_err(|e| validation_err(e.to_string()))
}

/// Validate both resource type and name from URL path segments.
#[allow(clippy::result_large_err)]
fn validate_resource_path(rtype: &str, name: &str) -> Result<ResourceType, Response> {
    let rt = parse_resource_type(rtype)?;
    parse_name(name)?;
    Ok(rt)
}

// ---------------------------------------------------------------------------
// JSON response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ResourceResponse {
    name: String,
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

impl From<Resource> for ResourceResponse {
    fn from(r: Resource) -> Self {
        Self {
            name: r.name,
            resource_type: r.resource_type,
            description: r.description,
            latest_version: r.latest_version,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Serialize)]
struct VersionResponse {
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    published_at: Option<String>,
}

impl From<Version> for VersionResponse {
    fn from(v: Version) -> Self {
        Self {
            version: v.version,
            checksum: v.checksum,
            published_at: v.published_at,
        }
    }
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(rename = "type")]
    resource_type: Option<String>,
    /// Full-text search query. When present, returns ranked search results.
    q: Option<String>,
}

#[derive(Deserialize)]
struct CreateBody {
    description: Option<String>,
}

#[derive(Deserialize)]
struct ResolveQuery {
    version: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/resources?type=skill&q=search+term
///
/// When `q` is provided, performs a full-text search using FTS5 and returns
/// ranked results. Otherwise lists all resources with optional type filter.
async fn list_resources(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> Response {
    let rt = match &query.resource_type {
        Some(t) => match parse_resource_type(t) {
            Ok(rt) => Some(rt),
            Err(resp) => return resp,
        },
        None => None,
    };

    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let result = if let Some(ref q) = query.q {
        store.search(q, rt)
    } else {
        store.list_resources(rt)
    };

    match result {
        Ok(resources) => {
            let body: Vec<ResourceResponse> = resources.into_iter().map(Into::into).collect();
            Json(body).into_response()
        }
        Err(e) => store_err(e),
    }
}

/// GET /api/v1/resources/:type/:name
async fn get_resource(
    State(state): State<Arc<AppState>>,
    Path((rtype, name)): Path<(String, String)>,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };

    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match store.get_resource(None, &name, rt) {
        Ok(resource) => Json(ResourceResponse::from(resource)).into_response(),
        Err(e) => store_err(e),
    }
}

/// POST /api/v1/resources/:type/:name
async fn create_resource(
    State(state): State<Arc<AppState>>,
    Path((rtype, name)): Path<(String, String)>,
    Json(body): Json<CreateBody>,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };

    let resource = Resource {
        id: 0,
        scope: None,
        name: name.clone(),
        resource_type: rt.to_string(),
        description: body.description,
        latest_version: None,
        metadata_json: None,
        updated_at: None,
    };

    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match store.create_resource(&resource) {
        Ok(created) => (StatusCode::CREATED, Json(ResourceResponse::from(created))).into_response(),
        Err(e) => store_err(e),
    }
}

/// DELETE /api/v1/resources/:type/:name
async fn delete_resource(
    State(state): State<Arc<AppState>>,
    Path((rtype, name)): Path<(String, String)>,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };

    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match store.delete_resource(None, &name, rt) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => store_err(e),
    }
}

/// GET /api/v1/resources/:type/:name/versions
async fn list_versions(
    State(state): State<Arc<AppState>>,
    Path((rtype, name)): Path<(String, String)>,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };

    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let resource = match store.get_resource(None, &name, rt) {
        Ok(r) => r,
        Err(e) => return store_err(e),
    };

    match store.list_versions(resource.id) {
        Ok(versions) => {
            let body: Vec<VersionResponse> = versions.into_iter().map(Into::into).collect();
            Json(body).into_response()
        }
        Err(e) => store_err(e),
    }
}

/// GET /api/v1/resources/:type/:name/versions/:version
async fn get_version(
    State(state): State<Arc<AppState>>,
    Path((rtype, name, version)): Path<(String, String, String)>,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };
    if let Err(resp) = parse_version(&version) {
        return resp;
    }

    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let resource = match store.get_resource(None, &name, rt) {
        Ok(r) => r,
        Err(e) => return store_err(e),
    };

    match store.get_version(resource.id, &version) {
        Ok(v) => Json(VersionResponse::from(v)).into_response(),
        Err(e) => store_err(e),
    }
}

/// Per-file checksum entry in the checksums response.
#[derive(Debug, Serialize, Deserialize)]
struct FileChecksumEntry {
    path: String,
    sha256: String,
}

/// Response from the checksums endpoint.
#[derive(Debug, Serialize)]
struct ChecksumsResponse {
    version: String,
    files: Vec<FileChecksumEntry>,
}

/// GET /api/v1/resources/:type/:name/versions/:version/checksums
///
/// Returns per-file SHA-256 checksums for a published version.
/// Used by the CLI for change detection before publishing.
async fn get_version_checksums(
    State(state): State<Arc<AppState>>,
    Path((rtype, name, version)): Path<(String, String, String)>,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };
    if let Err(resp) = parse_version(&version) {
        return resp;
    }

    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let resource = match store.get_resource(None, &name, rt) {
        Ok(r) => r,
        Err(e) => return store_err(e),
    };

    let ver = match store.get_version(resource.id, &version) {
        Ok(v) => v,
        Err(e) => return store_err(e),
    };

    // Legacy versions without manifest_json have no checksums to return.
    let manifest_str = match ver.manifest_json.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiError::new(format!(
                    "no checksums available for {rtype}/{name}@{version} (legacy version)"
                ))),
            )
                .into_response();
        }
    };

    // Parse per-file checksums from manifest_json, propagating corrupt data as 500.
    let log_and_500 = |detail: &str, err: &dyn std::fmt::Display| -> Response {
        let label = if detail.is_empty() {
            String::new()
        } else {
            format!(" {detail}")
        };
        eprintln!(
            "[relava-server] corrupt manifest_json{label} for {rtype}/{name}@{version}: {err}"
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError::new("internal server error")),
        )
            .into_response()
    };

    let parsed: serde_json::Value = match serde_json::from_str(manifest_str) {
        Ok(v) => v,
        Err(e) => return log_and_500("", &e),
    };

    let files_val = parsed.get("files").cloned().unwrap_or_default();
    let files: Vec<FileChecksumEntry> = match serde_json::from_value(files_val) {
        Ok(f) => f,
        Err(e) => return log_and_500("files", &e),
    };

    Json(ChecksumsResponse {
        version: ver.version,
        files,
    })
    .into_response()
}

/// GET /api/v1/resolve/:type/:name?version=<ver>
async fn resolve_deps(
    State(state): State<Arc<AppState>>,
    Path((rtype, name)): Path<(String, String)>,
    Query(query): Query<ResolveQuery>,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };

    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    match resolve::resolve(&*store, rt, &name, query.version.as_deref()) {
        Ok(response) => Json(response).into_response(),
        Err(resolve::ResolveError::NotFound(msg)) => {
            (StatusCode::NOT_FOUND, Json(ApiError::new(msg))).into_response()
        }
        Err(e @ resolve::ResolveError::CyclicDependency(_))
        | Err(e @ resolve::ResolveError::DepthLimitExceeded { .. }) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ApiError::new(e.to_string())),
        )
            .into_response(),
        Err(resolve::ResolveError::Store(e)) => store_err(e),
    }
}

// ---------------------------------------------------------------------------
// Publish types
// ---------------------------------------------------------------------------

/// Metadata for a published file (sent as the "metadata" multipart field).
#[derive(Debug, Deserialize)]
struct PublishMetadata {
    files: Vec<PublishFileEntry>,
}

#[derive(Debug, Deserialize)]
struct PublishFileEntry {
    path: String,
    sha256: String,
    #[allow(dead_code)]
    size: u64,
}

/// Response from the publish endpoint.
#[derive(Debug, Serialize)]
struct PublishResponseBody {
    name: String,
    #[serde(rename = "type")]
    resource_type: String,
    version: String,
}

// ---------------------------------------------------------------------------
// Publish handler
// ---------------------------------------------------------------------------

/// Maximum body size for publish uploads (50 MB + overhead).
const MAX_PUBLISH_BODY: usize = 55 * 1024 * 1024;

/// POST /api/v1/resources/:type/:name/publish
///
/// Accepts a multipart form with:
/// - `metadata`: JSON with file checksums
/// - `file` (repeated): resource files
///
/// Auto-increments the patch version if no version is found in frontmatter.
async fn publish_resource(
    State(state): State<Arc<AppState>>,
    Path((rtype, name)): Path<(String, String)>,
    mut multipart: Multipart,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };

    // Parse multipart fields
    let mut metadata: Option<PublishMetadata> = None;
    let mut file_data: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total_size: u64 = 0;

    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let field_name = field.name().unwrap_or("").to_string();

                match field_name.as_str() {
                    "metadata" => {
                        let bytes = match field.bytes().await {
                            Ok(b) => b,
                            Err(e) => {
                                return validation_err(format!("failed to read metadata: {e}"));
                            }
                        };
                        metadata = match serde_json::from_slice(&bytes) {
                            Ok(m) => Some(m),
                            Err(e) => return validation_err(format!("invalid metadata JSON: {e}")),
                        };
                    }
                    "file" => {
                        let filename = field.file_name().unwrap_or("unknown").to_string();

                        // Path traversal protection
                        if is_unsafe_path(&filename) {
                            return validation_err(format!(
                                "unsafe filename rejected: '{filename}'"
                            ));
                        }

                        let bytes = match field.bytes().await {
                            Ok(b) => b,
                            Err(e) => {
                                return validation_err(format!(
                                    "failed to read file '{filename}': {e}"
                                ));
                            }
                        };
                        total_size += bytes.len() as u64;
                        if total_size > MAX_PUBLISH_BODY as u64 {
                            return validation_err("upload exceeds maximum size limit");
                        }
                        file_data.push((filename, bytes.to_vec()));
                    }
                    _ => {
                        // Ignore unknown fields
                    }
                }
            }
            Ok(None) => break,
            Err(e) => return validation_err(format!("multipart parse error: {e}")),
        }
    }

    let metadata = match metadata {
        Some(m) => m,
        None => return validation_err("missing 'metadata' field in multipart upload"),
    };

    if file_data.is_empty() {
        return validation_err("no files uploaded");
    }

    // Validate file limits
    if file_data.len() > 100 {
        return validation_err(format!("too many files: {} (max 100)", file_data.len()));
    }

    for (filename, data) in &file_data {
        if data.len() as u64 > 10 * 1024 * 1024 {
            return validation_err(format!("file '{filename}' exceeds 10 MB limit"));
        }
    }

    if total_size > 50 * 1024 * 1024 {
        return validation_err("total upload size exceeds 50 MB limit");
    }

    // Validate metadata paths for traversal attacks
    for meta_file in &metadata.files {
        if is_unsafe_path(&meta_file.path) {
            return validation_err(format!(
                "unsafe path in metadata rejected: '{}'",
                meta_file.path
            ));
        }
    }

    // Verify checksums match (forward: metadata → uploads)
    for meta_file in &metadata.files {
        let uploaded = file_data.iter().find(|(name, _)| *name == meta_file.path);
        match uploaded {
            Some((_, data)) => {
                let computed = sha256_hex(data);
                if computed != meta_file.sha256 {
                    return validation_err(format!(
                        "checksum mismatch for '{}': expected {}, got {}",
                        meta_file.path, meta_file.sha256, computed
                    ));
                }
            }
            None => {
                return validation_err(format!(
                    "file '{}' declared in metadata but not uploaded",
                    meta_file.path
                ));
            }
        }
    }

    // Verify reverse: uploaded files must be declared in metadata
    for (filename, _) in &file_data {
        if !metadata.files.iter().any(|f| f.path == *filename) {
            return validation_err(format!(
                "file '{filename}' uploaded but not declared in metadata"
            ));
        }
    }

    // Determine version: extract from frontmatter or auto-increment
    let version_str = match determine_version(rt, &name, &file_data, &state) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // Validate semver format
    if let Err(resp) = parse_version(&version_str) {
        return resp;
    }

    // Check version monotonicity
    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let existing_resource = store.get_resource(None, &name, rt);
    if let Ok(ref resource) = existing_resource {
        // Check that the new version is newer than the latest
        if let Some(ref latest_str) = resource.latest_version {
            let latest = match SemVer::parse(latest_str) {
                Ok(v) => v,
                Err(_) => {
                    return validation_err(format!(
                        "existing latest version '{latest_str}' is not valid semver"
                    ));
                }
            };
            let new_ver = match SemVer::parse(&version_str) {
                Ok(v) => v,
                Err(_) => return validation_err(format!("'{version_str}' is not valid semver")),
            };
            if new_ver <= latest {
                return (
                    StatusCode::CONFLICT,
                    Json(ApiError::new(format!(
                        "version {version_str} is not newer than latest {latest_str}"
                    ))),
                )
                    .into_response();
            }
        }

        // Check for duplicate version
        if store.get_version(resource.id, &version_str).is_ok() {
            return (
                StatusCode::CONFLICT,
                Json(ApiError::new(format!(
                    "version {version_str} already exists for {name}"
                ))),
            )
                .into_response();
        }
    }

    // Store files via BlobStore
    let store_path = format!("{}/{name}/{version_str}", rt.store_dir_name());
    let blob_store = match &state.blob_store {
        Some(bs) => bs,
        None => {
            return validation_err("server not configured for file storage");
        }
    };

    for (filename, data) in &file_data {
        let blob_path = format!("{store_path}/{filename}");
        if let Err(e) = blob_store.store(&blob_path, data) {
            eprintln!("[relava-server] blob store error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("internal server error")),
            )
                .into_response();
        }
    }

    // Compute overall checksum (SHA-256 of all file checksums concatenated)
    let combined: String = metadata.files.iter().map(|f| f.sha256.as_str()).collect();
    let overall_checksum = sha256_hex(combined.as_bytes());

    // Publish to store
    let resource = Resource {
        id: 0,
        scope: None,
        name: name.clone(),
        resource_type: rt.to_string(),
        description: None,
        latest_version: None,
        metadata_json: None,
        updated_at: None,
    };

    // Store per-file checksums in manifest_json for change detection
    let file_checksums: Vec<serde_json::Value> = metadata
        .files
        .iter()
        .map(|f| serde_json::json!({ "path": f.path, "sha256": f.sha256 }))
        .collect();
    let manifest_json = match serde_json::to_string(&serde_json::json!({ "files": file_checksums }))
    {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[relava-server] failed to serialize manifest_json: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("internal server error")),
            )
                .into_response();
        }
    };

    let version = Version {
        id: 0,
        resource_id: 0,
        version: version_str.clone(),
        store_path: Some(store_path),
        checksum: Some(overall_checksum),
        manifest_json,
        published_by: None,
        published_at: None,
    };

    if let Err(e) = store.publish(&resource, &version) {
        return store_err(e);
    }

    // Drop store lock before responding
    drop(store);

    (
        StatusCode::CREATED,
        Json(PublishResponseBody {
            name,
            resource_type: rt.to_string(),
            version: version_str,
        }),
    )
        .into_response()
}

/// Build a tar archive from a list of `(path, data)` pairs.
fn build_tar_archive(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, std::io::Error> {
    let mut tar_data = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_data);
        for (path, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, data.as_slice())?;
        }
        builder.finish()?;
    }
    Ok(tar_data)
}

/// Check if a path contains traversal attacks or absolute paths.
fn is_unsafe_path(path: &str) -> bool {
    path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains("..")
        || path.contains('\0')
}

/// Determine the version for a publish operation.
///
/// 1. Try to extract version from frontmatter of the primary markdown file.
/// 2. If no version in frontmatter, auto-increment from the latest published version.
/// 3. If no published versions exist, default to "0.1.0".
#[allow(clippy::result_large_err)]
fn determine_version(
    rt: ResourceType,
    name: &str,
    file_data: &[(String, Vec<u8>)],
    state: &AppState,
) -> Result<String, Response> {
    // Try to extract version from frontmatter
    let primary_filename = match rt {
        ResourceType::Skill => "SKILL.md".to_string(),
        _ => format!("{name}.md"),
    };

    if let Some((_, data)) = file_data.iter().find(|(f, _)| *f == primary_filename)
        && let Ok(content) = std::str::from_utf8(data)
    {
        match extract_frontmatter_version(content) {
            Ok(Some(version)) => return Ok(version),
            Ok(None) => {} // No version in frontmatter, fall through to auto-increment
            Err(e) => return Err(validation_err(e)),
        }
    }

    // Auto-increment from latest
    let store = acquire_store(state)?;
    let latest = match store.get_resource(None, name, rt) {
        Ok(resource) => resource.latest_version,
        Err(StoreError::NotFound(_)) => None,
        Err(e) => return Err(store_err(e)),
    };

    match latest {
        Some(latest_str) => {
            let latest_ver = SemVer::parse(&latest_str)
                .map_err(|_| validation_err(format!("latest version '{latest_str}' is invalid")))?;
            Ok(format!(
                "{}.{}.{}",
                latest_ver.major,
                latest_ver.minor,
                latest_ver.patch + 1
            ))
        }
        None => Ok("0.1.0".to_string()),
    }
}

/// Extract version from YAML frontmatter.
///
/// Returns `Ok(Some(version))` if frontmatter has a version field,
/// `Ok(None)` if no frontmatter or no version field,
/// `Err` if frontmatter delimiters are present but YAML is malformed.
fn extract_frontmatter_version(content: &str) -> Result<Option<String>, String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Ok(None);
    }
    let after_open = &trimmed[3..];
    let Some(end) = after_open.find("\n---") else {
        return Ok(None); // No closing delimiter
    };
    let yaml_str = &after_open[..end];

    let yaml_value: serde_json::Value =
        serde_yaml::from_str(yaml_str).map_err(|e| format!("invalid frontmatter YAML: {e}"))?;

    Ok(yaml_value
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}

/// Compute SHA-256 hex digest of data.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}

// ---------------------------------------------------------------------------
// Download handler
// ---------------------------------------------------------------------------

/// GET /api/v1/resources/:type/:name/versions/:version/download
///
/// Serves resource files as a tar archive for the specified version.
async fn download_version(
    State(state): State<Arc<AppState>>,
    Path((rtype, name, version)): Path<(String, String, String)>,
) -> Response {
    let rt = match validate_resource_path(&rtype, &name) {
        Ok(rt) => rt,
        Err(resp) => return resp,
    };
    if let Err(resp) = parse_version(&version) {
        return resp;
    }

    // Verify resource and version exist
    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let resource = match store.get_resource(None, &name, rt) {
        Ok(r) => r,
        Err(e) => return store_err(e),
    };

    let ver = match store.get_version(resource.id, &version) {
        Ok(v) => v,
        Err(e) => return store_err(e),
    };
    drop(store);

    let store_path = match ver.store_path {
        Some(ref p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiError::new("no files stored for this version")),
            )
                .into_response();
        }
    };

    let blob_store = match &state.blob_store {
        Some(bs) => bs,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("server not configured for file storage")),
            )
                .into_response();
        }
    };

    // Collect all files under the version store path
    let files = match collect_blob_files(blob_store, &store_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[relava-server] download error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("internal server error")),
            )
                .into_response();
        }
    };

    if files.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("no files found for this version")),
        )
            .into_response();
    }

    // Build a tar archive
    let tar_data = match build_tar_archive(&files) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("[relava-server] tar build error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("internal server error")),
            )
                .into_response();
        }
    };

    (
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/x-tar".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}-{version}.tar\""),
            ),
        ],
        tar_data,
    )
        .into_response()
}

/// Collect all files under a blob store path.
///
/// For the LocalBlobStore, this walks the filesystem directory.
/// Returns `(relative_path, data)` pairs.
fn collect_blob_files(
    blob_store: &crate::store::LocalBlobStore,
    store_path: &str,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let root = blob_store.resolve(store_path);
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_blob_recursive(&root, &root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn collect_blob_recursive(
    base: &std::path::Path,
    dir: &std::path::Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read directory '{}': {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("directory read error: {e}"))?;
        let path = entry.path();

        if path.is_dir() {
            collect_blob_recursive(base, &path, files)?;
        } else {
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let data = std::fs::read(&path)
                .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
            files.push((relative, data));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Updates check
// ---------------------------------------------------------------------------

/// Request body for POST /api/v1/updates/check.
#[derive(Deserialize)]
struct UpdateCheckRequest {
    /// Resources to check: list of `{ type, name, version }`.
    resources: Vec<UpdateCheckEntry>,
}

#[derive(Deserialize)]
struct UpdateCheckEntry {
    #[serde(rename = "type")]
    resource_type: String,
    name: String,
    version: String,
}

/// A single update available.
#[derive(Serialize)]
struct UpdateAvailable {
    #[serde(rename = "type")]
    resource_type: String,
    name: String,
    installed_version: String,
    latest_version: String,
}

/// Response from POST /api/v1/updates/check.
#[derive(Serialize)]
struct UpdateCheckResponse {
    available: Vec<UpdateAvailable>,
}

/// POST /api/v1/updates/check
///
/// Accepts a list of installed resources with their versions, and returns
/// which ones have newer versions available. This endpoint is used by the
/// CLI and GUI for update notifications.
async fn check_updates(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateCheckRequest>,
) -> Response {
    let store = match acquire_store(&state) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let mut available = Vec::new();

    for entry in &body.resources {
        // Skip entries with invalid version format
        let installed = match SemVer::parse(&entry.version) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Skip entries with invalid resource type
        let rt = match ResourceType::from_str(&entry.resource_type) {
            Ok(rt) => rt,
            Err(_) => continue,
        };

        // Look up the resource to get its latest version
        let resource = match store.get_resource(None, &entry.name, rt) {
            Ok(r) => r,
            Err(_) => continue,
        };

        if let Some(ref latest_str) = resource.latest_version
            && let Ok(latest) = SemVer::parse(latest_str)
            && latest > installed
        {
            available.push(UpdateAvailable {
                resource_type: entry.resource_type.clone(),
                name: entry.name.clone(),
                installed_version: entry.version.clone(),
                latest_version: latest_str.clone(),
            });
        }
    }

    Json(UpdateCheckResponse { available }).into_response()
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the `/api/v1` resource routes.
pub fn resource_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/resources", get(list_resources))
        .route(
            "/resources/{type}/{name}",
            get(get_resource)
                .post(create_resource)
                .delete(delete_resource),
        )
        .route(
            "/resources/{type}/{name}/publish",
            axum::routing::post(publish_resource),
        )
        .route("/resources/{type}/{name}/versions", get(list_versions))
        .route(
            "/resources/{type}/{name}/versions/{version}",
            get(get_version),
        )
        .route(
            "/resources/{type}/{name}/versions/{version}/download",
            get(download_version),
        )
        .route(
            "/resources/{type}/{name}/versions/{version}/checksums",
            get(get_version_checksums),
        )
        .route("/resolve/{type}/{name}", get(resolve_deps))
        .route("/updates/check", axum::routing::post(check_updates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::SqliteResourceStore;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Mutex;
    use tower::ServiceExt;

    fn test_app() -> Router {
        crate::app_in_memory()
    }

    /// Send a request and return the response.
    async fn send(
        app: Router,
        method: &str,
        uri: &str,
        body: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        let body = if let Some(json) = body {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        } else {
            Body::empty()
        };
        app.oneshot(builder.body(body).unwrap()).await.unwrap()
    }

    /// Parse response body as JSON.
    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    // -- List resources --

    #[tokio::test]
    async fn list_resources_empty() {
        let app = test_app();
        let resp = send(app, "GET", "/api/v1/resources", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body, serde_json::json!([]));
    }

    #[tokio::test]
    async fn list_resources_with_type_filter() {
        let app = test_app();
        // Create a skill
        let resp = send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"a skill"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Filter by skill — should find it
        let resp = send(app.clone(), "GET", "/api/v1/resources?type=skill", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 1);

        // Filter by agent — should be empty
        let resp = send(app, "GET", "/api/v1/resources?type=agent", None).await;
        let body = json_body(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_resources_invalid_type_filter() {
        let app = test_app();
        let resp = send(app, "GET", "/api/v1/resources?type=invalid", None).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -- Get resource --

    #[tokio::test]
    async fn get_resource_not_found() {
        let app = test_app();
        let resp = send(app, "GET", "/api/v1/resources/skill/nonexistent", None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = json_body(resp).await;
        assert!(body["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn get_resource_returns_metadata() {
        let app = test_app();
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"Communication skill"}"#),
        )
        .await;

        let resp = send(app, "GET", "/api/v1/resources/skill/denden", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["name"], "denden");
        assert_eq!(body["type"], "skill");
        assert_eq!(body["description"], "Communication skill");
    }

    // -- Create resource --

    #[tokio::test]
    async fn create_resource_success() {
        let app = test_app();
        let resp = send(
            app,
            "POST",
            "/api/v1/resources/agent/debugger",
            Some(r#"{"description":"Debug agent"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = json_body(resp).await;
        assert_eq!(body["name"], "debugger");
        assert_eq!(body["type"], "agent");
        assert_eq!(body["description"], "Debug agent");
    }

    #[tokio::test]
    async fn create_resource_duplicate_returns_409() {
        let app = test_app();
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"first"}"#),
        )
        .await;

        let resp = send(
            app,
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"second"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn create_resource_invalid_type() {
        let app = test_app();
        let resp = send(
            app,
            "POST",
            "/api/v1/resources/invalid/denden",
            Some(r#"{"description":"test"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn create_resource_invalid_slug() {
        let app = test_app();
        let resp = send(
            app,
            "POST",
            "/api/v1/resources/skill/INVALID",
            Some(r#"{"description":"test"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -- Delete resource --

    #[tokio::test]
    async fn delete_resource_success() {
        let app = test_app();
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"test"}"#),
        )
        .await;

        let resp = send(
            app.clone(),
            "DELETE",
            "/api/v1/resources/skill/denden",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Confirm it's gone
        let resp = send(app, "GET", "/api/v1/resources/skill/denden", None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_resource_not_found() {
        let app = test_app();
        let resp = send(app, "DELETE", "/api/v1/resources/skill/nonexistent", None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- List versions --

    #[tokio::test]
    async fn list_versions_empty() {
        let app = test_app();
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"test"}"#),
        )
        .await;

        let resp = send(app, "GET", "/api/v1/resources/skill/denden/versions", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body, serde_json::json!([]));
    }

    #[tokio::test]
    async fn list_versions_resource_not_found() {
        let app = test_app();
        let resp = send(
            app,
            "GET",
            "/api/v1/resources/skill/nonexistent/versions",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Get version --

    #[tokio::test]
    async fn get_version_not_found() {
        let app = test_app();
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"test"}"#),
        )
        .await;

        let resp = send(
            app,
            "GET",
            "/api/v1/resources/skill/denden/versions/1.0.0",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_version_invalid_semver() {
        let app = test_app();
        let resp = send(
            app,
            "GET",
            "/api/v1/resources/skill/denden/versions/abc",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -- Versions via publish (integration) --

    #[tokio::test]
    async fn list_and_get_versions_after_publish() {
        // Build a test app with a pre-populated store (simulates `relava publish`).
        let store = SqliteResourceStore::open_in_memory().unwrap();
        let resource = crate::store::models::Resource {
            id: 0,
            scope: None,
            name: "denden".to_string(),
            resource_type: "skill".to_string(),
            description: Some("test".to_string()),
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        let version = crate::store::models::Version {
            id: 0,
            resource_id: 0,
            version: "1.0.0".to_string(),
            store_path: Some("skills/denden/1.0.0".to_string()),
            checksum: Some("abc123".to_string()),
            manifest_json: None,
            published_by: None,
            published_at: None,
        };
        store.publish(&resource, &version).unwrap();

        let state = Arc::new(AppState {
            started_at: std::time::Instant::now(),
            store: Mutex::new(store),
            blob_store: None,
            config: None,
        });
        let app = Router::new()
            .nest("/api/v1", resource_routes())
            .with_state(state);

        // List versions
        let resp = send(
            app.clone(),
            "GET",
            "/api/v1/resources/skill/denden/versions",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let versions = body.as_array().unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0]["version"], "1.0.0");
        assert_eq!(versions[0]["checksum"], "abc123");

        // Get specific version
        let resp = send(
            app,
            "GET",
            "/api/v1/resources/skill/denden/versions/1.0.0",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["version"], "1.0.0");
    }

    // -- Resolve endpoint --

    /// Publish a resource+version into a store with minimal boilerplate.
    fn publish(
        store: &SqliteResourceStore,
        rtype: &str,
        name: &str,
        ver: &str,
        manifest: Option<&str>,
    ) {
        use crate::store::models::{Resource as R, Version as V};
        store
            .publish(
                &R {
                    id: 0,
                    scope: None,
                    name: name.to_string(),
                    resource_type: rtype.to_string(),
                    description: None,
                    latest_version: None,
                    metadata_json: None,
                    updated_at: None,
                },
                &V {
                    id: 0,
                    resource_id: 0,
                    version: ver.to_string(),
                    store_path: None,
                    checksum: None,
                    manifest_json: manifest.map(str::to_string),
                    published_by: None,
                    published_at: None,
                },
            )
            .unwrap();
    }

    /// Build a test app from a pre-populated store.
    fn app_with_store(store: SqliteResourceStore) -> Router {
        let state = Arc::new(AppState {
            started_at: std::time::Instant::now(),
            store: Mutex::new(store),
            blob_store: None,
            config: None,
        });
        Router::new()
            .nest("/api/v1", resource_routes())
            .with_state(state)
    }

    #[tokio::test]
    async fn resolve_single_resource() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "denden", "1.0.0", None);

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resolve/skill/denden",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["root"], "skill/denden@1.0.0");
        let order = body["order"].as_array().unwrap();
        assert_eq!(order.len(), 1);
        assert_eq!(order[0]["name"], "denden");
        assert_eq!(order[0]["type"], "skill");
        assert_eq!(order[0]["version"], "1.0.0");
    }

    #[tokio::test]
    async fn resolve_with_dependencies() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(
            &store,
            "skill",
            "code-review",
            "1.0.0",
            Some(r#"{"skills":["security-baseline"]}"#),
        );
        publish(&store, "skill", "security-baseline", "0.5.0", None);

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resolve/skill/code-review",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let order = body["order"].as_array().unwrap();
        assert_eq!(order.len(), 2);
        assert_eq!(order[0]["name"], "security-baseline");
        assert_eq!(order[1]["name"], "code-review");
    }

    #[tokio::test]
    async fn resolve_with_version_query() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "denden", "1.0.0", None);
        publish(&store, "skill", "denden", "2.0.0", None);

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resolve/skill/denden?version=1.0.0",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["root"], "skill/denden@1.0.0");
    }

    #[tokio::test]
    async fn resolve_not_found() {
        let app = test_app();
        let resp = send(app, "GET", "/api/v1/resolve/skill/nonexistent", None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resolve_cycle_returns_422() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "a", "1.0.0", Some(r#"{"skills":["b"]}"#));
        publish(&store, "skill", "b", "1.0.0", Some(r#"{"skills":["a"]}"#));

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resolve/skill/a",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(resp).await;
        let error = body["error"].as_str().unwrap();
        assert!(
            error.contains("circular dependency"),
            "expected cycle error, got: {error}"
        );
        assert!(error.contains("skill/a"));
        assert!(error.contains("skill/b"));
    }

    #[tokio::test]
    async fn resolve_invalid_type() {
        let app = test_app();
        let resp = send(app, "GET", "/api/v1/resolve/invalid/denden", None).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn resolve_invalid_slug() {
        let app = test_app();
        let resp = send(app, "GET", "/api/v1/resolve/skill/INVALID", None).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn resolve_mixed_deps_via_endpoint() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(
            &store,
            "agent",
            "orchestrator",
            "1.0.0",
            Some(r#"{"skills":["notify"],"agents":["debugger"]}"#),
        );
        publish(&store, "skill", "notify", "0.3.0", None);
        publish(&store, "agent", "debugger", "0.5.0", None);

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resolve/agent/orchestrator",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let order = body["order"].as_array().unwrap();
        assert_eq!(order.len(), 3);
        // Leaf-first: notify and debugger before orchestrator
        assert_eq!(order[0]["name"], "notify");
        assert_eq!(order[0]["type"], "skill");
        assert_eq!(order[1]["name"], "debugger");
        assert_eq!(order[1]["type"], "agent");
        assert_eq!(order[2]["name"], "orchestrator");
        assert_eq!(order[2]["type"], "agent");
    }

    // -- Additional edge-case tests --

    #[tokio::test]
    #[allow(clippy::items_after_statements)]
    async fn create_resource_malformed_json_returns_error() {
        let app = test_app();
        // Send invalid JSON body
        let resp = send(
            app,
            "POST",
            "/api/v1/resources/skill/denden",
            Some("not json at all"),
        )
        .await;
        // Axum returns 400 Bad Request for malformed JSON
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_resource_empty_body_succeeds() {
        let app = test_app();
        // Empty JSON object — description is optional
        let resp = send(app, "POST", "/api/v1/resources/skill/denden", Some("{}")).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn list_resources_mixed_types() {
        let app = test_app();
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"a skill"}"#),
        )
        .await;
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/agent/debugger",
            Some(r#"{"description":"an agent"}"#),
        )
        .await;

        // Without type filter — returns both
        let resp = send(app, "GET", "/api/v1/resources", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn delete_resource_also_deletes_versions() {
        // Build a pre-populated store with a resource and versions.
        let store = SqliteResourceStore::open_in_memory().unwrap();
        let resource = crate::store::models::Resource {
            id: 0,
            scope: None,
            name: "denden".to_string(),
            resource_type: "skill".to_string(),
            description: Some("test".to_string()),
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        store
            .publish(
                &resource,
                &crate::store::models::Version {
                    id: 0,
                    resource_id: 0,
                    version: "1.0.0".to_string(),
                    store_path: None,
                    checksum: None,
                    manifest_json: None,
                    published_by: None,
                    published_at: None,
                },
            )
            .unwrap();

        let state = Arc::new(AppState {
            started_at: std::time::Instant::now(),
            store: Mutex::new(store),
            blob_store: None,
            config: None,
        });
        let app = Router::new()
            .nest("/api/v1", resource_routes())
            .with_state(state);

        // Verify version exists
        let resp = send(
            app.clone(),
            "GET",
            "/api/v1/resources/skill/denden/versions",
            None,
        )
        .await;
        assert_eq!(json_body(resp).await.as_array().unwrap().len(), 1);

        // Delete the resource
        let resp = send(
            app.clone(),
            "DELETE",
            "/api/v1/resources/skill/denden",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify resource and versions are gone
        let resp = send(app, "GET", "/api/v1/resources/skill/denden", None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Search (FTS5) tests --

    #[tokio::test]
    async fn search_returns_matching_resources() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "code-review", "1.0.0", None);
        publish(&store, "skill", "security-baseline", "0.5.0", None);

        let app = app_with_store(store);

        let resp = send(app, "GET", "/api/v1/resources?q=code", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let results = body.as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["name"], "code-review");
    }

    #[tokio::test]
    async fn search_with_type_filter() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "debugger", "1.0.0", None);
        publish(&store, "agent", "debugger", "1.0.0", None);

        let app = app_with_store(store);

        // Search for "debugger" filtered to agents only
        let resp = send(app, "GET", "/api/v1/resources?q=debugger&type=agent", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let results = body.as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["type"], "agent");
    }

    #[tokio::test]
    async fn search_no_results() {
        let app = test_app();
        let resp = send(app, "GET", "/api/v1/resources?q=nonexistent", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn search_by_description() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        let resource = crate::store::models::Resource {
            id: 0,
            scope: None,
            name: "notify".to_string(),
            resource_type: "skill".to_string(),
            description: Some("Send notifications to Slack channels".to_string()),
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        store
            .publish(
                &resource,
                &crate::store::models::Version {
                    id: 0,
                    resource_id: 0,
                    version: "1.0.0".to_string(),
                    store_path: None,
                    checksum: None,
                    manifest_json: None,
                    published_by: None,
                    published_at: None,
                },
            )
            .unwrap();

        let app = app_with_store(store);
        let resp = send(app, "GET", "/api/v1/resources?q=slack", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let results = body.as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["name"], "notify");
    }

    #[tokio::test]
    async fn search_deleted_resource_not_found() {
        let app = test_app();
        // Create, then delete
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/ephemeral",
            Some(r#"{"description":"temporary"}"#),
        )
        .await;
        send(
            app.clone(),
            "DELETE",
            "/api/v1/resources/skill/ephemeral",
            None,
        )
        .await;

        // Search should not find the deleted resource
        let resp = send(app, "GET", "/api/v1/resources?q=ephemeral", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn search_empty_query_returns_empty() {
        let app = test_app();
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"a skill"}"#),
        )
        .await;

        let resp = send(app, "GET", "/api/v1/resources?q=", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn search_without_q_returns_all() {
        let app = test_app();
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"a skill"}"#),
        )
        .await;

        // Without q param, should list all (not search)
        let resp = send(app, "GET", "/api/v1/resources", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 1);
    }

    // -- Publish endpoint tests --

    /// Helper to create a temp directory for blob storage in tests.
    fn blob_test_dir() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("relava-route-test-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a multipart body manually for testing.
    fn build_multipart_body(
        metadata: &serde_json::Value,
        files: &[(&str, &[u8])],
    ) -> (String, Vec<u8>) {
        let boundary = "----RelavaTestBoundary";
        let mut body = Vec::new();

        // Metadata part
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"metadata\"\r\n\r\n");
        body.extend_from_slice(serde_json::to_string(metadata).unwrap().as_bytes());
        body.extend_from_slice(b"\r\n");

        // File parts
        for (filename, data) in files {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(data);
            body.extend_from_slice(b"\r\n");
        }

        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    /// Compute SHA-256 hex of data for test assertions.
    fn test_sha256(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(data))
    }

    /// Send a multipart publish request.
    async fn send_publish(
        app: Router,
        rtype: &str,
        name: &str,
        metadata: &serde_json::Value,
        files: &[(&str, &[u8])],
    ) -> axum::response::Response {
        let (content_type, body_bytes) = build_multipart_body(metadata, files);
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/resources/{rtype}/{name}/publish"))
                .header("content-type", content_type)
                .body(Body::from(body_bytes))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn publish_auto_increments_version() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"---\n---\n# My Skill";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": sha, "size": file_content.len()}]
        });

        // First publish — should get 0.1.0
        let resp = send_publish(
            app.clone(),
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = json_body(resp).await;
        assert_eq!(body["version"], "0.1.0");
        assert_eq!(body["name"], "denden");
        assert_eq!(body["type"], "skill");

        // Second publish — should get 0.1.1
        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = json_body(resp).await;
        assert_eq!(body["version"], "0.1.1");
    }

    #[tokio::test]
    async fn publish_uses_frontmatter_version() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"---\nversion: 2.0.0\n---\n# My Skill";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": sha, "size": file_content.len()}]
        });

        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = json_body(resp).await;
        assert_eq!(body["version"], "2.0.0");
    }

    #[tokio::test]
    async fn publish_rejects_duplicate_version() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"---\nversion: 1.0.0\n---\n# Skill";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": sha, "size": file_content.len()}]
        });

        // First publish
        let resp = send_publish(
            app.clone(),
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Second publish with same version — should 409
        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn publish_rejects_older_version() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let v2_content = b"---\nversion: 2.0.0\n---\n# Skill";
        let v2_sha = test_sha256(v2_content);

        let v1_content = b"---\nversion: 1.0.0\n---\n# Skill";
        let v1_sha = test_sha256(v1_content);

        // Publish 2.0.0 first
        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": v2_sha, "size": v2_content.len()}]
        });
        let resp = send_publish(
            app.clone(),
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", v2_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Try to publish 1.0.0 — should 409 (not newer)
        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": v1_sha, "size": v1_content.len()}]
        });
        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", v1_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn publish_rejects_checksum_mismatch() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"---\nversion: 1.0.0\n---\n# Skill";
        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": "badhash", "size": file_content.len()}]
        });

        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(resp).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("checksum mismatch")
        );
    }

    #[tokio::test]
    async fn publish_rejects_missing_metadata() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        // Send a publish request with no metadata field — just files
        let boundary = "----TestBoundary";
        let mut body_bytes = Vec::new();
        body_bytes.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body_bytes.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"SKILL.md\"\r\n\
              Content-Type: application/octet-stream\r\n\r\nhello\r\n",
        );
        body_bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/resources/skill/denden/publish")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body_bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn publish_rejects_invalid_slug() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"hello";
        let sha = test_sha256(file_content);
        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": sha, "size": 5}]
        });

        let resp = send_publish(
            app,
            "skill",
            "INVALID",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -- Download endpoint tests --

    #[tokio::test]
    async fn download_published_version() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"---\nversion: 1.0.0\n---\n# My Skill";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": sha, "size": file_content.len()}]
        });

        // Publish first
        let resp = send_publish(
            app.clone(),
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Download
        let resp = send(
            app,
            "GET",
            "/api/v1/resources/skill/denden/versions/1.0.0/download",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("application/x-tar"));

        // Verify tar contains the file
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn download_nonexistent_version_returns_404() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        // Create resource first
        send(
            app.clone(),
            "POST",
            "/api/v1/resources/skill/denden",
            Some(r#"{"description":"test"}"#),
        )
        .await;

        let resp = send(
            app,
            "GET",
            "/api/v1/resources/skill/denden/versions/9.9.9/download",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn publish_multiple_files() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let skill_md = b"---\nversion: 1.0.0\n---\n# Skill";
        let util_md = b"# Utils";
        let skill_sha = test_sha256(skill_md);
        let util_sha = test_sha256(util_md);

        let metadata = serde_json::json!({
            "files": [
                {"path": "SKILL.md", "sha256": skill_sha, "size": skill_md.len()},
                {"path": "lib/utils.md", "sha256": util_sha, "size": util_md.len()},
            ]
        });

        let resp = send_publish(
            app.clone(),
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", skill_md), ("lib/utils.md", util_md)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Download and verify both files are in the tar
        let resp = send(
            app,
            "GET",
            "/api/v1/resources/skill/denden/versions/1.0.0/download",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let mut archive = tar::Archive::new(&body[..]);
        let entries: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(entries.contains(&"SKILL.md".to_string()));
        assert!(entries.contains(&"lib/utils.md".to_string()));
    }

    // -- Version auto-increment tests --

    #[tokio::test]
    async fn publish_first_without_version_defaults_to_0_1_0() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"# Simple skill with no frontmatter";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": sha, "size": file_content.len()}]
        });

        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = json_body(resp).await;
        assert_eq!(body["version"], "0.1.0");
    }

    // -- Path traversal protection tests --

    #[tokio::test]
    async fn publish_rejects_path_traversal_in_filename() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"evil content";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [{"path": "../../etc/evil", "sha256": sha, "size": file_content.len()}]
        });

        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("../../etc/evil", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(resp).await;
        assert!(body["error"].as_str().unwrap().contains("unsafe"));
    }

    #[tokio::test]
    async fn publish_rejects_absolute_path() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"evil";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [{"path": "/etc/passwd", "sha256": sha, "size": file_content.len()}]
        });

        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("/etc/passwd", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -- Undeclared file tests --

    #[tokio::test]
    async fn publish_rejects_undeclared_uploaded_file() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"---\nversion: 1.0.0\n---\n# Skill";
        let sha = test_sha256(file_content);

        // Metadata only declares SKILL.md but we also upload extra.md
        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": sha, "size": file_content.len()}]
        });

        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content), ("extra.md", b"undeclared")],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(resp).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("not declared in metadata")
        );
    }

    // -- No blob store tests --

    #[tokio::test]
    async fn publish_returns_error_without_blob_store() {
        let app = test_app(); // No blob store

        let file_content = b"---\nversion: 1.0.0\n---\n# Skill";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [{"path": "SKILL.md", "sha256": sha, "size": file_content.len()}]
        });

        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(resp).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("not configured for file storage")
        );
    }

    // -- Declared but not uploaded --

    #[tokio::test]
    async fn publish_rejects_file_declared_but_not_uploaded() {
        let blob_dir = blob_test_dir();
        let app = crate::app_with_blob_store(blob_dir);

        let file_content = b"---\nversion: 1.0.0\n---\n# Skill";
        let sha = test_sha256(file_content);

        let metadata = serde_json::json!({
            "files": [
                {"path": "SKILL.md", "sha256": sha, "size": file_content.len()},
                {"path": "missing.md", "sha256": "def", "size": 5},
            ]
        });

        let resp = send_publish(
            app,
            "skill",
            "denden",
            &metadata,
            &[("SKILL.md", file_content)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(resp).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("declared in metadata but not uploaded")
        );
    }

    // -- Checksums endpoint tests --

    #[tokio::test]
    async fn get_checksums_returns_per_file_hashes() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        let manifest = r#"{"files":[{"path":"SKILL.md","sha256":"abc123"},{"path":"lib/utils.md","sha256":"def456"}]}"#;
        publish(&store, "skill", "denden", "1.0.0", Some(manifest));

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resources/skill/denden/versions/1.0.0/checksums",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["version"], "1.0.0");
        let files = body["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["path"], "SKILL.md");
        assert_eq!(files[0]["sha256"], "abc123");
    }

    #[tokio::test]
    async fn get_checksums_legacy_version_returns_404() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        // Publish without manifest_json (legacy)
        publish(&store, "skill", "denden", "0.1.0", None);

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resources/skill/denden/versions/0.1.0/checksums",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = json_body(resp).await;
        let error_msg = body["error"].as_str().unwrap();
        assert!(
            error_msg.contains("legacy version"),
            "should mention legacy version, got: {error_msg}"
        );
    }

    #[tokio::test]
    async fn get_checksums_corrupt_manifest_returns_500() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        // Publish with corrupt manifest_json
        publish(
            &store,
            "skill",
            "denden",
            "1.0.0",
            Some("not valid json{{{"),
        );

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resources/skill/denden/versions/1.0.0/checksums",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn get_checksums_resource_not_found() {
        let app = test_app();
        let resp = send(
            app,
            "GET",
            "/api/v1/resources/skill/nonexistent/versions/1.0.0/checksums",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_checksums_version_not_found() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "denden", "1.0.0", None);

        let resp = send(
            app_with_store(store),
            "GET",
            "/api/v1/resources/skill/denden/versions/9.9.9/checksums",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Updates check endpoint --

    #[tokio::test]
    async fn check_updates_empty_request() {
        let app = test_app();
        let resp = send(
            app,
            "POST",
            "/api/v1/updates/check",
            Some(r#"{"resources":[]}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["available"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn check_updates_finds_newer_version() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "denden", "2.0.0", None);

        let resp = send(
            app_with_store(store),
            "POST",
            "/api/v1/updates/check",
            Some(r#"{"resources":[{"type":"skill","name":"denden","version":"1.0.0"}]}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let available = body["available"].as_array().unwrap();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0]["name"], "denden");
        assert_eq!(available[0]["installed_version"], "1.0.0");
        assert_eq!(available[0]["latest_version"], "2.0.0");
    }

    #[tokio::test]
    async fn check_updates_up_to_date() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "denden", "1.0.0", None);

        let resp = send(
            app_with_store(store),
            "POST",
            "/api/v1/updates/check",
            Some(r#"{"resources":[{"type":"skill","name":"denden","version":"1.0.0"}]}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["available"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn check_updates_skips_unknown_resource() {
        let app = test_app();
        let resp = send(
            app,
            "POST",
            "/api/v1/updates/check",
            Some(r#"{"resources":[{"type":"skill","name":"nonexistent","version":"1.0.0"}]}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["available"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn check_updates_skips_invalid_version() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "denden", "2.0.0", None);

        let resp = send(
            app_with_store(store),
            "POST",
            "/api/v1/updates/check",
            Some(r#"{"resources":[{"type":"skill","name":"denden","version":"invalid"}]}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["available"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn check_updates_skips_invalid_resource_type() {
        let app = test_app();
        let resp = send(
            app,
            "POST",
            "/api/v1/updates/check",
            Some(r#"{"resources":[{"type":"invalid","name":"denden","version":"1.0.0"}]}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["available"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn check_updates_no_downgrade_notification() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        // Server has 1.0.0, client has 2.0.0 (newer) → no update reported
        publish(&store, "skill", "denden", "1.0.0", None);

        let resp = send(
            app_with_store(store),
            "POST",
            "/api/v1/updates/check",
            Some(r#"{"resources":[{"type":"skill","name":"denden","version":"2.0.0"}]}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["available"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn check_updates_multiple_resources() {
        let store = SqliteResourceStore::open_in_memory().unwrap();
        publish(&store, "skill", "denden", "2.0.0", None);
        publish(&store, "agent", "debugger", "1.0.0", None);

        let resp = send(
            app_with_store(store),
            "POST",
            "/api/v1/updates/check",
            Some(
                r#"{"resources":[
                {"type":"skill","name":"denden","version":"1.0.0"},
                {"type":"agent","name":"debugger","version":"1.0.0"},
                {"type":"skill","name":"missing","version":"1.0.0"}
            ]}"#,
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let available = body["available"].as_array().unwrap();
        // denden has update (1.0.0 → 2.0.0), debugger is up-to-date, missing is skipped
        assert_eq!(available.len(), 1);
        assert_eq!(available[0]["name"], "denden");
    }
}
