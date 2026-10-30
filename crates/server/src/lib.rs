//! Sync-Relay-Server. Speichert nur verschlüsselte, signierte Envelopes und kann keine Inhalte
//! lesen. Siehe Abschnitt 6.3 der Design-Doku.
//!
//! Anmeldung ohne Passwort: Das Gerät signiert eine Nonce mit seinem Ed25519-Schlüssel und erhält
//! ein Token. Alle Bodies sind CBOR.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use doeverything_core::{DeviceId, EntityId};
use doeverything_proto::api::{
    AccountId, CONTENT_TYPE, ChallengeRequest, ChallengeResponse, CreateAccount, DeviceInfo,
    DevicesResponse, ErrorBody, PullResponse, PulledEnvelope, PushRequest, PushResponse,
    RegisterDevice, SessionResponse, SpacesResponse, VerifyRequest, VersionVectorResponse,
};
use doeverything_proto::crypto::{self, CTX_AUTH, CTX_DEVICE};
use doeverything_proto::{Envelope, decode, encode};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: i64 = 1;
const SCHEMA_V1: &str = "
CREATE TABLE accounts (
  id         BLOB PRIMARY KEY,
  created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE devices (
  id          BLOB PRIMARY KEY,
  account_id  BLOB NOT NULL,
  signing_pub BLOB NOT NULL,
  name        TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  revoked     INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX devices_account ON devices(account_id);
CREATE TABLE challenges (
  nonce      BLOB PRIMARY KEY,
  device_id  BLOB NOT NULL,
  expires_at INTEGER NOT NULL
) STRICT;
CREATE TABLE sessions (
  token_hash BLOB PRIMARY KEY,
  device_id  BLOB NOT NULL,
  account_id BLOB NOT NULL,
  expires_at INTEGER NOT NULL
) STRICT;
CREATE TABLE spaces (
  id         BLOB PRIMARY KEY,
  account_id BLOB NOT NULL
) STRICT;
CREATE TABLE envelopes (
  space_id    BLOB NOT NULL,
  server_seq  INTEGER NOT NULL,
  device_id   BLOB NOT NULL,
  seq         INTEGER NOT NULL,
  envelope    BLOB NOT NULL,
  received_at INTEGER NOT NULL,
  PRIMARY KEY (space_id, server_seq),
  UNIQUE (space_id, device_id, seq)
) STRICT;
CREATE TABLE pairing (
  id         BLOB PRIMARY KEY,
  account_id BLOB NOT NULL,
  message    BLOB NOT NULL,
  expires_at INTEGER NOT NULL
) STRICT;
";

const CHALLENGE_TTL_S: i64 = 120;
const SESSION_TTL_S: i64 = 24 * 60 * 60;
const PAIRING_TTL_S: i64 = 10 * 60;
const PULL_LIMIT_MAX: u32 = 1000;

#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Connection>>,
    open_registration: bool,
}

impl AppState {
    /// # Errors
    /// Wenn die Datenbank nicht geöffnet oder migriert werden kann.
    pub fn open(db_path: impl AsRef<Path>, open_registration: bool) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open(db_path)?, open_registration)
    }

    /// # Errors
    /// Nur bei internen SQLite-Fehlern.
    pub fn in_memory(open_registration: bool) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?, open_registration)
    }

    fn from_connection(mut conn: Connection, open_registration: bool) -> rusqlite::Result<Self> {
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        assert!(
            version <= SCHEMA_VERSION,
            "Server-Datenbank stammt von einer neueren Version ({version})"
        );
        let tx = conn.transaction()?;
        if version < 1 {
            tx.execute_batch(SCHEMA_V1)?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
            open_registration,
        })
    }

    /// Führt Datenbankarbeit außerhalb des async-Runtimes aus.
    async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, ApiError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let mut conn = db
                .lock()
                .map_err(|_| ApiError::Internal("Datenbank gesperrt".into()))?;
            f(&mut conn)
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/account", post(create_account))
        .route("/v1/auth/challenge", post(challenge))
        .route("/v1/auth/verify", post(verify))
        .route("/v1/devices", get(list_devices).post(register_device))
        .route("/v1/spaces", get(list_spaces))
        .route("/v1/spaces/{space}/vv", get(version_vector))
        .route("/v1/spaces/{space}/push", post(push))
        .route("/v1/spaces/{space}/pull", get(pull))
        .route(
            "/v1/pairing/{id}",
            put(put_pairing).get(get_pairing).delete(delete_pairing),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------------------------
// Fehler und Kodierung

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized,
    Forbidden(String),
    NotFound,
    Conflict(String),
    Internal(String),
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "nicht angemeldet".into()),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            Self::NotFound => (StatusCode::NOT_FOUND, "nicht gefunden".into()),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::Internal(m) => {
                tracing::error!("interner Fehler: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, "interner Fehler".into())
            }
        };
        cbor(status, &ErrorBody { message })
    }
}

