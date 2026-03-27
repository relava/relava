pub mod resolve;
pub mod routes;
pub mod store;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get};
use serde::Serialize;

use store::db::SqliteResourceStore;

/// Shared application state available to all handlers.
pub struct AppState {
    pub started_at: Instant,
    pub store: Mutex<SqliteResourceStore>,
}

/// Build the Relava API router with shared state.
///
/// Opens (or creates) the SQLite database at `db_path` and wires all routes.
pub fn app(db_path: &std::path::Path) -> Result<Router, store::StoreError> {
    let store = SqliteResourceStore::open(db_path)?;
    let state = Arc::new(AppState {
        started_at: Instant::now(),
        store: Mutex::new(store),
    });

    Ok(Router::new()
        .route("/health", get(health))
        .route("/stats", get(stats))
        .nest("/api/v1", routes::resource_routes())
        .with_state(state))
}

/// Build a test router with an in-memory SQLite store (for testing only).
#[cfg(test)]
pub fn app_in_memory() -> Router {
    let store = SqliteResourceStore::open_in_memory().unwrap();
    let state = Arc::new(AppState {
        started_at: Instant::now(),
        store: Mutex::new(store),
    });

    Router::new()
        .route("/health", get(health))
        .route("/stats", get(stats))
        .nest("/api/v1", routes::resource_routes())
        .with_state(state)
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
    let counts = store
        .resource_counts_by_type()
        .map_err(|e| e.to_string())?;
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
        });

        let app = Router::new()
            .route("/stats", axum::routing::get(stats))
            .with_state(state);

        let resp = app
            .oneshot(Request::builder().uri("/stats").body(Body::empty()).unwrap())
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
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
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
            .oneshot(Request::builder().uri("/stats").body(Body::empty()).unwrap())
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
}
