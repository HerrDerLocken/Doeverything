//! Konformitätstests für [`Store`]-Implementierungen: die Konfliktfälle aus
//! `docs/design/datenmodell-und-sync.md`, Abschnitt 3.3.
//!
//! Einbinden in einem Test-Modul mit `doeverything_core::konformitaets_tests!(|| MeinStore::neu());`

use crate::order::between;
use crate::{
    ApplyOutcome, DeviceId, EntityId, Kind, ReceiveError, Replica, Store, SysField, Value,
};

/// Erzeugt für jeden Konfliktfall einen `#[test]`, der mit frischen Stores aus `$neuer_store` läuft.
#[macro_export]
macro_rules! konformitaets_tests {
    ($neuer_store:expr) => {
        $crate::konformitaets_tests!(@tests $neuer_store;
            verschiedene_felder_bleiben_beide_erhalten,
            gleiches_feld_spaetere_aenderung_gewinnt,
            loeschen_gewinnt_gegen_bearbeiten_und_aenderung_bleibt_fuer_wiederherstellen,
            feld_umbenennen_beruehrt_keine_werte,
            gleichzeitiges_einsortieren_ist_deterministisch,
            reihenfolge_des_empfangs_ist_egal,
            set_vor_create_wird_nachgeholt,
            duplikate_und_luecken,
            geraet_mit_falscher_uhr_wird_markiert,
            alle_werttypen_bleiben_erhalten,
            spaces_werden_gelistet,
        );
    };
    (@tests $neuer_store:expr; $($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                $crate::testsuite::$name($neuer_store);
            }
        )*
    };
}

/// Synchronisiert zwei Replikas in beide Richtungen (wie über Server oder LAN).
pub fn sync<S: Store>(a: &mut Replica<S>, b: &mut Replica<S>, space: EntityId, now_ms: u64) {
    for cs in a
        .changesets_missing_from(space, &b.version_vector(space).unwrap())
        .unwrap()
    {
        b.receive(&cs, now_ms).unwrap();
    }
    for cs in b
        .changesets_missing_from(space, &a.version_vector(space).unwrap())
        .unwrap()
    {
        a.receive(&cs, now_ms).unwrap();
    }
    assert_eq!(
        a.store().entities(space).unwrap(),
        b.store().entities(space).unwrap(),
        "Replikas müssen nach Sync identisch sein"
    );
}

struct Einkaufsliste {
    space: EntityId,
    liste: EntityId,
    artikel: EntityId,
    menge: EntityId,
    erledigt: EntityId,
}

/// Handy legt eine Einkaufsliste an, Laptop synchronisiert sie.
fn setup<S: Store>(neu: impl Fn() -> S) -> (Replica<S>, Replica<S>, Einkaufsliste) {
    let mut handy = Replica::open(neu()).unwrap();
    let mut laptop = Replica::open(neu()).unwrap();
    let space = handy.create_space("Privat", 1000).unwrap();
    let liste = handy
        .transact(space, 1000, |tx| {
            let liste = tx.create(Kind::Collection, Some(space));
            tx.set(liste, SysField::Name, "Einkauf");
            tx.set(liste, SysField::Template, "shopping@1");
            let mut feld = |name: &str, typ: &str| {
                let f = tx.create(Kind::Field, Some(liste));
                tx.set(f, SysField::Name, name);
                tx.set(f, SysField::Type, typ);
                f
            };
            Einkaufsliste {
                space,
                liste,
                artikel: feld("Artikel", "text"),
                menge: feld("Menge", "int"),
                erledigt: feld("Erledigt", "bool"),
            }
        })
        .unwrap();
    sync(&mut handy, &mut laptop, space, 1000);
    (handy, laptop, liste)
}

fn neuer_artikel<S: Store>(
    r: &mut Replica<S>,
    l: &Einkaufsliste,
    name: &str,
    now_ms: u64,
) -> EntityId {
    r.transact(l.space, now_ms, |tx| {
        let rec = tx.create(Kind::Record, Some(l.liste));
        tx.set(rec, l.artikel, name);
        tx.set(rec, l.menge, 1);
        tx.set(rec, l.erledigt, false);
        rec
    })
    .unwrap()
}

