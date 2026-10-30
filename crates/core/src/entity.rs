use std::collections::BTreeMap;

use crate::ids::EntityId;
use crate::model::{FieldKey, Kind, SysField, Versioned};
use crate::value::Value;

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

/// Sortiert nach `_order` (fehlend → ans Ende), dann nach ID.
pub fn sort_children(children: &mut [(EntityId, Entity)]) {
    children.sort_by(|(a_id, a), (b_id, b)| {
        let a_order = a.get(SysField::Order).and_then(Value::as_text);
        let b_order = b.get(SysField::Order).and_then(Value::as_text);
        (a_order.is_none(), a_order, a_id).cmp(&(b_order.is_none(), b_order, b_id))
    });
}
