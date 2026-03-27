pub mod resolve;
pub mod routes;
pub mod seed;
pub mod store;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{
    Json, Router,
    routing::{get, post},
};
use serde::Serialize;
use tower_http::services::{ServeDir, ServeFile};

use store::db::SqliteResourceStore;

/// Server configuration exposed via `GET /config`.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// Shared application state available to all handlers.
pub struct AppState {
    pub started_at: Instant,
    pub store: Mutex<SqliteResourceStore>,
    pub blob_store: Option<store::LocalBlobStore>,
    pub config: Option<ServerConfig>,
}

/// Wire all API routes onto shared state, producing a router without static
/// file serving. Used by both the real app builder and the test helpers to
/// avoid duplicating the route table.
fn api_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/stats", get(stats))
        .route("/config", get(server_config))
        .route("/cache/clean", post(clean_cache))
        .nest("/api/v1", routes::resource_routes())
        .with_state(state)
}

/// Build the Relava API router with shared state.
///
/// Opens (or creates) the SQLite database at `db_path` and wires all routes.
/// The blob store root defaults to `~/.relava/store/`.
///
/// If `gui_dir` is provided and the directory exists, static files are served
/// from it at `/` with SPA fallback (index.html for unmatched non-API routes).
/// API routes always take priority over static file serving.
pub fn app(db_path: &Path, gui_dir: Option<&Path>) -> Result<Router, store::StoreError> {
    app_with_config(db_path, gui_dir, None)
}

/// Build the Relava API router with shared state and server configuration.
///
/// Like [`app`], but also stores server configuration for the `GET /config`
/// endpoint.
pub fn app_with_config(
    db_path: &Path,
    gui_dir: Option<&Path>,
    config: Option<ServerConfig>,
) -> Result<Router, store::StoreError> {
    let store = SqliteResourceStore::open(db_path)?;

    // Derive blob store root from db_path's parent (e.g., ~/.relava/store/)
    let blob_root = db_path
        .parent()
        .map(|p| p.join("store"))
        .unwrap_or_else(|| PathBuf::from("store"));
    let blob_store = store::LocalBlobStore::new(blob_root);

    // Seed default resources (non-fatal on failure)
    if let Err(e) = seed::seed(&store, &blob_store) {
        eprintln!("[relava-server] failed to seed defaults: {e}");
    }

    let state = Arc::new(AppState {
        started_at: Instant::now(),
        store: Mutex::new(store),
        blob_store: Some(blob_store),
        config,
    });

    Ok(with_static_files(api_router(state), gui_dir))
}

/// Wrap an API router with static file serving if the GUI directory exists.
///
/// Uses `tower_http::services::ServeDir` with an `index.html` fallback for
/// SPA routing. API routes are registered first so they always take priority.
fn with_static_files(api_router: Router, gui_dir: Option<&Path>) -> Router {
    let Some(dir) = gui_dir else {
        return api_router;
    };

    if !dir.is_dir() {
        eprintln!(
            "[relava-server] GUI directory {} does not exist, skipping static file serving",
            dir.display()
        );
        return api_router;
    }

    let index_path = dir.join("index.html");
    let serve_dir = ServeDir::new(dir).fallback(ServeFile::new(index_path));

    // API routes are defined first so they take priority; the fallback
    // service handles everything else (static files + SPA fallback).
    api_router.fallback_service(serve_dir)
}

/// Build a test router with an in-memory SQLite store (for testing only).
#[cfg(test)]
pub fn app_in_memory() -> Router {
    app_in_memory_with_gui(None)
}

/// Build a test router with an in-memory SQLite store and optional GUI dir.
#[cfg(test)]
pub fn app_in_memory_with_gui(gui_dir: Option<&Path>) -> Router {
    let store = SqliteResourceStore::open_in_memory().unwrap();
    let state = Arc::new(AppState {
        started_at: Instant::now(),
        store: Mutex::new(store),
        blob_store: None,
        config: None,
    });

    with_static_files(api_router(state), gui_dir)
}