fn cbor<T: serde::Serialize>(status: StatusCode, value: &T) -> Response {
    match encode(value) {
        Ok(body) => (status, [(header::CONTENT_TYPE, CONTENT_TYPE)], body).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn ok<T: serde::Serialize>(value: &T) -> Response {
    cbor(StatusCode::OK, value)
}

fn body<T: for<'de> Deserialize<'de>>(bytes: &Bytes) -> Result<T, ApiError> {
    decode(bytes).map_err(|e| ApiError::BadRequest(e.to_string()))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn blob16(bytes: &[u8]) -> Result<[u8; 16], ApiError> {
    bytes
        .try_into()
        .map_err(|_| ApiError::Internal("ID mit falscher Länge".into()))
}

fn parse_space(s: &str) -> Result<EntityId, ApiError> {
    s.parse()
        .map_err(|_| ApiError::BadRequest("ungültige Space-ID".into()))
}

fn parse_pairing_id(s: &str) -> Result<[u8; 16], ApiError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| ApiError::BadRequest("ungültige Kopplungs-ID".into()))?;
    bytes
        .try_into()
        .map_err(|_| ApiError::BadRequest("ungültige Kopplungs-ID".into()))
}

fn seq_i64(seq: u64) -> Result<i64, ApiError> {
    i64::try_from(seq).map_err(|_| ApiError::BadRequest("seq zu groß".into()))
}

fn verify_device_proof(device: &DeviceInfo, proof: &[u8; 64]) -> Result<(), ApiError> {
    crypto::verify(
        &device.signing_pub,
        CTX_DEVICE,
        &crypto::device_proof_message(device),
        proof,
    )
    .map_err(|_| ApiError::BadRequest("Gerätenachweis ungültig".into()))
}

// ---------------------------------------------------------------------------------------------
// Anmeldung

struct Auth {
    account: AccountId,
    device: DeviceId,
    signing_pub: [u8; 32],
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<Auth, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?
        .to_owned();
    state
        .db(move |conn| {
            let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = conn
                .query_row(
                    "SELECT s.account_id, s.device_id, d.signing_pub
                     FROM sessions s JOIN devices d ON d.id = s.device_id
                     WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND d.revoked = 0",
                    params![token_hash(&token), now()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let (account, device, signing_pub) = row.ok_or(ApiError::Unauthorized)?;
            Ok(Auth {
                account: AccountId(blob16(&account)?),
                device: DeviceId(blob16(&device)?),
                signing_pub: signing_pub
                    .try_into()
                    .map_err(|_| ApiError::Internal("Schlüssel mit falscher Länge".into()))?,
            })
        })
        .await
}

async fn create_account(State(state): State<AppState>, bytes: Bytes) -> Result<Response, ApiError> {
    if !state.open_registration {
        return Err(ApiError::Forbidden(
            "Registrierung auf diesem Server geschlossen".into(),
        ));
    }
    let req: CreateAccount = body(&bytes)?;
    verify_device_proof(&req.device, &req.proof)?;
    state
        .db(move |conn| {
            let tx = conn.transaction()?;
            let inserted = tx.execute(
                "INSERT INTO accounts (id, created_at) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                params![&req.account.0[..], now()],
            )?;
            if inserted == 0 {
                return Err(ApiError::Conflict("Account existiert bereits".into()));
            }
            insert_device(&tx, req.account, &req.device)?;
            tx.commit()?;
            Ok(StatusCode::CREATED.into_response())
        })
        .await
}

fn insert_device(
    conn: &Connection,
    account: AccountId,
    device: &DeviceInfo,
) -> Result<(), ApiError> {
    let existing: Option<Vec<u8>> = conn
        .query_row(
            "SELECT account_id FROM devices WHERE id = ?1",
            params![&device.device.0[..]],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(a) if a == account.0 => Ok(()),
        Some(_) => Err(ApiError::Conflict(
            "Gerät gehört zu einem anderen Account".into(),
        )),
        None => {
            conn.execute(
                "INSERT INTO devices (id, account_id, signing_pub, name, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![&device.device.0[..], &account.0[..], &device.signing_pub[..], device.name, now()],
            )?;
            Ok(())
        }
    }
}

async fn challenge(State(state): State<AppState>, bytes: Bytes) -> Result<Response, ApiError> {
    let req: ChallengeRequest = body(&bytes)?;
    let nonce: [u8; 32] = crypto::random();
    state
        .db(move |conn| {
            let known: bool = conn.query_row(
                "SELECT count(*) > 0 FROM devices WHERE id = ?1 AND revoked = 0",
                params![&req.device.0[..]],
                |r| r.get(0),
            )?;
            if !known {
                return Err(ApiError::Unauthorized);
            }
            conn.execute(
                "DELETE FROM challenges WHERE expires_at <= ?1",
                params![now()],
            )?;
            conn.execute(
                "INSERT INTO challenges (nonce, device_id, expires_at) VALUES (?1, ?2, ?3)",
                params![&nonce[..], &req.device.0[..], now() + CHALLENGE_TTL_S],
            )?;
            Ok(ok(&ChallengeResponse { nonce }))
        })
        .await
}

async fn verify(State(state): State<AppState>, bytes: Bytes) -> Result<Response, ApiError> {
    let req: VerifyRequest = body(&bytes)?;
    let token = URL_SAFE_NO_PAD.encode(crypto::random::<32>());
    state
        .db(move |conn| {
            let tx = conn.transaction()?;
            // Nonce ist einmalig: sofort löschen, egal ob die Signatur stimmt
            let found = tx.execute(
                "DELETE FROM challenges WHERE nonce = ?1 AND device_id = ?2 AND expires_at > ?3",
                params![&req.nonce[..], &req.device.0[..], now()],
            )?;
            let device: Option<(Vec<u8>, Vec<u8>)> = tx
                .query_row(
                    "SELECT account_id, signing_pub FROM devices WHERE id = ?1 AND revoked = 0",
                    params![&req.device.0[..]],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            tx.commit()?;
            let (Some((account, signing_pub)), 1) = (device, found) else {
                return Err(ApiError::Unauthorized);
            };
            let signing_pub: [u8; 32] =
                signing_pub.try_into().map_err(|_| ApiError::Internal("Schlüssellänge".into()))?;
            crypto::verify(&signing_pub, CTX_AUTH, &req.nonce, &req.signature).map_err(|_| ApiError::Unauthorized)?;

            conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", params![now()])?;
            conn.execute(
                "INSERT INTO sessions (token_hash, device_id, account_id, expires_at) VALUES (?1, ?2, ?3, ?4)",
                params![token_hash(&token), &req.device.0[..], &account, now() + SESSION_TTL_S],
            )?;
            Ok(ok(&SessionResponse {
                token,
                account: AccountId(blob16(&account)?),
                expires_in_s: SESSION_TTL_S.unsigned_abs(),
            }))
        })
        .await
}

// ---------------------------------------------------------------------------------------------
// Geräte und Spaces

async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    let req: RegisterDevice = body(&bytes)?;
    verify_device_proof(&req.device, &req.proof)?;
    state
        .db(move |conn| {
            insert_device(conn, auth.account, &req.device)?;
            Ok(StatusCode::CREATED.into_response())
        })
        .await
}

async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    state
        .db(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, signing_pub, name FROM devices WHERE account_id = ?1 AND revoked = 0 ORDER BY created_at",
            )?;
            let mut rows = stmt.query(params![&auth.account.0[..]])?;
            let mut devices = Vec::new();
            while let Some(row) = rows.next()? {
                devices.push(DeviceInfo {
                    device: DeviceId(blob16(&row.get::<_, Vec<u8>>(0)?)?),
                    signing_pub: row
                        .get::<_, Vec<u8>>(1)?
                        .try_into()
                        .map_err(|_| ApiError::Internal("Schlüssellänge".into()))?,
                    name: row.get(2)?,
                });
            }
            Ok(ok(&DevicesResponse { devices }))
        })
        .await
}

