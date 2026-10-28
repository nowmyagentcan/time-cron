use crate::error::Error;
use crate::schedule::Dialect::{self, Aws, Quartz, Unix};
use crate::schedule::Kind::{self, DayOfMonth, DayOfWeek, Hour, Minute, Month, Second, Year};
use crate::schedule::{DayRule, Dom, DomSpecial, Dow, Field, Schedule, Set};

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
const DAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

const NICKNAMES: [(&str, &str); 8] = [
    ("@yearly", "0 0 1 1 *"),
    ("@annually", "0 0 1 1 *"),
    ("@monthly", "0 0 1 * *"),
    ("@weekly", "0 0 * * 0"),
    ("@daily", "0 0 * * *"),
    ("@midnight", "0 0 * * *"),
    ("@hourly", "0 * * * *"),
    ("@reboot", ""),
];

/// Parses an expression in the given dialect, or the detected one when `forced` is `None`.
pub fn parse(text: &str, forced: Option<Dialect>) -> Result<Schedule, Error> {
    let (dialect, texts) = read(text, forced)?;
    let kinds: &[Kind] = match (dialect, texts.len()) {
        (Unix, _) => &[Minute, Hour, DayOfMonth, Month, DayOfWeek],
        (Quartz, 6) => &[Second, Minute, Hour, DayOfMonth, Month, DayOfWeek],
        (Quartz, _) => &[Second, Minute, Hour, DayOfMonth, Month, DayOfWeek, Year],
        (Aws, _) => &[Minute, Hour, DayOfMonth, Month, DayOfWeek, Year],
    };
    let mut s = Schedule {
        dialect,
        fields: kinds
            .iter()
            .zip(&texts)
            .map(|(&kind, text)| Field {
                kind,
                text: text.to_string(),
            })
            .collect(),
        second: [0].into_iter().collect(),
        minute: Set::EMPTY,
        hour: Set::EMPTY,
        dom: Dom {
            days: Set::EMPTY,
            specials: vec![],
        },
        month: Set::EMPTY,
        dow: Dow::Days(Set::EMPTY),
        year: (1970..=2199).collect(),
        day_rule: DayRule::And,
        fixed_time: false,
        reboot: texts.is_empty(),
    };
    if s.reboot {
        return Ok(s);
    }
    let p = Parser(dialect);
    for f in &s.fields {
        match f.kind {
            Second => s.second = p.set(f)?,
            Minute => s.minute = p.set(f)?,
            Hour => s.hour = p.set(f)?,
            DayOfMonth => s.dom = p.dom(f)?,
            Month => s.month = p.set(f)?,
            DayOfWeek => s.dow = p.dow(f)?,
            Year => s.year = p.set(f)?,
        }
    }
    let field = |kind| s.fields.iter().find(|f| f.kind == kind).unwrap();
    let (dom, dow) = (field(DayOfMonth), field(DayOfWeek));
    s.day_rule = if dialect != Unix {
        match (dom.text == "?", dow.text == "?") {
            (true, false) => DayRule::DowOnly,
            (false, true) => DayRule::DomOnly,
            _ => {
                return Err(p.question_mark(format!(
                    "{} needs '?' in exactly one of day of month and day of week.",
                    dialect.title()
                )));
            }
        }
    } else if !dom.star() && !dow.star() {
        DayRule::Or
    } else if s.dom.days == (1..=31).collect() {
        DayRule::DowOnly
    } else if s.dow == Dow::Days((0..7).collect()) {
        DayRule::DomOnly
    } else {
        DayRule::And
    };
    s.fixed_time = !field(Minute).star() && !field(Hour).star();
    Ok(s)
}

struct Parser(Dialect);

impl Parser {
    fn bounds(&self, kind: Kind) -> (u16, u16) {
        match kind {
            Second | Minute => (0, 59),
            Hour => (0, 23),
            DayOfMonth => (1, 31),
            Month => (1, 12),
            DayOfWeek if self.0 == Unix => (0, 7),
            DayOfWeek => (1, 7),
            Year => (1970, 2199),
        }
    }

