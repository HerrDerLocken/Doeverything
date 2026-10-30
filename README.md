# Doeverything

Universeller Tracker: To-Do-Listen, Einkaufslisten, Finanzen, Fitness und mehr.
Local-first, Ende-zu-Ende-verschlüsselter Sync über einen selbst hostbaren Server oder im LAN.

## Aufbau

| Pfad | Inhalt |
|---|---|
| `app/` | Flutter-App (Linux, Android, Windows, macOS, iOS) |
| `app/rust` | Brücke zwischen App und Core (flutter_rust_bridge) |
| `crates/core` | Datenmodell, HLC, Konfliktlösung, Replikation |
| `crates/store` | Lokale Speicherung in SQLite, optional verschlüsselt (Feature `sqlcipher`) |
| `crates/sync` | Sync-Client: Konto, Gerätekopplung, Push/Pull |
| `crates/proto` | Sync-Nachrichten und CBOR-Wire-Format |
| `crates/server` | Sync-Relay-Server (axum) |
| `docs/design/` | Design-Dokumente |

## Entwickeln

Voraussetzung: [rustup](https://rustup.rs). Die Toolchain wird über `rust-toolchain.toml` automatisch gewählt.

```sh
cargo test                         # alle Tests
cargo test --all-features          # inkl. verschlüsselter Datenbank (kompiliert OpenSSL mit)
cargo clippy --all-targets         # Lints
cargo run -p doeverything-server   # Server auf 127.0.0.1:8080 (DOEVERYTHING_BIND zum Ändern)
```

App (Flutter-SDK im `PATH`, Rust wird beim Build automatisch mitkompiliert):

```sh
cd app
flutter run -d linux                         # starten
flutter test integration_test -d linux       # UI-Test
flutter_rust_bridge_codegen generate         # nach Änderungen an app/rust/src/api
```

Einmalig: `cargo install flutter_rust_bridge_codegen --locked`

### Sync-Server selbst hosten

Im Container (Podman lokal, Docker-kompatibel):

```sh
podman compose up --build -d                          # oder: docker compose up --build -d
DOEVERYTHING_PORT=18080 podman compose up --build -d  # falls Port 8080 belegt ist
curl localhost:8080/health
```

Daten liegen im Volume `server-data`. Einstellungen (Umgebungsvariablen):

| Variable | Standard | Bedeutung |
|---|---|---|
| `DOEVERYTHING_BIND` | `127.0.0.1:8080` (Container: `0.0.0.0:8080`) | Adresse |
| `DOEVERYTHING_DB` | `doeverything-server.sqlite` (Container: `/data/server.sqlite`) | Datenbank |
| `DOEVERYTHING_OPEN_REGISTRATION` | `true` | `false` verbietet neue Konten, sobald alle eigenen Geräte verbunden sind |

Der Server speichert nur verschlüsselte, signierte Pakete; er kann keine Inhalte lesen. Für
den Betrieb im Internet einen Reverse-Proxy mit TLS davorschalten (z. B. Caddy) und die App mit
`https://…` verbinden. In der App: Wolken-Symbol oben rechts → Server-Adresse eintragen
(ohne `http://` wird bei lokalen Adressen `http://`, sonst `https://` ergänzt).

Test gegen einen laufenden Server:
`DOEVERYTHING_TEST_SERVER=localhost:8080 cargo test -p doeverything-sync -- --ignored`

In VSCodium: den Ordner `Doeverything` selbst öffnen (nicht einen Elternordner), empfohlene Extensions installieren, dann *Terminal → Run Task* bzw. *Run and Debug*.

## Lizenz

AGPL-3.0-or-later