/// Build a test router with an in-memory SQLite store and a temporary blob store.
#[cfg(test)]
pub fn app_with_blob_store(blob_root: PathBuf) -> Router {
    let store = SqliteResourceStore::open_in_memory().unwrap();
    let blob_store = store::LocalBlobStore::new(blob_root);
    let state = Arc::new(AppState {
        started_at: Instant::now(),
        store: Mutex::new(store),
        blob_store: Some(blob_store),
        config: None,
    });

    api_router(state)
}

/// Response payload for `GET /health`.
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    uptime_secs: u64,
    database_connected: bool,
}

/// Health check endpoint used by `relava doctor` and monitoring.
///
/// Lightweight: runs a `SELECT 1` probe against the database to verify
/// connectivity without scanning any tables.
async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let db_ok = match state.store.lock() {
        Ok(s) => {
            if s.is_healthy() {
                true
            } else {
                eprintln!("[relava-server] health: database probe failed");
                false
            }
        }
        Err(_) => {
            eprintln!("[relava-server] health: store mutex poisoned");
            false
        }
    };

    Json(HealthResponse {
        status: if db_ok { "ok" } else { "degraded" },
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: state.started_at.elapsed().as_secs(),
        database_connected: db_ok,
    })
}

/// Response payload for `GET /stats`.
#[derive(Debug, Serialize)]
struct StatsResponse {
    resource_count: i64,
    resource_counts_by_type: HashMap<String, i64>,
    version_count: i64,
    database_size_bytes: i64,
}

/// Gather stats from the store, returning a structured response or an error message.
fn gather_stats(store: &SqliteResourceStore) -> Result<StatsResponse, String> {
    let counts = store.resource_counts_by_type().map_err(|e| e.to_string())?;
    let version_count = store.total_version_count().map_err(|e| e.to_string())?;
    let database_size_bytes = store.database_size_bytes().map_err(|e| e.to_string())?;

    let resource_count = counts.iter().map(|(_, c)| c).sum();
    let resource_counts_by_type = counts.into_iter().collect();

    Ok(StatsResponse {
        resource_count,
        resource_counts_by_type,
        version_count,
        database_size_bytes,
    })
}

/// Statistics endpoint returning resource/version counts and database size.
async fn stats(State(state): State<Arc<AppState>>) -> axum::response::Response {
    let result = state
        .store
        .lock()
        .map_err(|_| "store mutex poisoned".to_string())
        .and_then(|store| gather_stats(&store));

    match result {
        Ok(stats) => Json(stats).into_response(),
        Err(msg) => {
            eprintln!("[relava-server] stats error: {msg}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal server error"})),
            )
                .into_response()
        }
    }
}

/// Response payload for `GET /config`.
#[derive(Debug, Serialize)]
struct ConfigResponse {
    host: String,
    port: u16,
    data_dir: String,
    cache_dir: String,
    cache_size_bytes: u64,
}

/// Recursively sum file sizes under `path`. Returns 0 if the directory does
/// not exist or is unreadable.
fn dir_size_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size_bytes(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Server configuration endpoint returning host, port, data directory, cache
/// directory, and cache size.
async fn server_config(State(state): State<Arc<AppState>>) -> axum::response::Response {
    let Some(ref cfg) = state.config else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "server config not available"})),
        )
            .into_response();
    };

    let cache_path = cfg.cache_dir.clone();
    let cache_size_bytes = tokio::task::spawn_blocking(move || dir_size_bytes(&cache_path))
        .await
        .unwrap_or(0);

    Json(ConfigResponse {
        host: cfg.host.clone(),
        port: cfg.port,
        data_dir: cfg.data_dir.display().to_string(),
        cache_dir: cfg.cache_dir.display().to_string(),
        cache_size_bytes,
    })
    .into_response()
}