    /// The field's list items, after turning away syntax from other schedulers.
    fn items<'a>(&self, f: &'a Field) -> Result<Vec<&'a str>, Error> {
        let items: Vec<&str> = f.text.split(',').collect();
        for item in &items {
            let up = item.to_ascii_uppercase();
            if item.contains('~') {
                return Err(Error::new(
                    "random_value",
                    "'~' picks a random time when cron loads the crontab, so the run times can't be predicted.",
                )
                .hint("Replace it with a fixed value."));
            }
            if up == "H" || up.starts_with("H/") || up.starts_with("H(") {
                return Err(Error::new(
                    "jenkins_hash",
                    "'H' is Jenkins syntax, which time-cron doesn't read.",
                )
                .hint("Replace it with a fixed value."));
            }
            if self.0 == Unix
                && let Some(c) = item.chars().find(|c| matches!(c, '?' | '#'))
            {
                return Err(unix_special(c));
            }
            if item.contains('?') && !matches!(f.kind, DayOfMonth | DayOfWeek) {
                return Err(
                    self.question_mark("'?' is only allowed in day of month or day of week.")
                );
            }
        }
        Ok(items)
    }

    fn set<const W: usize>(&self, f: &Field) -> Result<Set<W>, Error> {
        let mut set = Set::EMPTY;
        for item in self.items(f)? {
            self.values(f.kind, item)?
                .into_iter()
                .for_each(|v| set.insert(v));
        }
        Ok(set)
    }

    fn dom(&self, f: &Field) -> Result<Dom, Error> {
        let mut dom = Dom {
            days: Set::EMPTY,
            specials: vec![],
        };
        if self.0 != Unix && f.text == "?" {
            return Ok(dom);
        }
        for item in self.items(f)? {
            let up = item.to_ascii_uppercase();
            if self.0 != Unix
                && let Some(rest) = up.strip_prefix('L')
            {
                let (rest, weekday) = rest.strip_suffix('W').map_or((rest, false), |r| (r, true));
                let offset = match rest.strip_prefix('-').map(digits) {
                    None if rest.is_empty() => 0,
                    Some(Some(n)) if n <= 30 => n as u8,
                    Some(Some(_)) => {
                        let n = &rest[1..];
                        return Err(Error::new(
                            "l_offset",
                            format!("L-{n}: the offset can be at most 30."),
                        ));
                    }
                    _ => return Err(self.bad_syntax(DayOfMonth, item)),
                };
                dom.specials.push(DomSpecial::Last { offset, weekday });
            } else if self.0 != Unix
                && let Some(day) = up.strip_suffix('W')
            {
                if day.contains(['-', '/', '*']) {
                    return Err(Error::new(
                        "w_needs_day",
                        "'W' needs a single day of the month, like 15W.",
                    ));
                }
                let day = self.value(DayOfMonth, day, item)?;
                dom.specials.push(DomSpecial::Nearest(day as u8));
            } else {
                for v in self.values(DayOfMonth, item)? {
                    dom.days.insert(v);
                }
            }
        }
        Ok(dom)
    }

    fn dow(&self, f: &Field) -> Result<Dow, Error> {
        if self.0 == Unix {
            // Unix allows 7 for Sunday as well as 0.
            return Ok(Dow::Days(self.set::<1>(f)?.iter().map(|d| d % 7).collect()));
        }
        if f.text == "?" {
            return Ok(Dow::Days(Set::EMPTY));
        }
        let items = self.items(f)?;
        // Quartz counts from 1 = Sunday.
        let day = |tok: &str, item: &str| self.value(DayOfWeek, tok, item).map(|d| d as u8 - 1);
        if let Some((d, n)) = f.text.split_once('#') {
            if items.len() > 1 || n.contains('#') {
                return Err(Error::new(
                    "multiple_nth",
                    "Only one '#' day is allowed, on its own.",
                ));
            }
            let d_num = day(d, &f.text)?;
            return match digits(n) {
                Some(m @ 1..=5) => Ok(Dow::Nth(d_num, m as u8)),
                Some(_) => Err(Error::new(
                    "nth_range",
                    format!("In {d}#{n}, {n} must be 1-5."),
                )),
                None => Err(self.bad_syntax(DayOfWeek, &f.text)),
            };
        }
        if self.0 == Aws && f.text.contains('/') {
            return Err(
                Error::new("aws_dow_step", "AWS cron doesn't allow '/' in day of week.")
                    .hint("List the days instead, e.g. MON,WED,FRI."),
            );
        }
        let mut set = Set::EMPTY;
        for item in &items {
            let up = item.to_ascii_uppercase();
            if let Some(d) = up.strip_suffix('L')
                && (d.is_empty() || digits(d).is_some() || DAYS.contains(&d))
            {
                if items.len() > 1 {
                    return Err(Error::new(
                        "l_in_list",
                        "Quartz doesn't allow 'L' in a list of days of the week.",
                    ));
                }
                // A bare L is Saturday, the last day of the week.
                return Ok(if d.is_empty() {
                    Dow::Days([6].into_iter().collect())
                } else {
                    Dow::Last(day(d, item)?)
                });
            }
            for v in self.values(DayOfWeek, item)? {
                set.insert(v - 1);
            }
        }
        Ok(Dow::Days(set))
    }

    fn question_mark(&self, message: impl Into<String>) -> Error {
        Error::new("question_mark", message).hint(if self.0 == Aws {
            "Example: cron(0 9 ? * MON-FRI *)"
        } else {
            "Example: 0 0 9 ? * MON-FRI"
        })
    }

    /// Expands `*`, `n`, `a-b`, `a/s` and `a-b/s`.
    fn values(&self, kind: Kind, item: &str) -> Result<Vec<u16>, Error> {
        let (lo, hi) = self.bounds(kind);
        // Hints name Saturday as the last day, not Unix's second Sunday.
        let max = if kind == DayOfWeek { hi.min(6) } else { hi };
        let (range, step) = match item.split_once('/') {
            Some((r, s)) => (r, Some((s, self.step(kind, s, item)?))),
            None => (item, None),
        };
        let (a, mut b) = match range.split_once('-') {
            _ if range == "*" => (lo, hi),
            Some((a, b)) => (self.value(kind, a, item)?, self.value(kind, b, item)?),
            None => {
                let a = self.value(kind, range, item)?;
                match step {
                    None => (a, a),
                    Some((s, _)) if self.0 == Unix => {
                        return Err(Error::new(
                            "lone_step",
                            format!("Unix cron doesn't allow '{range}/{s}'."),
                        )
                        .hint(format!("Write {range}-{max}/{s} or */{s}.")));
                    }
                    Some(_) => (a, hi),
                }
            }
        };
        if self.0 == Unix && kind == DayOfWeek && b == 0 && a > b {
            b = 7; // `5-0` means Friday through Sunday
        }
        let span = hi - lo + 1;
        let end = if a <= b {
            b
        } else if self.0 != Unix && kind != Year {
            b + span // Quartz ranges wrap: `22-2` is 22, 23, 0, 1, 2
        } else {
            let (ta, tb) = range.split_once('-').unwrap();
            let e = Error::new(
                "backward_range",
                format!("The range {range} goes backwards."),
            );
            return Err(if kind == Year {
                e.hint("Write the smaller value first.")
            } else {
                e.hint(format!(
                    "Write the smaller value first, or split it: {ta}-{max},{lo}-{tb}."
                ))
            });
        };
        Ok((a..=end)
            .step_by(step.map_or(1, |(_, n)| n))
            .map(|v| if v > hi { v - span } else { v })
            .collect())
    }

    fn step(&self, kind: Kind, text: &str, item: &str) -> Result<usize, Error> {
        let n = digits(text).ok_or_else(|| self.bad_syntax(kind, item))?;
        if n == 0 {
            return Err(Error::new("zero_step", "A step must be 1 or more."));
        }
        let limit = match kind {
            Second | Minute => 60,
            Hour => 24,
            DayOfMonth => 31,
            Month => 12,
            DayOfWeek => 7,
            Year => u32::MAX,
        };
        if self.0 != Unix && n >= limit {
            return Err(Error::new(
                "step_too_large",
                format!(
                    "Step /{text} is too large for the {} field (Quartz needs less than {limit}).",
                    kind.words()
                ),
            ));
        }
        Ok(n as usize)
    }

    /// A number or a three-letter name, checked against the field's range.
    fn value(&self, kind: Kind, tok: &str, item: &str) -> Result<u16, Error> {
        let (lo, hi) = self.bounds(kind);
        if let Some(v) = digits(tok) {
            if (lo as u32..=hi as u32).contains(&v) {
                return Ok(v as u16);
            }
            if kind == Year {
                return Err(Error::new(
                    "year_range",
                    format!("Year {tok} is out of range 1970-2199."),
                ));
            }
            let words = kind.words();
            let e = Error::new(
                "out_of_range",
                format!(
                    "{}{} value {tok} is out of range {lo}-{hi}.",
                    words[..1].to_ascii_uppercase(),
                    &words[1..]
                ),
            );
            return Err(if kind == DayOfWeek && self.0 != Unix {
                e.hint("Day 1 is Sunday and 7 is Saturday here.")
            } else {
                e
            });
        }
        let up = tok.to_ascii_uppercase();
        let names: &[&str] = match kind {
            Month => &MONTHS,
            DayOfWeek => &DAYS,
            _ => &[],
        };
        if let Some(i) = names.iter().position(|n| *n == up) {
            let first = if kind == DayOfWeek && self.0 == Unix {
                0
            } else {
                1
            };
            return Ok(i as u16 + first);
        }
        if self.0 == Unix
            && up
                .chars()
                .all(|c| c.is_ascii_digit() || c == 'L' || c == 'W')
            && let Some(c) = up.chars().find(|c| !c.is_ascii_digit())
        {
            return Err(unix_special(c));
        }
        if !names.is_empty() && !tok.is_empty() && tok.chars().all(|c| c.is_ascii_alphabetic()) {
            let what = if kind == Month { "month" } else { "day" };
            return Err(
                Error::new("unknown_name", format!("Unknown {what} name '{tok}'."))
                    .hint("Use three-letter names: JAN-DEC, SUN-SAT."),
            );
        }
        Err(self.bad_syntax(kind, item))
    }

    fn bad_syntax(&self, kind: Kind, item: &str) -> Error {
        let hint = "Use numbers, *, ranges (1-5), lists (1,3) and steps (*/15)";
        Error::new(
            "bad_syntax",
            format!("Can't read '{item}' in the {} field.", kind.words()),
        )
        .hint(if self.0 == Unix {
            format!("{hint}.")
        } else {
            format!("{hint}, and ? L W # where the field allows them.")
        })
    }
}

