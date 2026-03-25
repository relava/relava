use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let host = std::env::var("RELAVA_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("RELAVA_PORT")
        .unwrap_or_else(|_| "7420".to_string())
        .parse()
        .expect("RELAVA_PORT must be a valid port number (0-65535)");
    let addr = format!("{host}:{port}");

    let app = relava_server::app();

    eprintln!("relava-server listening on {addr}");
    let listener = TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind to {addr}: {e}"));
    axum::serve(listener, app)
        .await
        .expect("server exited with error");
}
