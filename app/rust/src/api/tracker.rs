//! Schnittstelle für die Flutter-App. flutter_rust_bridge erzeugt daraus `app/lib/src/rust/`
//! (`flutter_rust_bridge_codegen generate` im Ordner `app/`).
//!
//! IDs werden als Text übergeben, damit Dart nichts über ihr Binärformat wissen muss.

use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use doeverything_core::hlc::now_ms;
use doeverything_core::templates::{self, field_with_role, Role};
use doeverything_core::{Entity, EntityId, FieldKey, Kind, Replica, Store, SysField, Value};
use doeverything_store::SqliteStore;
use flutter_rust_bridge::frb;

#[frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

pub struct TemplateInfo {
    pub id: String,
    pub name: String,
}

pub struct CollectionInfo {
    pub id: String,
    pub name: String,
    pub template: String,
    pub open_count: u32,
    pub total_count: u32,
}

pub struct ItemInfo {
    pub id: String,
    pub title: String,
    pub done: bool,
    pub quantity: Option<String>,
}

/// Verfügbare Modul-Vorlagen für "Neue Liste".
#[frb(sync)]
pub fn templates() -> Vec<TemplateInfo> {
    templates::ALL
        .iter()
        .map(|t| TemplateInfo {
            id: t.id.to_owned(),
            name: t.name.to_owned(),
        })
        .collect()
}

#[frb(opaque)]
pub struct Tracker {
    inner: Mutex<Inner>,
}

struct Inner {
    replica: Replica<SqliteStore>,
    space: EntityId,
}

/// Felder einer Collection, über ihre Rolle aufgelöst.
struct Fields {
    title: Option<EntityId>,
    done: Option<EntityId>,
    quantity: Option<EntityId>,
}

impl Fields {
    fn is_done(&self, item: &Entity) -> bool {
        self.done.and_then(|f| item.get(f)).and_then(Value::as_bool) == Some(true)
    }
}

impl Tracker {
    /// Öffnet oder erzeugt die Datenbank. Beim ersten Start wird der Space "Privat" angelegt.
    pub fn open(db_path: String) -> Result<Tracker> {
        let store = SqliteStore::open(&db_path, None)
            .with_context(|| format!("Datenbank {db_path} öffnen"))?;
        let mut replica = Replica::open(store)?;
        let space = match replica.store().spaces()?.first() {
            Some(space) => *space,
            None => replica.create_space("Privat", now_ms())?,
        };
        Ok(Self {
            inner: Mutex::new(Inner { replica, space }),
        })
    }

    pub fn collections(&self) -> Result<Vec<CollectionInfo>> {
        self.with(|i| {
            let mut out = Vec::new();
            for (id, c) in i.replica.children(i.space, Kind::Collection)? {
                let fields = i.fields(id)?;
                let items = i.replica.children(id, Kind::Record)?;
                let open = items.iter().filter(|(_, e)| !fields.is_done(e)).count();
                out.push(CollectionInfo {
                    id: id.to_string(),
                    name: text(&c, SysField::Name),
                    template: text(&c, SysField::Template),
                    open_count: count(open),
                    total_count: count(items.len()),
                });
            }
            Ok(out)
        })
    }

    pub fn create_collection(&self, name: String, template_id: String) -> Result<String> {
        let template = templates::by_id(&template_id)
            .ok_or_else(|| anyhow!("unbekannte Vorlage {template_id}"))?;
        self.with(|i| {
            let space = i.space;
            let last = i
                .replica
                .children(space, Kind::Collection)?
                .last()
                .map(|(_, c)| text(c, SysField::Order));
            let id = i.replica.transact(space, now_ms(), |tx| {
                templates::instantiate(
                    tx,
                    space,
                    template,
                    &name,
                    last.as_deref().filter(|o| !o.is_empty()),
                )
            })?;
            Ok(id.to_string())
        })
    }

    pub fn rename_collection(&self, collection_id: String, name: String) -> Result<()> {
        let id = parse(&collection_id)?;
        self.with(|i| i.set(id, SysField::Name, Value::Text(name)))
    }

    pub fn delete_collection(&self, collection_id: String) -> Result<()> {
        let id = parse(&collection_id)?;
        self.with(|i| i.set(id, SysField::Deleted, Value::Bool(true)))
    }

    pub fn restore_collection(&self, collection_id: String) -> Result<()> {
        let id = parse(&collection_id)?;
        self.with(|i| i.set(id, SysField::Deleted, Value::Bool(false)))
    }

