use serde::{Deserialize, Serialize};

use crate::hlc::Hlc;
use crate::ids::{DeviceId, EntityId};
use crate::model::{FieldKey, Kind};
use crate::value::Value;

/// Atomare Änderung. Anwenden ist idempotent und kommutativ.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Op {
    Create {
        id: EntityId,
        kind: Kind,
        parent: Option<EntityId>,
        hlc: Hlc,
    },
    Set {
        id: EntityId,
        key: FieldKey,
        value: Value,
        hlc: Hlc,
    },
}

impl Op {
    pub fn hlc(&self) -> Hlc {
        match self {
            Self::Create { hlc, .. } | Self::Set { hlc, .. } => *hlc,
        }
    }
}

/// `seq` ist pro Gerät und Space lückenlos ab 1.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct ChangesetId {
    pub device: DeviceId,
    pub seq: u64,
}

/// Operationen einer lokalen Transaktion; Einheit für Speicherung, Verschlüsselung und Sync.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Changeset {
    pub id: ChangesetId,
    pub space: EntityId,
    pub ops: Vec<Op>,
}
