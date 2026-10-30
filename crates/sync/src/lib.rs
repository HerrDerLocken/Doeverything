//! Sync mit dem Server: Konto anlegen, Geräte koppeln, Änderungen austauschen.
//!
//! Alle Zugangsdaten (Server, Account, Account-Schlüssel, Geräteschlüssel, Token) liegen im
//! `meta`-Bereich des lokalen Speichers, bei SQLite also in der verschlüsselten Datenbank.
//!
//! Die Funktionen nehmen ein `Mutex<Replica>` und halten die Sperre nur für lokale Schritte,
//! nie während Netzwerkzugriffen. So bleibt die App bedienbar, während synchronisiert wird.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use doeverything_core::hlc::now_ms;
use doeverything_core::{
    ApplyOutcome, DeviceId, EntityId, ReceiveError, Replica, Store, VersionVector,
};
use doeverything_proto::api::{
    AccountId, CONTENT_TYPE, ChallengeRequest, ChallengeResponse, CreateAccount, DeviceInfo,
    DevicesResponse, ErrorBody, PullResponse, PushRequest, PushResponse, RegisterDevice,
    SessionResponse, SpacesResponse, VerifyRequest, VersionVectorResponse,
};
use doeverything_proto::crypto::{
    self, CTX_AUTH, CTX_DEVICE, CryptoError, DeviceKeys, PairingCode, PairingMessage, PairingSecret,
};
use doeverything_proto::{decode, encode};
use serde::Serialize;
use serde::de::DeserializeOwned;

const META_SERVER: &str = "sync.server";
const META_ACCOUNT: &str = "sync.account";
const META_ACCOUNT_KEY: &str = "sync.account_key";
const META_SIGNING_KEY: &str = "sync.signing_key";
const META_EXCHANGE_KEY: &str = "sync.exchange_key";
const META_PAIRING: &str = "sync.pairing";
const META_TOKEN: &str = "sync.token";
const META_CURSOR_PREFIX: &str = "sync.cursor.";

/// Changesets pro Push-Anfrage.
const PUSH_BATCH: usize = 200;
/// Aktuelle Schlüsselepoche (siehe `crypto::space_key`).
const KEY_EPOCH: u32 = 0;

