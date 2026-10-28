//! Rewrites a schedule for another dialect.

use crate::describe::{numbers, step};
use crate::error::Error;
use crate::expr::DAYS;
use crate::schedule::Dialect::{self, Aws, Quartz, Unix};
use crate::schedule::Kind::{self, DayOfMonth, DayOfWeek, Hour, Minute, Month, Second, Year};
use crate::schedule::{DayRule, DomSpecial, Dow, Schedule};

/// The schedule written in `to`: one expression, or two when Quartz or AWS must split cron's
/// "day of month OR day of week" into separate triggers.
pub fn convert(s: &Schedule, to: Dialect) -> Result<Vec<String>, Error> {
    let not = |reason: String| {
        Error::new(
            "not_convertible",
            format!("{} can't express this: {reason}.", to.title()),
        )
    };
    if s.reboot {
        return Err(not("@reboot has no schedule".into()));
    }
    let seconds = s.values(Second);
    let year = field(&s.values(Year), Year, to, true);
    match to {
        Unix if seconds != [0] => {
            let at = match &seconds[..] {
                [one] => format!("second {one}"),
                _ => format!("seconds {}", numbers(&seconds)),
            };
            return Err(not(format!(
                "Unix cron has no seconds field (this runs at {at})"
            )));
        }
        Aws if seconds != [0] => return Err(not("AWS cron has no seconds field".into())),
        Unix if year != "*" => return Err(not("Unix cron has no year field".into())),
        Unix if !s.dom.specials.is_empty() || !matches!(s.dow, Dow::Days(_)) => {
            return Err(not("Unix cron has no L, W or #".into()));
        }
        _ => {}
    }

    let all_days = s.dom.specials.is_empty() && s.values(DayOfMonth).len() == 31;
    let all_weekdays = s.dow == Dow::Days((0..7).collect());
    let every_day = match s.day_rule {
        DayRule::DomOnly => all_days,
        DayRule::DowOnly => all_weekdays,
        DayRule::Or => all_days || all_weekdays,
        DayRule::And => false,
    };
    let any = if to == Unix { "*" } else { "?" };
    let days: Vec<(String, String)> = match s.day_rule {
        _ if every_day => vec![("*".into(), any.into())],
        DayRule::DomOnly => vec![(dom(s, to, true), any.into())],
        DayRule::DowOnly => vec![(any.into(), dow(s, to, true))],
        // Neither field may start with `*`, or cron would AND them.
        DayRule::Or if to == Unix => vec![(dom(s, to, false), dow(s, to, false))],
        DayRule::Or => vec![
            (dom(s, to, true), "?".into()),
            ("?".into(), dow(s, to, true)),
        ],
        DayRule::And if to == Unix => {
            let (mut d, mut w) = (dom(s, to, true), dow(s, to, true));
            // Cron ANDs the day fields only while one starts with `*`: keep that one as written.
            if !d.starts_with('*') && !w.starts_with('*') {
                let text = |kind| s.field(kind).map_or("*", |f| &f.text).to_string();
                if text(DayOfMonth).starts_with('*') {
                    d = text(DayOfMonth);
                } else {
                    w = text(DayOfWeek);
                }
            }
            vec![(d, w)]
        }
        DayRule::And => {
            return Err(not(
                "it needs both a day of month and a day of week to match, and Quartz and AWS can only use one".into(),
            )
            .hint("Use one day field and check the other inside the job."));
        }
    };

    // A Unix fixed-time job stays one, so cron keeps replaying its runs across DST changes.
    let star = !(s.dialect == Unix && s.fixed_time);
    let second = field(&seconds, Second, to, true);
    let minute = field(&s.values(Minute), Minute, to, star);
    let hour = field(&s.values(Hour), Hour, to, star);
    let month = field(&s.values(Month), Month, to, true);
    let write = |(d, w): (String, String)| match to {
        Unix => format!("{minute} {hour} {d} {month} {w}"),
        Quartz if year == "*" => format!("{second} {minute} {hour} {d} {month} {w}"),
        Quartz => format!("{second} {minute} {hour} {d} {month} {w} {year}"),
        Aws => format!("cron({minute} {hour} {d} {month} {w} {year})"),
    };
    Ok(days.into_iter().map(write).collect())
}

