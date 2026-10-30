//! Die Konfliktfälle aus `docs/design/datenmodell-und-sync.md`, Abschnitt 3.3.

use doeverything_core::order::between;
use doeverything_core::state::ApplyError;
use doeverything_core::{ApplyOutcome, DeviceId, EntityId, Kind, Replica, SysField, Value};

/// Synchronisiert zwei Replikas in beide Richtungen (wie über Server oder LAN).
fn sync(a: &mut Replica, b: &mut Replica, space: EntityId, now_ms: u64) {
    for cs in a.changesets_missing_from(space, &b.version_vector(space)) {
        b.receive(cs, now_ms).unwrap();
    }
    for cs in b.changesets_missing_from(space, &a.version_vector(space)) {
        a.receive(cs, now_ms).unwrap();
    }
    assert_eq!(
        a.state(),
        b.state(),
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
fn setup() -> (Replica, Replica, Einkaufsliste) {
    let mut handy = Replica::new(DeviceId::new());
    let mut laptop = Replica::new(DeviceId::new());
    let space = handy.create_space("Privat", 1000);
    let liste = handy.transact(space, 1000, |tx| {
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
    });
    sync(&mut handy, &mut laptop, space, 1000);
    (handy, laptop, liste)
}

fn neuer_artikel(r: &mut Replica, l: &Einkaufsliste, name: &str, now_ms: u64) -> EntityId {
    r.transact(l.space, now_ms, |tx| {
        let rec = tx.create(Kind::Record, Some(l.liste));
        tx.set(rec, l.artikel, name);
        tx.set(rec, l.menge, 1);
        tx.set(rec, l.erledigt, false);
        rec
    })
}

#[test]
fn verschiedene_felder_bleiben_beide_erhalten() {
    let (mut handy, mut laptop, l) = setup();
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);

    // beide offline
    handy.transact(l.space, 3000, |tx| tx.set(milch, l.erledigt, true));
    laptop.transact(l.space, 3000, |tx| tx.set(milch, l.menge, 2));
    sync(&mut handy, &mut laptop, l.space, 4000);

    let e = handy.state().entity(milch).unwrap();
    assert_eq!(e.get(l.erledigt), Some(&Value::Bool(true)));
    assert_eq!(e.get(l.menge), Some(&Value::Int(2)));
}

#[test]
fn gleiches_feld_spaetere_aenderung_gewinnt() {
    let (mut handy, mut laptop, l) = setup();
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);

    laptop.transact(l.space, 3000, |tx| tx.set(milch, l.menge, 2));
    handy.transact(l.space, 3500, |tx| tx.set(milch, l.menge, 3));
    sync(&mut handy, &mut laptop, l.space, 4000);

    assert_eq!(
        laptop.state().entity(milch).unwrap().get(l.menge),
        Some(&Value::Int(3))
    );
}

#[test]
fn loeschen_gewinnt_gegen_bearbeiten_und_aenderung_bleibt_fuer_wiederherstellen() {
    let (mut handy, mut laptop, l) = setup();
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);

    handy.transact(l.space, 3000, |tx| tx.delete(milch));
    laptop.transact(l.space, 3500, |tx| tx.set(milch, l.menge, 5)); // später als das Löschen
    sync(&mut handy, &mut laptop, l.space, 4000);

    assert_eq!(handy.state().children(l.liste, Kind::Record).len(), 0);

    handy.transact(l.space, 5000, |tx| tx.restore(milch));
    sync(&mut handy, &mut laptop, l.space, 5000);
    let sichtbar = laptop.state().children(l.liste, Kind::Record);
    assert_eq!(sichtbar.len(), 1);
    assert_eq!(sichtbar[0].1.get(l.menge), Some(&Value::Int(5)));
}

#[test]
fn feld_umbenennen_beruehrt_keine_werte() {
    let (mut handy, mut laptop, l) = setup();
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);

    handy.transact(l.space, 3000, |tx| {
        tx.set(l.menge, SysField::Name, "Anzahl");
    });
    laptop.transact(l.space, 3000, |tx| tx.set(milch, l.menge, 4));
    sync(&mut handy, &mut laptop, l.space, 4000);

    let feld = handy.state().entity(l.menge).unwrap();
    assert_eq!(feld.get(SysField::Name), Some(&Value::from("Anzahl")));
    assert_eq!(
        handy.state().entity(milch).unwrap().get(l.menge),
        Some(&Value::Int(4))
    );
}

