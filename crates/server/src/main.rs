//! Sync-Relay-Server. Speichert nur verschlüsselte Envelopes; siehe Abschnitt 6.3 der Design-Doku.

use std::net::SocketAddr;

use axum::{Router, routing::get};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr: SocketAddr = std::env::var("DOEVERYTHING_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()
        .expect("DOEVERYTHING_BIND muss eine Socket-Adresse sein, z. B. 0.0.0.0:8080");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Port belegt?");
    tracing::info!("lausche auf http://{addr}");
    axum::serve(listener, app())
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await
        .expect("Server-Fehler");
}

fn app() -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}
