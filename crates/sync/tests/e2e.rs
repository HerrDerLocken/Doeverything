//! Ende-zu-Ende: echte Server-Instanz (axum + SQLite) im Testprozess, Geräte als Replikas.

use std::path::PathBuf;
use std::sync::Mutex;

use doeverything_core::templates::{self, Role, field_with_role};
use doeverything_core::{
    EntityId, Kind, MemoryStore, Replica, Store, SysField, Value, VersionVector,
};
use doeverything_proto::api::{AccountId, DeviceInfo, PushRequest, PushResponse, RegisterDevice};
use doeverything_proto::crypto::{self, DeviceKeys, Identity};
use doeverything_server::{AppState, serve};
use doeverything_sync::{self as sync, Client, SyncError, SyncStatus};

type Device = Mutex<Replica<MemoryStore>>;

fn device() -> Device {
    Mutex::new(Replica::open(MemoryStore::new()).unwrap())
}

/// Startet einen Server auf einem freien Port und gibt seine Adresse zurück.
fn server_with(state: AppState) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            serve(listener, state, std::future::pending())
                .await
                .unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

fn server() -> String {
    server_with(AppState::in_memory(true).unwrap())
}

/// Koppelt `neu` an den Account von `bestehend`.
fn koppeln(server: &str, bestehend: &Device, neu: &Device) {
    let code = sync::start_pairing(neu, server, "Laptop").unwrap();
    assert!(matches!(
        sync::status(neu).unwrap(),
        SyncStatus::Pairing { .. }
    ));
    assert!(
        !sync::finish_pairing(neu).unwrap(),
        "vor der Bestätigung nicht fertig"
    );
    assert_eq!(sync::approve_pairing(bestehend, &code).unwrap(), "Laptop");
    assert!(sync::finish_pairing(neu).unwrap());
    assert!(matches!(
        sync::status(neu).unwrap(),
        SyncStatus::Connected { .. }
    ));
}

fn einkaufsliste(d: &Device) -> (EntityId, EntityId, EntityId) {
    let mut r = d.lock().unwrap();
    let space = r.create_space("Privat", 1000).unwrap();
    let liste = r
        .transact(space, 1000, |tx| {
            templates::instantiate(tx, space, &templates::SHOPPING, "Einkauf", None)
        })
        .unwrap();
    let titel = field_with_role(&r.children(liste, Kind::Field).unwrap(), Role::Title).unwrap();
    (space, liste, titel)
}

fn artikel(d: &Device, space: EntityId, liste: EntityId, titel: EntityId, name: &str) -> EntityId {
    d.lock()
        .unwrap()
        .transact(space, doeverything_core::hlc::now_ms(), |tx| {
            let rec = tx.create(Kind::Record, Some(liste));
            tx.set(rec, titel, name);
            rec
        })
        .unwrap()
}

fn titel_der_artikel(d: &Device, liste: EntityId, titel: EntityId) -> Vec<String> {
    let mut namen: Vec<_> = d
        .lock()
        .unwrap()
        .children(liste, Kind::Record)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| e.get(titel).and_then(Value::as_text).map(str::to_owned))
        .collect();
    namen.sort();
    namen
}

fn gleicher_stand(a: &Device, b: &Device, space: EntityId) {
    assert_eq!(
        a.lock().unwrap().store().entities(space).unwrap(),
        b.lock().unwrap().store().entities(space).unwrap()
    );
}

#[test]
fn konto_anlegen_koppeln_und_synchronisieren() {
    let server = server();
    let handy = device();
    let laptop = device();
    assert_eq!(sync::status(&handy).unwrap(), SyncStatus::Off);
    assert!(matches!(sync::sync(&handy), Err(SyncError::NotConfigured)));

    let (space, liste, titel) = einkaufsliste(&handy);
    artikel(&handy, space, liste, titel, "Milch");
    sync::create_account(&handy, &format!("{server}/"), "Handy").unwrap();
    assert!(matches!(
        sync::create_account(&handy, &server, "Handy"),
        Err(SyncError::AlreadyConfigured)
    ));
    let bericht = sync::sync(&handy).unwrap();
    assert_eq!(bericht.pushed, 3, "Space, Liste, Artikel");

    koppeln(&server, &handy, &laptop);
    let bericht = sync::sync(&laptop).unwrap();
    assert_eq!(bericht.pulled, 3);
    assert_eq!(titel_der_artikel(&laptop, liste, titel), ["Milch"]);

    artikel(&laptop, space, liste, titel, "Brot");
    sync::sync(&laptop).unwrap();
    let bericht = sync::sync(&handy).unwrap();
    assert_eq!((bericht.pulled, bericht.pushed), (1, 0));
    assert_eq!(titel_der_artikel(&handy, liste, titel), ["Brot", "Milch"]);
    gleicher_stand(&handy, &laptop, space);

    // Nichts Neues: leerer Durchgang
    assert_eq!(sync::sync(&handy).unwrap(), sync::SyncReport::default());
}

