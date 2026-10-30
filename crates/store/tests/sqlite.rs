use doeverything_core::{Kind, MemoryStore, Replica, Store, SysField, Value};
use doeverything_store::{DbFile, SqliteStore, StoreError, inspect};

/// Dieselben Konfliktfälle wie für die Referenzimplementierung.
mod konformitaet {
    use doeverything_store::SqliteStore;

    doeverything_core::konformitaets_tests!(|| SqliteStore::open_in_memory().unwrap());
}

#[test]
fn daten_ueberleben_neustart() {
    let dir = tempfile::tempdir().unwrap();
    let pfad = dir.path().join("daten.sqlite");

    let mut r = Replica::open(SqliteStore::open(&pfad, None).unwrap()).unwrap();
    let geraet = r.device();
    let space = r.create_space("Privat", 5000).unwrap();
    let liste = r
        .transact(space, 5000, |tx| {
            let liste = tx.create(Kind::Collection, Some(space));
            tx.set(liste, SysField::Name, "Einkauf");
            liste
        })
        .unwrap();
    drop(r);

    // Uhr steht nach dem Neustart "in der Vergangenheit" → Zeitstempel müssen trotzdem steigen
    let mut r = Replica::open(SqliteStore::open(&pfad, None).unwrap()).unwrap();
    assert_eq!(r.device(), geraet, "Geräte-ID bleibt erhalten");
    r.transact(space, 1000, |tx| {
        tx.set(liste, SysField::Name, "Wocheneinkauf");
    })
    .unwrap();
    assert_eq!(
        r.entity(liste).unwrap().unwrap().get(SysField::Name),
        Some(&Value::from("Wocheneinkauf")),
        "spätere Änderung gewinnt trotz zurückgestellter Uhr"
    );
    assert_eq!(r.version_vector(space).unwrap().get(geraet), 3);
}

#[test]
fn sqlite_und_speicher_replizieren_untereinander() {
    let mut speicher = Replica::open(MemoryStore::new()).unwrap();
    let mut sqlite = Replica::open(SqliteStore::open_in_memory().unwrap()).unwrap();
    let space = speicher.create_space("Privat", 1000).unwrap();
    speicher
        .transact(space, 1000, |tx| {
            let l = tx.create(Kind::Collection, Some(space));
            tx.set(l, SysField::Name, "Liste");
        })
        .unwrap();
    sqlite
        .transact(space, 1500, |tx| tx.set(space, SysField::Name, "Zuhause"))
        .unwrap();

    for cs in speicher
        .changesets_missing_from(space, &sqlite.version_vector(space).unwrap())
        .unwrap()
    {
        sqlite.receive(&cs, 2000).unwrap();
    }
    for cs in sqlite
        .changesets_missing_from(space, &speicher.version_vector(space).unwrap())
        .unwrap()
    {
        speicher.receive(&cs, 2000).unwrap();
    }
    assert_eq!(
        speicher.store().entities(space).unwrap(),
        sqlite.store().entities(space).unwrap()
    );
}

#[cfg(not(feature = "sqlcipher"))]
#[test]
fn schluessel_ohne_sqlcipher_wird_abgelehnt() {
    let dir = tempfile::tempdir().unwrap();
    let err = SqliteStore::open(dir.path().join("x.sqlite"), Some(&[1; 32])).unwrap_err();
    assert!(matches!(err, StoreError::EncryptionUnavailable));
}

#[cfg(feature = "sqlcipher")]
#[test]
fn verschluesselte_datenbank() {
    let dir = tempfile::tempdir().unwrap();
    let pfad = dir.path().join("geheim.sqlite");
    let schluessel = [42u8; 32];

    let mut r = Replica::open(SqliteStore::open(&pfad, Some(&schluessel)).unwrap()).unwrap();
    let space = r.create_space("Privat", 1000).unwrap();
    r.transact(space, 1000, |tx| {
        tx.set(space, SysField::Name, "STRENG GEHEIM");
    })
    .unwrap();
    drop(r);

    let roh = std::fs::read(&pfad).unwrap();
    assert!(
        !roh.windows(13).any(|w| w == b"STRENG GEHEIM"),
        "Klartext in Datei"
    );
    assert!(
        !roh.starts_with(b"SQLite format 3"),
        "Datei ist unverschlüsselt"
    );

    assert!(
        SqliteStore::open(&pfad, Some(&[7; 32])).is_err(),
        "falscher Schlüssel"
    );
    assert!(SqliteStore::open(&pfad, None).is_err(), "ohne Schlüssel");

    let r = Replica::open(SqliteStore::open(&pfad, Some(&schluessel)).unwrap()).unwrap();
    assert_eq!(
        r.entity(space).unwrap().unwrap().get(SysField::Name),
        Some(&Value::from("STRENG GEHEIM"))
    );
}

