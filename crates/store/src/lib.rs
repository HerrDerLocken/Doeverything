//! Lokale Speicherung in SQLite (optional mit `SQLCipher` verschlüsselt).
//!
//! Schema und Semantik: `docs/design/datenmodell-und-sync.md`, Abschnitt 4. Die
//! Last-Writer-Wins-Regel steckt direkt im Upsert: `field_values.hlc` ist eine
//! Big-Endian-Kodierung, deren Byte-Vergleich (SQLite vergleicht BLOBs per `memcmp`)
//! der Ordnung der HLC entspricht.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use doeverything_core::entity::sort_children;
use doeverything_core::{
    Changeset, ChangesetId, DeviceId, Entity, EntityId, FieldKey, Hlc, Kind, Op, Store, SysField,
    Value, VersionVector, Versioned,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// Bei jeder Schemaänderung erhöhen und in [`migrate`] einen Schritt ergänzen.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value BLOB NOT NULL
) STRICT;

CREATE TABLE changesets (
  space_id  BLOB NOT NULL,
  device_id BLOB NOT NULL,
  seq       INTEGER NOT NULL,
  max_hlc   BLOB NOT NULL,
  ops       BLOB NOT NULL,
  PRIMARY KEY (space_id, device_id, seq)
) STRICT, WITHOUT ROWID;
CREATE INDEX changesets_max_hlc ON changesets(max_hlc);

CREATE TABLE entities (
  id        BLOB PRIMARY KEY,
  space_id  BLOB NOT NULL,
  kind      INTEGER,
  parent_id BLOB,
  deleted   INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;
CREATE INDEX entities_parent ON entities(parent_id, kind, deleted);
CREATE INDEX entities_space ON entities(space_id);

CREATE TABLE field_values (
  entity_id BLOB NOT NULL,
  field_key BLOB NOT NULL,
  value     BLOB NOT NULL,
  hlc       BLOB NOT NULL,
  PRIMARY KEY (entity_id, field_key)
) STRICT, WITHOUT ROWID;

CREATE TABLE version_vector (
  space_id  BLOB NOT NULL,
  device_id BLOB NOT NULL,
  max_seq   INTEGER NOT NULL,
  PRIMARY KEY (space_id, device_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE sync_state (
  peer_id  BLOB NOT NULL,
  space_id BLOB NOT NULL,
  cursor   INTEGER,
  PRIMARY KEY (peer_id, space_id)
) STRICT, WITHOUT ROWID;
";

/// Spalten für [`SqliteStore::load_entities`]; Abfragen ergänzen nur `WHERE`.
const ENTITY_SELECT: &str = "
SELECT e.id, e.kind, e.parent_id, f.field_key, f.value, f.hlc
FROM entities e LEFT JOIN field_values f ON f.entity_id = e.id";

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Codec(String),
    /// Daten in der Datenbank sind nicht interpretierbar.
    Corrupt(String),
    /// Ein Schlüssel wurde übergeben, aber das Crate ist ohne Feature `sqlcipher` gebaut.
    EncryptionUnavailable,
    /// Die Datenbank stammt von einer neueren App-Version.
    SchemaTooNew(i64),
    Io(std::io::Error),
    /// Die Datei ist nicht im erwarteten Zustand (z. B. Verschlüsseln einer bereits verschlüsselten Datei).
    UnexpectedFileState(DbFile),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "SQLite: {e}"),
            Self::Codec(e) => write!(f, "Kodierung: {e}"),
            Self::Corrupt(e) => write!(f, "beschädigte Daten: {e}"),
            Self::EncryptionUnavailable => write!(
                f,
                "ohne Feature `sqlcipher` gebaut, Verschlüsselung nicht verfügbar"
            ),
            Self::SchemaTooNew(v) => write!(
                f,
                "Datenbank-Schema {v} ist neuer als unterstützt ({SCHEMA_VERSION})"
            ),
            Self::Io(e) => write!(f, "Dateizugriff: {e}"),
            Self::UnexpectedFileState(state) => {
                write!(f, "Datenbankdatei ist im Zustand {state:?}")
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug)]
pub struct SqliteStore {
    conn: Connection,
    device: DeviceId,
}

impl SqliteStore {
    /// Öffnet oder erzeugt die Datenbank. Mit `key` wird sie per `SQLCipher` verschlüsselt
    /// (der Schlüssel kommt aus dem Schlüsselspeicher des Betriebssystems).
    ///
    /// # Errors
    /// Falscher Schlüssel, beschädigte Datei oder Schema einer neueren Version.
    pub fn open(path: impl AsRef<Path>, key: Option<&[u8; 32]>) -> Result<Self> {
        Self::init(Connection::open(path)?, key)
    }

    /// # Errors
    /// Nur bei internen SQLite-Fehlern.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(mut conn: Connection, key: Option<&[u8; 32]>) -> Result<Self> {
        if let Some(key) = key {
            apply_key(&conn, key)?;
        }
        // Erster Lesezugriff: schlägt bei falschem Schlüssel mit "file is not a database" fehl.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        migrate(&mut conn)?;
        let device = load_or_create_device_id(&conn)?;
        Ok(Self { conn, device })
    }

    /// Lädt Entities samt Feldwerten; `filter` ist eine `WHERE`-Klausel über `e`.
    fn load_entities(
        &self,
        filter: &str,
        params: impl rusqlite::Params,
    ) -> Result<BTreeMap<EntityId, Entity>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{ENTITY_SELECT} WHERE {filter}"))?;
        let mut rows = stmt.query(params)?;
        let mut out: BTreeMap<EntityId, Entity> = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let id = EntityId(blob16(&row.get::<_, Vec<u8>>(0)?)?);
            let entity = out.entry(id).or_default();
            if let Some(code) = row.get::<_, Option<u8>>(1)? {
                entity.kind = Some(
                    Kind::from_code(code)
                        .ok_or_else(|| StoreError::Corrupt(format!("Kind {code}")))?,
                );
            }
            if let Some(parent) = row.get::<_, Option<Vec<u8>>>(2)? {
                entity.parent = Some(EntityId(blob16(&parent)?));
            }
            if let Some(key) = row.get::<_, Option<Vec<u8>>>(3)? {
                let key = FieldKey::from_bytes(&key)
                    .ok_or_else(|| StoreError::Corrupt(format!("Feldschlüssel {key:?}")))?;
                let value = decode(&row.get::<_, Vec<u8>>(4)?)?;
                let hlc = decode_hlc(&row.get::<_, Vec<u8>>(5)?)?;
                entity.fields.insert(key, Versioned { value, hlc });
            }
        }
        Ok(out)
    }
}

