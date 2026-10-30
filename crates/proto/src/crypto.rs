//! Ende-zu-Ende-Verschlüsselung der Sync-Pakete und der Gerätekopplung.
//! Siehe Abschnitt 5 der Design-Doku. Keine eigene Kryptografie: XChaCha20-Poly1305 (AEAD),
//! Ed25519 (Signaturen), X25519 (Schlüsselaustausch), HKDF-SHA256 (Schlüsselableitung).

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use doeverything_core::{Changeset, ChangesetId, EntityId, Op};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::api::{AccountId, DeviceInfo};
use crate::{Envelope, EnvelopeHeader, PROTOCOL_VERSION, decode, encode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// Entschlüsselung fehlgeschlagen: falscher Schlüssel oder manipulierte Daten.
    Decrypt,
    BadSignature,
    /// Inhalt nach dem Entschlüsseln nicht lesbar.
    Malformed(String),
    UnsupportedProtocol(u16),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decrypt => write!(f, "Entschlüsselung fehlgeschlagen"),
            Self::BadSignature => write!(f, "ungültige Signatur"),
            Self::Malformed(e) => write!(f, "unlesbarer Inhalt: {e}"),
            Self::UnsupportedProtocol(v) => write!(f, "Protokollversion {v} nicht unterstützt"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Zufällige Bytes vom Betriebssystem.
///
/// # Panics
/// Wenn das Betriebssystem keine Zufallszahlen liefert.
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("Zufallsgenerator des Betriebssystems nicht verfügbar");
    out
}

fn hkdf32(ikm: &[u8], salt: Option<&[u8]>, info: &[&[u8]]) -> [u8; 32] {
    let mut okm = [0u8; 32];
    Hkdf::<Sha256>::new(salt, ikm)
        .expand(&info.concat(), &mut okm)
        .expect("32 Byte sind eine gültige HKDF-Länge");
    okm
}

/// Schlüssel eines Space, abgeleitet aus dem Account Root Key.
///
/// Abweichung von der Design-Doku (zufällige, gewrappte Space-Schlüssel): Ableitung ist für v1
/// einfacher. Echte Rotation beim Entfernen eines Geräts erfordert dann einen neuen Root Key.
pub fn space_key(account_key: &[u8; 32], space: EntityId, epoch: u32) -> [u8; 32] {
    hkdf32(
        account_key,
        None,
        &[b"doeverything space key v1", &space.0, &epoch.to_be_bytes()],
    )
}

/// Schlüssel eines Geräts. Liegen in der verschlüsselten lokalen Datenbank.
pub struct DeviceKeys {
    pub signing: SigningKey,
    pub exchange: StaticSecret,
}

impl DeviceKeys {
    pub fn generate() -> Self {
        Self::from_bytes(random(), random())
    }

    pub fn from_bytes(signing: [u8; 32], exchange: [u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&signing),
            exchange: StaticSecret::from(exchange),
        }
    }

    pub fn signing_public(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    pub fn exchange_public(&self) -> [u8; 32] {
        PublicKey::from(&self.exchange).to_bytes()
    }
}

/// Signatur mit Kontext-Präfix, damit eine Signatur nie für einen anderen Zweck gilt.
pub fn sign(key: &SigningKey, context: &[u8], message: &[u8]) -> [u8; 64] {
    key.sign(&[context, message].concat()).to_bytes()
}

/// # Errors
/// [`CryptoError::BadSignature`] bei ungültigem Schlüssel oder ungültiger Signatur.
pub fn verify(
    public: &[u8; 32],
    context: &[u8],
    message: &[u8],
    signature: &[u8; 64],
) -> Result<(), CryptoError> {
    let key = VerifyingKey::from_bytes(public).map_err(|_| CryptoError::BadSignature)?;
    key.verify(
        &[context, message].concat(),
        &Signature::from_bytes(signature),
    )
    .map_err(|_| CryptoError::BadSignature)
}

pub const CTX_ENVELOPE: &[u8] = b"doeverything envelope v1";
pub const CTX_AUTH: &[u8] = b"doeverything auth v1";
pub const CTX_DEVICE: &[u8] = b"doeverything device v1";

/// Beweis, dass ein Gerät den privaten Schlüssel zu seinem öffentlichen besitzt.
pub fn device_proof(keys: &DeviceKeys, device: &DeviceInfo) -> [u8; 64] {
    sign(&keys.signing, CTX_DEVICE, &device_proof_message(device))
}

pub fn device_proof_message(device: &DeviceInfo) -> Vec<u8> {
    [&device.device.0[..], &device.signing_pub].concat()
}

/// Polsterung auf Größenstufen (256 B, 512 B, 1 KiB, …), damit der Server nicht an der
/// Länge erkennt, was geändert wurde. Format: Länge (u32 LE) + Daten + Nullen.
fn pad(data: &[u8]) -> Vec<u8> {
    let needed = data.len() + 4;
    let size = needed.next_power_of_two().max(256);
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(data);
    out.resize(size, 0);
    out
}

fn unpad(padded: &[u8]) -> Result<&[u8], CryptoError> {
    let len_bytes: [u8; 4] = padded
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| CryptoError::Malformed("zu kurz".into()))?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    padded
        .get(4..4 + len)
        .ok_or_else(|| CryptoError::Malformed("Länge passt nicht".into()))
}