fn wert<S: Store>(r: &Replica<S>, id: EntityId, feld: EntityId) -> Option<Value> {
    r.entity(id).unwrap().and_then(|e| e.get(feld).cloned())
}

pub fn verschiedene_felder_bleiben_beide_erhalten<S: Store>(neu: impl Fn() -> S) {
    let (mut handy, mut laptop, l) = setup(neu);
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);

    // beide offline
    handy
        .transact(l.space, 3000, |tx| tx.set(milch, l.erledigt, true))
        .unwrap();
    laptop
        .transact(l.space, 3000, |tx| tx.set(milch, l.menge, 2))
        .unwrap();
    sync(&mut handy, &mut laptop, l.space, 4000);

    assert_eq!(wert(&handy, milch, l.erledigt), Some(Value::Bool(true)));
    assert_eq!(wert(&handy, milch, l.menge), Some(Value::Int(2)));
}

pub fn gleiches_feld_spaetere_aenderung_gewinnt<S: Store>(neu: impl Fn() -> S) {
    let (mut handy, mut laptop, l) = setup(neu);
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);

    laptop
        .transact(l.space, 3000, |tx| tx.set(milch, l.menge, 2))
        .unwrap();
    handy
        .transact(l.space, 3500, |tx| tx.set(milch, l.menge, 3))
        .unwrap();
    sync(&mut handy, &mut laptop, l.space, 4000);

    assert_eq!(wert(&laptop, milch, l.menge), Some(Value::Int(3)));
}

pub fn loeschen_gewinnt_gegen_bearbeiten_und_aenderung_bleibt_fuer_wiederherstellen<S: Store>(
    neu: impl Fn() -> S,
) {
    let (mut handy, mut laptop, l) = setup(neu);
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);

    handy
        .transact(l.space, 3000, |tx| tx.delete(milch))
        .unwrap();
    laptop
        .transact(l.space, 3500, |tx| tx.set(milch, l.menge, 5))
        .unwrap(); // später als das Löschen
    sync(&mut handy, &mut laptop, l.space, 4000);

    assert_eq!(handy.children(l.liste, Kind::Record).unwrap().len(), 0);
    assert!(handy.entity(milch).unwrap().unwrap().is_deleted());

    handy
        .transact(l.space, 5000, |tx| tx.restore(milch))
        .unwrap();
    sync(&mut handy, &mut laptop, l.space, 5000);
    let sichtbar = laptop.children(l.liste, Kind::Record).unwrap();
    assert_eq!(sichtbar.len(), 1);
    assert_eq!(sichtbar[0].1.get(l.menge), Some(&Value::Int(5)));
}

pub fn feld_umbenennen_beruehrt_keine_werte<S: Store>(neu: impl Fn() -> S) {
    let (mut handy, mut laptop, l) = setup(neu);
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);

    handy
        .transact(l.space, 3000, |tx| {
            tx.set(l.menge, SysField::Name, "Anzahl");
        })
        .unwrap();
    laptop
        .transact(l.space, 3000, |tx| tx.set(milch, l.menge, 4))
        .unwrap();
    sync(&mut handy, &mut laptop, l.space, 4000);

    let feld = handy.entity(l.menge).unwrap().unwrap();
    assert_eq!(feld.get(SysField::Name), Some(&Value::from("Anzahl")));
    assert_eq!(wert(&handy, milch, l.menge), Some(Value::Int(4)));
}

