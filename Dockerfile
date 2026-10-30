# Baut den Sync-Server. Funktioniert mit `podman build` und `docker build`.

FROM docker.io/library/rust:1.99-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Gehört zum Workspace, Cargo braucht das Manifest; gebaut wird nur der Server
COPY app/rust/Cargo.toml ./app/rust/Cargo.toml
COPY app/rust/src ./app/rust/src
RUN cargo build --release --locked -p doeverything-server && mkdir -p /out/data

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /src/target/release/doeverything-server /usr/local/bin/doeverything-server
# Datenverzeichnis gehört dem unprivilegierten Benutzer (65532 = nonroot)
COPY --from=build --chown=65532:65532 /out/data /data
ENV DOEVERYTHING_BIND=0.0.0.0:8080 \
    DOEVERYTHING_DB=/data/server.sqlite
VOLUME /data
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/usr/local/bin/doeverything-server"]