fn envelope_signed_bytes(aad: &[u8], nonce: &[u8; 24], ciphertext: &[u8]) -> Vec<u8> {
    [aad, nonce, ciphertext].concat()
}

/// Verschlüsselt und signiert ein Changeset.
///
/// # Errors
/// Nur, wenn die Operationen nicht serialisierbar sind.
pub fn seal_changeset(
    cs: &Changeset,
    space_key: &[u8; 32],
    key_epoch: u32,
    signing: &SigningKey,
) -> Result<Envelope, CryptoError> {
    let header = EnvelopeHeader {
        proto: PROTOCOL_VERSION,
        space: cs.space,
        device: cs.id.device,
        seq: cs.id.seq,
        key_epoch,
    };
    let aad = encode(&header).map_err(|e| CryptoError::Malformed(e.to_string()))?;
    let plaintext = pad(&encode(&cs.ops).map_err(|e| CryptoError::Malformed(e.to_string()))?);
    let nonce: [u8; 24] = random();
    let cipher = XChaCha20Poly1305::new(&(*space_key).into());
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    let signature = sign(
        signing,
        CTX_ENVELOPE,
        &envelope_signed_bytes(&aad, &nonce, &ciphertext),
    );
    Ok(Envelope {
        header,
        nonce,
        ciphertext,
        signature,
    })
}

/// Prüft nur die Signatur, ohne zu entschlüsseln (das kann der Server).
///
/// # Errors
/// [`CryptoError::BadSignature`]
pub fn verify_envelope(env: &Envelope, signer: &[u8; 32]) -> Result<(), CryptoError> {
    let aad = encode(&env.header).map_err(|e| CryptoError::Malformed(e.to_string()))?;
    verify(
        signer,
        CTX_ENVELOPE,
        &envelope_signed_bytes(&aad, &env.nonce, &env.ciphertext),
        &env.signature,
    )
}

/// Prüft Signatur, entschlüsselt und liefert das Changeset.
///
/// # Errors
/// Falsche Signatur, falscher Schlüssel, manipulierte Daten oder unbekannte Protokollversion.
pub fn open_envelope(
    env: &Envelope,
    space_key: &[u8; 32],
    signer: &[u8; 32],
) -> Result<Changeset, CryptoError> {
    if env.header.proto != PROTOCOL_VERSION {
        return Err(CryptoError::UnsupportedProtocol(env.header.proto));
    }
    verify_envelope(env, signer)?;
    let aad = encode(&env.header).map_err(|e| CryptoError::Malformed(e.to_string()))?;
    let cipher = XChaCha20Poly1305::new(&(*space_key).into());
    let padded = cipher
        .decrypt(
            &XNonce::from(env.nonce),
            Payload {
                msg: &env.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    let ops: Vec<Op> =
        decode(unpad(&padded)?).map_err(|e| CryptoError::Malformed(e.to_string()))?;
    Ok(Changeset {
        id: ChangesetId {
            device: env.header.device,
            seq: env.header.seq,
        },
        space: env.header.space,
        ops,
    })
}

/// Vom neuen Gerät angezeigter Code. Enthält nur öffentliche Schlüssel und ein Einmal-Geheimnis,
/// nie den Account-Schlüssel.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PairingCode {
    pub server: String,
    pub device: DeviceInfo,
    #[serde(with = "serde_bytes")]
    pub exchange_pub: [u8; 32],
    #[serde(with = "serde_bytes")]
    pub pairing_id: [u8; 16],
    #[serde(with = "serde_bytes")]
    pub secret: [u8; 16],
    /// [`device_proof`] des neuen Geräts.
    #[serde(with = "serde_bytes")]
    pub proof: [u8; 64],
}

impl PairingCode {
    /// Kompakte Textform zum Kopieren (später zusätzlich als QR-Code).
    ///
    /// # Panics
    /// Nie: alle Felder sind serialisierbar.
    pub fn to_text(&self) -> String {
        format!(
            "DOE1-{}",
            URL_SAFE_NO_PAD.encode(encode(self).expect("serialisierbar"))
        )
    }

    /// # Errors
    /// Wenn der Text kein gültiger Kopplungscode ist.
    pub fn from_text(text: &str) -> Result<Self, CryptoError> {
        let body = text
            .trim()
            .strip_prefix("DOE1-")
            .ok_or_else(|| CryptoError::Malformed("kein Kopplungscode (DOE1-…)".into()))?;
        let bytes = URL_SAFE_NO_PAD
            .decode(body.trim())
            .map_err(|e| CryptoError::Malformed(e.to_string()))?;
        decode(&bytes).map_err(|e| CryptoError::Malformed(e.to_string()))
    }
}

/// Was das bestehende Gerät dem neuen übergibt.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PairingSecret {
    pub account: AccountId,
    #[serde(with = "serde_bytes")]
    pub account_key: [u8; 32],
}