fn unix_special(c: char) -> Error {
    Error::new("unix_special", format!("'{c}' isn't Unix cron syntax."))
        .hint("Pass --dialect quartz or aws, or list the days.")
}

/// A run of ASCII digits; too many digits read as `u32::MAX` so they fail range checks.
fn digits(text: &str) -> Option<u32> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().unwrap_or(u32::MAX))
}

/// Checks the raw text, expands a nickname, strips `cron(...)` and works out the dialect
/// (`forced` when the user named one). Returns the dialect and the field texts; `@reboot`
/// is the only input that yields no fields.
fn read(text: &str, forced: Option<Dialect>) -> Result<(Dialect, Vec<&str>), Error> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::new("empty", "No cron expression given.")
            .hint("Example: time-cron next \"0 9 * * 1-5\""));
    }
    if text.len() > 1024 {
        return Err(Error::new(
            "too_long",
            "The expression is longer than 1024 characters.",
        ));
    }
    if text.contains(['\n', '\r']) {
        return Err(Error::new(
            "multiline",
            "The expression must be on one line.",
        ));
    }
    if starts_with_ignore_case(text, "rate(") {
        return Err(
            Error::new("rate_expression", "rate() expressions aren't cron.")
                .hint("Use cron(...) or a cron expression."),
        );
    }
    if let Some(var) = ["CRON_TZ", "TZ"]
        .into_iter()
        .find(|v| text.strip_prefix(v).is_some_and(|r| r.starts_with('=')))
    {
        return Err(Error::new(
            "cron_tz_prefix",
            format!("Put the time zone in --tz instead of {var}=."),
        )
        .hint("Example: time-cron next \"0 9 * * *\" --tz Europe/Berlin"));
    }
    if text.starts_with('@') {
        if matches!(forced, Some(Quartz | Aws)) {
            return Err(Error::new(
                "nickname_dialect",
                "Nicknames like @daily are Unix cron only.",
            )
            .hint("Drop --dialect, or write the fields out."));
        }
        let (_, fields) = NICKNAMES.iter().find(|(n, _)| *n == text).ok_or_else(|| {
            Error::new("unknown_nickname", format!("Unknown nickname '{text}'.")).hint(
                "Use @yearly, @annually, @monthly, @weekly, @daily, @midnight, @hourly or @reboot.",
            )
        })?;
        return Ok((Unix, fields.split_whitespace().collect()));
    }

    let inner = (forced.is_none() || forced == Some(Aws))
        .then(|| aws_inner(text))
        .flatten();
    let fields: Vec<&str> = inner.unwrap_or(text).split_whitespace().collect();
    let n = fields.len();
    let question = |i: usize| fields[i] == "?";
    let dialect = match forced {
        Some(d) => {
            let (ok, want) = match d {
                Unix => (n == 5, "5"),
                Quartz => (n == 6 || n == 7, "6 or 7"),
                Aws => (n == 6, "6"),
            };
            if !ok {
                return Err(Error::new(
                    "dialect_field_count",
                    format!("{} has {want} fields; got {n}.", d.title()),
                )
                .hint("Leave out --dialect to detect it."));
            }
            d
        }
        None if inner.is_some() && n == 6 => Aws,
        None if inner.is_none() && n == 5 => Unix,
        None if inner.is_none() && n == 7 => Quartz,
        None if inner.is_none() && n == 6 => {
            if question(3) || question(5) {
                Quartz
            } else if question(2) || question(4) {
                Aws
            } else {
                return Err(Error::new(
                    "ambiguous_dialect",
                    "6 fields without '?' could be Quartz (seconds first) or AWS (year last).",
                )
                .hint("Put ? in day of month or day of week, or pass --dialect quartz or --dialect aws."));
            }
        }
        None => {
            return Err(Error::new(
                "field_count",
                format!(
                    "Expected 5 fields (Unix), 6 or 7 (Quartz) or cron(...) with 6 (AWS); got {n}."
                ),
            )
            .hint("Separate fields with spaces and quote the expression."));
        }
    };
    Ok((dialect, fields))
}

