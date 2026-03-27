use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::resolve;
use crate::store::db::SqliteResourceStore;
use crate::store::models::{Resource, Version};
use crate::store::traits::{ResourceStore, StoreError};
use relava_types::validate::{ResourceType, validate_slug, validate_version};

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
        .route("/resources/{type}/{name}/versions", get(list_versions))
        .route(
            "/resources/{type}/{name}/versions/{version}",
            get(get_version),
        )
        .route("/resolve/{type}/{name}", get(resolve_deps))
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
}
