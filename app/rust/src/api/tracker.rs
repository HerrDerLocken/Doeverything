//! Schnittstelle für die Flutter-App. flutter_rust_bridge erzeugt daraus `app/lib/src/rust/`
//! (`flutter_rust_bridge_codegen generate` im Ordner `app/`).
//!
//! IDs werden als Text übergeben, damit Dart nichts über ihr Binärformat wissen muss.

use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use doeverything_core::hlc::now_ms;
use doeverything_core::ledger::{self, Date};
use doeverything_core::money;
use doeverything_core::templates::{self, enum_options, field_with_role, Role};
use doeverything_core::{Entity, EntityId, FieldKey, Kind, Replica, Store, SysField, Value};
use doeverything_store::{DbFile, SqliteStore};
use flutter_rust_bridge::frb;

#[frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

pub struct TemplateInfo {
    pub id: String,
    pub name: String,
}

/// Welche Ansicht eine Collection bekommt. Ergibt sich aus ihren Feldern, nicht aus der Vorlage.
pub enum CollectionView {
    /// Einträge zum Abhaken (To-Do, Einkauf).
    Checklist,
    /// Buchungen mit Betrag und Datum (Haushaltsbuch).
    Ledger,
}

pub struct CollectionInfo {
    pub id: String,
    pub name: String,
    pub template: String,
    pub view: CollectionView,
    pub open_count: u32,
    pub total_count: u32,
    /// Nur `Ledger`: Saldo des Monats, in dem `today` liegt.
    pub month_balance_minor: Option<i64>,
    pub currency: Option<String>,
}

pub struct CategoryInfo {
    pub id: String,
    pub name: String,
}

pub struct CategorySum {
    pub name: String,
    pub expense_minor: i64,
}

pub struct LedgerEntry {
    pub id: String,
    pub description: String,
    /// Negativ = Ausgabe.
    pub amount_minor: i64,
    /// `YYYY-MM-DD`
    pub date: String,
    pub category_id: Option<String>,
    pub category_name: Option<String>,
}

pub struct MonthView {
    pub year: i32,
    pub month: u32,
    pub income_minor: i64,
    pub expense_minor: i64,
    pub balance_minor: i64,
    pub categories: Vec<CategorySum>,
    pub entries: Vec<LedgerEntry>,
}

pub struct LedgerData {
    pub currency: String,
    pub categories: Vec<CategoryInfo>,
    /// Neuester Monat zuerst.
    pub months: Vec<MonthView>,
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

/// Zustand der Datenbankdatei, bevor ein Schlüssel gelesen oder erzeugt wird.
pub enum DatabaseState {
    Missing,
    Plaintext,
    Encrypted,
}

#[frb(sync)]
pub fn database_state(db_path: String) -> Result<DatabaseState> {
    Ok(match doeverything_store::inspect(&db_path)? {
        DbFile::Missing => DatabaseState::Missing,
        DbFile::Plaintext => DatabaseState::Plaintext,
        DbFile::Encrypted => DatabaseState::Encrypted,
    })
}

/// Neuer zufälliger 32-Byte-Schlüssel für die Datenbank.
#[frb(sync)]
pub fn generate_db_key() -> Vec<u8> {
    doeverything_store::generate_key().to_vec()
}

/// Deutsche Schreibweise, z. B. `1.234,56 €`.
#[frb(sync)]
pub fn format_amount(minor: i64, currency: String) -> String {
    money::format_amount(minor, &currency)
}

/// Betrag in Cent aus Benutzereingabe (`12,34`, `1.234`, …); `None` bei ungültiger Eingabe.
#[frb(sync)]
pub fn parse_amount(text: String) -> Option<i64> {
    money::parse_amount(&text)
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
    all: Vec<(EntityId, Entity)>,
    title: Option<EntityId>,
    done: Option<EntityId>,
    quantity: Option<EntityId>,
    amount: Option<EntityId>,
    date: Option<EntityId>,
    category: Option<EntityId>,
}

impl Fields {
    fn is_done(&self, item: &Entity) -> bool {
        self.done.and_then(|f| item.get(f)).and_then(Value::as_bool) == Some(true)
    }

    fn view(&self) -> CollectionView {
        if self.amount.is_some() {
            CollectionView::Ledger
        } else {
            CollectionView::Checklist
        }
    }