    /// Einträge: offene zuerst, innerhalb der Gruppen in manueller Reihenfolge.
    pub fn items(&self, collection_id: String) -> Result<Vec<ItemInfo>> {
        let collection = parse(&collection_id)?;
        self.with(|i| {
            let fields = i.fields(collection)?;
            let mut items: Vec<_> = i
                .replica
                .children(collection, Kind::Record)?
                .into_iter()
                .map(|(id, e)| ItemInfo {
                    id: id.to_string(),
                    title: fields.title.map(|f| text(&e, f)).unwrap_or_default(),
                    done: fields.is_done(&e),
                    quantity: fields
                        .quantity
                        .map(|f| text(&e, f))
                        .filter(|q| !q.is_empty()),
                })
                .collect();
            items.sort_by_key(|item| item.done);
            Ok(items)
        })
    }

    pub fn add_item(
        &self,
        collection_id: String,
        title: String,
        quantity: Option<String>,
    ) -> Result<String> {
        let collection = parse(&collection_id)?;
        self.with(|i| {
            let fields = i.fields(collection)?;
            let title_field = fields
                .title
                .ok_or_else(|| anyhow!("Liste hat kein Titelfeld"))?;
            let last = i
                .replica
                .children(collection, Kind::Record)?
                .last()
                .map(|(_, e)| text(e, SysField::Order));
            let id = i.replica.transact(i.space, now_ms(), |tx| {
                let item = tx.create(Kind::Record, Some(collection));
                tx.set(item, title_field, title);
                if let Some(done) = fields.done {
                    tx.set(item, done, false);
                }
                if let (Some(field), Some(q)) =
                    (fields.quantity, quantity.filter(|q| !q.trim().is_empty()))
                {
                    tx.set(item, field, q);
                }
                let after = last.as_deref().filter(|o| !o.is_empty());
                tx.set(
                    item,
                    SysField::Order,
                    doeverything_core::order::between(after, None),
                );
                item
            })?;
            Ok(id.to_string())
        })
    }

    pub fn update_item(
        &self,
        item_id: String,
        title: String,
        quantity: Option<String>,
    ) -> Result<()> {
        let item = parse(&item_id)?;
        self.with(|i| {
            let fields = i.fields_of_item(item)?;
            i.replica.transact(i.space, now_ms(), |tx| {
                if let Some(f) = fields.title {
                    tx.set(item, f, title);
                }
                if let Some(f) = fields.quantity {
                    let q = quantity.filter(|q| !q.trim().is_empty());
                    tx.set(item, f, q.map_or(Value::Null, Value::Text));
                }
            })?;
            Ok(())
        })
    }

    pub fn set_done(&self, item_id: String, done: bool) -> Result<()> {
        let item = parse(&item_id)?;
        self.with(|i| {
            let field = i
                .fields_of_item(item)?
                .done
                .ok_or_else(|| anyhow!("Liste hat kein Erledigt-Feld"))?;
            i.set(item, field, Value::Bool(done))
        })
    }

    pub fn delete_item(&self, item_id: String) -> Result<()> {
        let id = parse(&item_id)?;
        self.with(|i| i.set(id, SysField::Deleted, Value::Bool(true)))
    }

    /// Macht Löschungen rückgängig (z. B. nach "Erledigte entfernen").
    pub fn restore_items(&self, item_ids: Vec<String>) -> Result<()> {
        let ids = item_ids
            .iter()
            .map(|s| parse(s))
            .collect::<Result<Vec<_>>>()?;
        self.with(|i| {
            i.replica.transact(i.space, now_ms(), |tx| {
                ids.iter().for_each(|id| tx.restore(*id))
            })?;
            Ok(())
        })
    }

    /// Löscht alle erledigten Einträge und gibt ihre IDs für "Rückgängig" zurück.
    pub fn clear_done(&self, collection_id: String) -> Result<Vec<String>> {
        let collection = parse(&collection_id)?;
        self.with(|i| {
            let fields = i.fields(collection)?;
            let done: Vec<EntityId> = i
                .replica
                .children(collection, Kind::Record)?
                .into_iter()
                .filter(|(_, e)| fields.is_done(e))
                .map(|(id, _)| id)
                .collect();
            i.replica.transact(i.space, now_ms(), |tx| {
                done.iter().for_each(|id| tx.delete(*id))
            })?;
            Ok(done.iter().map(ToString::to_string).collect())
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> Result<R>) -> Result<R> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("interner Zustand nach Absturz gesperrt"))?;
        f(&mut inner)
    }
}

impl Inner {
    fn set(&mut self, id: EntityId, key: impl Into<FieldKey>, value: Value) -> Result<()> {
        let key = key.into();
        self.replica
            .transact(self.space, now_ms(), |tx| tx.set(id, key, value))?;
        Ok(())
    }

    fn fields(&self, collection: EntityId) -> Result<Fields> {
        let fields = self.replica.children(collection, Kind::Field)?;
        Ok(Fields {
            title: field_with_role(&fields, Role::Title),
            done: field_with_role(&fields, Role::Done),
            quantity: field_with_role(&fields, Role::Quantity),
        })
    }