pub fn gleichzeitiges_einsortieren_ist_deterministisch<S: Store>(neu: impl Fn() -> S) {
    let (mut handy, mut laptop, l) = setup(neu);
    let a = neuer_artikel(&mut handy, &l, "A", 2000);
    let c = neuer_artikel(&mut handy, &l, "C", 2000);
    let k_a = between(None, None);
    let k_c = between(Some(&k_a), None);
    handy
        .transact(l.space, 2000, |tx| {
            tx.set(a, SysField::Order, k_a.clone());
            tx.set(c, SysField::Order, k_c.clone());
        })
        .unwrap();
    sync(&mut handy, &mut laptop, l.space, 2000);

    // beide fügen offline einen Artikel zwischen A und C ein → gleicher Schlüssel
    let mitte = between(Some(&k_a), Some(&k_c));
    let b1 = neuer_artikel(&mut handy, &l, "B1", 3000);
    handy
        .transact(l.space, 3000, |tx| {
            tx.set(b1, SysField::Order, mitte.clone());
        })
        .unwrap();
    let b2 = neuer_artikel(&mut laptop, &l, "B2", 3000);
    laptop
        .transact(l.space, 3000, |tx| {
            tx.set(b2, SysField::Order, mitte.clone());
        })
        .unwrap();
    sync(&mut handy, &mut laptop, l.space, 4000);

    let reihenfolge: Vec<_> = handy
        .children(l.liste, Kind::Record)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let (erster, zweiter) = if b1 < b2 { (b1, b2) } else { (b2, b1) };
    assert_eq!(reihenfolge, vec![a, erster, zweiter, c]);
}

pub fn reihenfolge_des_empfangs_ist_egal<S: Store>(neu: impl Fn() -> S) {
    let mut tablet = Replica::open(neu()).unwrap();
    let (mut handy, mut laptop, l) = setup(neu);
    sync(&mut handy, &mut tablet, l.space, 1000);

    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);
    sync(&mut handy, &mut tablet, l.space, 2000);
    handy
        .transact(l.space, 3000, |tx| tx.set(milch, l.menge, 2))
        .unwrap();
    laptop
        .transact(l.space, 3100, |tx| tx.set(milch, l.artikel, "Hafermilch"))
        .unwrap();
    tablet
        .transact(l.space, 3200, |tx| tx.set(milch, l.menge, 3))
        .unwrap();

    // Ringförmig synchronisieren statt sternförmig
    sync(&mut tablet, &mut laptop, l.space, 4000);
    sync(&mut laptop, &mut handy, l.space, 4000);
    sync(&mut handy, &mut tablet, l.space, 4000);

    assert_eq!(
        handy.store().entities(l.space).unwrap(),
        laptop.store().entities(l.space).unwrap()
    );
    assert_eq!(wert(&tablet, milch, l.menge), Some(Value::Int(3)));
    assert_eq!(
        wert(&tablet, milch, l.artikel),
        Some(Value::from("Hafermilch"))
    );
}

pub fn set_vor_create_wird_nachgeholt<S: Store>(neu: impl Fn() -> S) {
    let (mut handy, mut laptop, l) = setup(neu);
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    handy
        .transact(l.space, 2100, |tx| tx.set(milch, l.menge, 7))
        .unwrap();
    let css = handy
        .changesets_missing_from(l.space, &laptop.version_vector(l.space).unwrap())
        .unwrap();

    // Das Set kommt über ein anderes Gerät an, bevor das Create eintrifft
    let mut nur_set = css[1].clone();
    nur_set.id.seq = 1;
    nur_set.id.device = DeviceId::new();
    laptop.receive(&nur_set, 2200).unwrap();
    assert!(
        laptop.entity(milch).unwrap().is_none(),
        "ohne Create noch nicht sichtbar"
    );

    for cs in css {
        laptop.receive(&cs, 2200).unwrap();
    }
    assert_eq!(wert(&laptop, milch, l.menge), Some(Value::Int(7)));
}

