//! Datenmodell, Konfliktlösung und Replikationslogik von Doeverything.
//!
//! Siehe `docs/design/datenmodell-und-sync.md` für die Spezifikation.

pub mod hlc;
pub mod ids;
pub mod model;
pub mod op;
pub mod order;
pub mod replica;
pub mod state;
pub mod value;
pub mod version_vector;

pub use hlc::{Clock, Hlc};
pub use ids::{DeviceId, EntityId};
pub use model::{FieldKey, Kind, SysField, Versioned};
pub use op::{Changeset, ChangesetId, Op};
pub use replica::Replica;
pub use state::{ApplyOutcome, Entity, State};
pub use value::Value;
pub use version_vector::VersionVector;
