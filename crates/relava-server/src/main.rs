use std::process::ExitCode;

use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    let host = std::env::var("RELAVA_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = match std::env::var("RELAVA_PORT")
        .unwrap_or_else(|_| "7420".to_string())
        .parse()
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[relava-server] invalid RELAVA_PORT: {e}");
            return ExitCode::FAILURE;
        }
    };

    let addr = format!("{host}:{port}");
    let app = relava_server::app();

    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[relava-server] failed to bind to {addr}: {e}");
            if e.kind() == std::io::ErrorKind::AddrInUse {
                eprintln!(
                    "[relava-server] port {port} is already in use. \
                     Stop the other process or choose a different port with RELAVA_PORT."
                );
            }
            return ExitCode::FAILURE;
        }
    };

    eprintln!("[relava-server] listening on {addr}");

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(relava_server::shutdown_signal())
        .await
    {
        eprintln!("[relava-server] server error: {e}");
        return ExitCode::FAILURE;
    }

    eprintln!("[relava-server] shutdown complete");
    ExitCode::SUCCESS
}
