//! Rewrites a schedule for another dialect.

use crate::describe::step;
use crate::expr::DAYS;
use crate::schedule::Dialect::{self, Aws, Unix};
use crate::schedule::Kind::{self, DayOfWeek};

/// One field's values as `to` writes them. `star` allows a leading `*`, which Unix cron reads
/// as "unrestricted" in a day field and as "not a fixed time" in the minute and hour fields.
pub fn field(v: &[u16], kind: Kind, to: Dialect, star: bool) -> String {
    let (lo, hi) = kind.range();
    if star && v.len() == usize::from(hi - lo + 1) {
        return "*".into();
    }
    // Quartz and AWS days of the week are names, without steps.
    let names = to != Unix && kind == DayOfWeek;
    if let (Some(n), [a, .., b]) = (step(v), v)
        && v.len() >= 3
        && n > 1
        && !names
    {
        let to_end = b + n > hi;
        if to_end && to != Unix {
            return format!("{a}/{n}");
        }
        if to_end && *a == lo && star {
            return format!("*/{n}");
        }
        if to != Aws {
            return format!("{a}-{b}/{n}");
        }
    }
    let name = |x: u16| {
        if names {
            DAYS[x as usize].to_string()
        } else {
            x.to_string()
        }
    };
    let items = v.chunk_by(|a, b| a + 1 == *b).flat_map(|run| match run {
        [a, .., b] if run.len() >= 3 => vec![format!("{}-{}", name(*a), name(*b))],
        _ => run.iter().map(|&x| name(x)).collect(),
    });
    items.collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::Dialect::Quartz;
    use crate::schedule::Kind::{DayOfMonth, Hour, Minute, Month, Year};

    fn v(values: impl IntoIterator<Item = u16>) -> Vec<u16> {
        values.into_iter().collect()
    }

    #[test]
    #[rustfmt::skip]
    fn writes_each_field_in_each_dialect() {
        let quarters = v([0, 15, 30, 45]);
        // (values, field, unix, quartz, aws)
        let cases: &[(Vec<u16>, Kind, &str, &str, &str)] = &[
            (v(0..60), Minute, "*", "*", "*"),
            (quarters.clone(), Minute, "*/15", "0/15", "0/15"),
            (v([5, 20, 35, 50]), Minute, "5-50/15", "5/15", "5/15"),
            (v([9, 11, 13, 15, 17]), Hour, "9-17/2", "9-17/2", "9,11,13,15,17"),
            (v(9..=17), Hour, "9-17", "9-17", "9-17"),
            (v([0, 1, 2, 22, 23]), Hour, "0-2,22,23", "0-2,22,23", "0-2,22,23"),
            (v([0, 30]), Minute, "0,30", "0,30", "0,30"),
            (v([0]), Minute, "0", "0", "0"),
            (v((1..=31).step_by(2)), DayOfMonth, "*/2", "1/2", "1/2"),
            (v([1, 3, 5, 7, 9, 11]), Month, "*/2", "1/2", "1/2"),
            (v(1..=5), DayOfWeek, "1-5", "MON-FRI", "MON-FRI"),
            (v([0, 2, 4, 6]), DayOfWeek, "*/2", "SUN,TUE,THU,SAT", "SUN,TUE,THU,SAT"),
            (v([0, 5, 6]), DayOfWeek, "0,5,6", "SUN,FRI,SAT", "SUN,FRI,SAT"),
            (v(0..7), DayOfWeek, "*", "*", "*"),
            (v(1970..=2199), Year, "*", "*", "*"),
            (v((2026..=2199).step_by(4)), Year, "2026-2198/4", "2026/4", "2026/4"),
            (v([2030]), Year, "2030", "2030", "2030"),
        ];
        for (values, kind, unix, quartz, aws) in cases {
            let got = [Unix, Quartz, Aws].map(|to| field(values, *kind, to, true));
            assert_eq!(got, [*unix, *quartz, *aws], "{kind:?} {values:?}");
        }
        // Without a leading `*`, cron keeps its OR day rule and its fixed-time DST rule.
        assert_eq!(field(&v((1..=31).step_by(2)), DayOfMonth, Unix, false), "1-31/2");
        assert_eq!(field(&quarters, Minute, Unix, false), "0-45/15");
        assert_eq!(field(&v(0..60), Minute, Unix, false), "0-59");
    }
}
