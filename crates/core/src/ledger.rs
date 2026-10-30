//! Auswertungen für Buchungen (Haushaltsbuch): Gruppierung nach Monat, Summen je Kategorie.
//! Wird lokal berechnet und nie synchronisiert.

use std::collections::BTreeMap;

use crate::ids::EntityId;
use crate::value::Value;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Date {
    pub y: i16,
    pub m: u8,
    pub d: u8,
}

impl Date {
    /// Liest `YYYY-MM-DD` und prüft, ob der Tag existiert (inklusive Schaltjahren).
    pub fn parse_iso(s: &str) -> Option<Self> {
        let mut parts = s.splitn(3, '-');
        let y: i16 = parts.next()?.parse().ok()?;
        let m: u8 = parts.next()?.parse().ok()?;
        let d: u8 = parts.next()?.parse().ok()?;
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let days = match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        (1..=days).contains(&d).then_some(Self { y, m, d })
    }

    pub fn to_iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }

    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Date { y, m, d } => Some(Self {
                y: *y,
                m: *m,
                d: *d,
            }),
            _ => None,
        }
    }

    pub fn to_value(self) -> Value {
        Value::Date {
            y: self.y,
            m: self.m,
            d: self.d,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Entry {
    pub id: EntityId,
    /// Negativ = Ausgabe.
    pub amount_minor: i64,
    pub date: Date,
    pub category: Option<EntityId>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Month {
    pub year: i16,
    pub month: u8,
    pub income: i64,
    /// Summe der Ausgaben als positive Zahl.
    pub expense: i64,
    /// Ausgaben je Kategorie (positiv), größte zuerst; `None` = ohne Kategorie.
    pub expense_by_category: Vec<(Option<EntityId>, i64)>,
    /// Neueste zuerst.
    pub entries: Vec<Entry>,
}

impl Month {
    pub fn balance(&self) -> i64 {
        self.income.saturating_sub(self.expense)
    }
}

/// Gruppiert Buchungen nach Monat, neuester Monat zuerst.
pub fn by_month(entries: impl IntoIterator<Item = Entry>) -> Vec<Month> {
    let mut months: BTreeMap<(i16, u8), Vec<Entry>> = BTreeMap::new();
    for e in entries {
        months.entry((e.date.y, e.date.m)).or_default().push(e);
    }
    months
        .into_iter()
        .rev()
        .map(|((year, month), mut entries)| {
            entries.sort_by_key(|e| std::cmp::Reverse((e.date, e.id)));
            let mut income = 0i64;
            let mut expense = 0i64;
            let mut by_category: BTreeMap<Option<EntityId>, i64> = BTreeMap::new();
            for e in &entries {
                if e.amount_minor >= 0 {
                    income = income.saturating_add(e.amount_minor);
                } else {
                    let abs = e.amount_minor.saturating_neg();
                    expense = expense.saturating_add(abs);
                    let sum = by_category.entry(e.category).or_default();
                    *sum = sum.saturating_add(abs);
                }
            }
            let mut expense_by_category: Vec<_> = by_category.into_iter().collect();
            expense_by_category.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            Month {
                year,
                month,
                income,
                expense,
                expense_by_category,
                entries,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse_iso(s).unwrap()
    }

    #[test]
    fn datum() {
        assert_eq!(d("2026-10-30").to_iso(), "2026-10-30");
        assert!(Date::parse_iso("2024-02-29").is_some());
        assert!(Date::parse_iso("2026-02-29").is_none());
        assert!(Date::parse_iso("2000-02-29").is_some());
        assert!(Date::parse_iso("1900-02-29").is_none());
        assert!(Date::parse_iso("2026-04-31").is_none());
        assert!(Date::parse_iso("2026-13-01").is_none());
        assert!(Date::parse_iso("2026-10").is_none());
        assert!(Date::parse_iso("heute").is_none());
        assert_eq!(
            Date::from_value(&d("2026-01-05").to_value()),
            Some(d("2026-01-05"))
        );
    }

    #[test]
    fn monatssummen() {
        let lebensmittel = EntityId::new();
        let freizeit = EntityId::new();
        let e = |betrag, datum, kat| Entry {
            id: EntityId::new(),
            amount_minor: betrag,
            date: d(datum),
            category: kat,
        };
        let monate = by_month([
            e(-1234, "2026-10-02", Some(lebensmittel)),
            e(250_000, "2026-10-01", None),
            e(-5000, "2026-10-15", Some(freizeit)),
            e(-766, "2026-10-20", Some(lebensmittel)),
            e(-999, "2026-09-30", None),
        ]);

        assert_eq!(monate.len(), 2);
        let okt = &monate[0];
        assert_eq!((okt.year, okt.month), (2026, 10));
        assert_eq!(
            (okt.income, okt.expense, okt.balance()),
            (250_000, 7000, 243_000)
        );
        assert_eq!(
            okt.expense_by_category,
            [(Some(freizeit), 5000), (Some(lebensmittel), 2000)]
        );
        let tage: Vec<_> = okt.entries.iter().map(|e| e.date.d).collect();
        assert_eq!(tage, [20, 15, 2, 1], "neueste zuerst");

        let sep = &monate[1];
        assert_eq!((sep.month, sep.balance()), (9, -999));
        assert_eq!(sep.expense_by_category, [(None, 999)]);
    }
}