/// Zustand einer Datenbankdatei, ohne sie zu öffnen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFile {
    /// Existiert nicht oder ist leer.
    Missing,
    /// Unverschlüsselte SQLite-Datenbank.
    Plaintext,
    /// Kein SQLite-Klartext-Header, also (vermutlich) per `SQLCipher` verschlüsselt.
    Encrypted,
}

/// Erkennt am Datei-Header, ob eine Datenbank verschlüsselt ist.
///
/// # Errors
/// Wenn die Datei existiert, aber nicht gelesen werden kann.
pub fn inspect(path: impl AsRef<Path>) -> Result<DbFile> {
    use std::io::Read;
    let mut header = [0u8; 16];
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DbFile::Missing),
        Err(e) => return Err(e.into()),
    };
    let read = file.read(&mut header)?;
    Ok(match read {
        0 => DbFile::Missing,
        _ if &header[..read] == b"SQLite format 3\0" => DbFile::Plaintext,
        _ => DbFile::Encrypted,
    })
}

/// Neuer zufälliger Datenbankschlüssel aus dem Zufallsgenerator des Betriebssystems.
///
/// # Panics
/// Wenn das Betriebssystem keine Zufallszahlen liefert (dann ist keine sichere Verschlüsselung möglich).
pub fn generate_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).expect("Zufallsgenerator des Betriebssystems nicht verfügbar");
    key
}

#[cfg(feature = "sqlcipher")]
fn key_literal(key: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(64);
    for b in key {
        write!(hex, "{b:02x}").expect("String-Write schlägt nicht fehl");
    }
    // Rohschlüssel-Syntax von SQLCipher: kein Passwort-KDF, der Schlüssel ist bereits zufällig.
    format!("\"x'{hex}'\"")
}

#[cfg(feature = "sqlcipher")]
fn apply_key(conn: &Connection, key: &[u8; 32]) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA key = {};", key_literal(key)))?;
    Ok(())
}