    fn fields_of_item(&self, item: EntityId) -> Result<Fields> {
        let entity = self
            .replica
            .entity(item)?
            .ok_or_else(|| anyhow!("Eintrag {item} existiert nicht"))?;
        let collection = entity
            .parent
            .ok_or_else(|| anyhow!("Eintrag {item} gehört zu keiner Liste"))?;
        self.fields(collection)
    }
}

fn parse(id: &str) -> Result<EntityId> {
    id.parse().with_context(|| format!("ungültige ID {id:?}"))
}

fn text(e: &Entity, key: impl Into<FieldKey>) -> String {
    e.get(key)
        .and_then(Value::as_text)
        .unwrap_or_default()
        .to_owned()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> (tempfile_dir::Dir, Tracker) {
        let dir = tempfile_dir::Dir::new();
        let t = Tracker::open(dir.path("test.sqlite")).unwrap();
        (dir, t)
    }

    /// Minimaler Ersatz für `tempfile`, damit die Brücke keine Test-Abhängigkeit braucht.
    mod tempfile_dir {
        pub struct Dir(std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let p = std::env::temp_dir().join(format!(
                    "doeverything-test-{}",
                    doeverything_core::EntityId::new()
                ));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
            pub fn path(&self, name: &str) -> String {
                self.0.join(name).to_string_lossy().into_owned()
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn einkaufsliste_durchspielen() {
        let (_dir, t) = tracker();
        let liste = t
            .create_collection("Wocheneinkauf".into(), "shopping@1".into())
            .unwrap();
        let milch = t
            .add_item(liste.clone(), "Milch".into(), Some("2 l".into()))
            .unwrap();
        let brot = t.add_item(liste.clone(), "Brot".into(), None).unwrap();
        t.add_item(liste.clone(), "Käse".into(), Some("  ".into()))
            .unwrap();

        t.set_done(milch.clone(), true).unwrap();
        let items = t.items(liste.clone()).unwrap();
        let titel: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titel, ["Brot", "Käse", "Milch"], "erledigte ans Ende");
        assert_eq!(items[2].quantity.as_deref(), Some("2 l"));
        assert_eq!(
            items[1].quantity, None,
            "leere Menge wird nicht gespeichert"
        );

        let c = &t.collections().unwrap()[0];
        assert_eq!(
            (c.name.as_str(), c.open_count, c.total_count),
            ("Wocheneinkauf", 2, 3)
        );

        let entfernt = t.clear_done(liste.clone()).unwrap();
        assert_eq!(entfernt, [milch]);
        assert_eq!(t.items(liste.clone()).unwrap().len(), 2);
        t.restore_items(entfernt).unwrap();
        assert_eq!(t.items(liste.clone()).unwrap().len(), 3);

        t.update_item(brot.clone(), "Vollkornbrot".into(), Some("1".into()))
            .unwrap();
        let brot_neu = t
            .items(liste)
            .unwrap()
            .into_iter()
            .find(|i| i.id == brot)
            .unwrap();
        assert_eq!(
            (brot_neu.title.as_str(), brot_neu.quantity.as_deref()),
            ("Vollkornbrot", Some("1"))
        );
    }

    #[test]
    fn daten_bleiben_nach_neustart() {
        let dir = tempfile_dir::Dir::new();
        let pfad = dir.path("persist.sqlite");
        let t = Tracker::open(pfad.clone()).unwrap();
        let todo = t.create_collection("Uni".into(), "todo@1".into()).unwrap();
        t.add_item(todo.clone(), "Abgabe Blatt 3".into(), None)
            .unwrap();
        drop(t);

        let t = Tracker::open(pfad).unwrap();
        let listen = t.collections().unwrap();
        assert_eq!(listen.len(), 1, "kein zweiter Space/keine doppelte Liste");
        assert_eq!(t.items(todo).unwrap()[0].title, "Abgabe Blatt 3");
    }

    #[test]
    fn listen_reihenfolge_und_loeschen() {
        let (_dir, t) = tracker();
        let a = t.create_collection("A".into(), "todo@1".into()).unwrap();
        t.create_collection("B".into(), "shopping@1".into())
            .unwrap();
        let namen = |t: &Tracker| {
            t.collections()
                .unwrap()
                .into_iter()
                .map(|c| c.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(namen(&t), ["A", "B"]);
        t.delete_collection(a.clone()).unwrap();
        assert_eq!(namen(&t), ["B"]);
        t.restore_collection(a).unwrap();
        assert_eq!(namen(&t), ["A", "B"]);
        assert!(t
            .create_collection("X".into(), "gibtsnicht@1".into())
            .is_err());
    }
}