#[test]
fn gleichzeitige_offline_aenderungen_konvergieren() {
    let server = server();
    let (a, b, c) = (device(), device(), device());
    let (space, liste, titel) = einkaufsliste(&a);
    sync::create_account(&a, &server, "A").unwrap();
    koppeln(&server, &a, &b);
    koppeln(&server, &a, &c);
    sync::sync(&a).unwrap();
    sync::sync(&b).unwrap();
    sync::sync(&c).unwrap();

    let milch = artikel(&a, space, liste, titel, "Milch");
    sync::sync(&a).unwrap();
    sync::sync(&b).unwrap();
    sync::sync(&c).unwrap();

    // Alle drei ändern offline
    b.lock()
        .unwrap()
        .transact(space, 5000, |tx| tx.set(milch, titel, "Hafermilch"))
        .unwrap();
    c.lock()
        .unwrap()
        .transact(space, 6000, |tx| tx.delete(milch))
        .unwrap();
    artikel(&a, space, liste, titel, "Eier");

    for _ in 0..2 {
        for d in [&a, &b, &c] {
            sync::sync(d).unwrap();
        }
    }
    gleicher_stand(&a, &b, space);
    gleicher_stand(&b, &c, space);
    assert_eq!(
        titel_der_artikel(&b, liste, titel),
        ["Eier"],
        "Milch gelöscht"
    );
    let geloescht = b.lock().unwrap().entity(milch).unwrap().unwrap();
    assert_eq!(
        geloescht.get(titel),
        Some(&Value::from("Hafermilch")),
        "Änderung bleibt für Wiederherstellen"
    );
}

#[test]
fn server_speichert_keinen_klartext() {
    let dir = tempfile::tempdir().unwrap();
    let db: PathBuf = dir.path().join("server.sqlite");
    let server = server_with(AppState::open(&db, true).unwrap());
    let handy = device();
    let (space, liste, titel) = einkaufsliste(&handy);
    artikel(&handy, space, liste, titel, "STRENG GEHEIMER ARTIKEL");
    sync::create_account(&handy, &server, "Handy").unwrap();
    sync::sync(&handy).unwrap();

    let mut bytes = std::fs::read(&db).unwrap();
    if let Ok(wal) = std::fs::read(dir.path().join("server.sqlite-wal")) {
        bytes.extend(wal);
    }
    for klartext in [&b"STRENG GEHEIM"[..], b"Einkauf", b"Artikel", b"shopping@1"] {
        assert!(
            !bytes.windows(klartext.len()).any(|w| w == klartext),
            "{} im Server gefunden",
            String::from_utf8_lossy(klartext)
        );
    }
}

#[test]
fn fremder_account_sieht_nichts() {
    let server = server();
    let alice = device();
    let mallory = device();
    let (space, liste, titel) = einkaufsliste(&alice);
    artikel(&alice, space, liste, titel, "Geschenk");
    sync::create_account(&alice, &server, "Alice").unwrap();
    sync::sync(&alice).unwrap();
    sync::create_account(&mallory, &server, "Mallory").unwrap();
    sync::sync(&mallory).unwrap();
    assert!(mallory.lock().unwrap().entity(liste).unwrap().is_none());

    // Direkt über die API: fremden Space lesen oder beschreiben
    let mut client = Client::new(&server);
    let keys = mallory_keys(&mallory);
    client
        .login(mallory.lock().unwrap().device(), &keys)
        .unwrap();
    let err = client
        .get::<doeverything_proto::api::PullResponse>(&format!("/v1/spaces/{space}/pull"))
        .unwrap_err();
    assert!(
        matches!(err, SyncError::Server { status: 403, .. }),
        "{err}"
    );
}

fn meta32(d: &Device, key: &str) -> [u8; 32] {
    d.lock()
        .unwrap()
        .store()
        .meta(key)
        .unwrap()
        .unwrap()
        .try_into()
        .unwrap()
}

