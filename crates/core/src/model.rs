use serde::{Deserialize, Serialize};

use crate::hlc::Hlc;
use crate::ids::EntityId;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Kind {
    Space,
    Collection,
    Field,
    View,
    Record,
}

impl Kind {
    /// Stabiler Code für die Speicherung. Werte dürfen sich nie ändern.
    pub fn code(self) -> u8 {
        match self {
            Self::Space => 1,
            Self::Collection => 2,
            Self::Field => 3,
            Self::View => 4,
            Self::Record => 5,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Space,
            2 => Self::Collection,
            3 => Self::Field,
            4 => Self::View,
            5 => Self::Record,
            _ => return None,
        })
    }
}

/// Reservierte Felder, die für alle Entity-Arten gelten können.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum SysField {
    /// Tombstone (`Bool`). Entity ist ausgeblendet, solange `true`.
    Deleted,
    /// Fraktionaler Index für manuelle Sortierung (`Text`), siehe [`crate::order`].
    Order,
    /// Name von Space, Collection, Field oder View (`Text`).
    Name,
    /// Modul-Vorlage einer Collection, z. B. `shopping@1` (`Text`).
    Template,
    /// Feldtyp einer Field-Entity, z. B. `money` (`Text`).
    Type,
    /// Typ-Optionen bzw. Ansichts-Konfiguration (`Json`).
    Config,
}

impl SysField {
    /// Stabiler Code für die Speicherung. Werte dürfen sich nie ändern.
    pub fn code(self) -> u8 {
        match self {
            Self::Deleted => 1,
            Self::Order => 2,
            Self::Name => 3,
            Self::Template => 4,
            Self::Type => 5,
            Self::Config => 6,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Deleted,
            2 => Self::Order,
            3 => Self::Name,
            4 => Self::Template,
            5 => Self::Type,
            6 => Self::Config,
            _ => return None,
        })
    }
}

/// Schlüssel eines Feldwerts. Benutzerfelder werden über die ID ihrer Field-Entity
/// adressiert, sodass Umbenennen keine Daten berührt.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum FieldKey {
    System(SysField),
    User(EntityId),
}

impl FieldKey {
    /// `[0, code]` für Systemfelder, `[1, id…]` für Benutzerfelder.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::System(f) => vec![0, f.code()],
            Self::User(id) => {
                let mut out = Vec::with_capacity(17);
                out.push(1);
                out.extend_from_slice(&id.0);
                out
            }
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [0, code] => SysField::from_code(*code).map(Self::System),
            [1, rest @ ..] => Some(Self::User(EntityId(rest.try_into().ok()?))),
            _ => None,
        }
    }
}

impl From<SysField> for FieldKey {
    fn from(f: SysField) -> Self {
        Self::System(f)
    }
}

impl From<EntityId> for FieldKey {
    fn from(id: EntityId) -> Self {
        Self::User(id)
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Versioned<T> {
    pub value: T,
    pub hlc: Hlc,
}