async fn list_spaces(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    state
        .db(move |conn| {
            let mut stmt =
                conn.prepare("SELECT id FROM spaces WHERE account_id = ?1 ORDER BY id")?;
            let spaces = stmt
                .query_map(params![&auth.account.0[..]], |r| r.get::<_, Vec<u8>>(0))?
                .map(|id| Ok(EntityId(blob16(&id?)?)))
                .collect::<Result<Vec<_>, ApiError>>()?;
            Ok(ok(&SpacesResponse { spaces }))
        })
        .await
}

/// Prüft, ob der Space dem Account gehört. `claim`: unbekannte Spaces dem Account zuordnen.
fn check_space(
    conn: &Connection,
    space: EntityId,
    account: AccountId,
    claim: bool,
) -> Result<bool, ApiError> {
    let owner: Option<Vec<u8>> = conn
        .query_row(
            "SELECT account_id FROM spaces WHERE id = ?1",
            params![&space.0[..]],
            |r| r.get(0),
        )
        .optional()?;
    match owner {
        Some(o) if o == account.0 => Ok(true),
        Some(_) => Err(ApiError::Forbidden(
            "Space gehört zu einem anderen Account".into(),
        )),
        None if claim => {
            conn.execute(
                "INSERT INTO spaces (id, account_id) VALUES (?1, ?2)",
                params![&space.0[..], &account.0[..]],
            )?;
            Ok(true)
        }
        None => Ok(false),
    }
}

