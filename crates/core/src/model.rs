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

/// Schlüssel eines Feldwerts. Benutzerfelder werden über die ID ihrer Field-Entity
/// adressiert, sodass Umbenennen keine Daten berührt.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum FieldKey {
    System(SysField),
    User(EntityId),
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
