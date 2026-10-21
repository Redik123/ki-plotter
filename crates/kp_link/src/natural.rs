//! Natural ordering for port and printer names: digit runs compare by value
//! (COM2 < COM10, /dev/ttyUSB2 < /dev/ttyUSB10), letters case-insensitively.

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

pub(crate) fn cmp(a: &str, b: &str) -> Ordering {
    let (mut ia, mut ib) = (a.chars().peekable(), b.chars().peekable());
    loop {
        let ord = match (ia.peek().copied(), ib.peek().copied()) {
            // Total order: fall back to the raw strings ("com4" vs "COM4", "COM04" vs "COM4").
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (da, db) = (digits(&mut ia), digits(&mut ib));
                let (da, db) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                // No overflow: longer (zero-trimmed) run is the bigger number.
                da.len().cmp(&db.len()).then_with(|| da.cmp(db))
            }
            (Some(x), Some(y)) => {
                ia.next();
                ib.next();
                x.to_lowercase().cmp(y.to_lowercase())
            }
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
}

fn digits(it: &mut Peekable<Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(c) = it.next_if(char::is_ascii_digit) {
        s.push(c);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::cmp;

    #[test]
    fn sorts_naturally() {
        let mut v = vec![
            "COM10",
            "COM2",
            "COM1",
            "/dev/ttyUSB10",
            "/dev/ttyUSB2",
            "com3",
            "COM100000000000000000000000",
        ];
        v.sort_by(|a, b| cmp(a, b));
        assert_eq!(
            v,
            vec![
                "/dev/ttyUSB2",
                "/dev/ttyUSB10",
                "COM1",
                "COM2",
                "com3",
                "COM10",
                "COM100000000000000000000000",
            ]
        );
    }
}
