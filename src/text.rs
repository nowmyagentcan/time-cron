//! Text for people: each answer as one string of lines. Styles are always written as ANSI
//! codes; the output stream drops them unless it is a colour terminal and NO_COLOR is unset.

use std::fmt::Write;

use crate::error::{Error, Warning};
use crate::report::{Converted, Explained, Upcoming};
use crate::runs::Dst;
use crate::schedule::Dialect;

/// What the tool is for, in one sentence: the help's first line and the banner's second.
pub const SENTENCE: &str = "Explain a cron schedule and see exactly when it runs.";

pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
pub(crate) const RESET: &str = "\x1b[0m";

pub fn explained(e: &Explained) -> String {
    let mut out = format!(
        "{BOLD}{}{RESET}\n{DIM}Dialect: {}",
        e.description,
        e.dialect.name()
    );
    if !e.fields.is_empty() {
        let _ = write!(out, " ({} fields)", e.fields.len());
    }
    out += RESET;
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
    format!(
        "{BOLD}{}{RESET}\n{DIM}{} · tzdata {} · {rules} rules{RESET}\n{}{}",
        u.description,
        u.timezone,
        u.tzdata,
        runs(u),
        warnings(&u.warnings)
    )
}

/// One line per run, then one per skipped wall time, each indented 2.
pub fn runs(u: &Upcoming) -> String {
    let mut out = String::new();
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
        let offset = r.time.strftime("%:z");
        let _ = writeln!(out, "  {day} {wall} {DIM}{offset}{RESET}{note}");
    }
    for at in &u.skipped {
        let _ = writeln!(
            out,
            "  {DIM}skipped {} {} (DST gap){RESET}",
            at.strftime("%a %Y-%m-%d"),
            at.strftime(clock)
        );
    }
    out
}

/// The converted expressions, one per line: all that `$(time-cron convert …)` captures.
pub fn converted(c: &Converted) -> String {
    c.results.iter().map(|r| format!("{r}\n")).collect()
}

/// The description and warnings that go with a conversion, for stderr.
pub fn notes(c: &Converted) -> String {
    format!("{BOLD}{}{RESET}\n", c.description) + &warnings(&c.warnings)
}

/// What failed, then how to fix it.
pub fn error(e: &Error) -> String {
    let mut out = format!("{RED}✗{RESET} {}\n", e.message);
    if let Some(hint) = &e.hint {
        let _ = writeln!(out, "  {hint}");
    }
    out
}

pub fn warnings(warnings: &[Warning]) -> String {
    warnings
        .iter()
        .map(|w| format!("{YELLOW}!{RESET} {}\n", w.message))
        .collect()
}

/// The text with plain ASCII in place of the marks a console without UTF-8 can't show.
pub fn ascii(text: &str) -> String {
    text.replace(['╭', '╮', '╰', '╯'], "+")
        .replace(['─', '·'], "-")
        .replace('│', "|")
        .replace('✻', "*")
        .replace('›', ">")
        .replace('✗', "x")
        .replace('…', "...")
}

/// Whether the console shows Unicode: on Windows only when its output code page is UTF-8.
pub fn unicode() -> bool {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetConsoleOutputCP() -> u32;
        }
        // SAFETY: takes no arguments and only reads the console's state.
        unsafe { GetConsoleOutputCP() == 65001 }
    }
    #[cfg(not(windows))]
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{explain, next};

    fn plain(text: String) -> String {
        anstream::adapter::strip_str(&text).to_string()
    }

    #[test]
    fn styles_marks_and_headlines() {
        let e = next("0 0 30 2 *", None, Some("UTC"), None, 1).unwrap_err();
        assert!(error(&e).starts_with("\x1b[31m✗\x1b[0m This schedule never runs"));
        let u = next("0 0 1,15 * FRI", None, Some("UTC"), None, 1).unwrap();
        let text = upcoming(&u);
        assert!(text.starts_with(
            "\x1b[1mAt 00:00, on day 1 and 15 of the month, or on Friday.\x1b[0m\n\x1b[2mUTC"
        ));
        assert!(text.ends_with("\x1b[33m!\x1b[0m Runs when the day of month OR the day of week matches, not only when both do.\n"));
    }

    #[test]
    fn falls_back_to_ascii_marks() {
        let e = explain("0 9 */2 * MON", None).unwrap();
        assert!(
            ascii(&plain(explained(&e)))
                .starts_with("At 09:00, on every 2nd day of the month (1, 3, 5, ...), only")
        );
        let e = next("0 0 30 2 *", None, Some("UTC"), None, 1).unwrap_err();
        assert!(ascii(&plain(error(&e))).starts_with("x This schedule"));
        let u = next("@daily", None, Some("UTC"), None, 1).unwrap();
        assert!(ascii(&plain(upcoming(&u))).contains("\nUTC - tzdata 2026f - Unix cron rules\n"));
    }

    #[test]
    fn lines_up_the_fields() {
        let e = explain("*/15 9-17 * * 1-5", None).unwrap();
        assert_eq!(
            plain(explained(&e)),
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
            plain(upcoming(&u)),
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
        assert!(
            plain(upcoming(&u)).contains("  Sun 2026-11-01 01:00 -04:00  first 01:00 of two\n")
        );
        let u = next("*/15 2 * * *", None, tz, Some("2027-03-13T12:00"), 1).unwrap();
        assert!(plain(upcoming(&u)).contains("  skipped Sun 2027-03-14 02:45 (DST gap)\n"));
    }

    #[test]
    fn shows_seconds_only_when_a_run_has_them() {
        let u = next("15 30 9 * * ?", None, Some("UTC"), Some("2026-10-29"), 1).unwrap();
        assert!(plain(upcoming(&u)).contains("  Thu 2026-10-29 09:30:15 +00:00\n"));
    }

    #[test]
    fn keeps_converted_expressions_apart_from_notes() {
        let c = crate::report::convert("0 0 1,15 * FRI", None, Dialect::Quartz).unwrap();
        assert_eq!(converted(&c), "0 0 0 1,15 * ?\n0 0 0 ? * FRI\n");
        assert_eq!(
            plain(notes(&c)),
            "At 00:00, on day 1 and 15 of the month, or on Friday.
! Quartz can't match day of month OR day of week in one expression, so this needs two triggers.
! DST rules differ: Quartz skips times in a gap and runs a repeated time once, the second time, while Unix cron runs a time in a gap right after the jump and a repeated time once, the first time.
"
        );
    }

    #[test]
    fn puts_the_hint_under_the_error() {
        let e = next("0 0 30 2 *", None, Some("UTC"), None, 1).unwrap_err();
        assert_eq!(
            plain(error(&e)),
            "✗ This schedule never runs: day 30 doesn't occur in February.
  Pick a day that exists in every chosen month, or use L in Quartz or AWS.
"
        );
    }
}