/// Liest die Geräteschlüssel aus dem Speicher (nur im Test).
fn mallory_keys(d: &Device) -> DeviceKeys {
    DeviceKeys::from_bytes(
        meta32(d, "sync.signing_key"),
        meta32(d, "sync.exchange_key"),
    )
}

fn account_id(d: &Device) -> AccountId {
    AccountId(
        d.lock()
            .unwrap()
            .store()
            .meta("sync.account")
            .unwrap()
            .unwrap()
            .try_into()
            .unwrap(),
    )
}

/// Ein manipulierter Server trägt ein Gerät in die Geräteliste ein, das niemand gekoppelt hat.
/// Selbst wenn es den Space-Schlüssel kennte: Ohne Zertifikat der Account-Identität werden
/// seine Changesets nicht übernommen, sondern zurückgestellt.
#[test]
fn untergeschobenes_geraet_ohne_zertifikat() {
    let server = server();
    let handy = device();
    let laptop = device();
    let (space, liste, titel) = einkaufsliste(&handy);
    artikel(&handy, space, liste, titel, "Milch");
    sync::create_account(&handy, &server, "Handy").unwrap();
    sync::sync(&handy).unwrap();
    koppeln(&server, &handy, &laptop);

    // Das Gerät wird über die API eingetragen, so wie es der Server auch selbst könnte
    let fremd = device();
    let fremd_keys = DeviceKeys::generate();
    let fremd_info = DeviceInfo {
        device: fremd.lock().unwrap().device(),
        signing_pub: fremd_keys.signing_public(),
        name: "Unbekannt".into(),
    };
    let handy_keys = mallory_keys(&handy);
    let handy_device = handy.lock().unwrap().device();
    let mut handy_client = Client::new(&server);
    handy_client.login(handy_device, &handy_keys).unwrap();
    handy_client
        .post::<_, ()>(
            "/v1/devices",
            &RegisterDevice {
                proof: crypto::device_proof(&fremd_keys, &fremd_info),
                device: fremd_info.clone(),
            },
        )
        .ok();

    // Schlimmster Fall: Es kennt sogar den Space-Schlüssel
    let ark = meta32(&handy, "sync.account_key");
    artikel(&fremd, space, liste, titel, "Untergeschoben");
    let cs = fremd
        .lock()
        .unwrap()
        .changesets_missing_from(space, &VersionVector::default())
        .unwrap();
    let envelopes = cs
        .iter()
        .map(|cs| {
            crypto::seal_changeset(
                cs,
                &crypto::space_key(&ark, space, 0),
                0,
                &fremd_keys.signing,
            )
            .unwrap()
        })
        .collect();
    let mut fremd_client = Client::new(&server);
    fremd_client.login(fremd_info.device, &fremd_keys).unwrap();
    fremd_client
        .post::<_, PushResponse>(
            &format!("/v1/spaces/{space}/push"),
            &PushRequest { envelopes },
        )
        .unwrap();

    let bericht = sync::sync(&laptop).unwrap();
    assert_eq!((bericht.pulled, bericht.deferred), (3, 1));
    assert_eq!(titel_der_artikel(&laptop, liste, titel), ["Milch"]);

    // Zertifikat einer fremden Identität hilft nicht
    let falsch = crypto::issue_cert(
        &Identity::from_account_key(&crypto::random()),
        account_id(&handy),
        fremd_info.device,
        &fremd_keys,
        0,
    );
    fremd_client
        .put(&format!("/v1/devices/{}/cert", fremd_info.device), &falsch)
        .unwrap();
    let bericht = sync::sync(&laptop).unwrap();
    assert_eq!(
        (bericht.pulled, bericht.deferred),
        (0, 1),
        "erneut geprüft, weiter zurückgestellt"
    );
    assert_eq!(titel_der_artikel(&laptop, liste, titel), ["Milch"]);

    // Mit gültigem Zertifikat (nur mit dem Account-Schlüssel möglich) wird es übernommen
    let echt = crypto::issue_cert(
        &Identity::from_account_key(&ark),
        account_id(&handy),
        fremd_info.device,
        &fremd_keys,
        0,
    );
    fremd_client
        .put(&format!("/v1/devices/{}/cert", fremd_info.device), &echt)
        .unwrap();
    let bericht = sync::sync(&laptop).unwrap();
    assert_eq!((bericht.pulled, bericht.deferred), (1, 0));
    assert_eq!(
        titel_der_artikel(&laptop, liste, titel),
        ["Milch", "Untergeschoben"]
    );
    assert_eq!(sync::sync(&laptop).unwrap(), sync::SyncReport::default());
}