/// Wandelt eine unverschlüsselte Datenbank in eine verschlüsselte um.
///
/// Ablauf: WAL zurückschreiben, per `sqlcipher_export` in eine Nachbardatei kopieren, diese mit
/// dem Schlüssel probeweise öffnen und erst dann atomar über das Original schieben. Bricht der
/// Vorgang vorher ab, bleibt das Original unverändert.
///
/// # Errors
/// [`StoreError::UnexpectedFileState`], wenn die Datei nicht unverschlüsselt ist.
#[cfg(feature = "sqlcipher")]
pub fn encrypt_plaintext(path: impl AsRef<Path>, key: &[u8; 32]) -> Result<()> {
    let path = path.as_ref();
    let state = inspect(path)?;
    if state != DbFile::Plaintext {
        return Err(StoreError::UnexpectedFileState(state));
    }
    let tmp = sibling(path, ".encrypting");
    remove_if_exists(&tmp)?;

    {
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let tmp_sql = tmp.to_string_lossy().replace('\'', "''");
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{tmp_sql}' AS encrypted KEY {};",
            key_literal(key)
        ))?;
        conn.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
        conn.execute_batch(&format!(
            "PRAGMA encrypted.user_version = {version}; DETACH DATABASE encrypted;"
        ))?;
    }

    // Probe: lässt sich die Kopie mit dem Schlüssel öffnen?
    drop(SqliteStore::open(&tmp, Some(key))?);
    std::fs::rename(&tmp, path)?;
    // Die alte (leere) WAL gehört zur Klartext-Datei und darf nicht auf die neue angewendet werden.
    remove_if_exists(&sibling(path, "-wal"))?;
    remove_if_exists(&sibling(path, "-shm"))?;
    Ok(())
}

#[cfg(feature = "sqlcipher")]
fn sibling(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

#[cfg(feature = "sqlcipher")]
fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(not(feature = "sqlcipher"))]
fn apply_key(_conn: &Connection, _key: &[u8; 32]) -> Result<()> {
    Err(StoreError::EncryptionUnavailable)
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew(version));
    }
    let tx = conn.transaction()?;
    if version < 1 {
        tx.execute_batch(SCHEMA_V1)?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

fn load_or_create_device_id(conn: &Connection) -> Result<DeviceId> {
    let existing: Option<Vec<u8>> = conn
        .query_row("SELECT value FROM meta WHERE key = 'device_id'", [], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(bytes) = existing {
        return Ok(DeviceId(blob16(&bytes)?));
    }
    let device = DeviceId::new();
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('device_id', ?1)",
        params![&device.0[..]],
    )?;
    Ok(device)
}

impl Store for SqliteStore {
    type Error = StoreError;

    fn device_id(&self) -> DeviceId {
        self.device
    }

    fn max_hlc(&self) -> Result<Option<Hlc>> {
        let bytes: Option<Vec<u8>> =
            self.conn
                .query_row("SELECT max(max_hlc) FROM changesets", [], |r| r.get(0))?;
        bytes.map(|b| decode_hlc(&b)).transpose()
    }

    fn version_vector(&self, space: EntityId) -> Result<VersionVector> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT device_id, max_seq FROM version_vector WHERE space_id = ?1")?;
        let mut rows = stmt.query(params![&space.0[..]])?;
        let mut vv = VersionVector::default();
        while let Some(row) = rows.next()? {
            let device = DeviceId(blob16(&row.get::<_, Vec<u8>>(0)?)?);
            let seq = u64::try_from(row.get::<_, i64>(1)?)
                .map_err(|_| StoreError::Corrupt("negative seq".into()))?;
            vv.0.insert(device, seq);
        }
        Ok(vv)
    }

    fn apply_changeset(&mut self, cs: &Changeset) -> Result<()> {
        let seq =
            i64::try_from(cs.id.seq).map_err(|_| StoreError::Corrupt("seq zu groß".into()))?;
        let max_hlc = cs
            .ops
            .iter()
            .map(Op::hlc)
            .max()
            .ok_or_else(|| StoreError::Corrupt("leeres Changeset".into()))?;
        let space = &cs.space.0[..];
        let device = &cs.id.device.0[..];

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO changesets (space_id, device_id, seq, max_hlc, ops) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![space, device, seq, &max_hlc.to_bytes()[..], encode(&cs.ops)?],
        )?;
        tx.execute(
            "INSERT INTO version_vector (space_id, device_id, max_seq) VALUES (?1, ?2, ?3)
             ON CONFLICT (space_id, device_id) DO UPDATE SET max_seq = max(max_seq, excluded.max_seq)",
            params![space, device, seq],
        )?;
        for op in &cs.ops {
            apply_op(&tx, cs.space, op)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn changesets_missing_from(
        &self,
        space: EntityId,
        vv: &VersionVector,
    ) -> Result<Vec<Changeset>> {
        // Pro Gerät nur ab der ersten fehlenden seq lesen. Geräte, die `vv` nicht kennt, vollständig.
        let mut stmt = self.conn.prepare_cached(
            "SELECT device_id, seq, ops FROM changesets WHERE space_id = ?1 ORDER BY device_id, seq",
        )?;
        let mut rows = stmt.query(params![&space.0[..]])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let device = DeviceId(blob16(&row.get::<_, Vec<u8>>(0)?)?);
            let seq = u64::try_from(row.get::<_, i64>(1)?)
                .map_err(|_| StoreError::Corrupt("negative seq".into()))?;
            let id = ChangesetId { device, seq };
            if !vv.contains(id) {
                let ops = decode(&row.get::<_, Vec<u8>>(2)?)?;
                out.push(Changeset { id, space, ops });
            }
        }
        Ok(out)
    }

    fn entity(&self, id: EntityId) -> Result<Option<Entity>> {
        let mut found =
            self.load_entities("e.id = ?1 AND e.kind IS NOT NULL", params![&id.0[..]])?;
        Ok(found.remove(&id))
    }

    fn children(&self, parent: EntityId, kind: Kind) -> Result<Vec<(EntityId, Entity)>> {
        let mut out: Vec<_> = self
            .load_entities(
                "e.parent_id = ?1 AND e.kind = ?2 AND e.deleted = 0",
                params![&parent.0[..], kind.code()],
            )?
            .into_iter()
            .collect();
        sort_children(&mut out);
        Ok(out)
    }

    fn meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    fn set_meta(&mut self, key: &str, value: Option<&[u8]>) -> Result<()> {
        match value {
            Some(v) => self.conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, v],
            )?,
            None => self
                .conn
                .execute("DELETE FROM meta WHERE key = ?1", params![key])?,
        };
        Ok(())
    }

    fn spaces(&self) -> Result<Vec<EntityId>> {
        let found =
            self.load_entities("e.kind = ?1 AND e.deleted = 0", params![Kind::Space.code()])?;
        Ok(found.into_keys().collect())
    }

    fn entities(&self, space: EntityId) -> Result<BTreeMap<EntityId, Entity>> {
        self.load_entities("e.space_id = ?1", params![&space.0[..]])
    }
}

