//! Modul-Vorlagen: ein Satz von Feldern, der beim Anlegen einer Collection erzeugt wird.
//! Danach gehört das Schema der Collection und kann frei angepasst werden.
//!
//! Felder tragen in `_config` eine optionale Rolle (`{"role": "title"}`), über die eine
//! generische Oberfläche sie findet, ohne die Vorlage zu kennen.

use serde_json::json;

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
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Done => "done",
            Self::Quantity => "quantity",
            Self::Notes => "notes",
            Self::Due => "due",
        }
    }

    /// Rolle aus der `_config` einer Field-Entity.
    pub fn of_field(field: &Entity) -> Option<Self> {
        let Some(Value::Json(config)) = field.get(SysField::Config) else {
            return None;
        };
        match config.get("role")?.as_str()? {
            "title" => Some(Self::Title),
            "done" => Some(Self::Done),
            "quantity" => Some(Self::Quantity),
            "notes" => Some(Self::Notes),
            "due" => Some(Self::Due),
            _ => None,
        }
    }
}

pub struct FieldSpec {
    pub name: &'static str,
    /// Wert für `_type`, z. B. `text`, `bool`, `date`.
    pub typ: &'static str,
    pub role: Option<Role>,
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
        FieldSpec {
            name: "Aufgabe",
            typ: "text",
            role: Some(Role::Title),
        },
        FieldSpec {
            name: "Erledigt",
            typ: "bool",
            role: Some(Role::Done),
        },
        FieldSpec {
            name: "Fällig",
            typ: "date",
            role: Some(Role::Due),
        },
        FieldSpec {
            name: "Notiz",
            typ: "text",
            role: Some(Role::Notes),
        },
    ],
};

pub const SHOPPING: Template = Template {
    id: "shopping@1",
    name: "Einkaufsliste",
    fields: &[
        FieldSpec {
            name: "Artikel",
            typ: "text",
            role: Some(Role::Title),
        },
        FieldSpec {
            name: "Menge",
            typ: "text",
            role: Some(Role::Quantity),
        },
        FieldSpec {
            name: "Erledigt",
            typ: "bool",
            role: Some(Role::Done),
        },
    ],
};

pub const ALL: &[Template] = &[TODO, SHOPPING];

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
        if let Some(role) = spec.role {
            tx.set(
                field,
                SysField::Config,
                Value::Json(json!({ "role": role.as_str() })),
            );
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
