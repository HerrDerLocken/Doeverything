use std::collections::{BTreeMap, BTreeSet};

use crate::hlc::Clock;
use crate::ids::{DeviceId, EntityId};
use crate::model::{FieldKey, Kind, SysField};
use crate::op::{Changeset, ChangesetId, Op};
use crate::state::{ApplyError, ApplyOutcome, State};
use crate::value::Value;
use crate::version_vector::VersionVector;

/// Ein Gerät: Uhr, Zustand und Log aller bekannten Changesets (In-Memory-Referenzimplementierung).
#[derive(Debug, Clone)]
pub struct Replica {
    clock: Clock,
    state: State,
    log: BTreeMap<EntityId, Vec<Changeset>>,
    drifted_devices: BTreeSet<DeviceId>,
}

impl Replica {
    pub fn new(device: DeviceId) -> Self {
        Self {
            clock: Clock::new(device),
            state: State::default(),
            log: BTreeMap::new(),
            drifted_devices: BTreeSet::new(),
        }
    }

    pub fn device(&self) -> DeviceId {
        self.clock.device()
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Geräte, deren Zeitstempel unplausibel weit in der Zukunft lagen.
    pub fn drifted_devices(&self) -> &BTreeSet<DeviceId> {
        &self.drifted_devices
    }

    pub fn create_space(&mut self, name: &str, now_ms: u64) -> EntityId {
        let id = EntityId::new();
        self.transact(id, now_ms, |tx| {
            tx.create_with_id(id, Kind::Space, None);
            tx.set(id, SysField::Name, name);
        });
        id
    }

    /// Führt lokale Änderungen als ein Changeset aus. Ohne Operationen entsteht kein Changeset.
    ///
    /// # Panics
    /// Nie im Normalbetrieb: die lokale `seq` ist per Konstruktion die nächste.
    pub fn transact<R>(
        &mut self,
        space: EntityId,
        now_ms: u64,
        f: impl FnOnce(&mut Tx<'_>) -> R,
    ) -> R {
        let mut tx = Tx {
            clock: &mut self.clock,
            now_ms,
            ops: Vec::new(),
        };
        let result = f(&mut tx);
        let ops = tx.ops;
        if !ops.is_empty() {
            let device = self.clock.device();
            let seq = self.state.version_vector(space).next_seq(device);
            let cs = Changeset {
                id: ChangesetId { device, seq },
                space,
                ops,
            };
            self.state
                .apply_changeset(&cs)
                .expect("lokale seq ist immer die nächste");
            self.log.entry(space).or_default().push(cs);
        }
        result
    }

    /// Wendet ein empfangenes Changeset an.
    ///
    /// # Errors
    /// [`ApplyError::Gap`], wenn vorherige Changesets desselben Geräts fehlen.
    pub fn receive(&mut self, cs: Changeset, now_ms: u64) -> Result<ApplyOutcome, ApplyError> {
        let outcome = self.state.apply_changeset(&cs)?;
        if outcome == ApplyOutcome::Applied {
            for op in &cs.ops {
                if self.clock.observe(op.hlc(), now_ms).is_err() {
                    self.drifted_devices.insert(cs.id.device);
                }
            }
            self.log.entry(cs.space).or_default().push(cs);
        }
        Ok(outcome)
    }

    pub fn version_vector(&self, space: EntityId) -> VersionVector {
        self.state.version_vector(space)
    }

    /// Alle Changesets, die laut `vv` beim Gegenüber fehlen, pro Gerät in `seq`-Reihenfolge.
    pub fn changesets_missing_from(&self, space: EntityId, vv: &VersionVector) -> Vec<Changeset> {
        let mut out: Vec<_> = self
            .log
            .get(&space)
            .into_iter()
            .flatten()
            .filter(|cs| !vv.contains(cs.id))
            .cloned()
            .collect();
        out.sort_by_key(|cs| cs.id);
        out
    }
}

/// Sammelt die Operationen einer Transaktion; jede Operation bekommt einen eigenen Zeitstempel.
pub struct Tx<'a> {
    clock: &'a mut Clock,
    now_ms: u64,
    ops: Vec<Op>,
}

impl Tx<'_> {
    pub fn create(&mut self, kind: Kind, parent: Option<EntityId>) -> EntityId {
        let id = EntityId::new();
        self.create_with_id(id, kind, parent);
        id
    }

    pub fn create_with_id(&mut self, id: EntityId, kind: Kind, parent: Option<EntityId>) {
        let hlc = self.clock.tick(self.now_ms);
        self.ops.push(Op::Create {
            id,
            kind,
            parent,
            hlc,
        });
    }

    pub fn set(&mut self, id: EntityId, key: impl Into<FieldKey>, value: impl Into<Value>) {
        let hlc = self.clock.tick(self.now_ms);
        self.ops.push(Op::Set {
            id,
            key: key.into(),
            value: value.into(),
            hlc,
        });
    }

    pub fn delete(&mut self, id: EntityId) {
        self.set(id, SysField::Deleted, true);
    }

    pub fn restore(&mut self, id: EntityId) {
        self.set(id, SysField::Deleted, false);
    }
}
