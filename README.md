# Doeverything

Universeller Tracker: To-Do-Listen, Einkaufslisten, Finanzen, Fitness und mehr.
Local-first, Ende-zu-Ende-verschlüsselter Sync über einen selbst hostbaren Server oder im LAN.

## Aufbau

| Pfad | Inhalt |
|---|---|
| `crates/core` | Datenmodell, HLC, Konfliktlösung, Replikation |
| `crates/proto` | Sync-Nachrichten und CBOR-Wire-Format |
| `crates/server` | Sync-Relay-Server (axum) |
| `docs/design/` | Design-Dokumente |

## Entwickeln

Voraussetzung: [rustup](https://rustup.rs). Die Toolchain wird über `rust-toolchain.toml` automatisch gewählt.

```sh
cargo test                         # alle Tests
cargo clippy --all-targets         # Lints
cargo run -p doeverything-server   # Server auf 127.0.0.1:8080 (DOEVERYTHING_BIND zum Ändern)
```

Server im Container (Podman lokal, Docker-kompatibel):

```sh
podman compose up --build          # oder: docker compose up --build
curl localhost:8080/health
```

In VSCodium: den Ordner `Doeverything` selbst öffnen (nicht einen Elternordner), empfohlene Extensions installieren, dann *Terminal → Run Task* bzw. *Run and Debug*.

## Lizenz

AGPL-3.0-or-later
