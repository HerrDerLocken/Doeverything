//! Geldbeträge als ganze Cent: Eingabe deutscher und englischer Schreibweise, deutsche Ausgabe.

/// Liest einen Betrag in Cent. Akzeptiert `12`, `12,5`, `1.234,56`, `-3,10`, `12.34`, `1.234`,
/// optional mit `€` und Leerzeichen. Mehr als zwei Nachkommastellen werden abgelehnt.
pub fn parse_amount(input: &str) -> Option<i64> {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '€')
        .collect();
    let (negative, s) = match cleaned.strip_prefix(['-', '−']) {
        Some(rest) => (true, rest),
        None => (false, cleaned.strip_prefix('+').unwrap_or(&cleaned)),
    };

    // Komma ist immer Dezimaltrenner. Ohne Komma ist ein Punkt nur dann Dezimaltrenner,
    // wenn er einmal vorkommt und höchstens zwei Ziffern folgen ("12.34", nicht "1.234").
    let (int_part, frac) = if let Some((int, frac)) = s.rsplit_once(',') {
        (int.replace('.', ""), frac)
    } else {
        match s.rsplit_once('.') {
            Some((int, frac)) if frac.len() <= 2 && !int.contains('.') => (int.to_owned(), frac),
            _ => (s.replace('.', ""), ""),
        }
    };
    let digits = |t: &str| t.chars().all(|c| c.is_ascii_digit());
    if frac.len() > 2
        || !digits(frac)
        || !digits(&int_part)
        || (int_part.is_empty() && frac.is_empty())
    {
        return None;
    }
    let int: i64 = if int_part.is_empty() {
        0
    } else {
        int_part.parse().ok()?
    };
    let cents: i64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().ok()? * 10,
        _ => frac.parse().ok()?,
    };
    let value = int.checked_mul(100)?.checked_add(cents)?;
    Some(if negative { -value } else { value })
}

/// Deutsche Schreibweise: `1.234,56 €`, `-3,10 €`. Zwischen Zahl und Symbol ein geschütztes Leerzeichen.
pub fn format_amount(minor: i64, currency: &str) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let abs = minor.unsigned_abs();
    let int = (abs / 100).to_string();
    let mut grouped = String::with_capacity(int.len() + int.len() / 3);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            grouped.push('.');
        }
        grouped.push(c);
    }
    let symbol = match currency {
        "EUR" => "€",
        "USD" => "$",
        "GBP" => "£",
        other => other,
    };
    format!("{sign}{grouped},{:02}\u{a0}{symbol}", abs % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eingaben() {
        let faelle = [
            ("12", Some(1200)),
            ("12,5", Some(1250)),
            ("12,34", Some(1234)),
            ("1.234,56", Some(123_456)),
            ("1.234", Some(123_400)),
            ("1.234.567", Some(123_456_700)),
            ("12.34", Some(1234)),
            ("0,99", Some(99)),
            (",5", Some(50)),
            ("-3,10", Some(-310)),
            ("−3,10 €", Some(-310)),
            ("+5", Some(500)),
            (" 12,00 € ", Some(1200)),
            ("", None),
            ("-", None),
            ("abc", None),
            ("1,234", None),
            ("1,2,3", None),
            ("99999999999999999999", None),
        ];
        for (eingabe, erwartet) in faelle {
            assert_eq!(parse_amount(eingabe), erwartet, "Eingabe {eingabe:?}");
        }
    }

    #[test]
    fn ausgaben() {
        assert_eq!(format_amount(0, "EUR"), "0,00\u{a0}€");
        assert_eq!(format_amount(5, "EUR"), "0,05\u{a0}€");
        assert_eq!(format_amount(-310, "EUR"), "-3,10\u{a0}€");
        assert_eq!(format_amount(123_456, "EUR"), "1.234,56\u{a0}€");
        assert_eq!(format_amount(100_000_000, "USD"), "1.000.000,00\u{a0}$");
        assert_eq!(
            format_amount(i64::MIN, "CHF"),
            "-92.233.720.368.547.758,08\u{a0}CHF"
        );
    }

    #[test]
    fn hin_und_zurueck() {
        for cents in [0, 1, 99, 100, 123_456, -987_654_321] {
            assert_eq!(parse_amount(&format_amount(cents, "EUR")), Some(cents));
        }
    }
}