#[test]
fn gleichzeitiges_einsortieren_ist_deterministisch() {
    let (mut handy, mut laptop, l) = setup();
    let a = neuer_artikel(&mut handy, &l, "A", 2000);
    let c = neuer_artikel(&mut handy, &l, "C", 2000);
    let k_a = between(None, None);
    let k_c = between(Some(&k_a), None);
    handy.transact(l.space, 2000, |tx| {
        tx.set(a, SysField::Order, k_a.clone());
        tx.set(c, SysField::Order, k_c.clone());
    });
    sync(&mut handy, &mut laptop, l.space, 2000);

    // beide fügen offline einen Artikel zwischen A und C ein → gleicher Schlüssel
    let mitte = between(Some(&k_a), Some(&k_c));
    let b1 = neuer_artikel(&mut handy, &l, "B1", 3000);
    handy.transact(l.space, 3000, |tx| {
        tx.set(b1, SysField::Order, mitte.clone());
    });
    let b2 = neuer_artikel(&mut laptop, &l, "B2", 3000);
    laptop.transact(l.space, 3000, |tx| {
        tx.set(b2, SysField::Order, mitte.clone());
    });
    sync(&mut handy, &mut laptop, l.space, 4000);

    let reihenfolge: Vec<_> = handy
        .state()
        .children(l.liste, Kind::Record)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let (erster, zweiter) = if b1 < b2 { (b1, b2) } else { (b2, b1) };
    assert_eq!(reihenfolge, vec![a, erster, zweiter, c]);
}

#[test]
fn reihenfolge_des_empfangs_ist_egal() {
    let (mut handy, mut laptop, l) = setup();
    let mut tablet = Replica::new(DeviceId::new());
    sync(&mut handy, &mut tablet, l.space, 1000);

    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    sync(&mut handy, &mut laptop, l.space, 2000);
    sync(&mut handy, &mut tablet, l.space, 2000);
    handy.transact(l.space, 3000, |tx| tx.set(milch, l.menge, 2));
    laptop.transact(l.space, 3100, |tx| tx.set(milch, l.artikel, "Hafermilch"));
    tablet.transact(l.space, 3200, |tx| tx.set(milch, l.menge, 3));

    // Ringförmig synchronisieren statt sternförmig
    sync(&mut tablet, &mut laptop, l.space, 4000);
    sync(&mut laptop, &mut handy, l.space, 4000);
    sync(&mut handy, &mut tablet, l.space, 4000);

    assert_eq!(handy.state(), laptop.state());
    let e = tablet.state().entity(milch).unwrap();
    assert_eq!(e.get(l.menge), Some(&Value::Int(3)));
    assert_eq!(e.get(l.artikel), Some(&Value::from("Hafermilch")));
}

#[test]
fn set_vor_create_wird_nachgeholt() {
    let (mut handy, mut laptop, l) = setup();
    let milch = neuer_artikel(&mut handy, &l, "Milch", 2000);
    handy.transact(l.space, 2100, |tx| tx.set(milch, l.menge, 7));
    let css = handy.changesets_missing_from(l.space, &laptop.version_vector(l.space));

    // Das Set kommt über ein anderes Gerät an, bevor das Create eintrifft
    let mut nur_set = css[1].clone();
    nur_set.id.seq = 1;
    nur_set.id.device = DeviceId::new();
    laptop.receive(nur_set, 2200).unwrap();
    assert!(
        laptop.state().entity(milch).is_none(),
        "ohne Create noch nicht sichtbar"
    );

    for cs in css {
        laptop.receive(cs, 2200).unwrap();
    }
    assert_eq!(
        laptop.state().entity(milch).unwrap().get(l.menge),
        Some(&Value::Int(7))
    );
}

#[test]
fn duplikate_und_luecken() {
    let (mut handy, mut laptop, l) = setup();
    neuer_artikel(&mut handy, &l, "A", 2000);
    neuer_artikel(&mut handy, &l, "B", 2000);
    let css = handy.changesets_missing_from(l.space, &laptop.version_vector(l.space));
    assert_eq!(css.len(), 2);

    assert_eq!(
        laptop.receive(css[1].clone(), 2000),
        Err(ApplyError::Gap {
            expected: css[0].id.seq,
            got: css[1].id.seq
        })
    );
    assert_eq!(
        laptop.receive(css[0].clone(), 2000),
        Ok(ApplyOutcome::Applied)
    );
    assert_eq!(
        laptop.receive(css[0].clone(), 2000),
        Ok(ApplyOutcome::Duplicate)
    );
    assert_eq!(
        laptop.receive(css[1].clone(), 2000),
        Ok(ApplyOutcome::Applied)
    );
    assert_eq!(handy.state(), laptop.state());
}

#[test]
fn geraet_mit_falscher_uhr_wird_markiert() {
    let (mut handy, mut laptop, l) = setup();
    let ein_jahr = 365 * 24 * 60 * 60 * 1000;
    neuer_artikel(&mut handy, &l, "Zukunft", ein_jahr);
    for cs in handy.changesets_missing_from(l.space, &laptop.version_vector(l.space)) {
        laptop.receive(cs, 2000).unwrap();
    }
    assert!(laptop.drifted_devices().contains(&handy.device()));
}