    fn entity(&self, id: Option<EntityId>) -> Option<&Entity> {
        let id = id?;
        self.all.iter().find(|(f, _)| *f == id).map(|(_, e)| e)
    }

    /// Währung aus der `_config` des Betragsfeldes, Standard EUR.
    fn currency(&self) -> String {
        self.entity(self.amount)
            .and_then(templates::config)
            .and_then(|c| c.get("currency"))
            .and_then(|c| c.as_str())
            .unwrap_or("EUR")
            .to_owned()
    }

    fn categories(&self) -> Vec<(EntityId, String)> {
        self.entity(self.category)
            .map(enum_options)
            .unwrap_or_default()
    }

    fn ledger_entry(&self, id: EntityId, item: &Entity) -> ledger::Entry {
        let amount_minor = match self.amount.and_then(|f| item.get(f)) {
            Some(Value::Money { minor, .. }) => *minor,
            _ => 0,
        };
        // Fehlendes Datum (z. B. unvollständig synchronisiert) nicht verstecken, sondern ganz unten zeigen.
        let date = self
            .date
            .and_then(|f| item.get(f))
            .and_then(Date::from_value)
            .unwrap_or(Date {
                y: 1970,
                m: 1,
                d: 1,
            });
        let category = match self.category.and_then(|f| item.get(f)) {
            Some(Value::Enum(c)) => Some(*c),
            _ => None,
        };
        ledger::Entry {
            id,
            amount_minor,
            date,
            category,
        }
    }
}

impl Tracker {
    /// Öffnet oder erzeugt die verschlüsselte Datenbank. Eine unverschlüsselte Datenbank älterer
    /// App-Versionen wird dabei einmalig verschlüsselt. Beim ersten Start wird der Space "Privat" angelegt.
    pub fn open(db_path: String, key: Vec<u8>) -> Result<Tracker> {
        let key: [u8; 32] = key
            .try_into()
            .map_err(|k: Vec<u8>| anyhow!("Schlüssel hat {} statt 32 Bytes", k.len()))?;
        if doeverything_store::inspect(&db_path)? == DbFile::Plaintext {
            doeverything_store::encrypt_plaintext(&db_path, &key)
                .with_context(|| format!("Datenbank {db_path} verschlüsseln"))?;
        }
        let store = SqliteStore::open(&db_path, Some(&key))
            .with_context(|| format!("Datenbank {db_path} öffnen (falscher Schlüssel?)"))?;
        let mut replica = Replica::open(store)?;
        let space = match replica.store().spaces()?.first() {
            Some(space) => *space,
            None => replica.create_space("Privat", now_ms())?,
        };
        Ok(Self {
            inner: Mutex::new(Inner { replica, space }),
        })
    }

    /// Alle Listen. `today` (`YYYY-MM-DD`) bestimmt den Monat für den Saldo von Haushaltsbüchern.
    pub fn collections(&self, today: String) -> Result<Vec<CollectionInfo>> {
        let today = Date::parse_iso(&today).ok_or_else(|| anyhow!("ungültiges Datum {today:?}"))?;
        self.with(|i| {
            let mut out = Vec::new();
            for (id, c) in i.replica.children(i.space, Kind::Collection)? {
                let fields = i.fields(id)?;
                let items = i.replica.children(id, Kind::Record)?;
                let view = fields.view();
                let (open, month_balance_minor, currency) = match view {
                    CollectionView::Checklist => (
                        items.iter().filter(|(_, e)| !fields.is_done(e)).count(),
                        None,
                        None,
                    ),
                    CollectionView::Ledger => {
                        let balance = ledger::by_month(
                            items.iter().map(|(id, e)| fields.ledger_entry(*id, e)),
                        )
                        .into_iter()
                        .find(|m| (m.year, m.month) == (today.y, today.m))
                        .map_or(0, |m| m.balance());
                        (0, Some(balance), Some(fields.currency()))
                    }
                };
                out.push(CollectionInfo {
                    id: id.to_string(),
                    name: text(&c, SysField::Name),
                    template: text(&c, SysField::Template),
                    view,
                    open_count: count(open),
                    total_count: count(items.len()),
                    month_balance_minor,
                    currency,
                });
            }
            Ok(out)
        })
    }