async fn version_vector(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(space): UrlPath<String>,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    let space = parse_space(&space)?;
    state
        .db(move |conn| {
            let mut out = VersionVectorResponse::default();
            if check_space(conn, space, auth.account, false)? {
                let mut stmt =
                    conn.prepare("SELECT device_id, max(seq) FROM envelopes WHERE space_id = ?1 GROUP BY device_id")?;
                let mut rows = stmt.query(params![&space.0[..]])?;
                while let Some(row) = rows.next()? {
                    let device = DeviceId(blob16(&row.get::<_, Vec<u8>>(0)?)?);
                    out.vv.insert(device, row.get::<_, i64>(1)?.unsigned_abs());
                }
            }
            Ok(ok(&out))
        })
        .await
}

async fn push(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(space): UrlPath<String>,
    bytes: Bytes,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    let space = parse_space(&space)?;
    let req: PushRequest = body(&bytes)?;
    for env in &req.envelopes {
        if env.header.space != space {
            return Err(ApiError::BadRequest(
                "Envelope gehört zu einem anderen Space".into(),
            ));
        }
        if env.header.device != auth.device {
            return Err(ApiError::Forbidden(
                "nur eigene Changesets hochladen".into(),
            ));
        }
        crypto::verify_envelope(env, &auth.signing_pub)
            .map_err(|_| ApiError::BadRequest("ungültige Signatur".into()))?;
    }
    state
        .db(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_space(&tx, space, auth.account, true)?;
            let (mut stored, mut duplicates) = (0u32, 0u32);
            let mut envelopes = req.envelopes;
            envelopes.sort_by_key(|e| e.header.seq);
            for env in envelopes {
                let max_seq: i64 = tx.query_row(
                    "SELECT coalesce(max(seq), 0) FROM envelopes WHERE space_id = ?1 AND device_id = ?2",
                    params![&space.0[..], &auth.device.0[..]],
                    |r| r.get(0),
                )?;
                let seq = seq_i64(env.header.seq)?;
                if seq <= max_seq {
                    duplicates += 1;
                    continue;
                }
                if seq != max_seq + 1 {
                    return Err(ApiError::Conflict(format!("Lücke: erwartet seq {}, bekommen {seq}", max_seq + 1)));
                }
                let server_seq: i64 = tx.query_row(
                    "SELECT coalesce(max(server_seq), 0) + 1 FROM envelopes WHERE space_id = ?1",
                    params![&space.0[..]],
                    |r| r.get(0),
                )?;
                let blob = encode(&env).map_err(|e| ApiError::Internal(e.to_string()))?;
                tx.execute(
                    "INSERT INTO envelopes (space_id, server_seq, device_id, seq, envelope, received_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![&space.0[..], server_seq, &auth.device.0[..], seq, blob, now()],
                )?;
                stored += 1;
            }
            tx.commit()?;
            Ok(ok(&PushResponse { stored, duplicates }))
        })
        .await
}