/// Clean the cache directory by removing all its contents.
async fn clean_cache(State(state): State<Arc<AppState>>) -> axum::response::Response {
    let Some(ref cfg) = state.config else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "server config not available"})),
        )
            .into_response();
    };

    let cache_dir = cfg.cache_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        if !cache_dir.is_dir() {
            return Ok(0u64);
        }

        let size_before = dir_size_bytes(&cache_dir);

        // Remove all entries inside the cache directory (but keep the directory itself).
        std::fs::read_dir(&cache_dir).and_then(|entries| {
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    std::fs::remove_dir_all(&path)?;
                } else {
                    std::fs::remove_file(&path)?;
                }
            }
            Ok(size_before)
        })
    })
    .await;

    match result {
        Ok(Ok(freed_bytes)) => Json(serde_json::json!({
            "cleaned": true,
            "freed_bytes": freed_bytes,
        }))
        .into_response(),
        Ok(Err(e)) => {
            eprintln!("[relava-server] cache clean error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("failed to clean cache: {e}")})),
            )
                .into_response()
        }
        Err(e) => {
            eprintln!("[relava-server] cache clean task panicked: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal server error"})),
            )
                .into_response()
        }
    }
}

/// Wait for a SIGTERM or SIGINT signal (graceful shutdown trigger).
///
/// On Unix, listens for both SIGTERM and SIGINT. On non-Unix platforms,
/// falls back to `ctrl_c` only.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                eprintln!("[relava-server] received SIGTERM, shutting down gracefully");
            }
            _ = sigint.recv() => {
                eprintln!("[relava-server] received SIGINT, shutting down gracefully");
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
        eprintln!("[relava-server] received Ctrl+C, shutting down gracefully");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use store::traits::ResourceStore;
    use tower::ServiceExt;

    /// Send a GET request to the given URI and return the response.
    async fn get(uri: &str) -> axum::response::Response {
        app_in_memory()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// Parse a response body as JSON.
    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    // -- Health endpoint tests --

    #[tokio::test]
    async fn health_returns_ok_status() {
        assert_eq!(get("/health").await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_returns_json_with_required_fields() {
        let json = json_body(get("/health").await).await;

        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert!(json["uptime_secs"].is_number());
        assert_eq!(json["database_connected"], true);
    }

    #[tokio::test]
    async fn health_content_type_is_json() {
        let response = get("/health").await;
        let content_type = response
            .headers()
            .get("content-type")
            .expect("missing content-type header")
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("application/json"),
            "expected application/json, got {content_type}"
        );
    }

    // -- Stats endpoint tests --

    #[tokio::test]
    async fn stats_returns_ok_status() {
        assert_eq!(get("/stats").await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stats_returns_json_with_required_fields() {
        let json = json_body(get("/stats").await).await;

        assert_eq!(json["resource_count"], 0);
        assert!(json["resource_counts_by_type"].is_object());
        assert_eq!(json["version_count"], 0);
        assert!(json["database_size_bytes"].is_number());
    }

    #[tokio::test]
    async fn stats_reflects_published_resources() {
        let store = store::db::SqliteResourceStore::open_in_memory().unwrap();

        // Publish two skills and one agent.
        let skill = store::models::Resource {
            id: 0,
            scope: None,
            name: "denden".to_string(),
            resource_type: "skill".to_string(),
            description: Some("comm skill".to_string()),
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        let ver = store::models::Version {
            id: 0,
            resource_id: 0,
            version: "1.0.0".to_string(),
            store_path: None,
            checksum: None,
            manifest_json: None,
            published_by: None,
            published_at: None,
        };
        store.publish(&skill, &ver).unwrap();

        let mut agent = skill.clone();
        agent.name = "debugger".to_string();
        agent.resource_type = "agent".to_string();
        store.publish(&agent, &ver).unwrap();

        let mut skill2 = skill.clone();
        skill2.name = "reviewer".to_string();
        let ver2 = store::models::Version {
            version: "2.0.0".to_string(),
            ..ver
        };
        store.publish(&skill2, &ver2).unwrap();

        let state = Arc::new(AppState {
            started_at: Instant::now(),
            store: Mutex::new(store),
            blob_store: None,
            config: None,
        });

        let app = Router::new()
            .route("/stats", axum::routing::get(stats))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let json = json_body(resp).await;
        assert_eq!(json["resource_count"], 3);
        assert_eq!(json["resource_counts_by_type"]["skill"], 2);
        assert_eq!(json["resource_counts_by_type"]["agent"], 1);
        assert_eq!(json["version_count"], 3);
    }

    #[tokio::test]
    async fn stats_content_type_is_json() {
        let response = get("/stats").await;
        let content_type = response
            .headers()
            .get("content-type")
            .expect("missing content-type header")
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("application/json"),
            "expected application/json, got {content_type}"
        );
    }

    #[tokio::test]
    async fn health_degraded_when_mutex_poisoned() {
        let store = store::db::SqliteResourceStore::open_in_memory().unwrap();
        let state = Arc::new(AppState {
            started_at: Instant::now(),
            store: Mutex::new(store),
            blob_store: None,
            config: None,
        });

        // Poison the mutex by panicking while holding the lock.
        let state_clone = Arc::clone(&state);
        let _ = std::thread::spawn(move || {
            let _guard = state_clone.store.lock().unwrap();
            panic!("intentional panic to poison mutex");
        })
        .join();

        // The mutex should now be poisoned.
        assert!(state.store.lock().is_err());

        let app = Router::new()
            .route("/health", axum::routing::get(health))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = json_body(resp).await;
        assert_eq!(json["status"], "degraded");
        assert_eq!(json["database_connected"], false);
    }

    #[tokio::test]
    async fn stats_returns_500_when_mutex_poisoned() {
        let store = store::db::SqliteResourceStore::open_in_memory().unwrap();
        let state = Arc::new(AppState {
            started_at: Instant::now(),
            store: Mutex::new(store),
            blob_store: None,
            config: None,
        });

        // Poison the mutex.
        let state_clone = Arc::clone(&state);
        let _ = std::thread::spawn(move || {
            let _guard = state_clone.store.lock().unwrap();
            panic!("intentional panic to poison mutex");
        })
        .join();

        let app = Router::new()
            .route("/stats", axum::routing::get(stats))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let json = json_body(resp).await;
        assert!(json["error"].as_str().is_some());
    }

    // -- Misc tests --

    #[tokio::test]
    async fn unknown_route_returns_404() {
        assert_eq!(get("/nonexistent").await.status(), StatusCode::NOT_FOUND);
    }

    // -- Static file serving tests --

    /// Create a temporary GUI directory with test files.
    fn create_gui_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!DOCTYPE html><html><body>SPA</body></html>",
        )
        .unwrap();
        std::fs::write(dir.path().join("style.css"), "body { color: red; }").unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "console.log('hello');").unwrap();
        dir
    }

    /// Helper to build a test app with a GUI directory.
    fn app_with_gui(gui_dir: &std::path::Path) -> Router {
        app_in_memory_with_gui(Some(gui_dir))
    }

    /// Helper to send a GET request to a specific app instance.
    async fn get_from(app: Router, uri: &str) -> axum::response::Response {
        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// Helper to read a response body as a string.
    async fn body_string(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Extract the `content-type` header value as a string.
    fn content_type(response: &axum::response::Response) -> String {
        response
            .headers()
            .get("content-type")
            .expect("missing content-type header")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn static_serves_index_html_at_root() {
        let gui = create_gui_dir();
        let app = app_with_gui(gui.path());
        let resp = get_from(app, "/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("SPA"), "expected index.html content");
    }

    #[tokio::test]
    async fn static_serves_css_with_correct_mime() {
        let gui = create_gui_dir();
        let app = app_with_gui(gui.path());
        let resp = get_from(app, "/style.css").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = content_type(&resp);
        assert!(ct.contains("text/css"), "expected text/css, got {ct}");
        let body = body_string(resp).await;
        assert!(body.contains("color: red"));
    }

    #[tokio::test]
    async fn static_serves_js_from_subdirectory() {
        let gui = create_gui_dir();
        let app = app_with_gui(gui.path());
        let resp = get_from(app, "/assets/app.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = content_type(&resp);
        assert!(
            ct.contains("javascript"),
            "expected javascript MIME type, got {ct}"
        );
    }

    #[tokio::test]
    async fn static_spa_fallback_serves_index_for_unknown_path() {
        let gui = create_gui_dir();
        let app = app_with_gui(gui.path());
        let resp = get_from(app, "/some/spa/route").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("SPA"), "SPA fallback should serve index.html");
    }

    #[tokio::test]
    async fn static_api_routes_take_priority() {
        let gui = create_gui_dir();
        let app = app_with_gui(gui.path());
        let resp = get_from(app, "/health").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = json_body(resp).await;
        assert_eq!(
            json["status"], "ok",
            "API route should take priority over static files"
        );
    }

    #[tokio::test]
    async fn static_stats_route_takes_priority() {
        let gui = create_gui_dir();
        let app = app_with_gui(gui.path());
        let resp = get_from(app, "/stats").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = json_body(resp).await;
        assert!(
            json["resource_count"].is_number(),
            "stats API route should take priority"
        );
    }

    #[tokio::test]
    async fn static_server_starts_without_gui_dir() {
        let nonexistent = std::path::PathBuf::from("/tmp/relava-nonexistent-gui-dir");
        let app = app_in_memory_with_gui(Some(&nonexistent));
        let resp = get_from(app, "/health").await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "server works without GUI dir"
        );
    }

    #[tokio::test]
    async fn static_no_gui_dir_returns_404_for_unknown() {
        // Without GUI dir, unknown routes still 404
        let resp = get("/nonexistent").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn static_path_traversal_blocked() {
        let gui = create_gui_dir();
        let app = app_with_gui(gui.path());
        let resp = get_from(app, "/../../etc/passwd").await;
        // ServeDir normalizes the path so it cannot escape the root.
        // The SPA fallback serves index.html, which is the correct behavior.
        let body = body_string(resp).await;
        assert!(
            !body.contains("root:"),
            "path traversal must not serve files outside the GUI directory"
        );
    }

    #[tokio::test]
    async fn static_missing_index_html_returns_not_found() {
        // GUI dir exists but has no index.html — SPA fallback should not 500
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("style.css"), "body {}").unwrap();
        let app = app_with_gui(dir.path());
        let resp = get_from(app, "/unknown/route").await;
        assert_ne!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "missing index.html should not cause 500"
        );
    }

    // -- Config endpoint tests --

    /// Helper to build a test app with a specific ServerConfig.
    fn app_with_server_config(config: Option<ServerConfig>) -> Router {
        let store = store::db::SqliteResourceStore::open_in_memory().unwrap();
        let state = Arc::new(AppState {
            started_at: Instant::now(),
            store: Mutex::new(store),
            blob_store: None,
            config,
        });

        Router::new()
            .route("/config", axum::routing::get(server_config))
            .route("/cache/clean", axum::routing::post(clean_cache))
            .with_state(state)
    }

    #[tokio::test]
    async fn config_returns_503_when_no_config() {
        let app = app_with_server_config(None);
        let resp = get_from(app, "/config").await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let json = json_body(resp).await;
        assert!(json["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn config_returns_server_configuration() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        // Put a 100-byte file in the cache to verify size calculation
        std::fs::write(cache_dir.join("test.bin"), &[0u8; 100]).unwrap();

        let config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 7420,
            data_dir: tmp.path().to_path_buf(),
            cache_dir: cache_dir.clone(),
        };
        let app = app_with_server_config(Some(config));
        let resp = get_from(app, "/config").await;

        assert_eq!(resp.status(), StatusCode::OK);
        let json = json_body(resp).await;
        assert_eq!(json["host"], "127.0.0.1");
        assert_eq!(json["port"], 7420);
        assert_eq!(json["data_dir"], tmp.path().display().to_string());
        assert_eq!(json["cache_dir"], cache_dir.display().to_string());
        assert_eq!(json["cache_size_bytes"], 100);
    }

    #[tokio::test]
    async fn config_cache_size_zero_when_dir_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();

        let config = ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8080,
            data_dir: tmp.path().to_path_buf(),
            cache_dir,
        };
        let app = app_with_server_config(Some(config));
        let json = json_body(get_from(app, "/config").await).await;
        assert_eq!(json["cache_size_bytes"], 0);
    }

    #[tokio::test]
    async fn config_cache_size_sums_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let sub = cache_dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(cache_dir.join("a.bin"), &[0u8; 50]).unwrap();
        std::fs::write(sub.join("b.bin"), &[0u8; 75]).unwrap();

        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 7420,
            data_dir: tmp.path().to_path_buf(),
            cache_dir,
        };
        let app = app_with_server_config(Some(config));
        let json = json_body(get_from(app, "/config").await).await;
        assert_eq!(json["cache_size_bytes"], 125);
    }

    #[tokio::test]
    async fn config_cache_size_zero_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 7420,
            data_dir: tmp.path().to_path_buf(),
            cache_dir: tmp.path().join("nonexistent-cache"),
        };
        let app = app_with_server_config(Some(config));
        let json = json_body(get_from(app, "/config").await).await;
        assert_eq!(json["cache_size_bytes"], 0);
    }

    // -- Cache clean endpoint tests --

    /// Send a POST request to the given URI and return the response.
    async fn post_to(app: Router, uri: &str) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn clean_cache_returns_503_when_no_config() {
        let app = app_with_server_config(None);
        let resp = post_to(app, "/cache/clean").await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let json = json_body(resp).await;
        assert!(json["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn clean_cache_succeeds_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 7420,
            data_dir: tmp.path().to_path_buf(),
            cache_dir: tmp.path().join("nonexistent"),
        };
        let app = app_with_server_config(Some(config));
        let resp = post_to(app, "/cache/clean").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = json_body(resp).await;
        assert_eq!(json["cleaned"], true);
        assert_eq!(json["freed_bytes"], 0);
    }

    #[tokio::test]
    async fn clean_cache_removes_files_and_reports_freed_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let sub = cache_dir.join("subdir");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(cache_dir.join("a.bin"), &[0u8; 200]).unwrap();
        std::fs::write(sub.join("b.bin"), &[0u8; 300]).unwrap();

        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 7420,
            data_dir: tmp.path().to_path_buf(),
            cache_dir: cache_dir.clone(),
        };
        let app = app_with_server_config(Some(config));
        let resp = post_to(app, "/cache/clean").await;

        assert_eq!(resp.status(), StatusCode::OK);
        let json = json_body(resp).await;
        assert_eq!(json["cleaned"], true);
        assert_eq!(json["freed_bytes"], 500);

        // Verify the cache directory still exists but is empty
        assert!(cache_dir.is_dir(), "cache dir should still exist");
        let entries: Vec<_> = std::fs::read_dir(&cache_dir).unwrap().collect();
        assert!(entries.is_empty(), "cache dir should be empty after clean");
    }

    // -- dir_size_bytes unit tests --

    #[test]
    fn dir_size_bytes_nonexistent_returns_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert_eq!(dir_size_bytes(&missing), 0);
    }

    #[test]
    fn dir_size_bytes_empty_dir_returns_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(dir_size_bytes(tmp.path()), 0);
    }

    #[test]
    fn dir_size_bytes_sums_files_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(tmp.path().join("f1"), &[1u8; 40]).unwrap();
        std::fs::write(sub.join("f2"), &[2u8; 60]).unwrap();
        assert_eq!(dir_size_bytes(tmp.path()), 100);
    }

    // -- Seed integration test --

    #[tokio::test]
    async fn app_with_config_seeds_default_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("relava.db");

        let app = app_with_config(&db_path, None, None).unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/resources/skill/relava")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = json_body(resp).await;
        assert_eq!(json["name"], "relava");
        assert_eq!(json["type"], "skill");
        assert_eq!(json["latest_version"], "0.1.0");
    }
}