/// Day of month: plain days, then `L`, `L-n`, `LW`, `L-nW` and `nW`.
fn dom(s: &Schedule, to: Dialect, star: bool) -> String {
    let days = s.values(DayOfMonth);
    let plain = (!days.is_empty()).then(|| field(&days, DayOfMonth, to, star));
    let specials = s.dom.specials.iter().map(|&special| match special {
        DomSpecial::Last { offset, weekday } => {
            let offset = if offset > 0 {
                format!("-{offset}")
            } else {
                String::new()
            };
            format!("L{offset}{}", if weekday { "W" } else { "" })
        }
        DomSpecial::Nearest(n) => format!("{n}W"),
    });
    plain
        .into_iter()
        .chain(specials)
        .collect::<Vec<_>>()
        .join(",")
}

/// Day of week; `L` and `#` keep the numeric form AWS documents (1 = Sunday).
fn dow(s: &Schedule, to: Dialect, star: bool) -> String {
    match s.dow {
        Dow::Days(set) => field(&set.iter().collect::<Vec<_>>(), DayOfWeek, to, star),
        Dow::Last(d) => format!("{}L", d + 1),
        Dow::Nth(d, n) => format!("{}#{n}", d + 1),
    }
}

/// One field's values as `to` writes them. `star` allows a leading `*`, which Unix cron reads
/// as "unrestricted" in a day field and as "not a fixed time" in the minute and hour fields.
fn field(v: &[u16], kind: Kind, to: Dialect, star: bool) -> String {
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
    use crate::expr::parse;

    fn to(text: &str, to: Dialect) -> Result<Vec<String>, (String, Option<String>)> {
        convert(&parse(text, None).unwrap(), to).map_err(|e| (e.message, e.hint))
    }

    #[test]
    #[rustfmt::skip]
    fn matches_every_conversion_golden() {
        let cases: &[(&str, Dialect, &[&str])] = &[
            ("0 9 * * 1-5", Quartz, &["0 0 9 ? * MON-FRI"]),
            ("0 9 * * 1-5", Aws, &["cron(0 9 ? * MON-FRI *)"]),
            ("*/15 9-17 * * 1-5", Quartz, &["0 0/15 9-17 ? * MON-FRI"]),
            ("*/15 9-17 * * 1-5", Aws, &["cron(0/15 9-17 ? * MON-FRI *)"]),
            ("0 0 * * 0", Quartz, &["0 0 0 ? * SUN"]),
            ("@daily", Quartz, &["0 0 0 * * ?"]),
            ("@daily", Unix, &["0 0 * * *"]),
            ("0 0 1,15 * FRI", Quartz, &["0 0 0 1,15 * ?", "0 0 0 ? * FRI"]),
            ("0 0 1,15 * FRI", Aws, &["cron(0 0 1,15 * ? *)", "cron(0 0 ? * FRI *)"]),
            ("0 30 9 ? * 2-6", Unix, &["30 9 * * 1-5"]),
            ("cron(0 18 ? * MON-FRI *)", Unix, &["0 18 * * 1-5"]),
            ("cron(0 18 ? * MON-FRI *)", Quartz, &["0 0 18 ? * MON-FRI"]),
            ("0 0 12 1 1 ? 2030", Aws, &["cron(0 12 1 1 ? 2030)"]),
            ("0 0 12 ? * MON#2", Aws, &["cron(0 12 ? * 2#2 *)"]),
        ];
        for &(text, dialect, want) in cases {
            assert_eq!(to(text, dialect), Ok(want.iter().map(|w| w.to_string()).collect()), "{text} to {dialect:?}");
        }
    }

    #[test]
    #[rustfmt::skip]
    fn keeps_specials_and_cron_day_rules() {
        let cases: &[(&str, Dialect, &str)] = &[
            ("0 0 12 L-3 * ?", Aws, "cron(0 12 L-3 * ? *)"),
            ("cron(0 9 LW * ? *)", Quartz, "0 0 9 LW * ?"),
            ("0 0 9 15W,L-2W * ?", Quartz, "0 0 9 15W,L-2W * ?"),
            ("0 0 12 L,15 * ?", Quartz, "0 0 12 15,L * ?"),
            ("0 15 10 ? * 6L", Aws, "cron(15 10 ? * 6L *)"),
            ("0 0 12 ? * L", Unix, "0 12 * * 6"),
            ("0 0 22-2 ? * FRI-MON", Unix, "0 0-2,22,23 * * 0,1,5,6"),
            // Cron's OR rule needs both day fields free of a leading `*`, its AND rule one.
            ("0 0 1-31/2 * MON", Unix, "0 0 1-31/2 * 1"),
            ("0 9 */2 * MON", Unix, "0 9 */2 * 1"),
            ("0 9 1-15 * */2", Unix, "0 9 1-15 * */2"),
            ("0 9 */2,4 * MON", Unix, "0 9 */2,4 * 1"),
            // Every day of the month OR Monday is every day.
            ("0 9 1-31 * MON", Quartz, "0 0 9 * * ?"),
            // A fixed-time job stays fixed-time; a wildcard one stays a wildcard.
            ("0,20,40 2 * * *", Unix, "0-40/20 2 * * *"),
            ("0 0/20 2 * * ?", Unix, "*/20 2 * * *"),
            ("cron(0 12 1 1 ? 2026/4)", Quartz, "0 0 12 1 1 ? 2026/4"),
        ];
        for &(text, dialect, want) in cases {
            assert_eq!(to(text, dialect), Ok(vec![want.to_string()]), "{text} to {dialect:?}");
        }
    }

    #[test]
    #[rustfmt::skip]
    fn says_what_the_target_cant_express() {
        const AND: &str = "it needs both a day of month and a day of week to match, and Quartz and AWS can only use one.";
        let hint = Some("Use one day field and check the other inside the job.".to_string());
        let cases: &[(&str, Dialect, String, Option<String>)] = &[
            ("0 9 */2 * MON", Quartz, format!("Quartz can't express this: {AND}"), hint.clone()),
            ("0 9 */2 * MON", Aws, format!("AWS cron can't express this: {AND}"), hint),
            ("0 0 12 ? * MON#2", Unix, "Unix cron can't express this: Unix cron has no L, W or #.".into(), None),
            ("0 0 12 L * ?", Unix, "Unix cron can't express this: Unix cron has no L, W or #.".into(), None),
            ("15 0 9 * * ?", Unix, "Unix cron can't express this: Unix cron has no seconds field (this runs at second 15).".into(), None),
            ("*/30 0 9 * * ?", Unix, "Unix cron can't express this: Unix cron has no seconds field (this runs at seconds 0 and 30).".into(), None),
            ("15 0 9 * * ?", Aws, "AWS cron can't express this: AWS cron has no seconds field.".into(), None),
            ("0 0 12 1 1 ? 2030", Unix, "Unix cron can't express this: Unix cron has no year field.".into(), None),
            ("@reboot", Quartz, "Quartz can't express this: @reboot has no schedule.".into(), None),
        ];
        for (text, dialect, message, hint) in cases {
            assert_eq!(to(text, *dialect), Err((message.clone(), hint.clone())), "{text} to {dialect:?}");
        }
        let e = convert(&parse("@reboot", None).unwrap(), Unix).unwrap_err();
        assert_eq!(e.code, "not_convertible");
    }

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
