//! Rewrites a schedule for another dialect.

use crate::describe::{numbers, step};
use crate::error::{Error, Warning};
use crate::expr::{DAYS, parse};
use crate::lint::lint;
use crate::runs;
use crate::schedule::Dialect::{self, Aws, Quartz, Unix};
use crate::schedule::Kind::{self, DayOfMonth, DayOfWeek, Hour, Minute, Month, Second, Year};
use crate::schedule::{DayRule, DomSpecial, Dow, Schedule};

/// The schedule written in `to`, with its warnings, given only once each result is shown to
/// run at the same times as the source.
pub fn convert(s: &Schedule, to: Dialect) -> Result<(Vec<String>, Vec<Warning>), Error> {
    let mut results = rewrite(s, to)?;
    // A split part on days that never occur (day 30 in February) adds no runs: leave it out.
    results.retain(|r| parse(r, Some(to)).is_ok_and(|r| runs::ever(&r, 1).is_ok()));
    let mut warnings = vec![];
    if results.len() == 2 {
        let many = if to == Quartz {
            "triggers"
        } else {
            "rules or schedules"
        };
        warnings.push(Warning::new(
            "split",
            format!(
                "{} can't match day of month OR day of week in one expression, so this needs two {many}.",
                to.title()
            ),
        ));
    }
    warnings.extend(check(s, to, &results)?);
    // The self-check runs in UTC, where no clock changes, so say how the rules differ.
    if to != s.dialect {
        let result = parse(&results[0], Some(to)).expect("the self-check read it");
        warnings.push(Warning::new(
            "dst_rules",
            format!(
                "DST rules differ: {}, while {}.",
                runs::dst_rules(&result),
                runs::dst_rules(s)
            ),
        ));
    }
    Ok((results, warnings))
}

/// Re-reads each result and compares the first 100 runs from 1970 in UTC with the source's
/// (the results' together, for a split). Returns the results' warnings about AWS
/// syntax that AWS doesn't document, the only ones the source can't have.
fn check(s: &Schedule, to: Dialect, results: &[String]) -> Result<Vec<Warning>, Error> {
    let failed = || {
        Error::new(
            "self_check",
            "The conversion didn't reproduce the same run times, so no result was given.",
        )
        .hint("Please report this expression.")
    };
    let want = runs::ever(s, 100)?;
    let (mut got, mut warnings) = (vec![], vec![]);
    for text in results {
        let result = parse(text, Some(to)).map_err(|_| failed())?;
        got.extend(runs::ever(&result, 100).map_err(|_| failed())?);
        warnings.extend(
            lint(&result)
                .into_iter()
                .filter(|w| w.code == "aws_undocumented"),
        );
    }
    got.sort();
    got.dedup();
    got.truncate(100);
    if got == want {
        Ok(warnings)
    } else {
        Err(failed())
    }
}

/// The schedule written in `to`: one expression, or two when Quartz or AWS must split cron's
/// "day of month OR day of week" into separate triggers.
fn rewrite(s: &Schedule, to: Dialect) -> Result<Vec<String>, Error> {
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
        // Cron ANDs the day fields only while one starts with `*`.
        DayRule::And if to == Unix => vec![starred(
            s,
            [DayOfMonth, DayOfWeek],
            (dom(s, to, true), dow(s, to, true)),
        )],
        DayRule::And => {
            return Err(not(
                "it needs both a day of month and a day of week to match, and Quartz and AWS can only use one".into(),
            )
            .hint("Use one day field and check the other inside the job."));
        }
    };

    // A Unix job keeps its DST class: cron replays a fixed-time job's runs across a shift,
    // while a job whose minute or hour starts with `*` skips gaps and runs folds twice.
    let star = !(s.dialect == Unix && s.fixed_time);
    let second = field(&seconds, Second, to, true);
    let (mut minute, mut hour) = (
        field(&s.values(Minute), Minute, to, star),
        field(&s.values(Hour), Hour, to, star),
    );
    if to == Unix && s.dialect == Unix && star {
        (minute, hour) = starred(s, [Minute, Hour], (minute, hour));
    }
    let month = field(&s.values(Month), Month, to, true);
    let write = |(d, w): (String, String)| match to {
        Unix => format!("{minute} {hour} {d} {month} {w}"),
        Quartz if year == "*" => format!("{second} {minute} {hour} {d} {month} {w}"),
        Quartz => format!("{second} {minute} {hour} {d} {month} {w} {year}"),
        Aws => format!("cron({minute} {hour} {d} {month} {w} {year})"),
    };
    Ok(days.into_iter().map(write).collect())
}