#[derive(Deserialize)]
struct PullQuery {
    #[serde(default)]
    after: u64,
    limit: Option<u32>,
}

async fn pull(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(space): UrlPath<String>,
    Query(q): Query<PullQuery>,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    let space = parse_space(&space)?;
    let limit = q.limit.unwrap_or(500).clamp(1, PULL_LIMIT_MAX);
    let after = seq_i64(q.after)?;
    state
        .db(move |conn| {
            if !check_space(conn, space, auth.account, false)? {
                return Ok(ok(&PullResponse {
                    items: Vec::new(),
                    more: false,
                }));
            }
            let mut stmt = conn.prepare(
                "SELECT server_seq, envelope FROM envelopes
                 WHERE space_id = ?1 AND server_seq > ?2 ORDER BY server_seq LIMIT ?3",
            )?;
            let mut rows = stmt.query(params![&space.0[..], after, i64::from(limit) + 1])?;
            let mut items = Vec::new();
            while let Some(row) = rows.next()? {
                let envelope: Envelope = decode(&row.get::<_, Vec<u8>>(1)?)
                    .map_err(|e| ApiError::Internal(e.to_string()))?;
                items.push(PulledEnvelope {
                    server_seq: row.get::<_, i64>(0)?.unsigned_abs(),
                    envelope,
                });
            }
            let more = items.len() > limit as usize;
            items.truncate(limit as usize);
            Ok(ok(&PullResponse { items, more }))
        })
        .await
}

// ---------------------------------------------------------------------------------------------
// Kopplungs-Postfach

async fn put_pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    bytes: Bytes,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    let id = parse_pairing_id(&id)?;
    // Nur prüfen, dass es eine Kopplungsnachricht ist; lesen kann der Server sie nicht.
    let _: crypto::PairingMessage = body(&bytes)?;
    state
        .db(move |conn| {
            conn.execute("DELETE FROM pairing WHERE expires_at <= ?1", params![now()])?;
            conn.execute(
                "INSERT INTO pairing (id, account_id, message, expires_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET message = excluded.message, expires_at = excluded.expires_at
                 WHERE pairing.account_id = excluded.account_id",
                params![&id[..], &auth.account.0[..], &bytes[..], now() + PAIRING_TTL_S],
            )?;
            Ok(StatusCode::NO_CONTENT.into_response())
        })
        .await
}

async fn get_pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    let id = parse_pairing_id(&id)?;
    state
        .db(move |conn| {
            let message: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT message FROM pairing WHERE id = ?1 AND account_id = ?2 AND expires_at > ?3",
                    params![&id[..], &auth.account.0[..], now()],
                    |r| r.get(0),
                )
                .optional()?;
            let message = message.ok_or(ApiError::NotFound)?;
            Ok((StatusCode::OK, [(header::CONTENT_TYPE, CONTENT_TYPE)], message).into_response())
        })
        .await
}

async fn delete_pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Response, ApiError> {
    let auth = authenticate(&state, &headers).await?;
    let id = parse_pairing_id(&id)?;
    state
        .db(move |conn| {
            conn.execute(
                "DELETE FROM pairing WHERE id = ?1 AND account_id = ?2",
                params![&id[..], &auth.account.0[..]],
            )?;
            Ok(StatusCode::NO_CONTENT.into_response())
        })
        .await
}

/// Startet den Server auf einem bereits gebundenen Listener (für Tests und `main`).
///
/// # Errors
/// Wenn der Listener nicht mehr angenommen werden kann.
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: AppState,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown)
        .await
}
