use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// ID einer Entity (Space, Collection, Field, View, Record). `UUIDv7`, zeitlich sortierbar.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(pub [u8; 16]);

impl EntityId {
    pub fn new() -> Self {
        Self(*Uuid::now_v7().as_bytes())
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

/// Textform wie eine UUID (`0192f0c4-…`), z. B. für die Übergabe an die Oberfläche.
impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Uuid::from_bytes(self.0).fmt(f)
    }
}

impl std::str::FromStr for EntityId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(*Uuid::parse_str(s)?.as_bytes()))
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({})", Uuid::from_bytes(self.0))
    }
}

/// ID eines Geräts. Zufällig (`UUIDv4`), dient auch als Tie-Breaker in der HLC.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub [u8; 16]);

impl DeviceId {
    pub fn new() -> Self {
        Self(*Uuid::new_v4().as_bytes())
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", Uuid::from_bytes(self.0))
    }
}
