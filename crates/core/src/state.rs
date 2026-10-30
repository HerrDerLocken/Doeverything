use std::collections::BTreeMap;

use crate::ids::EntityId;
use crate::model::{FieldKey, Kind, SysField, Versioned};
use crate::op::{Changeset, Op};
use crate::value::Value;
use crate::version_vector::VersionVector;

/// Materialisierter Zustand einer Entity.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct Entity {
    /// `None`, solange nur `Set`-Operationen, aber noch kein `Create` angekommen sind.
    pub kind: Option<Kind>,
    pub parent: Option<EntityId>,
    pub fields: BTreeMap<FieldKey, Versioned<Value>>,
}

impl Entity {
    pub fn get(&self, key: impl Into<FieldKey>) -> Option<&Value> {
        self.fields.get(&key.into()).map(|v| &v.value)
    }

    pub fn is_created(&self) -> bool {
        self.kind.is_some()
    }

    pub fn is_deleted(&self) -> bool {
        self.get(SysField::Deleted).and_then(Value::as_bool) == Some(true)
    }

    /// Angelegt und nicht gelöscht.
    pub fn is_visible(&self) -> bool {
        self.is_created() && !self.is_deleted()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    /// Changeset war schon bekannt; nichts geändert.
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyError {
    /// Changesets eines Geräts müssen lückenlos in `seq`-Reihenfolge ankommen.
    Gap { expected: u64, got: u64 },
}

/// Materialisierter Zustand aller Spaces plus Version Vectors.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct State {
    entities: BTreeMap<EntityId, Entity>,
    version_vectors: BTreeMap<EntityId, VersionVector>,
}

impl State {
    /// # Errors
    /// [`ApplyError::Gap`], wenn vorherige Changesets desselben Geräts fehlen.
    pub fn apply_changeset(&mut self, cs: &Changeset) -> Result<ApplyOutcome, ApplyError> {
        let vv = self.version_vectors.entry(cs.space).or_default();
        if vv.contains(cs.id) {
            return Ok(ApplyOutcome::Duplicate);
        }
        let expected = vv.next_seq(cs.id.device);
        if cs.id.seq != expected {
            return Err(ApplyError::Gap {
                expected,
                got: cs.id.seq,
            });
        }
        vv.advance(cs.id);
        for op in &cs.ops {
            self.apply_op(op);
        }
        Ok(ApplyOutcome::Applied)
    }

    fn apply_op(&mut self, op: &Op) {
        match op {
            Op::Create {
                id, kind, parent, ..
            } => {
                let entity = self.entities.entry(*id).or_default();
                if entity.kind.is_none() {
                    entity.kind = Some(*kind);
                    entity.parent = *parent;
                }
            }
            Op::Set {
                id,
                key,
                value,
                hlc,
            } => {
                let entity = self.entities.entry(*id).or_default();
                match entity.fields.get(key) {
                    Some(existing) if existing.hlc >= *hlc => {}
                    _ => {
                        entity.fields.insert(
                            *key,
                            Versioned {
                                value: value.clone(),
                                hlc: *hlc,
                            },
                        );
                    }
                }
            }
        }
    }

    pub fn version_vector(&self, space: EntityId) -> VersionVector {
        self.version_vectors
            .get(&space)
            .cloned()
            .unwrap_or_default()
    }

    /// Angelegte Entity, auch wenn gelöscht.
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(&id).filter(|e| e.is_created())
    }

    /// Sichtbare Kinder einer Art, sortiert nach `_order` (fehlend → ans Ende), dann nach ID.
    pub fn children(&self, parent: EntityId, kind: Kind) -> Vec<(EntityId, &Entity)> {
        let mut out: Vec<_> = self
            .entities
            .iter()
            .filter(|(_, e)| e.is_visible() && e.kind == Some(kind) && e.parent == Some(parent))
            .map(|(id, e)| (*id, e))
            .collect();
        out.sort_by(|(a_id, a), (b_id, b)| {
            let a_order = a.get(SysField::Order).and_then(Value::as_text);
            let b_order = b.get(SysField::Order).and_then(Value::as_text);
            (a_order.is_none(), a_order, a_id).cmp(&(b_order.is_none(), b_order, b_id))
        });
        out
    }
}
