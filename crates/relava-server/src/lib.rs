pub mod store;

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::{Json, Router, routing::get};
use serde::Serialize;

/// Shared application state available to all handlers.
#[derive(Clone)]
pub struct AppState {
    started_at: Instant,
}

/// Build the Relava API router with shared state.
pub fn app() -> Router {
    let state = Arc::new(AppState {
        started_at: Instant::now(),
    });

    Router::new()
        .route("/health", get(health))
        .with_state(state)
}

/// Response payload for `GET /health`.
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    uptime_secs: u64,
}

/// Health check endpoint used by `relava server status` to verify liveness.
async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: state.started_at.elapsed().as_secs(),
    })
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
        let mut sigint =
            signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");

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
    use tower::ServiceExt;

    /// Send a GET request to the given URI and return the response.
    async fn get(uri: &str) -> axum::response::Response {
        app()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// Send a GET to `/health` and parse the response body as JSON.
    async fn health_json() -> serde_json::Value {
        let response = get("/health").await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok_status() {
        assert_eq!(get("/health").await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_returns_json_with_required_fields() {
        let json = health_json().await;

        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert!(json["uptime_secs"].is_number());
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

    #[tokio::test]
    async fn unknown_route_returns_404() {
        assert_eq!(get("/nonexistent").await.status(), StatusCode::NOT_FOUND);
    }
}
