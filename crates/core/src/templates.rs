//! Modul-Vorlagen: ein Satz von Feldern, der beim Anlegen einer Collection erzeugt wird.
//! Danach gehört das Schema der Collection und kann frei angepasst werden.
//!
//! Felder tragen in `_config` eine optionale Rolle (`{"role": "title"}`), über die eine
//! generische Oberfläche sie findet, ohne die Vorlage zu kennen. Auswahlfelder (`enum`)
//! speichern ihre Optionen ebenfalls dort: `{"options": [{"id": "…", "name": "…"}]}`.

use serde_json::{Map, json};

use crate::entity::Entity;
use crate::ids::EntityId;
use crate::model::{Kind, SysField};
use crate::order::between;
use crate::replica::Tx;
use crate::value::Value;

/// Bedeutung eines Feldes für die Oberfläche.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Haupttext eines Eintrags.
    Title,
    /// Abhaken (`bool`).
    Done,
    /// Menge als Freitext ("2 kg", "1 Packung").
    Quantity,
    Notes,
    Due,
    /// Geldbetrag (`money`); negativ = Ausgabe.
    Amount,
    /// Buchungs- bzw. Ereignisdatum (`date`).
    Date,
    /// Kategorie (`enum`).
    Category,
}

impl Role {
    const ALL: [Self; 8] = [
        Self::Title,
        Self::Done,
        Self::Quantity,
        Self::Notes,
        Self::Due,
        Self::Amount,
        Self::Date,
        Self::Category,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Done => "done",
            Self::Quantity => "quantity",
            Self::Notes => "notes",
            Self::Due => "due",
            Self::Amount => "amount",
            Self::Date => "date",
            Self::Category => "category",
        }
    }

    /// Rolle aus der `_config` einer Field-Entity.
    pub fn of_field(field: &Entity) -> Option<Self> {
        let role = config(field)?.get("role")?.as_str()?;
        Self::ALL.into_iter().find(|r| r.as_str() == role)
    }
}

/// `_config` einer Field-Entity, falls gesetzt.
pub fn config(field: &Entity) -> Option<&serde_json::Value> {
    match field.get(SysField::Config) {
        Some(Value::Json(config)) => Some(config),
        _ => None,
    }
}

pub struct FieldSpec {
    pub name: &'static str,
    /// Wert für `_type`, z. B. `text`, `bool`, `date`.
    pub typ: &'static str,
    pub role: Option<Role>,
    /// Startoptionen für `enum`-Felder.
    pub options: &'static [&'static str],
    /// Weitere Einträge für `_config`, z. B. `("currency", "EUR")`.
    pub extra: &'static [(&'static str, &'static str)],
}

const fn field(name: &'static str, typ: &'static str, role: Role) -> FieldSpec {
    FieldSpec {
        name,
        typ,
        role: Some(role),
        options: &[],
        extra: &[],
    }
}

impl FieldSpec {
    const fn with_options(mut self, options: &'static [&'static str]) -> Self {
        self.options = options;
        self
    }

    const fn with_extra(mut self, extra: &'static [(&'static str, &'static str)]) -> Self {
        self.extra = extra;
        self
    }
}

pub struct Template {
    /// Stabile ID mit Version, landet in `_template`.
    pub id: &'static str,
    pub name: &'static str,
    pub fields: &'static [FieldSpec],
}

pub const TODO: Template = Template {
    id: "todo@1",
    name: "To-Do-Liste",
    fields: &[
        field("Aufgabe", "text", Role::Title),
        field("Erledigt", "bool", Role::Done),
        field("Fällig", "date", Role::Due),
        field("Notiz", "text", Role::Notes),
    ],
};

pub const SHOPPING: Template = Template {
    id: "shopping@1",
    name: "Einkaufsliste",
    fields: &[
        field("Artikel", "text", Role::Title),
        field("Menge", "text", Role::Quantity),
        field("Erledigt", "bool", Role::Done),
    ],
};

pub const FINANCE: Template = Template {
    id: "finance@1",
    name: "Haushaltsbuch",
    fields: &[
        field("Beschreibung", "text", Role::Title),
        field("Betrag", "money", Role::Amount).with_extra(&[("currency", "EUR")]),
        field("Datum", "date", Role::Date),
        field("Kategorie", "enum", Role::Category).with_options(&[
            "Lebensmittel",
            "Haushalt",
            "Wohnen",
            "Mobilität",
            "Freizeit",
            "Gesundheit",
            "Gehalt",
            "Sonstiges",
        ]),
        field("Notiz", "text", Role::Notes),
    ],
};

pub const ALL: &[Template] = &[TODO, SHOPPING, FINANCE];

pub fn by_id(id: &str) -> Option<&'static Template> {
    ALL.iter().find(|t| t.id == id)
}

