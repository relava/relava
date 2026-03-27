use std::path::PathBuf;
use std::process::ExitCode;

use relava_server::ServerConfig;
use relava_server::store::RelavaDir;
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

    // Set up the ~/.relava/ directory structure and open the database.
    let relava_dir = match RelavaDir::default_location() {
        Some(d) => d,
        None => {
            eprintln!("[relava-server] cannot determine home directory");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = relava_dir.ensure_dirs() {
        eprintln!("[relava-server] failed to create data directories: {e}");
        return ExitCode::FAILURE;
    }

    // Determine GUI directory: RELAVA_GUI_DIR env var overrides the default.
    let gui_dir = std::env::var("RELAVA_GUI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| relava_dir.gui_dir());

    let config = ServerConfig {
        host: host.clone(),
        port,
        data_dir: relava_dir.root().to_path_buf(),
        cache_dir: relava_dir.cache_dir(),
    };

    let app =
        match relava_server::app_with_config(&relava_dir.db_path(), Some(&gui_dir), Some(config)) {
            Ok(app) => app,
            Err(e) => {
                eprintln!("[relava-server] failed to open database: {e}");
                return ExitCode::FAILURE;
            }
        };

    let addr = format!("{host}:{port}");

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
