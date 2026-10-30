//! Sync-Nachrichten und ihr Wire-Format (CBOR). Transport-unabhängig: Server und LAN
//! verwenden dieselben Nachrichten. Siehe Abschnitt 6 der Design-Doku.

use std::collections::BTreeMap;

use doeverything_core::{DeviceId, EntityId};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

/// Für den Server lesbarer Kopf eines Changesets. Wird bei der Verschlüsselung als
/// Associated Data verwendet, damit Pakete nicht unbemerkt umetikettiert werden können.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EnvelopeHeader {
    pub proto: u16,
    pub space: EntityId,
    pub device: DeviceId,
    pub seq: u64,
    pub key_epoch: u32,
}

/// Verschlüsseltes, signiertes Changeset.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub header: EnvelopeHeader,
    #[serde(with = "serde_bytes")]
    pub nonce: [u8; 24],
    #[serde(with = "serde_bytes")]
    pub ciphertext: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub signature: [u8; 64],
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Msg {
    Hello {
        proto: u16,
        peer: DeviceId,
        spaces: Vec<EntityId>,
    },
    Have {
        space: EntityId,
        vv: BTreeMap<DeviceId, u64>,
    },
    Changesets {
        space: EntityId,
        items: Vec<Envelope>,
    },
    Ack {
        space: EntityId,
        vv: BTreeMap<DeviceId, u64>,
    },
    Snapshot {
        space: EntityId,
        covers: BTreeMap<DeviceId, u64>,
        blob_hash: [u8; 32],
    },
    Error {
        code: u16,
        msg: String,
    },
}

#[derive(Debug)]
pub enum CodecError {
    Encode(String),
    Decode(String),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(e) => write!(f, "CBOR-Kodierung fehlgeschlagen: {e}"),
            Self::Decode(e) => write!(f, "CBOR-Dekodierung fehlgeschlagen: {e}"),
        }
    }
}

impl std::error::Error for CodecError {}

/// # Errors
/// Wenn `value` nicht serialisierbar ist.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).map_err(|e| CodecError::Encode(e.to_string()))?;
    Ok(out)
}

/// # Errors
/// Wenn `bytes` kein gültiges CBOR für `T` ist.
pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, CodecError> {
    ciborium::from_reader(bytes).map_err(|e| CodecError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use doeverything_core::{DeviceId, EntityId, MemoryStore, Replica, SysField};

    use super::*;

    #[test]
    fn msg_roundtrip() {
        let env = Envelope {
            header: EnvelopeHeader {
                proto: PROTOCOL_VERSION,
                space: EntityId::new(),
                device: DeviceId::new(),
                seq: 42,
                key_epoch: 0,
            },
            nonce: [7; 24],
            ciphertext: vec![1, 2, 3],
            signature: [9; 64],
        };
        let msg = Msg::Changesets {
            space: env.header.space,
            items: vec![env],
        };
        assert_eq!(decode::<Msg>(&encode(&msg).unwrap()).unwrap(), msg);
    }

    #[test]
    fn changeset_roundtrip() {
        let mut r = Replica::open(MemoryStore::new()).unwrap();
        let space = r.create_space("Privat", 1000).unwrap();
        r.transact(space, 1000, |tx| tx.set(space, SysField::Name, "Umbenannt"))
            .unwrap();
        let alle = r.changesets_missing_from(space, &doeverything_core::VersionVector::default());
        for cs in alle.unwrap() {
            let bytes = encode(&cs).unwrap();
            assert_eq!(decode::<doeverything_core::Changeset>(&bytes).unwrap(), cs);
        }
    }
}