#[test]
fn sicherheitsnummer_gleich_auf_allen_geraeten() {
    let server = server();
    let (handy, laptop, fremd) = (device(), device(), device());
    sync::create_account(&handy, &server, "Handy").unwrap();
    koppeln(&server, &handy, &laptop);
    sync::create_account(&fremd, &server, "Fremd").unwrap();
    let nummer = |d: &Device| match sync::status(d).unwrap() {
        SyncStatus::Connected { safety_number, .. } => safety_number,
        other => panic!("{other:?}"),
    };
    assert_eq!(nummer(&handy), nummer(&laptop));
    assert_ne!(nummer(&handy), nummer(&fremd));
}

#[test]
fn kopplungscode_pruefungen() {
    let server_a = server();
    let server_b = server();
    let a = device();
    let b = device();
    sync::create_account(&a, &server_a, "A").unwrap();

    assert!(matches!(
        sync::approve_pairing(&a, "Quatsch"),
        Err(SyncError::Crypto(_))
    ));
    let code = sync::start_pairing(&b, &server_b, "B").unwrap();
    assert!(
        matches!(
            sync::approve_pairing(&a, &code),
            Err(SyncError::InvalidInput(_))
        ),
        "anderer Server"
    );

    // Eigener Code des Geräts wird abgelehnt
    let c = device();
    let eigener = sync::start_pairing(&c, &server_a, "C").unwrap();
    sync::disconnect(&c).unwrap();
    assert_eq!(sync::status(&c).unwrap(), SyncStatus::Off);
    assert!(matches!(
        sync::finish_pairing(&c),
        Err(SyncError::NotConfigured)
    ));
    let _ = eigener;
}

#[test]
fn geschlossene_registrierung() {
    let server = server_with(AppState::in_memory(false).unwrap());
    let a = device();
    let err = sync::create_account(&a, &server, "A").unwrap_err();
    assert!(
        matches!(err, SyncError::Server { status: 403, .. }),
        "{err}"
    );
    assert_eq!(
        sync::status(&a).unwrap(),
        SyncStatus::Off,
        "nichts gespeichert"
    );
}

#[test]
fn server_nicht_erreichbar() {
    let a = device();
    let err = sync::create_account(&a, "http://127.0.0.1:9", "A").unwrap_err();
    assert!(matches!(err, SyncError::Network(_)), "{err}");
    assert!(matches!(
        sync::create_account(&a, "ftp://x", "A"),
        Err(SyncError::InvalidInput(_))
    ));
}

#[test]
fn viele_aenderungen_in_mehreren_seiten() {
    let server = server();
    let a = device();
    let b = device();
    let (space, liste, titel) = einkaufsliste(&a);
    for i in 0..1200 {
        artikel(&a, space, liste, titel, &format!("Artikel {i:04}"));
    }
    sync::create_account(&a, &server, "A").unwrap();
    assert_eq!(sync::sync(&a).unwrap().pushed, 1202);
    koppeln(&server, &a, &b);
    assert_eq!(
        sync::sync(&b).unwrap().pulled,
        1202,
        "über mehrere Pull-Seiten"
    );
    gleicher_stand(&a, &b, space);
    let _ = SysField::Name;
}

/// Gegen einen laufenden Server, z. B. den Container:
/// `DOEVERYTHING_TEST_SERVER=http://localhost:8080 cargo test -p doeverything-sync -- --ignored`
#[test]
#[ignore = "braucht laufenden Server in DOEVERYTHING_TEST_SERVER"]
fn gegen_externen_server() {
    let server =
        std::env::var("DOEVERYTHING_TEST_SERVER").expect("DOEVERYTHING_TEST_SERVER setzen");
    let handy = device();
    let laptop = device();
    let (space, liste, titel) = einkaufsliste(&handy);
    artikel(&handy, space, liste, titel, "Milch");
    sync::create_account(&handy, &server, "Handy").unwrap();
    sync::sync(&handy).unwrap();
    koppeln(&server, &handy, &laptop);
    sync::sync(&laptop).unwrap();
    artikel(&laptop, space, liste, titel, "Brot");
    sync::sync(&laptop).unwrap();
    sync::sync(&handy).unwrap();
    assert_eq!(titel_der_artikel(&handy, liste, titel), ["Brot", "Milch"]);
    gleicher_stand(&handy, &laptop, space);
}