/// A written pair of Unix fields, with the source's field that starts with `*` put back as
/// written when neither result kept a leading `*`, which cron reads as a different rule.
fn starred(s: &Schedule, kinds: [Kind; 2], (a, b): (String, String)) -> (String, String) {
    if a.starts_with('*') || b.starts_with('*') {
        return (a, b);
    }
    let text = |kind| s.field(kind).map_or("*", |f| &f.text).to_string();
    if text(kinds[0]).starts_with('*') {
        (text(kinds[0]), b)
    } else {
        (a, text(kinds[1]))
    }
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

    fn to(text: &str, to: Dialect) -> Result<Vec<String>, (String, Option<String>)> {
        let (results, _) =
            convert(&parse(text, None).unwrap(), to).map_err(|e| (e.message, e.hint))?;
        Ok(results)
    }

    fn warnings(text: &str, to: Dialect) -> Vec<String> {
        let (_, warnings) = convert(&parse(text, None).unwrap(), to).unwrap();
        warnings
            .into_iter()
            .filter(|w| w.code != "dst_rules")
            .map(|w| w.message)
            .collect()
    }

    #[test]
    fn warns_about_splits_and_undocumented_aws_syntax() {
        let split = "can't match day of month OR day of week in one expression, so this needs two";
        assert_eq!(
            warnings("0 0 1,15 * FRI", Quartz),
            [format!("Quartz {split} triggers.")]
        );
        assert_eq!(
            warnings("0 0 1,15 * FRI", Aws),
            [format!("AWS cron {split} rules or schedules.")]
        );
        assert_eq!(
            warnings("0 0 9 LW * ?", Aws),
            ["AWS doesn't document LW; check it in the EventBridge console before relying on it."]
        );
        // The source's own warnings belong to explain, not to the conversion.
        assert!(warnings("0 0 12 ? * MON#2", Aws).is_empty());
        assert!(warnings("*/7 * * * *", Quartz).is_empty());
    }

    #[test]
    fn says_how_dst_rules_differ_across_dialects() {
        let rules = |text: &str, to| {
            let (_, warnings) = convert(&parse(text, None).unwrap(), to).unwrap();
            warnings
                .into_iter()
                .filter(|w| w.code == "dst_rules")
                .map(|w| w.message)
                .collect::<Vec<_>>()
        };
        let fixed = "Unix cron runs a time in a gap right after the jump and a repeated time once, the first time, when clocks change by up to 3 hours";
        let quartz = "Quartz skips times in a gap and runs a repeated time once, the second time";
        assert_eq!(
            rules("30 2 * * *", Quartz),
            [format!("DST rules differ: {quartz}, while {fixed}.")]
        );
        assert_eq!(
            rules("0 0/30 2 * * ?", Unix),
            [format!("DST rules differ: {fixed}, while {quartz}.")]
        );
        assert_eq!(
            rules("*/30 2 * * *", Aws),
            [
                "DST rules differ: EventBridge Scheduler skips times in a gap and runs a repeated time once, the first time, while Unix cron skips times in a gap and runs a repeated time both times."
            ]
        );
        assert!(rules("*/30 2 * * *", Unix).is_empty());
        assert!(rules("0 30 2 * * ?", Quartz).is_empty());
    }

    #[test]
    fn refuses_a_result_that_runs_at_other_times() {
        let s = parse("0 9 * * *", None).unwrap();
        let fail = |result: &str| check(&s, Unix, &[result.into()]).unwrap_err();
        for result in ["0 10 * * *", "0 9 * *", "0 9 30 2 *"] {
            let e = fail(result);
            assert_eq!(e.code, "self_check");
            assert_eq!(e.exit_code(), 3);
            assert_eq!(
                e.message,
                "The conversion didn't reproduce the same run times, so no result was given."
            );
            assert_eq!(e.hint.as_deref(), Some("Please report this expression."));
        }
        assert!(check(&s, Unix, &["0 9 * * 0-6".into()]).is_ok());
        // A split passes when its results together match the source.
        let or = parse("0 0 1,15 * FRI", None).unwrap();
        let parts = ["0 0 0 1,15 * ?".into(), "0 0 0 ? * FRI".into()];
        assert!(check(&or, Quartz, &parts).is_ok());
        assert!(check(&or, Quartz, &parts[..1]).is_err());
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
            // February 30 never comes, so only the Mondays are left.
            ("0 0 30 2 MON", Quartz, "0 0 0 ? 2 MON"),
            // Every day of the month OR Monday is every day.
            ("0 9 1-31 * MON", Quartz, "0 0 9 * * ?"),
            // A fixed-time job stays fixed-time; a wildcard one stays a wildcard.
            ("0,20,40 2 * * *", Unix, "0-40/20 2 * * *"),
            ("0 0/20 2 * * ?", Unix, "*/20 2 * * *"),
            ("*/30 2 * * *", Unix, "*/30 2 * * *"),
            ("0 */12 * * *", Unix, "0 */12 * * *"),
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
