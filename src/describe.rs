//! Plain English for a schedule, on a 24-hour clock.

use crate::schedule::Kind::{self, Hour, Minute, Second};
use crate::schedule::Schedule;

/// One sentence, e.g. "Every 15 minutes from 09:00 through 17:45, Monday through Friday."
pub fn describe(s: &Schedule) -> String {
    if s.reboot {
        return "Once, when cron starts.".into();
    }
    let (time, clock) = time(s);
    let parts = [time];
    let mut text = parts.join(", ");
    if clock && parts.len() == 1 {
        text += " every day";
    }
    text + "."
}

/// The time of day, and whether it names clock times ("At 09:00 and 17:00").
fn time(s: &Schedule) -> (String, bool) {
    const KINDS: [Kind; 3] = [Second, Minute, Hour];
    let sets = KINDS.map(|k| s.values(k));
    let [secs, mins, hours] = &sets;
    let show = secs != &[0];
    let at = |pick: fn(&[u16]) -> u16| clock(pick(hours), pick(mins), pick(secs), show);
    if secs.len() == 1 && mins.len() == 1 && hours.len() <= 6 {
        let times = hours.iter().map(|&h| clock(h, mins[0], secs[0], show));
        return (format!("At {}", list(times, "and")), true);
    }
    // Runs every n units, through the whole day or one window of it.
    if let Some((f, n)) = (0..3).find_map(|i| every(s, KINDS[i], &sets[i]).map(|n| (i, n))) {
        let coarser = &sets[f + 1..];
        let full = (coarser.iter().zip(&KINDS[f + 1..])).take_while(|(v, k)| is_full(v, **k));
        let rest = &coarser[full.count()..];
        if sets[..f].iter().all(|v| v.len() == 1)
            && rest
                .first()
                .is_none_or(|v| v[v.len() - 1] - v[0] + 1 == v.len() as u16)
            && rest.iter().skip(1).all(|v| v.len() == 1)
        {
            if rest.is_empty() && f == 2 && !show {
                let hour = if n == 1 {
                    "hour".into()
                } else {
                    format!("{} hour", ordinal(n))
                };
                return (format!("At minute {} of every {hour}", mins[0]), false);
            }
            let every = capitalised(&steps(s, KINDS[f], &sets[f]).unwrap());
            if rest.is_empty() && sets[..f].iter().all(|v| v == &[0]) {
                return (every, false);
            }
            let (first, last) = (at(|v| v[0]), at(|v| v[v.len() - 1]));
            return (format!("{every} from {first} through {last}"), false);
        }
    }
    // Anything else, field by field: "At minutes 5 and 35 of hours 1, 4 and 9".
    let parts: Vec<String> = (0..3)
        .filter(|&i| i > 0 || show)
        .map(|i| {
            let (v, unit) = (&sets[i], KINDS[i].words());
            steps(s, KINDS[i], v).unwrap_or_else(|| match v[..] {
                [one] => format!("{unit} {one}"),
                _ => format!("{unit}s {}", numbers(v)),
            })
        })
        .collect();
    let text = parts.join(" of ");
    let text = match text.strip_prefix("every") {
        Some(rest) => format!("Every{rest}"),
        None => format!("At {text}"),
    };
    (text, false)
}

/// "every minute" or "every 15 minutes" for a time field that runs at even steps.
fn steps(s: &Schedule, kind: Kind, v: &[u16]) -> Option<String> {
    let unit = kind.words();
    match every(s, kind, v) {
        _ if is_full(v, kind) => Some(format!("every {unit}")),
        Some(n) => Some(format!("every {n} {unit}s")),
        None => None,
    }
}

/// The step n when a field written `*`, `*/n` or `0/n` runs every n units across its range.
fn every(s: &Schedule, kind: Kind, v: &[u16]) -> Option<u16> {
    let text = &s.field(kind)?.text;
    let (lo, hi) = kind.range();
    let n = step(v)?;
    (v[0] == lo && v[v.len() - 1] + n > hi && (text == "*" || text.contains('/'))).then_some(n)
}

/// The common difference of 2 or more evenly spaced values.
fn step(v: &[u16]) -> Option<u16> {
    let n = v.get(1)?.checked_sub(v[0])?;
    v.windows(2).all(|w| w[1] - w[0] == n).then_some(n)
}

