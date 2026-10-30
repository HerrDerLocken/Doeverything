//! Anfragen und Antworten der Server-HTTP-API (`/v1/...`), kodiert als CBOR
//! (`Content-Type: application/cbor`). Siehe Abschnitt 6.3 der Design-Doku.

use std::collections::BTreeMap;
use std::fmt;

use doeverything_core::{DeviceId, EntityId};
use serde::{Deserialize, Serialize};

use crate::Envelope;

pub const CONTENT_TYPE: &str = "application/cbor";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccountId(#[serde(with = "serde_bytes")] pub [u8; 16]);

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccountId(")?;
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        write!(f, ")")
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device: DeviceId,
    #[serde(with = "serde_bytes")]
    pub signing_pub: [u8; 32],
    pub name: String,
}

/// `POST /v1/account` – legt einen Account mit dem ersten Gerät an.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CreateAccount {
    pub account: AccountId,
    pub device: DeviceInfo,
    /// `crypto::device_proof` des Geräts.
    #[serde(with = "serde_bytes")]
    pub proof: [u8; 64],
}

/// `POST /v1/devices` (angemeldet) – fügt dem eigenen Account ein Gerät hinzu.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RegisterDevice {
    pub device: DeviceInfo,
    #[serde(with = "serde_bytes")]
    pub proof: [u8; 64],
}

/// `POST /v1/auth/challenge`
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ChallengeRequest {
    pub device: DeviceId,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ChallengeResponse {
    #[serde(with = "serde_bytes")]
    pub nonce: [u8; 32],
}

/// `POST /v1/auth/verify` – Nonce signiert mit `crypto::CTX_AUTH`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct VerifyRequest {
    pub device: DeviceId,
    #[serde(with = "serde_bytes")]
    pub nonce: [u8; 32],
    #[serde(with = "serde_bytes")]
    pub signature: [u8; 64],
}

/// Token für `Authorization: Bearer …`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub token: String,
    pub account: AccountId,
    pub expires_in_s: u64,
}

/// `GET /v1/devices`
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DevicesResponse {
    pub devices: Vec<DeviceInfo>,
}

/// `GET /v1/spaces`
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SpacesResponse {
    pub spaces: Vec<EntityId>,
}

/// `GET /v1/spaces/{id}/vv` – was der Server pro Gerät hat.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct VersionVectorResponse {
    pub vv: BTreeMap<DeviceId, u64>,
}

/// `POST /v1/spaces/{id}/push`
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PushRequest {
    pub envelopes: Vec<Envelope>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PushResponse {
    pub stored: u32,
    pub duplicates: u32,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PulledEnvelope {
    pub server_seq: u64,
    pub envelope: Envelope,
}

/// `GET /v1/spaces/{id}/pull?after=N&limit=M`
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PullResponse {
    pub items: Vec<PulledEnvelope>,
    /// Weitere Einträge vorhanden, mit `after = letzter server_seq` erneut abfragen.
    pub more: bool,
}

/// Fehlerantwort aller Endpunkte.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub message: String,
}