fn apply_op(tx: &Transaction<'_>, space: EntityId, op: &Op) -> Result<()> {
    match op {
        Op::Create {
            id, kind, parent, ..
        } => {
            // Eine "pending" Entity (nur Sets bisher) bekommt hier Art und Elternteil.
            tx.execute(
                "INSERT INTO entities (id, space_id, kind, parent_id) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, parent_id = excluded.parent_id
                 WHERE entities.kind IS NULL",
                params![
                    &id.0[..],
                    &space.0[..],
                    kind.code(),
                    parent.as_ref().map(|p| &p.0[..])
                ],
            )?;
        }
        Op::Set {
            id,
            key,
            value,
            hlc,
        } => {
            let id_bytes = &id.0[..];
            tx.execute(
                "INSERT INTO entities (id, space_id) VALUES (?1, ?2) ON CONFLICT (id) DO NOTHING",
                params![id_bytes, &space.0[..]],
            )?;
            let key_bytes = key.to_bytes();
            tx.execute(
                "INSERT INTO field_values (entity_id, field_key, value, hlc) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (entity_id, field_key) DO UPDATE SET value = excluded.value, hlc = excluded.hlc
                 WHERE excluded.hlc > field_values.hlc",
                params![id_bytes, key_bytes, encode(value)?, &hlc.to_bytes()[..]],
            )?;
            if *key == FieldKey::System(SysField::Deleted) {
                // Spalte `deleted` ist ein Cache des aktuell gültigen Werts, für schnelle Listenabfragen.
                let current: Vec<u8> = tx.query_row(
                    "SELECT value FROM field_values WHERE entity_id = ?1 AND field_key = ?2",
                    params![id_bytes, key_bytes],
                    |r| r.get(0),
                )?;
                let deleted = decode::<Value>(&current)?.as_bool() == Some(true);
                tx.execute(
                    "UPDATE entities SET deleted = ?2 WHERE id = ?1",
                    params![id_bytes, deleted],
                )?;
            }
        }
    }
    Ok(())
}

fn blob16(bytes: &[u8]) -> Result<[u8; 16]> {
    bytes
        .try_into()
        .map_err(|_| StoreError::Corrupt(format!("ID mit {} statt 16 Bytes", bytes.len())))
}

fn decode_hlc(bytes: &[u8]) -> Result<Hlc> {
    let arr: &[u8; Hlc::ENCODED_LEN] = bytes
        .try_into()
        .map_err(|_| StoreError::Corrupt(format!("HLC mit {} Bytes", bytes.len())))?;
    Ok(Hlc::from_bytes(arr))
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).map_err(|e| StoreError::Codec(e.to_string()))?;
    Ok(out)
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ciborium::from_reader(bytes).map_err(|e| StoreError::Codec(e.to_string()))
}