pub fn duplikate_und_luecken<S: Store>(neu: impl Fn() -> S) {
    let (mut handy, mut laptop, l) = setup(neu);
    neuer_artikel(&mut handy, &l, "A", 2000);
    neuer_artikel(&mut handy, &l, "B", 2000);
    let css = handy
        .changesets_missing_from(l.space, &laptop.version_vector(l.space).unwrap())
        .unwrap();
    assert_eq!(css.len(), 2);

    let (erwartet, bekommen) = (css[0].id.seq, css[1].id.seq);
    assert!(matches!(
        laptop.receive(&css[1], 2000),
        Err(ReceiveError::Gap { expected, got }) if expected == erwartet && got == bekommen
    ));
    assert!(matches!(
        laptop.receive(&css[0], 2000),
        Ok(ApplyOutcome::Applied)
    ));
    assert!(matches!(
        laptop.receive(&css[0], 2000),
        Ok(ApplyOutcome::Duplicate)
    ));
    assert!(matches!(
        laptop.receive(&css[1], 2000),
        Ok(ApplyOutcome::Applied)
    ));
    assert_eq!(
        handy.store().entities(l.space).unwrap(),
        laptop.store().entities(l.space).unwrap()
    );
}

pub fn geraet_mit_falscher_uhr_wird_markiert<S: Store>(neu: impl Fn() -> S) {
    let (mut handy, mut laptop, l) = setup(neu);
    let ein_jahr = 365 * 24 * 60 * 60 * 1000;
    neuer_artikel(&mut handy, &l, "Zukunft", ein_jahr);
    for cs in handy
        .changesets_missing_from(l.space, &laptop.version_vector(l.space).unwrap())
        .unwrap()
    {
        laptop.receive(&cs, 2000).unwrap();
    }
    assert!(laptop.drifted_devices().contains(&handy.device()));
}

pub fn alle_werttypen_bleiben_erhalten<S: Store>(neu: impl Fn() -> S) {
    let (mut handy, mut laptop, l) = setup(neu);
    let werte = vec![
        Value::Null,
        Value::Bool(true),
        Value::Int(i64::MIN),
        Value::Decimal {
            mantissa: i128::MAX,
            scale: 4,
        },
        Value::Decimal {
            mantissa: -125,
            scale: 2,
        },
        Value::Money {
            minor: -1234,
            currency: *b"EUR",
        },
        Value::Text("Grüße 🛒".into()),
        Value::Date {
            y: 2026,
            m: 10,
            d: 30,
        },
        Value::Instant { unix_ms: -1 },
        Value::Duration { ms: 90_000 },
        Value::Enum(EntityId::new()),
        Value::MultiEnum([EntityId::new(), EntityId::new()].into_iter().collect()),
        Value::Ref(EntityId::new()),
        Value::Blob {
            hash: [7; 32],
            size: 1024,
            mime: "image/png".into(),
        },
        Value::Json(serde_json::json!({ "gruppe": "Kategorie", "sort": [1, 2.5, null] })),
    ];
    let rec = handy
        .transact(l.space, 2000, |tx| {
            let rec = tx.create(Kind::Record, Some(l.liste));
            for (i, w) in werte.iter().enumerate() {
                let feld = tx.create(Kind::Field, Some(l.liste));
                tx.set(feld, SysField::Name, format!("Feld {i}"));
                tx.set(rec, feld, w.clone());
            }
            rec
        })
        .unwrap();
    sync(&mut handy, &mut laptop, l.space, 2000);

    let e = laptop.entity(rec).unwrap().unwrap();
    let mut gespeichert: Vec<_> = e.fields.values().map(|v| v.value.clone()).collect();
    let mut erwartet = werte;
    let schluessel = |v: &Value| format!("{v:?}");
    gespeichert.sort_by_key(schluessel);
    erwartet.sort_by_key(schluessel);
    assert_eq!(gespeichert, erwartet);
}

pub fn spaces_werden_gelistet<S: Store>(neu: impl Fn() -> S) {
    let mut r = Replica::open(neu()).unwrap();
    assert_eq!(r.store().spaces().unwrap(), []);
    let privat = r.create_space("Privat", 1000).unwrap();
    let familie = r.create_space("Familie", 1001).unwrap();
    assert_eq!(r.store().spaces().unwrap(), [privat, familie]);
    r.transact(familie, 2000, |tx| tx.delete(familie)).unwrap();
    assert_eq!(r.store().spaces().unwrap(), [privat]);
}
