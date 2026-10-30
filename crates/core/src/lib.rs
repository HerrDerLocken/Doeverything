//! Datenmodell, Konfliktlösung und Replikationslogik von Doeverything.
//!
//! Siehe `docs/design/datenmodell-und-sync.md` für die Spezifikation.

pub mod entity;
pub mod hlc;
pub mod ids;
pub mod model;
pub mod op;
pub mod order;
pub mod replica;
pub mod store;
pub mod templates;
#[doc(hidden)]
pub mod testsuite;
pub mod value;
pub mod version_vector;

pub use entity::Entity;
pub use hlc::{Clock, Hlc};
pub use ids::{DeviceId, EntityId};
pub use model::{FieldKey, Kind, SysField, Versioned};
pub use op::{Changeset, ChangesetId, Op};
pub use replica::{ApplyOutcome, ReceiveError, Replica};
pub use store::{MemoryStore, Store};
pub use value::Value;
pub use version_vector::VersionVector;