    /// Buchungen nach Monat gruppiert, mit Summen.
    pub fn ledger(&self, collection_id: String) -> Result<LedgerData> {
        let collection = parse(&collection_id)?;
        self.with(|i| {
            let fields = i.fields(collection)?;
            let categories = fields.categories();
            let category_name = |id: Option<EntityId>| {
                id.and_then(|id| categories.iter().find(|(c, _)| *c == id))
                    .map(|(_, n)| n.clone())
            };
            let items = i.replica.children(collection, Kind::Record)?;
            let descriptions: std::collections::HashMap<EntityId, String> = items
                .iter()
                .map(|(id, e)| (*id, fields.title.map(|f| text(e, f)).unwrap_or_default()))
                .collect();
            let months = ledger::by_month(items.iter().map(|(id, e)| fields.ledger_entry(*id, e)))
                .into_iter()
                .map(|m| MonthView {
                    year: i32::from(m.year),
                    month: u32::from(m.month),
                    income_minor: m.income,
                    expense_minor: m.expense,
                    balance_minor: m.balance(),
                    categories: m
                        .expense_by_category
                        .iter()
                        .map(|(c, sum)| CategorySum {
                            name: category_name(*c).unwrap_or_else(|| "Ohne Kategorie".into()),
                            expense_minor: *sum,
                        })
                        .collect(),
                    entries: m
                        .entries
                        .iter()
                        .map(|e| LedgerEntry {
                            id: e.id.to_string(),
                            description: descriptions.get(&e.id).cloned().unwrap_or_default(),
                            amount_minor: e.amount_minor,
                            date: e.date.to_iso(),
                            category_id: e.category.map(|c| c.to_string()),
                            category_name: category_name(e.category),
                        })
                        .collect(),
                })
                .collect();
            Ok(LedgerData {
                currency: fields.currency(),
                categories: categories
                    .iter()
                    .map(|(id, name)| CategoryInfo {
                        id: id.to_string(),
                        name: name.clone(),
                    })
                    .collect(),
                months,
            })
        })
    }

    /// Neue Buchung. `amount_minor` negativ = Ausgabe, `date` als `YYYY-MM-DD`.
    pub fn add_entry(
        &self,
        collection_id: String,
        description: String,
        amount_minor: i64,
        date: String,
        category_id: Option<String>,
    ) -> Result<String> {
        let collection = parse(&collection_id)?;
        self.with(|i| {
            let id = i.write_entry(
                collection,
                None,
                description,
                amount_minor,
                &date,
                category_id.as_deref(),
            )?;
            Ok(id.to_string())
        })
    }

    pub fn update_entry(
        &self,
        entry_id: String,
        description: String,
        amount_minor: i64,
        date: String,
        category_id: Option<String>,
    ) -> Result<()> {
        let entry = parse(&entry_id)?;
        self.with(|i| {
            let collection = i.collection_of(entry)?;
            i.write_entry(
                collection,
                Some(entry),
                description,
                amount_minor,
                &date,
                category_id.as_deref(),
            )?;
            Ok(())
        })
    }

