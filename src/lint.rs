//! Warnings about likely mistakes that the expression alone reveals.

use jiff::civil::date;

use crate::describe::{MONTHS, list, meaning, numbers};
use crate::error::Warning;
use crate::expr::Parser;
use crate::schedule::Kind::{DayOfMonth, DayOfWeek, Hour, Minute, Month, Second};
use crate::schedule::{DayRule, Dialect, Schedule};

/// The static warnings for a schedule, in a fixed order.
pub fn lint(s: &Schedule) -> Vec<Warning> {
    let mut out = vec![];
    if s.reboot {
        return out;
    }
    let mut warn = |code, message: String| out.push(Warning::new(code, message));
    let text = |kind| s.field(kind).map_or("", |f| f.text.as_str());
    let count = |kind| s.values(kind).len();

    match s.day_rule {
        DayRule::Or if count(DayOfMonth) < 31 && count(DayOfWeek) < 7 => warn(
            "dom_dow_or",
            "Runs when the day of month OR the day of week matches, not only when both do.".into(),
        ),
        DayRule::And => {
            let field = if text(DayOfMonth).starts_with('*') {
                "day of month"
            } else {
                "day of week"
            };
            warn(
                "dom_dow_and",
                format!(
                    "The {field} field starts with '*', so cron runs only when both day fields match. Kubernetes reads it the other way: either may match."
                ),
            );
        }
        _ => {}
    }

    let dow = text(DayOfWeek);
    if s.dialect != Dialect::Unix && numeric_day(dow) {
        warn(
            "dow_one_is_sunday",
            format!(
                "Day 1 is Sunday here, so {dow} means {}. Use names like MON-FRI to avoid mistakes.",
                meaning(s, DayOfWeek)
            ),
        );
    }

    if count(Second) == 60 {
        warn(
            "every_second",
            "Runs every second. Set the seconds field to 0 to run once a minute.".into(),
        );
    } else if count(Minute) == 60 && count(Hour) < 24 {
        warn(
            "every_minute",
            format!(
                "Runs every minute while the hour is {}: {} runs a day. Set the minute field to 0 to run once at the start of the hour.",
                numbers(&s.values(Hour)),
                count(Second) * 60 * count(Hour)
            ),
        );
    }

    for (kind, span, cycle) in [
        (Second, 60, "minute"),
        (Minute, 60, "hour"),
        (Hour, 24, "day"),
    ] {
        let t = text(kind);
        let step = t.strip_prefix("*/").or(t.strip_prefix("0/"));
        if let Some(step) = step.and_then(|n| n.parse::<u16>().ok())
            && step < span
            && span % step != 0
        {
            let unit = kind.words();
            let gap = span - (span - 1) / step * step;
            let units = if gap == 1 {
                unit.to_string()
            } else {
                format!("{unit}s")
            };
            warn(
                "uneven_step",
                format!(
                    "The {unit} step /{step} restarts at 0 each {cycle}, so the last gap is {gap} {units}, not {step}."
                ),
            );
        }
    }

    let parser = Parser(s.dialect);
    for f in &s.fields {
        for item in f.text.split(',') {
            // A step of 1 skips nothing: a single value then comes from the range alone.
            if let Some((_, step)) = item.split_once('/')
                && step.parse::<u32>().is_ok_and(|n| n > 1)
                && let Ok(&[only]) = parser.values(f.kind, item).as_deref()
            {
                warn(
                    "big_step",
                    format!(
                        "Step /{step} is larger than the {} range, so it matches only {only}.",
                        f.kind.words()
                    ),
                );
            }
        }
    }

    if s.day_rule == DayRule::DomOnly && !text(DayOfMonth).starts_with('*') {
        let (days, months) = (s.values(DayOfMonth), s.values(Month));
        let length = |year, m: u16| date(year, m as i8, 1).days_in_month() as u16;
        // 2000 is a leap year, 2001 isn't.
        if months
            .iter()
            .any(|&m| days.iter().any(|&d| d <= length(2000, m)))
        {
            for &d in days.iter().filter(|&&d| d >= 29) {
                let short = months.iter().filter(|&&m| length(2001, m) < d);
                let short: Vec<String> = short.map(|&m| MONTHS[m as usize - 1].into()).collect();
                if short.is_empty() {
                    continue;
                }
                let when = if d == 29 {
                    "February except in leap years".into()
                } else {
                    list(short, "and")
                };
                warn(
                    "short_months",
                    format!("Day {d} doesn't exist in every month: no run on it in {when}."),
                );
            }
        }
    }

    if s.dialect == Dialect::Aws {
        for f in &s.fields {
            for item in f.text.split(',') {
                let up = item.to_ascii_uppercase();
                let range = up.split('/').next().unwrap_or_default();
                let undocumented = match f.kind {
                    DayOfMonth if up == "LW" || up.starts_with("L-") => true,
                    DayOfWeek if up.len() > 1 && up.ends_with('L') => true,
                    _ if up.contains('/') && (range == "*" || range.contains('-')) => true,
                    // Wrap-around ranges such as FRI-MON or 22-2.
                    _ => {
                        range.contains('-')
                            && parser
                                .values(f.kind, range)
                                .is_ok_and(|v| v.first() > v.last())
                    }
                };
                if undocumented {
                    warn(
                        "aws_undocumented",
                        format!(
                            "AWS doesn't document {item}; check it in the EventBridge console before relying on it."
                        ),
                    );
                }
            }
        }
    }
    out
}

