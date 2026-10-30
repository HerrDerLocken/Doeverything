use std::collections::BTreeMap;
use std::convert::Infallible;

use crate::entity::{Entity, sort_children};
use crate::hlc::Hlc;
use crate::ids::{DeviceId, EntityId};
use crate::model::{Kind, Versioned};
use crate::op::{Changeset, Op};
use crate::version_vector::VersionVector;

/// Persistenz eines Geräts. Die Konfliktsemantik ist in der Spezifikation festgelegt und wird
/// für jede Implementierung mit [`crate::konformitaets_tests`] geprüft.
#[allow(clippy::missing_errors_doc)] // Fehler stammen ausschließlich aus dem Speicher-Backend
pub trait Store {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Dauerhafte ID dieses Geräts; wird beim ersten Öffnen erzeugt.
    fn device_id(&self) -> DeviceId;

    /// Größter jemals gespeicherte Zeitstempel, damit die Uhr nach einem Neustart monoton bleibt.
    fn max_hlc(&self) -> Result<Option<Hlc>, Self::Error>;

    fn version_vector(&self, space: EntityId) -> Result<VersionVector, Self::Error>;

    /// Speichert das Changeset und wendet seine Operationen an, atomar.
    /// Der Aufrufer stellt sicher, dass `cs.id.seq` die nächste erwartete `seq` ist.
    fn apply_changeset(&mut self, cs: &Changeset) -> Result<(), Self::Error>;

    /// Alle Changesets, die laut `vv` fehlen, sortiert nach (Gerät, `seq`).
    fn changesets_missing_from(
        &self,
        space: EntityId,
        vv: &VersionVector,
    ) -> Result<Vec<Changeset>, Self::Error>;

    /// Angelegte Entity, auch wenn gelöscht.
    fn entity(&self, id: EntityId) -> Result<Option<Entity>, Self::Error>;

    /// Sichtbare Kinder einer Art, sortiert wie [`sort_children`].
    fn children(
        &self,
        parent: EntityId,
        kind: Kind,
    ) -> Result<Vec<(EntityId, Entity)>, Self::Error>;

    /// Alle Entities eines Space inklusive gelöschter und noch nicht angelegter (Export, Vergleiche).
    fn entities(&self, space: EntityId) -> Result<BTreeMap<EntityId, Entity>, Self::Error>;

    /// IDs aller sichtbaren Spaces, aufsteigend (= nach Erstellungszeit, da `UUIDv7`).
    fn spaces(&self) -> Result<Vec<EntityId>, Self::Error>;
}

/// Referenzimplementierung im Arbeitsspeicher.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    device: DeviceId,
    spaces: BTreeMap<EntityId, SpaceData>,
}

#[derive(Debug, Clone, Default)]
struct SpaceData {
    vv: VersionVector,
    log: Vec<Changeset>,
    entities: BTreeMap<EntityId, Entity>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            device: DeviceId::new(),
            spaces: BTreeMap::new(),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Store for MemoryStore {
    type Error = Infallible;

    fn device_id(&self) -> DeviceId {
        self.device
    }

    fn max_hlc(&self) -> Result<Option<Hlc>, Infallible> {
        Ok(self
            .spaces
            .values()
            .flat_map(|s| &s.log)
            .flat_map(|cs| &cs.ops)
            .map(Op::hlc)
            .max())
    }

    fn version_vector(&self, space: EntityId) -> Result<VersionVector, Infallible> {
        Ok(self
            .spaces
            .get(&space)
            .map(|s| s.vv.clone())
            .unwrap_or_default())
    }

    fn apply_changeset(&mut self, cs: &Changeset) -> Result<(), Infallible> {
        let data = self.spaces.entry(cs.space).or_default();
        data.vv.advance(cs.id);
        for op in &cs.ops {
            apply_op(&mut data.entities, op);
        }
        data.log.push(cs.clone());
        Ok(())
    }

    fn changesets_missing_from(
        &self,
        space: EntityId,
        vv: &VersionVector,
    ) -> Result<Vec<Changeset>, Infallible> {
        let mut out: Vec<_> = self
            .spaces
            .get(&space)
            .into_iter()
            .flat_map(|s| &s.log)
            .filter(|cs| !vv.contains(cs.id))
            .cloned()
            .collect();
        out.sort_by_key(|cs| cs.id);
        Ok(out)
    }

    fn entity(&self, id: EntityId) -> Result<Option<Entity>, Infallible> {
        Ok(self
            .spaces
            .values()
            .find_map(|s| s.entities.get(&id))
            .filter(|e| e.is_created())
            .cloned())
    }

    fn children(
        &self,
        parent: EntityId,
        kind: Kind,
    ) -> Result<Vec<(EntityId, Entity)>, Infallible> {
        let mut out: Vec<_> = self
            .spaces
            .values()
            .flat_map(|s| &s.entities)
            .filter(|(_, e)| e.is_visible() && e.kind == Some(kind) && e.parent == Some(parent))
            .map(|(id, e)| (*id, e.clone()))
            .collect();
        sort_children(&mut out);
        Ok(out)
    }

    fn spaces(&self) -> Result<Vec<EntityId>, Infallible> {
        Ok(self
            .spaces
            .iter()
            .filter(|(id, s)| {
                s.entities
                    .get(id)
                    .is_some_and(|e| e.is_visible() && e.kind == Some(Kind::Space))
            })
            .map(|(id, _)| *id)
            .collect())
    }

    fn entities(&self, space: EntityId) -> Result<BTreeMap<EntityId, Entity>, Infallible> {
        Ok(self
            .spaces
            .get(&space)
            .map(|s| s.entities.clone())
            .unwrap_or_default())
    }
}

fn apply_op(entities: &mut BTreeMap<EntityId, Entity>, op: &Op) {
    match op {
        Op::Create {
            id, kind, parent, ..
        } => {
            let entity = entities.entry(*id).or_default();
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
            let entity = entities.entry(*id).or_default();
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
