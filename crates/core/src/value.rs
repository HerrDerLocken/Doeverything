use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ids::EntityId;

/// Feldwert. Geldbeträge und Dezimalzahlen sind bewusst ganzzahlig kodiert (kein `f64`).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    /// `1.25` → `{ mantissa: 125, scale: 2 }`
    Decimal {
        mantissa: i128,
        scale: u8,
    },
    /// `12,34 €` → `{ minor: 1234, currency: *b"EUR" }`
    Money {
        minor: i64,
        currency: [u8; 3],
    },
    Text(String),
    /// Kalendertag ohne Zeitzone (Fälligkeit, Geburtstag).
    Date {
        y: i16,
        m: u8,
        d: u8,
    },
    /// Zeitpunkt in UTC.
    Instant {
        unix_ms: i64,
    },
    Duration {
        ms: i64,
    },
    /// ID einer Option aus der `_config` des Feldes.
    Enum(EntityId),
    MultiEnum(BTreeSet<EntityId>),
    /// Verweis auf einen anderen Record.
    Ref(EntityId),
    /// Anhang, Inhalt wird separat gespeichert.
    Blob {
        hash: [u8; 32],
        size: u64,
        mime: String,
    },
    /// Nur für `_config` von Feldern und Ansichten.
    Json(serde_json::Value),
}

impl Value {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Self::Int(i)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Self::Text(s.to_owned())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}
