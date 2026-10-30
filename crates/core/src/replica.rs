use std::collections::BTreeSet;
use std::fmt;

use crate::entity::Entity;
use crate::hlc::Clock;
use crate::ids::{DeviceId, EntityId};
use crate::model::{FieldKey, Kind, SysField};
use crate::op::{Changeset, ChangesetId, Op};
use crate::store::Store;
use crate::value::Value;
use crate::version_vector::VersionVector;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    /// Changeset war schon bekannt; nichts geändert.
    Duplicate,
}

#[derive(Debug)]
pub enum ReceiveError<E> {
    /// Changesets eines Geräts müssen lückenlos in `seq`-Reihenfolge ankommen.
    Gap {
        expected: u64,
        got: u64,
    },
    Store(E),
}

impl<E: fmt::Display> fmt::Display for ReceiveError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gap { expected, got } => {
                write!(f, "Lücke im Log: erwartet seq {expected}, bekommen {got}")
            }
            Self::Store(e) => write!(f, "Speicherfehler: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ReceiveError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Gap { .. } => None,
            Self::Store(e) => Some(e),
        }
    }
}

/// Ein Gerät: Uhr plus Speicher. Erzeugt lokale Changesets und nimmt fremde entgegen.
#[derive(Debug)]
pub struct Replica<S> {
    clock: Clock,
    store: S,
    drifted_devices: BTreeSet<DeviceId>,
}

impl<S: Store> Replica<S> {
    /// # Errors
    /// Wenn der Speicher nicht gelesen werden kann.
    pub fn open(store: S) -> Result<Self, S::Error> {
        let mut clock = Clock::new(store.device_id());
        if let Some(last) = store.max_hlc()? {
            clock.resume(last);
        }
        Ok(Self {
            clock,
            store,
            drifted_devices: BTreeSet::new(),
        })
    }

    pub fn device(&self) -> DeviceId {
        self.clock.device()
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn into_store(self) -> S {
        self.store
    }

    /// Geräte, deren Zeitstempel unplausibel weit in der Zukunft lagen.
    pub fn drifted_devices(&self) -> &BTreeSet<DeviceId> {
        &self.drifted_devices
    }

    /// # Errors
    /// Wenn der Speicher nicht geschrieben werden kann.
    pub fn create_space(&mut self, name: &str, now_ms: u64) -> Result<EntityId, S::Error> {
        let id = EntityId::new();
        self.transact(id, now_ms, |tx| {
            tx.create_with_id(id, Kind::Space, None);
            tx.set(id, SysField::Name, name);
        })?;
        Ok(id)
    }

    /// Führt lokale Änderungen als ein Changeset aus. Ohne Operationen entsteht kein Changeset.
    ///
    /// # Errors
    /// Wenn der Speicher nicht geschrieben werden kann.
    pub fn transact<R>(
        &mut self,
        space: EntityId,
        now_ms: u64,
        f: impl FnOnce(&mut Tx<'_>) -> R,
    ) -> Result<R, S::Error> {
        let mut tx = Tx {
            clock: &mut self.clock,
            now_ms,
            ops: Vec::new(),
        };
        let result = f(&mut tx);
        let ops = tx.ops;
        if !ops.is_empty() {
            let device = self.clock.device();
            let seq = self.store.version_vector(space)?.next_seq(device);
            let cs = Changeset {
                id: ChangesetId { device, seq },
                space,
                ops,
            };
            self.store.apply_changeset(&cs)?;
        }
        Ok(result)
    }

    /// Wendet ein empfangenes Changeset an.
    ///
    /// # Errors
    /// [`ReceiveError::Gap`], wenn vorherige Changesets desselben Geräts fehlen.
    pub fn receive(
        &mut self,
        cs: &Changeset,
        now_ms: u64,
    ) -> Result<ApplyOutcome, ReceiveError<S::Error>> {
        let vv = self
            .store
            .version_vector(cs.space)
            .map_err(ReceiveError::Store)?;
        if vv.contains(cs.id) {
            return Ok(ApplyOutcome::Duplicate);
        }
        let expected = vv.next_seq(cs.id.device);
        if cs.id.seq != expected {
            return Err(ReceiveError::Gap {
                expected,
                got: cs.id.seq,
            });
        }
        self.store
            .apply_changeset(cs)
            .map_err(ReceiveError::Store)?;
        for op in &cs.ops {
            if self.clock.observe(op.hlc(), now_ms).is_err() {
                self.drifted_devices.insert(cs.id.device);
            }
        }
        Ok(ApplyOutcome::Applied)
    }

    /// # Errors
    /// Wenn der Speicher nicht gelesen werden kann.
    pub fn version_vector(&self, space: EntityId) -> Result<VersionVector, S::Error> {
        self.store.version_vector(space)
    }

    /// # Errors
    /// Wenn der Speicher nicht gelesen werden kann.
    pub fn changesets_missing_from(
        &self,
        space: EntityId,
        vv: &VersionVector,
    ) -> Result<Vec<Changeset>, S::Error> {
        self.store.changesets_missing_from(space, vv)
    }

    /// # Errors
    /// Wenn der Speicher nicht gelesen werden kann.
    pub fn entity(&self, id: EntityId) -> Result<Option<Entity>, S::Error> {
        self.store.entity(id)
    }

    /// # Errors
    /// Wenn der Speicher nicht gelesen werden kann.
    pub fn children(
        &self,
        parent: EntityId,
        kind: Kind,
    ) -> Result<Vec<(EntityId, Entity)>, S::Error> {
        self.store.children(parent, kind)
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