/// Verschlüsselte Übergabe über das Server-Postfach.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PairingMessage {
    #[serde(with = "serde_bytes")]
    pub ephemeral_pub: [u8; 32],
    #[serde(with = "serde_bytes")]
    pub nonce: [u8; 24],
    #[serde(with = "serde_bytes")]
    pub ciphertext: Vec<u8>,
}

fn pairing_key(shared: &[u8; 32], code_secret: &[u8; 16], pairing_id: &[u8; 16]) -> [u8; 32] {
    hkdf32(
        shared,
        Some(code_secret),
        &[b"doeverything pairing v1", pairing_id],
    )
}

/// Verschlüsselt den Account-Schlüssel für das neue Gerät (X25519 mit Einmal-Schlüssel).
/// Öffnen kann es nur, wer den privaten Austauschschlüssel **und** das Geheimnis aus dem Code hat.
///
/// # Errors
/// Nur bei internen Kodierungsfehlern.
pub fn seal_pairing(
    code: &PairingCode,
    secret: &PairingSecret,
) -> Result<PairingMessage, CryptoError> {
    let ephemeral = StaticSecret::from(random::<32>());
    let shared = ephemeral.diffie_hellman(&PublicKey::from(code.exchange_pub));
    let key = pairing_key(shared.as_bytes(), &code.secret, &code.pairing_id);
    let nonce: [u8; 24] = random();
    let plaintext = encode(secret).map_err(|e| CryptoError::Malformed(e.to_string()))?;
    let ciphertext = XChaCha20Poly1305::new(&key.into())
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plaintext,
                aad: &code.pairing_id,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    Ok(PairingMessage {
        ephemeral_pub: PublicKey::from(&ephemeral).to_bytes(),
        nonce,
        ciphertext,
    })
}