#[test]
fn neueres_schema_wird_abgelehnt() {
    let dir = tempfile::tempdir().unwrap();
    let pfad = dir.path().join("neu.sqlite");
    drop(SqliteStore::open(&pfad, None).unwrap());
    rusqlite::Connection::open(&pfad)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert!(matches!(
        SqliteStore::open(&pfad, None),
        Err(StoreError::SchemaTooNew(99))
    ));
}

#[test]
fn dateizustand_erkennen() {
    let dir = tempfile::tempdir().unwrap();
    let pfad = dir.path().join("x.sqlite");
    assert_eq!(inspect(&pfad).unwrap(), DbFile::Missing);
    std::fs::write(&pfad, b"").unwrap();
    assert_eq!(inspect(&pfad).unwrap(), DbFile::Missing);
    std::fs::remove_file(&pfad).unwrap();
    drop(SqliteStore::open(&pfad, None).unwrap());
    assert_eq!(inspect(&pfad).unwrap(), DbFile::Plaintext);
    std::fs::write(&pfad, [0x8a; 4096]).unwrap();
    assert_eq!(inspect(&pfad).unwrap(), DbFile::Encrypted);
}

#[test]
fn schluessel_sind_zufaellig() {
    let a = doeverything_store::generate_key();
    let b = doeverything_store::generate_key();
    assert_ne!(a, b);
    assert_ne!(a, [0; 32]);
}

#[cfg(feature = "sqlcipher")]
#[test]
fn klartext_datenbank_verschluesseln() {
    use doeverything_store::encrypt_plaintext;

    let dir = tempfile::tempdir().unwrap();
    let pfad = dir.path().join("alt.sqlite");
    let mut r = Replica::open(SqliteStore::open(&pfad, None).unwrap()).unwrap();
    let geraet = r.device();
    let space = r.create_space("Privat", 1000).unwrap();
    r.transact(space, 1000, |tx| {
        tx.set(space, SysField::Name, "MEINE DATEN");
    })
    .unwrap();
    let vorher = r.store().entities(space).unwrap();
    drop(r);

    let schluessel = doeverything_store::generate_key();
    assert!(matches!(
        encrypt_plaintext(dir.path().join("gibtsnicht.sqlite"), &schluessel),
        Err(StoreError::UnexpectedFileState(DbFile::Missing))
    ));
    encrypt_plaintext(&pfad, &schluessel).unwrap();

    assert_eq!(inspect(&pfad).unwrap(), DbFile::Encrypted);
    let roh = std::fs::read(&pfad).unwrap();
    assert!(
        !roh.windows(11).any(|w| w == b"MEINE DATEN"),
        "Klartext in Datei"
    );
    let reste: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "alt.sqlite")
        .collect();
    assert_eq!(
        reste,
        Vec::<String>::new(),
        "keine Klartext-WAL oder Temp-Datei übrig"
    );

    assert!(SqliteStore::open(&pfad, None).is_err());
    let r = Replica::open(SqliteStore::open(&pfad, Some(&schluessel)).unwrap()).unwrap();
    assert_eq!(r.device(), geraet, "Geräte-ID übernommen");
    assert_eq!(
        r.store().entities(space).unwrap(),
        vorher,
        "alle Daten übernommen"
    );

    assert!(matches!(
        encrypt_plaintext(&pfad, &schluessel),
        Err(StoreError::UnexpectedFileState(DbFile::Encrypted))
    ));
}