fn is_full(v: &[u16], kind: Kind) -> bool {
    let (lo, hi) = kind.range();
    v.len() == (hi - lo + 1) as usize
}

fn clock(h: u16, m: u16, s: u16, seconds: bool) -> String {
    if seconds {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}")
    }
}

/// "1, 4 and 9 through 13": runs of 3 or more become ranges.
fn numbers(v: &[u16]) -> String {
    let items = v.chunk_by(|a, b| a + 1 == *b).flat_map(|run| match run {
        [a, .., b] if run.len() >= 3 => vec![format!("{a} through {b}")],
        _ => run.iter().map(u16::to_string).collect(),
    });
    list(items, "and")
}

fn ordinal(n: u16) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn capitalised(text: &str) -> String {
    text[..1].to_ascii_uppercase() + &text[1..]
}

/// "a", "a and b", "a, b and c" (or another last word).
pub(crate) fn list(items: impl IntoIterator<Item = String>, word: &str) -> String {
    let mut items: Vec<String> = items.into_iter().collect();
    let last = items.pop().unwrap_or_default();
    if items.is_empty() {
        last
    } else {
        format!("{} {word} {last}", items.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::parse;

    fn text(expr: &str) -> String {
        describe(&parse(expr, None).unwrap())
    }

    #[test]
    fn describes_times_of_day() {
        let cases = [
            ("30 2 * * *", "At 02:30 every day."),
            ("0 9,17 * * *", "At 09:00 and 17:00 every day."),
            ("15 30 9 * * ?", "At 09:30:15 every day."),
            (
                "0 0 */6 * * ?",
                "At 00:00, 06:00, 12:00 and 18:00 every day.",
            ),
            ("@hourly", "At minute 0 of every hour."),
            ("30 */2 * * *", "At minute 30 of every 2nd hour."),
            ("* * * * *", "Every minute."),
            ("* * * * * ?", "Every second."),
            ("*/30 * * * * ?", "Every 30 seconds."),
            ("cron(0/15 * * * ? *)", "Every 15 minutes."),
            ("*/7 * * * *", "Every 7 minutes."),
            ("@reboot", "Once, when cron starts."),
        ];
        for (expr, want) in cases {
            assert_eq!(text(expr), want, "{expr}");
        }
    }

    #[test]
    fn bounds_steps_by_their_first_and_last_run() {
        let cases = [
            ("* 9 * * *", "Every minute from 09:00 through 09:59."),
            (
                "*/15 9-17 * * *",
                "Every 15 minutes from 09:00 through 17:45.",
            ),
            (
                "0 0/5 14 * * ?",
                "Every 5 minutes from 14:00 through 14:55.",
            ),
            (
                "* 0 9 * * ?",
                "Every second from 09:00:00 through 09:00:59.",
            ),
            (
                "*/10 * 9-10 * * ?",
                "Every 10 seconds from 09:00:00 through 10:59:50.",
            ),
            (
                "15 */15 * * * ?",
                "Every 15 minutes from 00:00:15 through 23:45:15.",
            ),
        ];
        for (expr, want) in cases {
            assert_eq!(text(expr), want, "{expr}");
        }
    }

    #[test]
    fn falls_back_to_each_field() {
        let cases = [
            (
                "5,35 1,4,9 * * *",
                "At minutes 5 and 35 of hours 1, 4 and 9.",
            ),
            (
                "0 1,4,9-13 * * *",
                "At minute 0 of hours 1, 4 and 9 through 13.",
            ),
            ("0 9-17 * * *", "At minute 0 of hours 9 through 17."),
            ("5,35 * * * *", "At minutes 5 and 35 of every hour."),
            ("*/15 9,17 * * *", "Every 15 minutes of hours 9 and 17."),
            (
                "0,30 5 0-23 * * ?",
                "At seconds 0 and 30 of minute 5 of every hour.",
            ),
            (
                "30 9-21/2 * * *",
                "At minute 30 of hours 9, 11, 13, 15, 17, 19 and 21.",
            ),
        ];
        for (expr, want) in cases {
            assert_eq!(text(expr), want, "{expr}");
        }
    }

    #[test]
    fn writes_ordinals() {
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101]
            .map(ordinal)
            .into();
        let want = [
            "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd", "101st",
        ];
        assert_eq!(got, want);
    }
}
