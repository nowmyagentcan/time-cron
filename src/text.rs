//! Text for people: each answer as one string of plain lines.

use std::fmt::Write;

use crate::error::{Error, Warning};
use crate::report::{Explained, Upcoming};
use crate::runs::Dst;
use crate::schedule::Dialect;

pub fn explained(e: &Explained) -> String {
    let mut out = format!("{}\nDialect: {}", e.description, e.dialect.name());
    if !e.fields.is_empty() {
        let _ = write!(out, " ({} fields)", e.fields.len());
    }
    out.push('\n');
    let names: Vec<String> = e.fields.iter().map(|f| f.name.replace('_', " ")).collect();
    let name_width = names.iter().map(String::len).max().unwrap_or(0) + 2;
    let text_width = e.fields.iter().map(|f| f.text.len()).max().unwrap_or(0) + 2;
    for (name, f) in names.iter().zip(&e.fields) {
        let _ = writeln!(
            out,
            "  {name:name_width$}{:text_width$}{}",
            f.text, f.meaning
        );
    }
    out + &warnings(&e.warnings)
}

pub fn upcoming(u: &Upcoming) -> String {
    let rules = match u.dialect {
        Dialect::Unix => "Unix cron",
        Dialect::Quartz => "Quartz",
        Dialect::Aws => "EventBridge",
    };
    let mut out = format!(
        "{}\n{} · tzdata {} · {rules} rules\n",
        u.description, u.timezone, u.tzdata
    );
    let seconds =
        u.runs.iter().any(|r| r.time.second() != 0) || u.skipped.iter().any(|dt| dt.second() != 0);
    let clock = if seconds { "%H:%M:%S" } else { "%H:%M" };
    for r in &u.runs {
        let wall = r.time.strftime(clock);
        let note = match (r.dst, r.scheduled) {
            (Some(Dst::Moved), Some(at)) => format!("  moved from {}", at.strftime(clock)),
            (Some(Dst::First), _) => format!("  first {wall} of two"),
            (Some(Dst::Second), _) => format!("  second {wall} of two"),
            _ => String::new(),
        };
        let day = r.time.strftime("%a %Y-%m-%d");
        let _ = writeln!(out, "  {day} {wall} {}{note}", r.time.strftime("%:z"));
    }
    for at in &u.skipped {
        let _ = writeln!(
            out,
            "  skipped {} {} (DST gap)",
            at.strftime("%a %Y-%m-%d"),
            at.strftime(clock)
        );
    }
    out + &warnings(&u.warnings)
}

/// What failed, then how to fix it.
pub fn error(e: &Error) -> String {
    match &e.hint {
        Some(hint) => format!("✗ {}\n  {hint}\n", e.message),
        None => format!("✗ {}\n", e.message),
    }
}

fn warnings(warnings: &[Warning]) -> String {
    warnings
        .iter()
        .map(|w| format!("! {}\n", w.message))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{explain, next};

    #[test]
    fn lines_up_the_fields() {
        let e = explain("*/15 9-17 * * 1-5", None).unwrap();
        assert_eq!(
            explained(&e),
            "Every 15 minutes from 09:00 through 17:45, Monday through Friday.
Dialect: unix (5 fields)
  minute        */15  every 15 minutes
  hour          9-17  9 through 17
  day of month  *     any day
  month         *     any month
  day of week   1-5   Monday through Friday
"
        );
    }

    #[test]
    fn notes_dst_moves_and_skips() {
        let tz = Some("America/New_York");
        let u = next("30 2 * * *", None, tz, Some("2027-03-13T12:00"), 3).unwrap();
        assert_eq!(
            upcoming(&u),
            "At 02:30 every day.
America/New_York · tzdata 2026f · Unix cron rules
  Sun 2027-03-14 03:00 -04:00  moved from 02:30
  Mon 2027-03-15 02:30 -04:00
  Tue 2027-03-16 02:30 -04:00
! 2027-03-14: clocks jump from 02:00 to 03:00 in America/New_York. Cron runs 02:30 right after the jump.
! 2028-03-12: clocks jump from 02:00 to 03:00 in America/New_York. Cron runs 02:30 right after the jump.
"
        );
        let u = next("*/30 1 * * *", None, tz, Some("2026-10-31T23:00"), 3).unwrap();
        assert!(upcoming(&u).contains("  Sun 2026-11-01 01:00 -04:00  first 01:00 of two\n"));
        let u = next("*/15 2 * * *", None, tz, Some("2027-03-13T12:00"), 1).unwrap();
        assert!(upcoming(&u).contains("  skipped Sun 2027-03-14 02:45 (DST gap)\n"));
    }

    #[test]
    fn shows_seconds_only_when_a_run_has_them() {
        let u = next("15 30 9 * * ?", None, Some("UTC"), Some("2026-10-29"), 1).unwrap();
        assert!(upcoming(&u).contains("  Thu 2026-10-29 09:30:15 +00:00\n"));
    }

    #[test]
    fn puts_the_hint_under_the_error() {
        let e = next("0 0 30 2 *", None, Some("UTC"), None, 1).unwrap_err();
        assert_eq!(
            error(&e),
            "✗ This schedule never runs: day 30 doesn't occur in February.
  Pick a day that exists in every chosen month, or use L in Quartz or AWS.
"
        );
    }
}