    /// Fügt eine Kategorie hinzu und gibt ihre ID zurück.
    ///
    /// Die Optionen liegen als Ganzes in `_config`: Legen zwei Geräte gleichzeitig offline eine
    /// Kategorie an, bleibt nur eine erhalten (bekannte Einschränkung von LWW).
    pub fn add_category(&self, collection_id: String, name: String) -> Result<String> {
        let collection = parse(&collection_id)?;
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(anyhow!("Kategorie braucht einen Namen"));
        }
        self.with(|i| {
            let fields = i.fields(collection)?;
            let field_id = fields
                .category
                .ok_or_else(|| anyhow!("Liste hat kein Kategoriefeld"))?;
            let field = fields
                .entity(Some(field_id))
                .ok_or_else(|| anyhow!("Kategoriefeld fehlt"))?;
            if let Some((id, _)) = enum_options(field)
                .into_iter()
                .find(|(_, n)| n.eq_ignore_ascii_case(&name))
            {
                return Ok(id.to_string());
            }
            let id = EntityId::new();
            let config = templates::config_with_option(field, id, &name);
            i.set(field_id, SysField::Config, Value::Json(config))?;
            Ok(id.to_string())
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
        let all = self.replica.children(collection, Kind::Field)?;
        Ok(Fields {
            title: field_with_role(&all, Role::Title),
            done: field_with_role(&all, Role::Done),
            quantity: field_with_role(&all, Role::Quantity),
            amount: field_with_role(&all, Role::Amount),
            date: field_with_role(&all, Role::Date),
            category: field_with_role(&all, Role::Category),
            all,
        })
    }

    fn collection_of(&self, item: EntityId) -> Result<EntityId> {
        let entity = self
            .replica
            .entity(item)?
            .ok_or_else(|| anyhow!("Eintrag {item} existiert nicht"))?;
        entity
            .parent
            .ok_or_else(|| anyhow!("Eintrag {item} gehört zu keiner Liste"))
    }

    fn fields_of_item(&self, item: EntityId) -> Result<Fields> {
        let collection = self.collection_of(item)?;
        self.fields(collection)
    }

    /// Legt eine Buchung an (`entry = None`) oder überschreibt eine bestehende.
    fn write_entry(
        &mut self,
        collection: EntityId,
        entry: Option<EntityId>,
        description: String,
        amount_minor: i64,
        date: &str,
        category_id: Option<&str>,
    ) -> Result<EntityId> {
        let fields = self.fields(collection)?;
        let amount_field = fields
            .amount
            .ok_or_else(|| anyhow!("Liste hat kein Betragsfeld"))?;
        let date = Date::parse_iso(date).ok_or_else(|| anyhow!("ungültiges Datum {date:?}"))?;
        let category = category_id.map(parse).transpose()?;
        if let Some(c) = category {
            if !fields.categories().iter().any(|(id, _)| *id == c) {
                return Err(anyhow!("unbekannte Kategorie {c}"));
            }
        }
        let mut currency = [0u8; 3];
        currency.copy_from_slice(fields.currency().as_bytes().get(..3).unwrap_or(b"EUR"));
        let last = match entry {
            Some(_) => None,
            None => self
                .replica
                .children(collection, Kind::Record)?
                .last()
                .map(|(_, e)| text(e, SysField::Order)),
        };
        let id = self.replica.transact(self.space, now_ms(), |tx| {
            let id = entry.unwrap_or_else(|| tx.create(Kind::Record, Some(collection)));
            if let Some(f) = fields.title {
                tx.set(id, f, description.trim());
            }
            tx.set(
                id,
                amount_field,
                Value::Money {
                    minor: amount_minor,
                    currency,
                },
            );
            if let Some(f) = fields.date {
                tx.set(id, f, date.to_value());
            }
            if let Some(f) = fields.category {
                tx.set(id, f, category.map_or(Value::Null, Value::Enum));
            }
            if entry.is_none() {
                let after = last.as_deref().filter(|o| !o.is_empty());
                tx.set(
                    id,
                    SysField::Order,
                    doeverything_core::order::between(after, None),
                );
            }
            id
        })?;
        Ok(id)
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
        let t = Tracker::open(dir.path("test.sqlite"), generate_db_key()).unwrap();
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

        let c = &t.collections("2026-10-30".into()).unwrap()[0];
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
        let key = generate_db_key();
        let t = Tracker::open(pfad.clone(), key.clone()).unwrap();
        let todo = t.create_collection("Uni".into(), "todo@1".into()).unwrap();
        t.add_item(todo.clone(), "Abgabe Blatt 3".into(), None)
            .unwrap();
        drop(t);

        assert!(
            Tracker::open(pfad.clone(), generate_db_key()).is_err(),
            "falscher Schlüssel"
        );
        assert!(matches!(
            database_state(pfad.clone()).unwrap(),
            DatabaseState::Encrypted
        ));
        let t = Tracker::open(pfad, key).unwrap();
        let listen = t.collections("2026-10-30".into()).unwrap();
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
            t.collections("2026-10-30".into())
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

    #[test]
    fn haushaltsbuch_durchspielen() {
        let (_dir, t) = tracker();
        let buch = t
            .create_collection("Haushalt".into(), "finance@1".into())
            .unwrap();
        let daten = t.ledger(buch.clone()).unwrap();
        assert_eq!(daten.currency, "EUR");
        let lebensmittel = daten
            .categories
            .iter()
            .find(|c| c.name == "Lebensmittel")
            .unwrap()
            .id
            .clone();

        t.add_entry(
            buch.clone(),
            "Gehalt".into(),
            250_000,
            "2026-10-01".into(),
            None,
        )
        .unwrap();
        let einkauf = t
            .add_entry(
                buch.clone(),
                " Wocheneinkauf ".into(),
                -4_250,
                "2026-10-28".into(),
                Some(lebensmittel.clone()),
            )
            .unwrap();
        t.add_entry(
            buch.clone(),
            "Kino".into(),
            -1_200,
            "2026-09-12".into(),
            None,
        )
        .unwrap();
        assert!(t
            .add_entry(buch.clone(), "x".into(), 1, "2026-02-30".into(), None)
            .is_err());
        assert!(t
            .add_entry(
                buch.clone(),
                "x".into(),
                1,
                "2026-10-01".into(),
                Some(EntityId::new().to_string())
            )
            .is_err());

        let daten = t.ledger(buch.clone()).unwrap();
        assert_eq!(daten.months.len(), 2);
        let okt = &daten.months[0];
        assert_eq!(
            (
                okt.year,
                okt.month,
                okt.income_minor,
                okt.expense_minor,
                okt.balance_minor
            ),
            (2026, 10, 250_000, 4_250, 245_750)
        );
        assert_eq!(okt.categories[0].name, "Lebensmittel");
        assert_eq!(okt.entries[0].description, "Wocheneinkauf");
        assert_eq!(
            okt.entries[0].category_name.as_deref(),
            Some("Lebensmittel")
        );
        assert_eq!(daten.months[1].categories[0].name, "Ohne Kategorie");

        let c = &t.collections("2026-10-30".into()).unwrap()[0];
        assert!(matches!(c.view, CollectionView::Ledger));
        assert_eq!((c.month_balance_minor, c.total_count), (Some(245_750), 3));

        // Bearbeiten: Kategorie entfernen, Betrag ändern
        t.update_entry(
            einkauf.clone(),
            "Wocheneinkauf".into(),
            -5_000,
            "2026-10-28".into(),
            None,
        )
        .unwrap();
        let okt = &t.ledger(buch.clone()).unwrap().months[0];
        assert_eq!(
            (okt.expense_minor, okt.entries[0].category_id.clone()),
            (5_000, None)
        );

        // Neue Kategorie, Duplikat liefert dieselbe ID
        let urlaub = t.add_category(buch.clone(), "Urlaub".into()).unwrap();
        assert_eq!(
            t.add_category(buch.clone(), "urlaub".into()).unwrap(),
            urlaub
        );
        assert_eq!(
            t.ledger(buch.clone())
                .unwrap()
                .categories
                .last()
                .unwrap()
                .name,
            "Urlaub"
        );

        // Löschen mit Rückgängig wie bei Checklisten
        t.delete_item(einkauf.clone()).unwrap();
        assert_eq!(t.ledger(buch.clone()).unwrap().months[0].expense_minor, 0);
        t.restore_items(vec![einkauf]).unwrap();
        assert_eq!(t.ledger(buch).unwrap().months[0].expense_minor, 5_000);
    }

    #[test]
    fn betraege_ueber_die_bruecke() {
        assert_eq!(parse_amount("12,34".into()), Some(1234));
        assert_eq!(format_amount(-1234, "EUR".into()), "-12,34\u{a0}€");
    }

    #[test]
    fn alte_klartext_datenbank_wird_uebernommen() {
        let dir = tempfile_dir::Dir::new();
        let pfad = dir.path("alt.sqlite");
        // Stand vor der Verschlüsselung: Daten ohne Schlüssel angelegt
        let mut r = Replica::open(SqliteStore::open(&pfad, None).unwrap()).unwrap();
        let space = r.create_space("Privat", 1000).unwrap();
        let liste = r
            .transact(space, 1000, |tx| {
                templates::instantiate(tx, space, &templates::TODO, "Alt", None)
            })
            .unwrap();
        drop(r);
        assert!(matches!(
            database_state(pfad.clone()).unwrap(),
            DatabaseState::Plaintext
        ));

        let key = generate_db_key();
        assert!(
            Tracker::open(pfad.clone(), vec![1, 2, 3]).is_err(),
            "kurzer Schlüssel"
        );
        assert!(
            matches!(
                database_state(pfad.clone()).unwrap(),
                DatabaseState::Plaintext
            ),
            "nichts angefasst"
        );

        let t = Tracker::open(pfad.clone(), key).unwrap();
        assert!(matches!(
            database_state(pfad).unwrap(),
            DatabaseState::Encrypted
        ));
        let listen = t.collections("2026-10-30".into()).unwrap();
        assert_eq!((listen.len(), listen[0].id.clone()), (1, liste.to_string()));
    }
}