#[derive(Debug)]
pub enum SyncError {
    /// Sync ist auf diesem Gerät nicht eingerichtet.
    NotConfigured,
    /// Sync ist bereits eingerichtet (z. B. erneutes "Konto anlegen").
    AlreadyConfigured,
    InvalidInput(String),
    /// Netzwerk nicht erreichbar, Zeitüberschreitung, TLS-Fehler …
    Network(String),
    /// Der Server hat mit einem Fehler geantwortet.
    Server {
        status: u16,
        message: String,
    },
    Crypto(CryptoError),
    Store(String),
    /// Unerwarteter Zustand im Protokoll (z. B. Lücke im Log).
    Protocol(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => write!(f, "Sync ist nicht eingerichtet"),
            Self::AlreadyConfigured => write!(f, "Sync ist bereits eingerichtet"),
            Self::InvalidInput(m) => write!(f, "{m}"),
            Self::Network(m) => write!(f, "Server nicht erreichbar: {m}"),
            Self::Server { status, message } => write!(f, "Server meldet {status}: {message}"),
            Self::Crypto(e) => write!(f, "Kryptografie: {e}"),
            Self::Store(m) => write!(f, "lokaler Speicher: {m}"),
            Self::Protocol(m) => write!(f, "Protokollfehler: {m}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<CryptoError> for SyncError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

pub type Result<T> = std::result::Result<T, SyncError>;

fn store_err(e: impl fmt::Display) -> SyncError {
    SyncError::Store(e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    Off,
    /// Dieses Gerät wartet darauf, von einem bestehenden Gerät bestätigt zu werden.
    Pairing {
        server: String,
        code: String,
    },
    Connected {
        server: String,
        account: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncReport {
    pub pushed: u32,
    pub pulled: u32,
    /// Changesets unbekannter oder entfernter Geräte, die verworfen wurden.
    pub rejected: u32,
}

// ---------------------------------------------------------------------------------------------
// Zugangsdaten im lokalen Speicher

struct Credentials {
    server: String,
    account: AccountId,
    account_key: [u8; 32],
    keys: DeviceKeys,
    device: DeviceId,
}

fn lock<S>(replica: &Mutex<Replica<S>>) -> Result<MutexGuard<'_, Replica<S>>> {
    replica
        .lock()
        .map_err(|_| SyncError::Store("Sperre nach Absturz vergiftet".into()))
}

fn meta<S: Store>(r: &Replica<S>, key: &str) -> Result<Option<Vec<u8>>> {
    r.store().meta(key).map_err(store_err)
}

fn meta32(r: &Replica<impl Store>, key: &str) -> Result<Option<[u8; 32]>> {
    meta(r, key)?
        .map(|v| {
            v.try_into()
                .map_err(|_| SyncError::Store(format!("{key}: falsche Länge")))
        })
        .transpose()
}

fn set_meta<S: Store>(r: &mut Replica<S>, key: &str, value: Option<&[u8]>) -> Result<()> {
    r.store_mut().set_meta(key, value).map_err(store_err)
}

fn load_keys<S: Store>(r: &Replica<S>) -> Result<Option<DeviceKeys>> {
    match (meta32(r, META_SIGNING_KEY)?, meta32(r, META_EXCHANGE_KEY)?) {
        (Some(s), Some(x)) => Ok(Some(DeviceKeys::from_bytes(s, x))),
        _ => Ok(None),
    }
}

fn save_keys<S: Store>(r: &mut Replica<S>, keys: &DeviceKeys) -> Result<()> {
    set_meta(r, META_SIGNING_KEY, Some(&keys.signing.to_bytes()))?;
    set_meta(r, META_EXCHANGE_KEY, Some(&keys.exchange.to_bytes()))
}

fn load_credentials<S: Store>(r: &Replica<S>) -> Result<Option<Credentials>> {
    let server = meta(r, META_SERVER)?.map(|s| String::from_utf8_lossy(&s).into_owned());
    let account = meta(r, META_ACCOUNT)?;
    let account_key = meta32(r, META_ACCOUNT_KEY)?;
    let (Some(server), Some(account), Some(account_key), Some(keys)) =
        (server, account, account_key, load_keys(r)?)
    else {
        return Ok(None);
    };
    let account = AccountId(
        account
            .try_into()
            .map_err(|_| SyncError::Store("Account-ID: falsche Länge".into()))?,
    );
    Ok(Some(Credentials {
        server,
        account,
        account_key,
        keys,
        device: r.device(),
    }))
}

fn credentials<S: Store>(replica: &Mutex<Replica<S>>) -> Result<Credentials> {
    load_credentials(&*lock(replica)?)?.ok_or(SyncError::NotConfigured)
}

fn save_account<S: Store>(
    r: &mut Replica<S>,
    account: AccountId,
    account_key: &[u8; 32],
) -> Result<()> {
    set_meta(r, META_ACCOUNT, Some(&account.0))?;
    set_meta(r, META_ACCOUNT_KEY, Some(account_key))
}

/// Server-Adresse vereinheitlichen. Ohne Vorsilbe wird `https://` ergänzt, bei lokalen Adressen
/// (`localhost`, `*.local`, private IPv4-Bereiche) `http://`, weil ein Server im Heimnetz meist
/// kein Zertifikat hat. Andere Protokolle als HTTP(S) werden abgelehnt.
///
/// # Errors
/// [`SyncError::InvalidInput`] bei leerer oder unbrauchbarer Adresse.
pub fn normalize_server(input: &str) -> Result<String> {
    let s = input.trim();
    let (scheme, rest) = match s.split_once("://") {
        Some((scheme @ ("http" | "https"), rest)) => (Some(scheme), rest),
        Some((scheme, _)) => {
            return Err(SyncError::InvalidInput(format!(
                "Protokoll {scheme}:// wird nicht unterstützt, nur http:// oder https://"
            )));
        }
        None => (None, s),
    };
    let rest = rest.trim_end_matches('/');
    let host = rest.split(['/', ':']).next().unwrap_or_default();
    if host.is_empty() || rest.contains(char::is_whitespace) {
        return Err(SyncError::InvalidInput(if s.is_empty() {
            "Bitte eine Server-Adresse eingeben".into()
        } else {
            format!("„{s}“ ist keine gültige Server-Adresse")
        }));
    }
    let scheme = scheme.unwrap_or(if is_local_host(host) { "http" } else { "https" });
    Ok(format!("{scheme}://{rest}"))
}

fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.rsplit_once('.').is_some_and(|(_, tld)| tld == "local")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_private() || ip.is_link_local())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

// ---------------------------------------------------------------------------------------------
// HTTP

/// Dünne Hülle um die Server-API.
pub struct Client {
    agent: ureq::Agent,
    server: String,
    token: Option<String>,
}

impl Client {
    pub fn new(server: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self {
            agent,
            server: server.to_owned(),
            token: None,
        }
    }

    fn with_auth<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        match &self.token {
            Some(t) => req.header("Authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    fn finish(
        resp: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Vec<u8>> {
        let mut resp = resp.map_err(|e| SyncError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .read_to_vec()
            .map_err(|e| SyncError::Network(e.to_string()))?;
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            let message = decode::<ErrorBody>(&body).map_or_else(
                |_| String::from_utf8_lossy(&body).into_owned(),
                |e| e.message,
            );
            Err(SyncError::Server { status, message })
        }
    }

    fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
        decode(body).map_err(|e| SyncError::Protocol(e.to_string()))
    }

    fn cbor<T: Serialize>(value: &T) -> Result<Vec<u8>> {
        encode(value).map_err(|e| SyncError::Protocol(e.to_string()))
    }

    /// # Errors
    /// Netzwerk- oder Serverfehler.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let req = self.with_auth(self.agent.get(format!("{}{path}", self.server)));
        Self::parse(&Self::finish(req.call())?)
    }

    fn get_raw(&self, path: &str) -> Result<Vec<u8>> {
        let req = self.with_auth(self.agent.get(format!("{}{path}", self.server)));
        Self::finish(req.call())
    }

    fn send(&self, method: &str, path: &str, body: &[u8]) -> Result<Vec<u8>> {
        let url = format!("{}{path}", self.server);
        let resp = match method {
            "PUT" => self
                .with_auth(self.agent.put(url))
                .header("Content-Type", CONTENT_TYPE)
                .send(body),
            _ => self
                .with_auth(self.agent.post(url))
                .header("Content-Type", CONTENT_TYPE)
                .send(body),
        };
        Self::finish(resp)
    }

    /// # Errors
    /// Netzwerk- oder Serverfehler.
    pub fn post<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        path: &str,
        req: &Req,
    ) -> Result<Resp> {
        Self::parse(&self.send("POST", path, &Self::cbor(req)?)?)
    }

    fn post_no_reply<Req: Serialize>(&self, path: &str, req: &Req) -> Result<()> {
        self.send("POST", path, &Self::cbor(req)?).map(drop)
    }

    fn delete(&self, path: &str) -> Result<()> {
        let req = self.with_auth(self.agent.delete(format!("{}{path}", self.server)));
        Self::finish(req.call()).map(drop)
    }

    /// Meldet das Gerät per Challenge/Response an und merkt sich das Token.
    ///
    /// # Errors
    /// [`SyncError::Server`] mit Status 401, wenn das Gerät (noch) nicht registriert ist.
    pub fn login(&mut self, device: DeviceId, keys: &DeviceKeys) -> Result<SessionResponse> {
        self.token = None;
        let challenge: ChallengeResponse =
            self.post("/v1/auth/challenge", &ChallengeRequest { device })?;
        let signature = crypto::sign(&keys.signing, CTX_AUTH, &challenge.nonce);
        let session: SessionResponse = self.post(
            "/v1/auth/verify",
            &VerifyRequest {
                device,
                nonce: challenge.nonce,
                signature,
            },
        )?;
        self.token = Some(session.token.clone());
        Ok(session)
    }
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Angemeldeter Client; verwendet ein gespeichertes, noch gültiges Token weiter.
fn session<S: Store>(replica: &Mutex<Replica<S>>, creds: &Credentials) -> Result<Client> {
    let mut client = Client::new(&creds.server);
    let cached = meta(&*lock(replica)?, META_TOKEN)?;
    if let Some(cached) = cached {
        let text = String::from_utf8_lossy(&cached).into_owned();
        if let Some((expires, token)) = text.split_once(' ') {
            // Eine Minute Puffer, damit das Token nicht mitten im Sync abläuft
            if expires.parse::<u64>().is_ok_and(|e| e > now_s() + 60) {
                client.token = Some(token.to_owned());
                return Ok(client);
            }
        }
    }
    let session = client.login(creds.device, &creds.keys)?;
    let entry = format!("{} {}", now_s() + session.expires_in_s, session.token);
    set_meta(&mut *lock(replica)?, META_TOKEN, Some(entry.as_bytes()))?;
    Ok(client)
}

/// Wie [`session`], meldet sich bei abgelaufenem Token (401) einmal neu an.
fn with_session<S: Store, T>(
    replica: &Mutex<Replica<S>>,
    creds: &Credentials,
    f: impl Fn(&Client) -> Result<T>,
) -> Result<T> {
    let client = session(replica, creds)?;
    match f(&client) {
        Err(SyncError::Server { status: 401, .. }) => {
            set_meta(&mut *lock(replica)?, META_TOKEN, None)?;
            f(&session(replica, creds)?)
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------------------------
// Öffentliche Abläufe

/// # Errors
/// Nur bei Fehlern des lokalen Speichers.
pub fn status<S: Store>(replica: &Mutex<Replica<S>>) -> Result<SyncStatus> {
    let r = lock(replica)?;
    if let Some(creds) = load_credentials(&r)? {
        return Ok(SyncStatus::Connected {
            server: creds.server,
            account: hex(&creds.account.0),
        });
    }
    if let Some(pending) = meta(&r, META_PAIRING)? {
        let code: PairingCode = decode(&pending).map_err(store_err)?;
        return Ok(SyncStatus::Pairing {
            server: code.server.clone(),
            code: code.to_text(),
        });
    }
    Ok(SyncStatus::Off)
}

/// Legt auf dem Server einen neuen Account mit diesem Gerät an.
///
/// # Errors
/// [`SyncError::AlreadyConfigured`], Netzwerk- oder Serverfehler (z. B. Registrierung geschlossen).
pub fn create_account<S: Store>(
    replica: &Mutex<Replica<S>>,
    server: &str,
    device_name: &str,
) -> Result<()> {
    let server = normalize_server(server)?;
    let device = {
        let r = lock(replica)?;
        if load_credentials(&r)?.is_some() {
            return Err(SyncError::AlreadyConfigured);
        }
        r.device()
    };
    let keys = DeviceKeys::generate();
    let account = AccountId(crypto::random());
    let account_key: [u8; 32] = crypto::random();
    let info = DeviceInfo {
        device,
        signing_pub: keys.signing_public(),
        name: device_name.trim().to_owned(),
    };
    let proof = crypto::device_proof(&keys, &info);
    Client::new(&server).post_no_reply(
        "/v1/account",
        &CreateAccount {
            account,
            device: info,
            proof,
        },
    )?;

    let mut r = lock(replica)?;
    save_keys(&mut r, &keys)?;
    save_account(&mut r, account, &account_key)?;
    set_meta(&mut r, META_PAIRING, None)?;
    set_meta(&mut r, META_SERVER, Some(server.as_bytes()))
}

/// Neues Gerät: erzeugt Schlüssel und einen Kopplungscode, den ein bestehendes Gerät bestätigt.
///
/// # Errors
/// [`SyncError::AlreadyConfigured`] oder Fehler des lokalen Speichers.
pub fn start_pairing<S: Store>(
    replica: &Mutex<Replica<S>>,
    server: &str,
    device_name: &str,
) -> Result<String> {
    let server = normalize_server(server)?;
    let mut r = lock(replica)?;
    if load_credentials(&r)?.is_some() {
        return Err(SyncError::AlreadyConfigured);
    }
    let keys = DeviceKeys::generate();
    let info = DeviceInfo {
        device: r.device(),
        signing_pub: keys.signing_public(),
        name: device_name.trim().to_owned(),
    };
    let code = PairingCode {
        server: server.clone(),
        proof: crypto::device_proof(&keys, &info),
        device: info,
        exchange_pub: keys.exchange_public(),
        pairing_id: crypto::random(),
        secret: crypto::random(),
    };
    save_keys(&mut r, &keys)?;
    set_meta(
        &mut r,
        META_PAIRING,
        Some(&encode(&code).map_err(store_err)?),
    )?;
    set_meta(&mut r, META_SERVER, Some(server.as_bytes()))?;
    Ok(code.to_text())
}

/// Bestehendes Gerät: registriert das neue Gerät und übergibt ihm verschlüsselt den Account-Schlüssel.
/// Gibt den Namen des neuen Geräts zurück.
///
/// # Errors
/// Ungültiger Code, Code für einen anderen Server, Netzwerk- oder Serverfehler.
pub fn approve_pairing<S: Store>(replica: &Mutex<Replica<S>>, code: &str) -> Result<String> {
    let code = PairingCode::from_text(code)?;
    crypto::verify(
        &code.device.signing_pub,
        CTX_DEVICE,
        &crypto::device_proof_message(&code.device),
        &code.proof,
    )?;
    let creds = credentials(replica)?;
    if normalize_server(&code.server)? != creds.server {
        return Err(SyncError::InvalidInput(format!(
            "Der Code gehört zu Server {}, dieses Gerät nutzt {}",
            code.server, creds.server
        )));
    }
    if code.device.device == creds.device {
        return Err(SyncError::InvalidInput(
            "Das ist der Code dieses Geräts".into(),
        ));
    }
    let message = crypto::seal_pairing(
        &code,
        &PairingSecret {
            account: creds.account,
            account_key: creds.account_key,
        },
    )?;
    let message = Client::cbor(&message)?;
    let path = format!("/v1/pairing/{}", URL_SAFE_NO_PAD.encode(code.pairing_id));
    with_session(replica, &creds, |client| {
        client.post_no_reply(
            "/v1/devices",
            &RegisterDevice {
                device: code.device.clone(),
                proof: code.proof,
            },
        )?;
        client.send("PUT", &path, &message).map(drop)
    })?;
    Ok(code.device.name)
}

/// Neues Gerät: holt den Account-Schlüssel ab, sobald ein bestehendes Gerät bestätigt hat.
/// `Ok(false)`, solange noch nicht bestätigt wurde.
///
/// # Errors
/// [`SyncError::NotConfigured`], wenn keine Kopplung läuft; Netzwerk- oder Serverfehler.
pub fn finish_pairing<S: Store>(replica: &Mutex<Replica<S>>) -> Result<bool> {
    let (code, keys, device) = {
        let r = lock(replica)?;
        let pending = meta(&r, META_PAIRING)?.ok_or(SyncError::NotConfigured)?;
        let code: PairingCode = decode(&pending).map_err(store_err)?;
        let keys = load_keys(&r)?.ok_or(SyncError::NotConfigured)?;
        (code, keys, r.device())
    };
    let mut client = Client::new(&code.server);
    let session = match client.login(device, &keys) {
        Ok(s) => s,
        // Noch nicht registriert, d. h. noch nicht bestätigt
        Err(SyncError::Server { status: 401, .. }) => return Ok(false),
        Err(e) => return Err(e),
    };
    let path = format!("/v1/pairing/{}", URL_SAFE_NO_PAD.encode(code.pairing_id));
    let raw = match client.get_raw(&path) {
        Ok(raw) => raw,
        Err(SyncError::Server { status: 404, .. }) => return Ok(false),
        Err(e) => return Err(e),
    };
    let message: PairingMessage = Client::parse(&raw)?;
    let secret = crypto::open_pairing(&message, &keys.exchange, &code.secret, &code.pairing_id)?;
    if secret.account != session.account {
        return Err(SyncError::Protocol(
            "Kopplung gehört zu einem anderen Account".into(),
        ));
    }
    {
        let mut r = lock(replica)?;
        save_account(&mut r, secret.account, &secret.account_key)?;
        set_meta(&mut r, META_PAIRING, None)?;
        let entry = format!("{} {}", now_s() + session.expires_in_s, session.token);
        set_meta(&mut r, META_TOKEN, Some(entry.as_bytes()))?;
    }
    // Aufräumen ist optional; das Postfach läuft ohnehin nach 10 Minuten ab
    let _ = client.delete(&path);
    Ok(true)
}

/// Vergisst die Sync-Zugangsdaten auf diesem Gerät. Lokale Daten bleiben erhalten.
///
/// # Errors
/// Nur bei Fehlern des lokalen Speichers.
pub fn disconnect<S: Store>(replica: &Mutex<Replica<S>>) -> Result<()> {
    let mut r = lock(replica)?;
    let spaces = r.store().spaces().map_err(store_err)?;
    for key in [
        META_SERVER,
        META_ACCOUNT,
        META_ACCOUNT_KEY,
        META_SIGNING_KEY,
        META_EXCHANGE_KEY,
        META_PAIRING,
        META_TOKEN,
    ] {
        set_meta(&mut r, key, None)?;
    }
    for space in spaces {
        set_meta(&mut r, &format!("{META_CURSOR_PREFIX}{space}"), None)?;
    }
    Ok(())
}

/// Ein Sync-Durchgang: für jeden Space erst neue Changesets holen, dann eigene hochladen.
///
/// # Errors
/// [`SyncError::NotConfigured`], Netzwerk-, Server- oder Protokollfehler. Bereits angewendete
/// Changesets bleiben bei einem Fehler erhalten; der nächste Durchgang macht dort weiter.
pub fn sync<S: Store>(replica: &Mutex<Replica<S>>) -> Result<SyncReport> {
    let creds = credentials(replica)?;
    with_session(replica, &creds, |client| sync_with(replica, &creds, client))
}

fn sync_with<S: Store>(
    replica: &Mutex<Replica<S>>,
    creds: &Credentials,
    client: &Client,
) -> Result<SyncReport> {
    let devices: DevicesResponse = client.get("/v1/devices")?;
    let signers: BTreeMap<DeviceId, [u8; 32]> = devices
        .devices
        .iter()
        .map(|d| (d.device, d.signing_pub))
        .collect();
    let remote: SpacesResponse = client.get("/v1/spaces")?;
    let local = lock(replica)?.store().spaces().map_err(store_err)?;
    let spaces: BTreeSet<EntityId> = remote.spaces.into_iter().chain(local).collect();

    let mut report = SyncReport::default();
    for space in spaces {
        pull_space(replica, creds, client, &signers, space, &mut report)?;
        push_space(replica, creds, client, space, &mut report)?;
    }
    Ok(report)
}

fn pull_space<S: Store>(
    replica: &Mutex<Replica<S>>,
    creds: &Credentials,
    client: &Client,
    signers: &BTreeMap<DeviceId, [u8; 32]>,
    space: EntityId,
    report: &mut SyncReport,
) -> Result<()> {
    let cursor_key = format!("{META_CURSOR_PREFIX}{space}");
    let mut cursor = meta(&*lock(replica)?, &cursor_key)?
        .and_then(|c| c.try_into().ok())
        .map_or(0, u64::from_be_bytes);
    loop {
        let page: PullResponse = client.get(&format!("/v1/spaces/{space}/pull?after={cursor}"))?;
        let mut r = lock(replica)?;
        for item in &page.items {
            let env = &item.envelope;
            if env.header.device != creds.device {
                let Some(signer) = signers.get(&env.header.device) else {
                    report.rejected += 1;
                    cursor = item.server_seq;
                    continue;
                };
                let key = crypto::space_key(&creds.account_key, space, env.header.key_epoch);
                let cs = crypto::open_envelope(env, &key, signer)?;
                match r.receive(&cs, now_ms()) {
                    Ok(ApplyOutcome::Applied) => report.pulled += 1,
                    Ok(ApplyOutcome::Duplicate) => {}
                    Err(ReceiveError::Gap { expected, got }) => {
                        return Err(SyncError::Protocol(format!(
                            "Lücke vom Server: erwartet seq {expected}, bekommen {got}"
                        )));
                    }
                    Err(ReceiveError::Store(e)) => return Err(store_err(e)),
                }
            }
            cursor = item.server_seq;
        }
        set_meta(&mut r, &cursor_key, Some(&cursor.to_be_bytes()))?;
        if !page.more {
            return Ok(());
        }
    }
}

fn push_space<S: Store>(
    replica: &Mutex<Replica<S>>,
    creds: &Credentials,
    client: &Client,
    space: EntityId,
    report: &mut SyncReport,
) -> Result<()> {
    let remote: VersionVectorResponse = client.get(&format!("/v1/spaces/{space}/vv"))?;
    let on_server = remote.vv.get(&creds.device).copied().unwrap_or(0);
    // Nur eigene Changesets hochladen: fremde kann dieses Gerät nicht in deren Namen signieren.
    let own: Vec<_> = {
        let r = lock(replica)?;
        let mut have = VersionVector::default();
        have.0.insert(creds.device, on_server);
        r.changesets_missing_from(space, &have)
            .map_err(store_err)?
            .into_iter()
            .filter(|cs| cs.id.device == creds.device)
            .collect()
    };
    let key = crypto::space_key(&creds.account_key, space, KEY_EPOCH);
    for batch in own.chunks(PUSH_BATCH) {
        let envelopes = batch
            .iter()
            .map(|cs| crypto::seal_changeset(cs, &key, KEY_EPOCH, &creds.keys.signing))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let resp: PushResponse = client.post(
            &format!("/v1/spaces/{space}/push"),
            &PushRequest { envelopes },
        )?;
        report.pushed += resp.stored;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_adressen() {
        let ok = [
            ("https://sync.example.org/", "https://sync.example.org"),
            ("  http://localhost:8080 ", "http://localhost:8080"),
            ("sync.example.org", "https://sync.example.org"),
            (
                "sync.example.org:8443/doe",
                "https://sync.example.org:8443/doe",
            ),
            ("localhost:8080", "http://localhost:8080"),
            ("127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("192.168.178.20:8080", "http://192.168.178.20:8080"),
            ("10.0.0.5", "http://10.0.0.5"),
            ("nas.local:8080", "http://nas.local:8080"),
            ("NAS.Local", "http://NAS.Local"),
            ("8.8.8.8", "https://8.8.8.8"),
        ];
        for (eingabe, erwartet) in ok {
            assert_eq!(normalize_server(eingabe).unwrap(), erwartet, "{eingabe:?}");
        }
        for falsch in [
            "",
            "   ",
            "ftp://x",
            "http://",
            "https:///pfad",
            "mein server",
        ] {
            assert!(
                matches!(normalize_server(falsch), Err(SyncError::InvalidInput(_))),
                "{falsch:?}"
            );
        }
    }
}
