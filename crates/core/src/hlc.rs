use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;

/// Empfangene Zeitstempel, die weiter als das in der Zukunft liegen, werden nicht übernommen.
pub const MAX_DRIFT_MS: u64 = 24 * 60 * 60 * 1000;

/// Hybrid Logical Clock. Die abgeleitete Ordnung ist lexikografisch über
/// (`wall_ms`, `counter`, `device`) und damit total.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u16,
    pub device: DeviceId,
}

impl Hlc {
    pub const ENCODED_LEN: usize = 26;

    /// Big-Endian-Kodierung, deren Byte-Vergleich der `Ord`-Ordnung entspricht (für SQLite).
    pub fn to_bytes(&self) -> [u8; Self::ENCODED_LEN] {
        let mut out = [0u8; Self::ENCODED_LEN];
        out[..8].copy_from_slice(&self.wall_ms.to_be_bytes());
        out[8..10].copy_from_slice(&self.counter.to_be_bytes());
        out[10..].copy_from_slice(&self.device.0);
        out
    }

    pub fn from_bytes(bytes: &[u8; Self::ENCODED_LEN]) -> Self {
        let mut wall = [0u8; 8];
        wall.copy_from_slice(&bytes[..8]);
        let mut device = [0u8; 16];
        device.copy_from_slice(&bytes[10..]);
        Self {
            wall_ms: u64::from_be_bytes(wall),
            counter: u16::from_be_bytes([bytes[8], bytes[9]]),
            device: DeviceId(device),
        }
    }
}

/// Ein empfangener Zeitstempel lag zu weit in der Zukunft (Uhr des Senders falsch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDrift {
    pub remote: Hlc,
    pub local_now_ms: u64,
}

/// Uhr eines Geräts. Die aktuelle Zeit wird explizit übergeben, damit Tests deterministisch sind.
#[derive(Debug, Clone)]
pub struct Clock {
    device: DeviceId,
    wall_ms: u64,
    counter: u16,
}

impl Clock {
    pub fn new(device: DeviceId) -> Self {
        Self {
            device,
            wall_ms: 0,
            counter: 0,
        }
    }

    pub fn device(&self) -> DeviceId {
        self.device
    }

    /// Neuer Zeitstempel für eine lokale Änderung; immer größer als alle bisherigen.
    pub fn tick(&mut self, now_ms: u64) -> Hlc {
        if now_ms > self.wall_ms {
            self.wall_ms = now_ms;
            self.counter = 0;
        } else if self.counter == u16::MAX {
            self.wall_ms += 1;
            self.counter = 0;
        } else {
            self.counter += 1;
        }
        Hlc {
            wall_ms: self.wall_ms,
            counter: self.counter,
            device: self.device,
        }
    }

    /// Zieht die Uhr auf einen empfangenen Zeitstempel nach, sofern er plausibel ist.
    ///
    /// # Errors
    /// [`ClockDrift`], wenn `remote` mehr als [`MAX_DRIFT_MS`] vor `now_ms` liegt.
    /// Die Uhr bleibt dann unverändert; die zugehörige Operation wird trotzdem angewendet.
    pub fn observe(&mut self, remote: Hlc, now_ms: u64) -> Result<(), ClockDrift> {
        if remote.wall_ms > now_ms.saturating_add(MAX_DRIFT_MS) {
            return Err(ClockDrift {
                remote,
                local_now_ms: now_ms,
            });
        }
        if (remote.wall_ms, remote.counter) > (self.wall_ms, self.counter) {
            self.wall_ms = remote.wall_ms;
            self.counter = remote.counter;
        }
        Ok(())
    }
}

/// Aktuelle Systemzeit in Millisekunden seit 1970.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_strictly_monotonic_even_if_wall_clock_goes_back() {
        let mut clock = Clock::new(DeviceId::new());
        let a = clock.tick(1000);
        let b = clock.tick(1000);
        let c = clock.tick(500);
        let d = clock.tick(2000);
        assert!(a < b && b < c && c < d);
        assert_eq!(c.wall_ms, 1000);
        assert_eq!(d.counter, 0);
    }

    #[test]
    fn counter_overflow_advances_wall() {
        let mut clock = Clock::new(DeviceId::new());
        clock.tick(10);
        clock.counter = u16::MAX;
        let next = clock.tick(10);
        assert_eq!((next.wall_ms, next.counter), (11, 0));
    }

    #[test]
    fn observe_moves_clock_forward() {
        let mut a = Clock::new(DeviceId::new());
        let mut b = Clock::new(DeviceId::new());
        let remote = a.tick(5000);
        b.observe(remote, 1000).unwrap();
        assert!(b.tick(1000) > remote);
    }

    #[test]
    fn observe_rejects_far_future() {
        let mut clock = Clock::new(DeviceId::new());
        let remote = Hlc {
            wall_ms: 1000 + MAX_DRIFT_MS + 1,
            counter: 0,
            device: DeviceId::new(),
        };
        assert!(clock.observe(remote, 1000).is_err());
        assert_eq!(clock.tick(1000).wall_ms, 1000);
    }

    #[test]
    fn byte_encoding_preserves_order() {
        let d1 = DeviceId([1; 16]);
        let d2 = DeviceId([2; 16]);
        let mut stamps = vec![
            Hlc {
                wall_ms: 1,
                counter: 5,
                device: d2,
            },
            Hlc {
                wall_ms: 1,
                counter: 5,
                device: d1,
            },
            Hlc {
                wall_ms: 1,
                counter: 300,
                device: d1,
            },
            Hlc {
                wall_ms: 256,
                counter: 0,
                device: d1,
            },
            Hlc {
                wall_ms: 0,
                counter: u16::MAX,
                device: d2,
            },
        ];
        let mut by_bytes = stamps.clone();
        stamps.sort();
        by_bytes.sort_by_key(Hlc::to_bytes);
        assert_eq!(stamps, by_bytes);
        for s in &stamps {
            assert_eq!(Hlc::from_bytes(&s.to_bytes()), *s);
        }
    }
}