/// Legt eine Collection samt Feldern an. `after` ist der `_order`-Schlüssel der bisher
/// letzten Collection, damit die neue ans Ende kommt.
pub fn instantiate(
    tx: &mut Tx<'_>,
    space: EntityId,
    template: &Template,
    name: &str,
    after: Option<&str>,
) -> EntityId {
    let collection = tx.create(Kind::Collection, Some(space));
    tx.set(collection, SysField::Name, name);
    tx.set(collection, SysField::Template, template.id);
    tx.set(collection, SysField::Order, between(after, None));
    let mut order: Option<String> = None;
    for spec in template.fields {
        let field = tx.create(Kind::Field, Some(collection));
        tx.set(field, SysField::Name, spec.name);
        tx.set(field, SysField::Type, spec.typ);
        let key = between(order.as_deref(), None);
        tx.set(field, SysField::Order, key.clone());
        order = Some(key);

        let mut config = Map::new();
        if let Some(role) = spec.role {
            config.insert("role".into(), role.as_str().into());
        }
        for (k, v) in spec.extra {
            config.insert((*k).into(), (*v).into());
        }
        if !spec.options.is_empty() {
            let options: Vec<_> = spec
                .options
                .iter()
                .map(|name| json!({ "id": EntityId::new().to_string(), "name": name }))
                .collect();
            config.insert("options".into(), options.into());
        }
        if !config.is_empty() {
            tx.set(field, SysField::Config, Value::Json(config.into()));
        }
    }
    collection
}

/// ID des ersten Feldes mit der gegebenen Rolle.
pub fn field_with_role(fields: &[(EntityId, Entity)], role: Role) -> Option<EntityId> {
    fields
        .iter()
        .find(|(_, f)| Role::of_field(f) == Some(role))
        .map(|(id, _)| *id)
}

/// Optionen eines `enum`-Feldes als (ID, Name), in gespeicherter Reihenfolge.
/// Einträge mit ungültiger ID werden übersprungen.
pub fn enum_options(field: &Entity) -> Vec<(EntityId, String)> {
    config(field)
        .and_then(|c| c.get("options"))
        .and_then(|o| o.as_array())
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let id = o.get("id")?.as_str()?.parse().ok()?;
            let name = o.get("name")?.as_str()?.to_owned();
            Some((id, name))
        })
        .collect()
}

/// Neue `_config` mit einer zusätzlichen Option; die übrige Konfiguration bleibt erhalten.
pub fn config_with_option(field: &Entity, id: EntityId, name: &str) -> serde_json::Value {
    let mut config = config(field)
        .and_then(|c| c.as_object())
        .cloned()
        .unwrap_or_default();
    let options = config.entry("options").or_insert_with(|| json!([]));
    if let Some(list) = options.as_array_mut() {
        list.push(json!({ "id": id.to_string(), "name": name }));
    }
    config.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryStore, Replica};

    #[test]
    fn vorlage_anlegen_und_rollen_finden() {
        let mut r = Replica::open(MemoryStore::new()).unwrap();
        let space = r.create_space("Privat", 1000).unwrap();
        let liste = r
            .transact(space, 1000, |tx| {
                instantiate(tx, space, &SHOPPING, "Wocheneinkauf", None)
            })
            .unwrap();

        let c = r.entity(liste).unwrap().unwrap();
        assert_eq!(c.get(SysField::Template), Some(&Value::from("shopping@1")));
        let felder = r.children(liste, Kind::Field).unwrap();
        let namen: Vec<_> = felder
            .iter()
            .filter_map(|(_, f)| f.get(SysField::Name))
            .collect();
        assert_eq!(
            namen,
            [
                &Value::from("Artikel"),
                &Value::from("Menge"),
                &Value::from("Erledigt")
            ]
        );
        assert!(field_with_role(&felder, Role::Title).is_some());
        assert!(field_with_role(&felder, Role::Due).is_none());
    }

    #[test]
    fn haushaltsbuch_hat_kategorien_und_waehrung() {
        let mut r = Replica::open(MemoryStore::new()).unwrap();
        let space = r.create_space("Privat", 1000).unwrap();
        let buch = r
            .transact(space, 1000, |tx| {
                instantiate(tx, space, &FINANCE, "Haushalt", None)
            })
            .unwrap();
        let felder = r.children(buch, Kind::Field).unwrap();

        let betrag = field_with_role(&felder, Role::Amount).unwrap();
        let betrag = &felder.iter().find(|(id, _)| *id == betrag).unwrap().1;
        assert_eq!(config(betrag).unwrap()["currency"], "EUR");

        let kategorie = field_with_role(&felder, Role::Category).unwrap();
        let mut kategorie = felder
            .into_iter()
            .find(|(id, _)| *id == kategorie)
            .unwrap()
            .1;
        let optionen = enum_options(&kategorie);
        assert_eq!(optionen.len(), 8);
        assert_eq!(optionen[0].1, "Lebensmittel");

        let neu = EntityId::new();
        let cfg = config_with_option(&kategorie, neu, "Urlaub");
        kategorie
            .fields
            .get_mut(&SysField::Config.into())
            .unwrap()
            .value = Value::Json(cfg);
        let optionen = enum_options(&kategorie);
        assert_eq!(optionen.last(), Some(&(neu, "Urlaub".to_owned())));
        assert_eq!(
            Role::of_field(&kategorie),
            Some(Role::Category),
            "Rolle bleibt erhalten"
        );
    }

    #[test]
    fn ids_als_text() {
        let id = EntityId::new();
        assert_eq!(id.to_string().parse::<EntityId>().unwrap(), id);
        assert!("kein-uuid".parse::<EntityId>().is_err());
    }

    #[test]
    fn spaces_auflisten() {
        use crate::Store;
        let mut r = Replica::open(MemoryStore::new()).unwrap();
        assert_eq!(r.store().spaces().unwrap(), []);
        let a = r.create_space("A", 1000).unwrap();
        let b = r.create_space("B", 1001).unwrap();
        assert_eq!(r.store().spaces().unwrap(), [a, b]);
    }
}
