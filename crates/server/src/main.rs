//! Startet den Sync-Server.
//!
//! Umgebungsvariablen:
//! - `DOEVERYTHING_BIND`: Adresse, Standard `127.0.0.1:8080`
//! - `DOEVERYTHING_DB`: Datenbankdatei, Standard `doeverything-server.sqlite`
//! - `DOEVERYTHING_OPEN_REGISTRATION`: `false` verbietet neue Accounts (Standard `true`)

use std::net::SocketAddr;

use doeverything_server::{AppState, serve};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr: SocketAddr = std::env::var("DOEVERYTHING_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()
        .expect("DOEVERYTHING_BIND muss eine Socket-Adresse sein, z. B. 0.0.0.0:8080");
    let db =
        std::env::var("DOEVERYTHING_DB").unwrap_or_else(|_| "doeverything-server.sqlite".into());
    let open_registration =
        std::env::var("DOEVERYTHING_OPEN_REGISTRATION").map_or(true, |v| v != "false");

    let state = AppState::open(&db, open_registration).expect("Server-Datenbank öffnen");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Port belegt?");
    tracing::info!(
        "lausche auf http://{addr}, Datenbank {db}, Registrierung {}",
        if open_registration {
            "offen"
        } else {
            "geschlossen"
        }
    );
    serve(listener, state, async {
        tokio::signal::ctrl_c().await.ok();
    })
    .await
    .expect("Server-Fehler");
}
