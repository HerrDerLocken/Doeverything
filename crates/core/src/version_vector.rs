use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;
use crate::op::ChangesetId;

/// Pro Gerät die höchste lückenlos vorhandene `seq` (eines Space).
#[derive(Clone, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
pub struct VersionVector(pub BTreeMap<DeviceId, u64>);

impl VersionVector {
    pub fn get(&self, device: DeviceId) -> u64 {
        self.0.get(&device).copied().unwrap_or(0)
    }

    pub fn contains(&self, id: ChangesetId) -> bool {
        id.seq <= self.get(id.device)
    }

    /// Nächste erwartete `seq` dieses Geräts.
    pub fn next_seq(&self, device: DeviceId) -> u64 {
        self.get(device) + 1
    }

    pub fn advance(&mut self, id: ChangesetId) {
        let entry = self.0.entry(id.device).or_insert(0);
        *entry = (*entry).max(id.seq);
    }

    /// `true`, wenn `self` alles enthält, was `other` enthält.
    pub fn dominates(&self, other: &Self) -> bool {
        other.0.iter().all(|(d, s)| self.get(*d) >= *s)
    }
}