/// The text inside `cron(...)`, if it is wrapped.
fn aws_inner(text: &str) -> Option<&str> {
    starts_with_ignore_case(text, "cron(")
        .then(|| text[5..].strip_suffix(')'))
        .flatten()
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(text: &str) -> Schedule {
        parse(text, None).unwrap()
    }

    fn list<const W: usize>(set: Set<W>) -> Vec<u16> {
        set.iter().collect()
    }

    fn weekdays(s: &Schedule) -> Vec<u16> {
        match s.dow {
            Dow::Days(set) => list(set),
            other => panic!("not a day set: {other:?}"),
        }
    }

    #[test]
    fn parses_unix_fields() {
        let s = unix("*/15 9-17 * * 1-5");
        assert_eq!(list(s.second), [0]);
        assert_eq!(list(s.minute), [0, 15, 30, 45]);
        assert_eq!(list(s.hour), (9..=17).collect::<Vec<_>>());
        assert_eq!(list(s.dom.days), (1..=31).collect::<Vec<_>>());
        assert_eq!(list(s.month), (1..=12).collect::<Vec<_>>());
        assert_eq!(weekdays(&s), [1, 2, 3, 4, 5]);
        assert_eq!(s.year.iter().count(), 230);
        assert_eq!(s.fields[0].kind, Minute);
        assert_eq!(s.fields[4].text, "1-5");
        assert!(!s.fixed_time);
        assert!(unix("0 9 * * 1-5").fixed_time);
    }

    #[test]
    fn reads_lists_ranges_steps_and_names() {
        assert_eq!(
            list(unix("1,5-7,20-40/10 * * * *").minute),
            [1, 5, 6, 7, 20, 30, 40]
        );
        assert_eq!(list(unix("*/90 * * * *").minute), [0]);
        assert_eq!(list(unix("0 0 1 jan-Mar,DEC *").month), [1, 2, 3, 12]);
        assert_eq!(weekdays(&unix("0 0 * * 5-0")), [0, 5, 6]);
        assert_eq!(weekdays(&unix("0 0 * * FRI-SUN")), [0, 5, 6]);
        assert_eq!(weekdays(&unix("0 0 * * 7")), [0]);
        assert_eq!(weekdays(&unix("0 0 * * 0-7")), (0..7).collect::<Vec<_>>());
        assert_eq!(weekdays(&unix("0 0 * * */2")), [0, 2, 4, 6]);
        assert_eq!(list(unix("0 0 */31 2 MON").dom.days), [1]);
    }

    #[test]
    fn day_fields_combine_like_cronie() {
        let rule = |text| unix(text).day_rule;
        assert_eq!(rule("0 9 1 * MON"), DayRule::Or);
        assert_eq!(rule("0 9 1-31 * MON"), DayRule::Or);
        assert_eq!(rule("0 9 */2 * MON"), DayRule::And);
        assert_eq!(rule("0 9 * * MON"), DayRule::DowOnly);
        assert_eq!(rule("0 9 */2 * *"), DayRule::DomOnly);
        assert_eq!(rule("0 9 * * *"), DayRule::DowOnly);
    }

    #[test]
    fn parses_quartz_and_aws_fields() {
        let q = |text| parse(text, None).unwrap();
        let s = q("*/30 0/15 22-2 ? * 1-5");
        assert_eq!(s.dialect, Quartz);
        assert_eq!(list(s.second), [0, 30]);
        assert_eq!(list(s.minute), [0, 15, 30, 45]);
        assert_eq!(list(s.hour), [0, 1, 2, 22, 23]);
        assert_eq!(weekdays(&s), [0, 1, 2, 3, 4]);
        assert_eq!(s.day_rule, DayRule::DowOnly);
        assert_eq!(weekdays(&q("0 0 12 ? * FRI-MON")), [0, 1, 5, 6]);
        assert_eq!(list(q("0 0 12 28-2 * ?").dom.days), [1, 2, 28, 29, 30, 31]);
        assert_eq!(list(q("0 0 12 1 11-2 ?").month), [1, 2, 11, 12]);
        assert_eq!(list(q("0 0 12 1 1 ? 2030").year), [2030]);
        assert_eq!(q("0 0 12 1 1 ?").year.iter().count(), 230);

        let s = q("cron(0 18 ? * MON-FRI *)");
        assert_eq!((s.dialect, s.fields.len()), (Aws, 6));
        assert_eq!(
            (list(s.second), list(s.minute), list(s.hour)),
            (vec![0], vec![0], vec![18])
        );
        assert_eq!(weekdays(&s), [1, 2, 3, 4, 5]);
        assert_eq!(q("cron(0 8 1 * ? *)").day_rule, DayRule::DomOnly);
    }

    #[test]
    fn reads_quartz_day_specials() {
        let dow = |text| parse(text, None).unwrap().dow;
        assert_eq!(dow("0 0 12 ? * MON#2"), Dow::Nth(1, 2));
        assert_eq!(dow("0 0 12 ? * 2#5"), Dow::Nth(1, 5));
        assert_eq!(dow("0 15 10 ? * 6L"), Dow::Last(5));
        assert_eq!(dow("0 15 10 ? * fril"), Dow::Last(5));
        assert_eq!(dow("0 0 12 ? * L"), Dow::Days([6].into_iter().collect()));

        let dom = |text| parse(text, None).unwrap().dom;
        let last = |offset, weekday| DomSpecial::Last { offset, weekday };
        assert_eq!(dom("0 0 9 15W * ?").specials, [DomSpecial::Nearest(15)]);
        assert_eq!(dom("0 0 9 LW * ?").specials, [last(0, true)]);
        assert_eq!(dom("0 0 12 L-3 * ?").specials, [last(3, false)]);
        assert_eq!(dom("0 0 12 l-30w * ?").specials, [last(30, true)]);
        let d = dom("0 0 12 L,15 * ?");
        assert_eq!((d.specials, list(d.days)), (vec![last(0, false)], vec![15]));
        assert_eq!(dom("cron(0 12 L * ? *)").specials, [last(0, false)]);
    }

    #[test]
    fn nicknames_report_their_expansion() {
        let s = unix("@daily");
        assert_eq!(
            s.fields.iter().map(|f| &f.text[..]).collect::<Vec<_>>(),
            ["0", "0", "*", "*", "*"]
        );
        assert!(s.fixed_time);
        assert!(!unix("@hourly").fixed_time);
        let r = unix("@reboot");
        assert!(r.reboot && r.fields.is_empty());
    }

    fn dialect(text: &str) -> Result<Dialect, &'static str> {
        read(text, None).map(|(d, _)| d).map_err(|e| e.code)
    }

    #[test]
    fn detects_the_dialect() {
        assert_eq!(dialect("0 9 * * 1-5"), Ok(Unix));
        assert_eq!(dialect("0 0 12 ? * MON#2"), Ok(Quartz));
        assert_eq!(dialect("0 18 ? * MON-FRI *"), Ok(Aws));
        assert_eq!(dialect("cron(0 18 ? * MON-FRI *)"), Ok(Aws));
        assert_eq!(dialect("CRON( 0 18 ? * MON-FRI * )"), Ok(Aws));
        assert_eq!(dialect("0 0 12 * * *"), Err("ambiguous_dialect"));
        assert_eq!(dialect("0 0 12 ? * MON 2030"), Ok(Quartz));
        assert_eq!(dialect("0 0 12 * * ?"), Ok(Quartz));
        assert_eq!(dialect("0 12 * * ? *"), Ok(Aws));
        assert_eq!(dialect("@daily"), Ok(Unix));
    }

    #[test]
    fn trims_and_splits_on_any_blank_run() {
        let (_, fields) = read(" 0\t9  * *   1-5\r\n", None).unwrap();
        assert_eq!(fields, ["0", "9", "*", "*", "1-5"]);
    }

    #[test]
    fn expands_nicknames() {
        assert_eq!(read("@weekly", None).unwrap().1, ["0", "0", "*", "*", "0"]);
        assert_eq!(read("@reboot", None).unwrap(), (Unix, vec![]));
    }

    #[test]
    fn a_named_dialect_is_parsed_strictly() {
        assert_eq!(read("0 0 12 * * *", Some(Quartz)).unwrap().0, Quartz);
        assert_eq!(read("0 12 * * ? *", Some(Aws)).unwrap().0, Aws);
        assert_eq!(read("cron(0 12 * * ? *)", Some(Aws)).unwrap().0, Aws);
        let e = read("cron(0 12 * * ? *)", Some(Unix)).unwrap_err();
        assert_eq!(e.code, "dialect_field_count");
        assert_eq!(e.message, "Unix cron has 5 fields; got 6.");
    }
}