/// # Errors
/// [`CryptoError::Decrypt`], wenn die Nachricht nicht für dieses Gerät/diesen Code ist.
pub fn open_pairing(
    msg: &PairingMessage,
    exchange: &StaticSecret,
    code_secret: &[u8; 16],
    pairing_id: &[u8; 16],
) -> Result<PairingSecret, CryptoError> {
    let shared = exchange.diffie_hellman(&PublicKey::from(msg.ephemeral_pub));
    let key = pairing_key(shared.as_bytes(), code_secret, pairing_id);
    let plaintext = XChaCha20Poly1305::new(&key.into())
        .decrypt(
            &XNonce::from(msg.nonce),
            Payload {
                msg: &msg.ciphertext,
                aad: pairing_id,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    decode(&plaintext).map_err(|e| CryptoError::Malformed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use doeverything_core::{DeviceId, MemoryStore, Replica, Store, SysField, VersionVector};

    use super::*;

    fn changeset(text: &str) -> (Changeset, DeviceKeys) {
        let store = MemoryStore::new();
        let mut r = Replica::open(store).unwrap();
        let space = r.create_space("Privat", 1000).unwrap();
        r.transact(space, 1000, |tx| tx.set(space, SysField::Name, text))
            .unwrap();
        let cs = r
            .changesets_missing_from(space, &VersionVector::default())
            .unwrap()
            .remove(1);
        assert_eq!(r.store().device_id(), cs.id.device);
        (cs, DeviceKeys::generate())
    }

    #[test]
    fn changeset_hin_und_zurueck() {
        let (cs, keys) = changeset("Geheime Einkaufsliste");
        let ark = random::<32>();
        let key = space_key(&ark, cs.space, 0);
        let env = seal_changeset(&cs, &key, 0, &keys.signing).unwrap();

        assert!(
            !env.ciphertext.windows(8).any(|w| w == b"Geheime "),
            "Klartext im Chiffrat"
        );
        assert_eq!(env.ciphertext.len() % 256, 16, "gepolstert (+16 Byte Tag)");
        assert_eq!(
            open_envelope(&env, &key, &keys.signing_public()).unwrap(),
            cs
        );
    }

    #[test]
    fn manipulation_wird_erkannt() {
        let (cs, keys) = changeset("x");
        let key = space_key(&random(), cs.space, 0);
        let env = seal_changeset(&cs, &key, 0, &keys.signing).unwrap();
        let public = keys.signing_public();

        let mut umetikettiert = env.clone();
        umetikettiert.header.seq += 1;
        assert_eq!(
            open_envelope(&umetikettiert, &key, &public),
            Err(CryptoError::BadSignature)
        );

        let mut verfaelscht = env.clone();
        verfaelscht.ciphertext[0] ^= 1;
        assert_eq!(
            open_envelope(&verfaelscht, &key, &public),
            Err(CryptoError::BadSignature)
        );

        let fremd = DeviceKeys::generate();
        assert_eq!(
            open_envelope(&env, &key, &fremd.signing_public()),
            Err(CryptoError::BadSignature)
        );

        let falscher_schluessel = space_key(&random(), cs.space, 0);
        assert_eq!(
            open_envelope(&env, &falscher_schluessel, &public),
            Err(CryptoError::Decrypt)
        );

        // Gültig signiert, aber mit anderem Kopf neu signiert → AEAD scheitert am geänderten AAD
        let mut neu_signiert = env.clone();
        neu_signiert.header.device = DeviceId::new();
        let aad = encode(&neu_signiert.header).unwrap();
        neu_signiert.signature = sign(
            &keys.signing,
            CTX_ENVELOPE,
            &envelope_signed_bytes(&aad, &neu_signiert.nonce, &neu_signiert.ciphertext),
        );
        assert_eq!(
            open_envelope(&neu_signiert, &key, &public),
            Err(CryptoError::Decrypt)
        );
    }

    #[test]
    fn space_schluessel_sind_getrennt() {
        let ark = random::<32>();
        let a = EntityId::new();
        let b = EntityId::new();
        assert_ne!(space_key(&ark, a, 0), space_key(&ark, b, 0));
        assert_ne!(space_key(&ark, a, 0), space_key(&ark, a, 1));
        assert_eq!(space_key(&ark, a, 0), space_key(&ark, a, 0));
    }

    #[test]
    fn polsterung() {
        for len in [0, 1, 251, 252, 253, 1000, 70_000] {
            let data = vec![7u8; len];
            let padded = pad(&data);
            assert!(padded.len().is_power_of_two() && padded.len() >= 256);
            assert_eq!(unpad(&padded).unwrap(), &data[..]);
        }
        assert!(unpad(&[255, 255, 0, 0, 1]).is_err());
    }

    #[test]
    fn kopplung() {
        let neu = DeviceKeys::generate();
        let info = DeviceInfo {
            device: DeviceId::new(),
            signing_pub: neu.signing_public(),
            name: "Laptop".into(),
        };
        let code = PairingCode {
            server: "https://sync.example".into(),
            proof: device_proof(&neu, &info),
            device: info,
            exchange_pub: neu.exchange_public(),
            pairing_id: random(),
            secret: random(),
        };
        let text = code.to_text();
        assert!(text.starts_with("DOE1-") && !text.contains(['+', '/', '=']));
        let gelesen = PairingCode::from_text(&format!("  {text}\n")).unwrap();
        assert_eq!(gelesen, code);
        verify(
            &gelesen.device.signing_pub,
            CTX_DEVICE,
            &device_proof_message(&gelesen.device),
            &gelesen.proof,
        )
        .unwrap();

        let geheimnis = PairingSecret {
            account: AccountId(random()),
            account_key: random(),
        };
        let msg = seal_pairing(&gelesen, &geheimnis).unwrap();
        assert_eq!(
            open_pairing(&msg, &neu.exchange, &code.secret, &code.pairing_id).unwrap(),
            geheimnis
        );

        // Ohne das Geheimnis aus dem Code oder mit fremdem Schlüssel: nicht lesbar
        assert_eq!(
            open_pairing(&msg, &neu.exchange, &random(), &code.pairing_id),
            Err(CryptoError::Decrypt)
        );
        let fremd = DeviceKeys::generate();
        assert_eq!(
            open_pairing(&msg, &fremd.exchange, &code.secret, &code.pairing_id),
            Err(CryptoError::Decrypt)
        );

        assert!(PairingCode::from_text("hallo").is_err());
        assert!(PairingCode::from_text("DOE1-!!!").is_err());
    }
}
