//! Plain English for a schedule, on a 24-hour clock.

use crate::schedule::Kind::{self, DayOfMonth, DayOfWeek, Hour, Minute, Month, Second, Year};
use crate::schedule::{DayRule, DomSpecial, Dow, Schedule};

#[rustfmt::skip]
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
#[rustfmt::skip]
pub(crate) const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
];
const NTH: [&str; 5] = ["first", "second", "third", "fourth", "fifth"];

/// One sentence, e.g. "Every 15 minutes from 09:00 through 17:45, Monday through Friday."
pub fn describe(s: &Schedule) -> String {
    if s.reboot {
        return "Once, when cron starts.".into();
    }
    let (time, clock) = time(s);
    let mut parts = vec![time];
    match on_date(s) {
        Some(date) => parts.push(date),
        None => parts.extend(days(s).into_iter().chain(months(s))),
    }
    parts.extend(years(s));
    let mut text = parts.join(", ");
    if clock && parts.len() == 1 {
        text += " every day";
    }
    text + "."
}

/// One field as explained.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FieldInfo {
    /// The JSON name: "day_of_month".
    pub name: &'static str,
    pub text: String,
    /// None for `*` and `?`, and when the field holds L, W or #.
    pub values: Option<Values>,
    pub meaning: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum Values {
    Numbers(Vec<u16>),
    /// Day of week as SUN-SAT, so nobody has to know which dialect counts Sunday as 1.
    Days(Vec<&'static str>),
}

/// Each field as written, with its values and what it means.
pub fn fields(s: &Schedule) -> Vec<FieldInfo> {
    let field = |kind: Kind, text: &str| {
        let v = s.values(kind);
        let values = match kind {
            _ if text == "*" || text == "?" => None,
            DayOfMonth if !s.dom.specials.is_empty() => None,
            DayOfWeek if !matches!(s.dow, Dow::Days(_)) || text.eq_ignore_ascii_case("L") => None,
            DayOfWeek => Some(Values::Days(
                v.iter().map(|&d| crate::expr::DAYS[d as usize]).collect(),
            )),
            _ => Some(Values::Numbers(v)),
        };
        FieldInfo {
            name: kind.key(),
            text: text.into(),
            values,
            meaning: meaning(s, kind),
        }
    };
    s.fields.iter().map(|f| field(f.kind, &f.text)).collect()
}

/// What one field means: "every 15 minutes", "9 through 17", "Monday through Friday".
pub(crate) fn meaning(s: &Schedule, kind: Kind) -> String {
    let v = s.values(kind);
    let bare = |phrase: Option<String>, any: &str| match phrase {
        Some(p) => ["on ", "in ", "from "]
            .iter()
            .find_map(|prep| p.strip_prefix(prep))
            .unwrap_or(&p)
            .to_string(),
        None => any.into(),
    };
    match kind {
        Second | Minute | Hour => steps(s, kind, &v).unwrap_or_else(|| numbers(&v)),
        DayOfMonth | DayOfWeek if s.field(kind).is_some_and(|f| f.text == "?") => {
            let other = [DayOfWeek, DayOfMonth][(kind == DayOfWeek) as usize];
            format!("not used: the {} field decides", other.words())
        }
        DayOfMonth => bare(dom(s), "any day"),
        Month => bare(months(s), "any month"),
        DayOfWeek => bare(dow(s), "any day of the week"),
        Year => bare(years(s), "any year"),
    }
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

/// "on January 1": one day of one month, chosen by day of month alone.
fn on_date(s: &Schedule) -> Option<String> {
    match (s.day_rule, &s.values(DayOfMonth)[..], &s.values(Month)[..]) {
        (DayRule::DomOnly, &[d], &[m]) if s.dom.specials.is_empty() => {
            Some(format!("on {} {d}", MONTHS[m as usize - 1]))
        }
        _ => None,
    }
}

/// The day part, or None when every day qualifies.
fn days(s: &Schedule) -> Option<String> {
    match s.day_rule {
        DayRule::DomOnly => dom(s),
        DayRule::DowOnly => dow(s),
        DayRule::Or => Some(format!("{}, or {}", dom(s)?, dow(s)?)),
        DayRule::And => {
            let days = s.values(DayOfWeek).into_iter();
            let days = list(days.map(|d| DAYS[d as usize].to_string()), "or");
            Some(format!("{}, only when that day is a {days}", dom(s)?))
        }
    }
}

fn dom(s: &Schedule) -> Option<String> {
    let days = s.values(DayOfMonth);
    if is_full(&days, DayOfMonth) {
        return None;
    }
    let plain = match every(s, DayOfMonth, &days) {
        _ if days.is_empty() => None,
        Some(n) => {
            let mut shown: Vec<String> = days.iter().take(3).map(u16::to_string).collect();
            if days.len() > 3 {
                shown.push("…".into());
            }
            let shown = shown.join(", ");
            Some(format!(
                "on every {} day of the month ({shown})",
                ordinal(n)
            ))
        }
        None => Some(format!("on day {} of the month", numbers(&days))),
    };
    let specials = s.dom.specials.iter().map(|&special| match special {
        DomSpecial::Last { offset: 0, weekday } => {
            let day = if weekday { "weekday" } else { "day" };
            format!("on the last {day} of the month")
        }
        DomSpecial::Last { offset, weekday } => {
            let days = if offset == 1 { "day" } else { "days" };
            let before = format!("{offset} {days} before the last day of the month");
            if weekday {
                format!("on the weekday nearest {before}")
            } else {
                before
            }
        }
        DomSpecial::Nearest(n) => format!("on the weekday nearest day {n} of the month"),
    });
    let parts: Vec<String> = plain.into_iter().chain(specials).collect();
    (!parts.is_empty()).then(|| parts.join(" and "))
}

fn dow(s: &Schedule) -> Option<String> {
    match s.dow {
        Dow::Days(_) => named(&s.values(DayOfWeek), DayOfWeek, &DAYS, "on"),
        Dow::Last(d) => Some(format!("on the last {} of the month", DAYS[d as usize])),
        Dow::Nth(d, n) => Some(format!(
            "on the {} {} of the month",
            NTH[n as usize - 1],
            DAYS[d as usize]
        )),
    }
}

fn months(s: &Schedule) -> Option<String> {
    named(&s.values(Month), Month, &MONTHS, "in")
}

/// "Monday through Friday" for a run of 3 or more (`FRI-MON` wraps), else "on Monday and
/// Friday"; None when the field allows every value.
fn named(v: &[u16], kind: Kind, names: &[&str], prep: &str) -> Option<String> {
    let (lo, hi) = kind.range();
    let (n, len) = (hi - lo + 1, v.len() as u16);
    if len == 0 || len == n {
        return None;
    }
    // Positions count from lo and wrap past the end.
    let has = |i: u16| v.contains(&(lo + i % n));
    if len >= 3
        && let Some(start) = (0..n).find(|&i| has(i) && !has(i + n - 1))
        && (0..len).all(|k| has(start + k))
    {
        let end = (start + len - 1) % n;
        return Some(format!(
            "{} through {}",
            names[start as usize], names[end as usize]
        ));
    }
    let names = v.iter().map(|&x| names[(x - lo) as usize].to_string());
    Some(format!("{prep} {}", list(names, "and")))
}

fn years(s: &Schedule) -> Option<String> {
    let v = s.values(Year);
    let (&a, &b) = (v.first()?, v.last()?);
    Some(match step(&v) {
        _ if is_full(&v, Year) => return None,
        _ if a == b => format!("in {a}"),
        Some(1) => format!("from {a} through {b}"),
        Some(n) if v.len() >= 3 => format!("every {} year from {a} through {b}", ordinal(n)),
        _ => format!("in {}", numbers(&v)),
    })
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
pub(crate) fn step(v: &[u16]) -> Option<u16> {
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
pub(crate) fn numbers(v: &[u16]) -> String {
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
    #[rustfmt::skip]
    fn matches_every_description_golden() {
        let cases = [
            ("0 9 * * 1-5", "At 09:00, Monday through Friday."),
            ("*/15 9-17 * * 1-5", "Every 15 minutes from 09:00 through 17:45, Monday through Friday."),
            ("30 2 * * *", "At 02:30 every day."),
            ("@daily", "At 00:00 every day."),
            ("@hourly", "At minute 0 of every hour."),
            ("0 * * * *", "At minute 0 of every hour."),
            ("* * * * *", "Every minute."),
            ("* 9 * * *", "Every minute from 09:00 through 09:59."),
            ("0 9,17 * * *", "At 09:00 and 17:00 every day."),
            ("5 4 * * sun", "At 04:05, on Sunday."),
            ("0 9 * * 1,3,5", "At 09:00, on Monday, Wednesday and Friday."),
            ("0 0 1 1 *", "At 00:00, on January 1."),
            ("0 0,12 1 */2 *", "At 00:00 and 12:00, on day 1 of the month, in January, March, May, July, September and November."),
            ("0 0 1,15 * FRI", "At 00:00, on day 1 and 15 of the month, or on Friday."),
            ("0 9 */2 * MON", "At 09:00, on every 2nd day of the month (1, 3, 5, …), only when that day is a Monday."),
            ("@reboot", "Once, when cron starts."),
            ("0 15 10 L * ?", "At 10:15, on the last day of the month."),
            ("0 0 12 ? * MON#2", "At 12:00, on the second Monday of the month."),
            ("0 15 10 ? * 6L", "At 10:15, on the last Friday of the month."),
            ("0 0 9 15W * ?", "At 09:00, on the weekday nearest day 15 of the month."),
            ("0 0 9 LW * ?", "At 09:00, on the last weekday of the month."),
            ("0 0 12 L-3 * ?", "At 12:00, 3 days before the last day of the month."),
            ("0 0 12 ? * L", "At 12:00, on Saturday."),
            ("0 0/5 14 * * ?", "Every 5 minutes from 14:00 through 14:55."),
            ("*/30 * * * * ?", "Every 30 seconds."),
            ("15 30 9 * * ?", "At 09:30:15 every day."),
            ("0 0 12 ? * 1-5", "At 12:00, Sunday through Thursday."),
            ("0 0 12 1 1 ? 2030", "At 12:00, on January 1, in 2030."),
            ("cron(0 18 ? * MON-FRI *)", "At 18:00, Monday through Friday."),
            ("cron(0/15 * * * ? *)", "Every 15 minutes."),
            ("cron(0 8 1 * ? *)", "At 08:00, on day 1 of the month."),
        ];
        for (expr, want) in cases {
            assert_eq!(text(expr), want, "{expr}");
        }
    }

    #[test]
    #[rustfmt::skip]
    fn describes_other_times_of_day() {
        let cases = [
            ("0 0 */6 * * ?", "At 00:00, 06:00, 12:00 and 18:00 every day."),
            ("30 */2 * * *", "At minute 30 of every 2nd hour."),
            ("* * * * * ?", "Every second."),
            ("*/7 * * * *", "Every 7 minutes."),
            // Steps bounded by a window name its first and last run.
            ("* 0 9 * * ?", "Every second from 09:00:00 through 09:00:59."),
            ("*/10 * 9-10 * * ?", "Every 10 seconds from 09:00:00 through 10:59:50."),
            ("15 */15 * * * ?", "Every 15 minutes from 00:00:15 through 23:45:15."),
            // Anything else goes field by field.
            ("5,35 1,4,9 * * *", "At minutes 5 and 35 of hours 1, 4 and 9."),
            ("0 1,4,9-13 * * *", "At minute 0 of hours 1, 4 and 9 through 13."),
            ("5,35 * * * *", "At minutes 5 and 35 of every hour."),
            ("*/15 9,17 * * *", "Every 15 minutes of hours 9 and 17."),
            ("0,30 5 0-23 * * ?", "At seconds 0 and 30 of minute 5 of every hour."),
            ("30 9-21/2 * * *", "At minute 30 of hours 9, 11, 13, 15, 17, 19 and 21."),
        ];
        for (expr, want) in cases {
            assert_eq!(text(expr), want, "{expr}");
        }
    }

    #[test]
    #[rustfmt::skip]
    fn describes_days_months_and_years() {
        let cases = [
            ("0 12 * * 5-0", "At 12:00, Friday through Sunday."),
            ("0 0 12 ? * FRI-MON", "At 12:00, Friday through Monday."),
            ("0 9 * * 1,2", "At 09:00, on Monday and Tuesday."),
            ("0 0 1-15 * *", "At 00:00, on day 1 through 15 of the month."),
            ("0 0 */10 * *", "At 00:00, on every 10th day of the month (1, 11, 21, …)."),
            ("0 0 */15 * *", "At 00:00, on every 15th day of the month (1, 16, 31)."),
            ("0 0 12 L,15 * ?", "At 12:00, on day 15 of the month and on the last day of the month."),
            ("0 0 12 L-1 * ?", "At 12:00, 1 day before the last day of the month."),
            ("0 0 12 L-2W * ?", "At 12:00, on the weekday nearest 2 days before the last day of the month."),
            ("0 0 12 ? * 2#5", "At 12:00, on the fifth Monday of the month."),
            ("0 0 1 1-3 *", "At 00:00, on day 1 of the month, January through March."),
            ("0 0 0 * 11-2 ?", "At 00:00, November through February."),
            ("0 0 0 * 1,7 ?", "At 00:00, in January and July."),
            ("0 0 12 1 1 ? 2027-2028", "At 12:00, on January 1, from 2027 through 2028."),
            ("0 0 12 1 1 ? 2026/4", "At 12:00, on January 1, every 4th year from 2026 through 2198."),
            ("0 0 12 1 1 ? 2027,2029", "At 12:00, on January 1, in 2027 and 2029."),
            ("0 0 12 ? * MON 2030", "At 12:00, on Monday, in 2030."),
            ("0 9 1-15 * */2", "At 09:00, on day 1 through 15 of the month, only when that day is a Sunday, Tuesday, Thursday or Saturday."),
            // Cron's OR rule with every day of the month runs every day.
            ("0 9 1-31 * MON", "At 09:00 every day."),
        ];
        for (expr, want) in cases {
            assert_eq!(text(expr), want, "{expr}");
        }
    }

    /// Each field as `name text values meaning`, values as `-` when null.
    fn explained(expr: &str) -> Vec<String> {
        let s = parse(expr, None).unwrap();
        let fields = fields(&s).into_iter().map(|f| {
            let values = match f.values {
                None => "-".into(),
                Some(Values::Numbers(v)) => format!("{v:?}"),
                Some(Values::Days(v)) => format!("{v:?}"),
            };
            format!("{} {} {values} {}", f.name, f.text, f.meaning)
        });
        fields.collect()
    }

    #[test]
    #[rustfmt::skip]
    fn explains_each_field() {
        assert_eq!(
            explained("*/15 9-17 * * 1-5"),
            [
                "minute */15 [0, 15, 30, 45] every 15 minutes",
                "hour 9-17 [9, 10, 11, 12, 13, 14, 15, 16, 17] 9 through 17",
                "day_of_month * - any day",
                "month * - any month",
                "day_of_week 1-5 [\"MON\", \"TUE\", \"WED\", \"THU\", \"FRI\"] Monday through Friday",
            ]
        );
        assert_eq!(
            explained("0 0 12 ? * MON#2 2030"),
            [
                "second 0 [0] 0",
                "minute 0 [0] 0",
                "hour 12 [12] 12",
                "day_of_month ? - not used: the day of week field decides",
                "month * - any month",
                "day_of_week MON#2 - the second Monday of the month",
                "year 2030 [2030] 2030",
            ]
        );
        assert_eq!(
            explained("cron(5,35 * L-3 1,7 ? 2027-2028)"),
            [
                "minute 5,35 [5, 35] 5 and 35",
                "hour * - every hour",
                "day_of_month L-3 - 3 days before the last day of the month",
                "month 1,7 [1, 7] January and July",
                "day_of_week ? - not used: the day of month field decides",
                "year 2027-2028 [2027, 2028] 2027 through 2028",
            ]
        );
        let dow = |expr| explained(expr).swap_remove(5);
        assert_eq!(dow("0 0 12 ? * L"), "day_of_week L - Saturday");
        assert_eq!(dow("0 0 12 ? * 6L"), "day_of_week 6L - the last Friday of the month");
        assert_eq!(dow("0 0 12 ? * 1,7"), "day_of_week 1,7 [\"SUN\", \"SAT\"] Sunday and Saturday");
        assert_eq!(explained("0 9 * * 7")[4], "day_of_week 7 [\"SUN\"] Sunday");
        assert_eq!(explained("0 0 9 15W * ?")[3], "day_of_month 15W - the weekday nearest day 15 of the month");
        assert_eq!(explained("0 9 */2 * *")[2], "day_of_month */2 [1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31] every 2nd day of the month (1, 3, 5, …)");
        assert_eq!(explained("@daily")[0], "minute 0 [0] 0");
        assert!(explained("@reboot").is_empty());
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
