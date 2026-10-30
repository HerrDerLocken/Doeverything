# Baut den Sync-Server. Funktioniert mit `podman build` und `docker build`.

FROM docker.io/library/rust:1.99-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p doeverything-server

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /src/target/release/doeverything-server /usr/local/bin/doeverything-server
ENV DOEVERYTHING_BIND=0.0.0.0:8080
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/usr/local/bin/doeverything-server"]