/// Whether a Quartz or AWS day of week names a day by number (`1-5`, `6L`, `2#1`, not `MON#2`).
fn numeric_day(text: &str) -> bool {
    text.split([',', '-']).any(|term| {
        let day = term.split(['/', '#']).next().unwrap_or_default();
        let day = day.trim_end_matches(['L', 'l']);
        !day.is_empty() && day.bytes().all(|b| b.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::parse;

    fn warnings(expr: &str) -> Vec<String> {
        let s = parse(expr, None).unwrap();
        lint(&s)
            .into_iter()
            .map(|w| format!("{}: {}", w.code, w.message))
            .collect()
    }

    #[test]
    #[rustfmt::skip]
    fn warns_about_each_mistake() {
        let cases: &[(&str, &[&str])] = &[
            ("0 9 1-15 * */2", &["dom_dow_and: The day of week field starts with '*', so cron runs only when both day fields match. Kubernetes reads it the other way: either may match."]),
            ("cron(0 12 ? * 6L *)", &["dow_one_is_sunday: Day 1 is Sunday here, so 6L means the last Friday of the month. Use names like MON-FRI to avoid mistakes.", "aws_undocumented: AWS doesn't document 6L; check it in the EventBridge console before relying on it."]),
            ("* 9-17 * * *", &["every_minute: Runs every minute while the hour is 9 through 17: 540 runs a day. Set the minute field to 0 to run once at the start of the hour."]),
            ("* * 9 * * ?", &["every_second: Runs every second. Set the seconds field to 0 to run once a minute."]),
            ("*/7 * * * *", &["uneven_step: The minute step /7 restarts at 0 each hour, so the last gap is 4 minutes, not 7."]),
            ("0/59 * * * * ?", &["uneven_step: The second step /59 restarts at 0 each minute, so the last gap is 1 second, not 59."]),
            ("0 */5 * * *", &["uneven_step: The hour step /5 restarts at 0 each day, so the last gap is 4 hours, not 5."]),
            ("*/90 * * * *", &["big_step: Step /90 is larger than the minute range, so it matches only 0."]),
            ("0 0 10-20/15 * *", &["big_step: Step /15 is larger than the day of month range, so it matches only 10."]),
            ("0 0 31 * *", &["short_months: Day 31 doesn't exist in every month: no run on it in February, April, June, September and November."]),
            ("0 0 29 * *", &["short_months: Day 29 doesn't exist in every month: no run on it in February except in leap years."]),
            ("0 0 0 15,30 2 ?", &["short_months: Day 30 doesn't exist in every month: no run on it in February."]),
            ("cron(0 12 LW * ? *)", &["aws_undocumented: AWS doesn't document LW; check it in the EventBridge console before relying on it."]),
            ("cron(0 12 L-2 * ? *)", &["aws_undocumented: AWS doesn't document L-2; check it in the EventBridge console before relying on it."]),
            ("cron(0 22-2 * * ? *)", &["aws_undocumented: AWS doesn't document 22-2; check it in the EventBridge console before relying on it."]),
            ("cron(0 12 ? * FRI-MON *)", &["aws_undocumented: AWS doesn't document FRI-MON; check it in the EventBridge console before relying on it."]),
            ("cron(0-30/10 * * * ? *)", &["aws_undocumented: AWS doesn't document 0-30/10; check it in the EventBridge console before relying on it."]),
            ("cron(*/10 * * * ? *)", &["aws_undocumented: AWS doesn't document */10; check it in the EventBridge console before relying on it."]),
        ];
        for &(expr, want) in cases {
            assert_eq!(warnings(expr), want, "{expr}");
        }
    }

    /// The static warnings each next-run golden carries.
    #[test]
    #[rustfmt::skip]
    fn matches_the_next_run_goldens() {
        const OR: &str = "dom_dow_or: Runs when the day of month OR the day of week matches, not only when both do.";
        let cases: &[(&str, &[&str])] = &[
            ("0 9 1 * MON", &[OR]),
            ("0 9 */2 * MON", &["dom_dow_and: The day of month field starts with '*', so cron runs only when both day fields match. Kubernetes reads it the other way: either may match."]),
            ("0 0 1,15 * FRI", &[OR]),
            ("0 0 29 2 *", &["short_months: Day 29 doesn't exist in every month: no run on it in February except in leap years."]),
            ("0 15 10 ? * 6L", &["dow_one_is_sunday: Day 1 is Sunday here, so 6L means the last Friday of the month. Use names like MON-FRI to avoid mistakes."]),
            ("0 0 12 ? * 1-5", &["dow_one_is_sunday: Day 1 is Sunday here, so 1-5 means Sunday through Thursday. Use names like MON-FRI to avoid mistakes."]),
        ];
        for &(expr, want) in cases {
            assert_eq!(warnings(expr), want, "{expr}");
        }
        for expr in [
            "0 0 * * 7", "@daily", "30 2 * * *", "0,30 2 * * *", "*/15 2 * * *", "30 1 * * *",
            "*/30 1 * * *", "0 * * * *", "0 30 1 * * ?", "0 30 2 * * ?", "cron(30 2 * * ? *)",
            "cron(30 1 * * ? *)", "0 0 * * *", "0 0 12 ? * MON#2", "0 0 9 15W * ?", "0 0 9 LW * ?",
            "0 0 12 L-3 * ?", "0 0 12 ? * L", "cron(0 18 ? * MON-FRI *)", "0 0 30 2 *",
        ] {
            assert_eq!(warnings(expr), Vec::<String>::new(), "{expr}");
        }
    }

    #[test]
    fn keeps_quiet_about_sound_schedules() {
        for expr in [
            "0 9 * * 1-5",
            "*/15 9-17 * * 1-5",
            "0 9 1-31 * MON",
            "0 0 */2 * *",
            "0 0/20 * * * ?",
            "cron(0/15 * * * ? *)",
            "cron(0 12 L * ? *)",
            "@hourly",
            "@reboot",
        ] {
            assert_eq!(warnings(expr), Vec::<String>::new(), "{expr}");
        }
    }
}
