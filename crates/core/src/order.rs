//! Fraktionale Indizes für manuelle Sortierung: Zwischen zwei Schlüsseln lässt sich immer
//! ein neuer erzeugen, ohne andere Einträge umzuschreiben. Gleichstände (zwei Geräte
//! sortieren gleichzeitig an dieselbe Stelle) werden über die Entity-ID aufgelöst.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Schlüssel, der echt zwischen `before` und `after` liegt (`None` = Anfang bzw. Ende).
///
/// # Panics
/// Wenn `before >= after` oder ein Schlüssel ungültig ist (fremde Zeichen, endet auf `0`).
pub fn between(before: Option<&str>, after: Option<&str>) -> String {
    let a = before.unwrap_or("");
    assert!(before.is_none_or(is_valid), "ungültiger Schlüssel {a:?}");
    assert!(after.is_none_or(is_valid), "ungültiger Schlüssel {after:?}");
    if let Some(b) = after {
        assert!(a < b, "{a:?} muss kleiner als {b:?} sein");
    }
    let key = midpoint(a.as_bytes(), after.map(str::as_bytes));
    String::from_utf8(key).expect("DIGITS ist ASCII")
}

pub fn is_valid(key: &str) -> bool {
    !key.is_empty() && !key.ends_with('0') && key.bytes().all(|c| DIGITS.contains(&c))
}

fn digit_index(c: u8) -> usize {
    DIGITS.iter().position(|&d| d == c).expect("validiert")
}

fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(DIGITS[0]) == b[n] {
            n += 1;
        }
        if n > 0 {
            let mut out = b[..n].to_vec();
            out.extend(midpoint(a.get(n..).unwrap_or(&[]), Some(&b[n..])));
            return out;
        }
    }
    let da = a.first().map_or(0, |&c| digit_index(c));
    let db = b.map_or(DIGITS.len(), |b| digit_index(b[0]));
    if db - da > 1 {
        vec![DIGITS[(da + db).div_ceil(2)]]
    } else if let Some(b) = b.filter(|b| b.len() > 1) {
        vec![b[0]]
    } else {
        let mut out = vec![DIGITS[da]];
        out.extend(midpoint(a.get(1..).unwrap_or(&[]), None));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_sorted_valid(keys: &[String]) {
        for w in keys.windows(2) {
            assert!(w[0] < w[1], "{:?} < {:?}", w[0], w[1]);
        }
        assert!(keys.iter().all(|k| is_valid(k)), "{keys:?}");
    }

    #[test]
    fn append_and_prepend() {
        let mut keys = vec![between(None, None)];
        for _ in 0..500 {
            keys.push(between(keys.last().map(String::as_str), None));
            keys.insert(0, between(None, keys.first().map(String::as_str)));
        }
        assert_sorted_valid(&keys);
    }

    #[test]
    fn repeated_insert_between_neighbours() {
        let mut keys = vec![between(None, None)];
        keys.push(between(Some(&keys[0]), None));
        // immer direkt hinter dem ersten Element einfügen → Schlüssel werden länger, bleiben gültig
        for _ in 0..500 {
            let k = between(Some(&keys[0]), Some(&keys[1]));
            keys.insert(1, k);
        }
        assert_sorted_valid(&keys);
        // und immer direkt vor dem letzten
        for _ in 0..500 {
            let n = keys.len();
            let k = between(Some(&keys[n - 2]), Some(&keys[n - 1]));
            keys.insert(n - 1, k);
        }
        assert_sorted_valid(&keys);
    }

    #[test]
    #[should_panic(expected = "muss kleiner")]
    fn rejects_wrong_order() {
        between(Some("b"), Some("a"));
    }
}
