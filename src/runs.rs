use jiff::civil::{Date, DateTime, Time, date};
use jiff::tz::{self, AmbiguousOffset, Offset, TimeZone};
use jiff::{Timestamp, ToSpan, Zoned};

use crate::error::{Error, Warning};
use crate::schedule::{DayRule, Dialect, Schedule};

/// Quartz and AWS stop at 2199, and so does every search.
const LAST_YEAR: i16 = 2199;

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[derive(Debug)]
pub struct Next {
    pub timezone: String,
    pub from: Zoned,
    pub runs: Vec<Run>,
    /// Wall times the scheduler drops because a DST gap swallows them.
    pub skipped: Vec<DateTime>,
    pub warnings: Vec<Warning>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub time: Zoned,
    pub dst: Option<Dst>,
    /// The wall time a run moved out of a DST gap was meant for.
    pub scheduled: Option<DateTime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dst {
    Moved,
    First,
    Second,
}

/// The next `count` runs strictly after `from` (default now) in zone `tz` (default the
/// computer's zone, or UTC for AWS).
pub fn next(
    s: &Schedule,
    tz: Option<&str>,
    from: Option<&str>,
    count: usize,
) -> Result<Next, Error> {
    if s.reboot {
        return Err(Error::new(
            "reboot",
            "@reboot runs once when cron starts, so it has no run times.",
        ));
    }
    let mut warnings = vec![];
    let tz = zone(s, tz, &mut warnings)?;
    let from = start(from, &tz)?;
    let mut runs = vec![];
    let mut skipped = vec![];
    for day in days(s, from.date()) {
        let mut today = vec![];
        for t in times(s) {
            resolve(s, &tz, day.to_datetime(t), &mut today, &mut skipped);
        }
        today.sort_by_key(|r| r.time.timestamp());
        runs.extend(
            today
                .into_iter()
                .filter(|r| r.time.timestamp() > from.timestamp()),
        );
        if runs.len() >= count {
            break;
        }
    }
    runs.truncate(count);
    let Some(last) = runs.last().map(|r| r.time.timestamp()) else {
        return Err(never_runs(s, &from));
    };
    let skipped = skipped
        .into_iter()
        .filter(|(at, _)| *at > from.timestamp() && *at <= last)
        .map(|(_, dt)| dt)
        .collect();
    if runs.len() < count {
        let year = s.year.iter().last().unwrap_or(LAST_YEAR as u16);
        warnings.push(Warning::new(
            "few_runs",
            format!(
                "Only {} of {count} runs exist before the end of {year}.",
                runs.len()
            ),
        ));
    }
    Ok(Next {
        timezone: tz.iana_name().unwrap_or("UTC").to_string(),
        from,
        runs,
        skipped,
        warnings,
    })
}

/// Adds the run(s) the scheduler makes for wall time `dt`, or the time it skips, by its own
/// DST rules: cronie replays fixed-time jobs across shifts of up to 3 hours, Quartz takes
/// the second pass of a fold, EventBridge the first, and both drop gap times.
fn resolve(
    s: &Schedule,
    tz: &TimeZone,
    dt: DateTime,
    runs: &mut Vec<Run>,
    skipped: &mut Vec<(Timestamp, DateTime)>,
) {
    let cronie = |shift: i32| s.dialect == Dialect::Unix && s.fixed_time && shift.abs() <= 3 * 3600;
    let at = |offset: Offset| offset.to_timestamp(dt).unwrap();
    let mut run = |ts: Timestamp, dst: Dst| {
        runs.push(Run {
            time: ts.to_zoned(tz.clone()),
            dst: Some(dst),
            scheduled: (dst == Dst::Moved).then_some(dt),
        })
    };
    match tz.to_ambiguous_timestamp(dt).offset() {
        AmbiguousOffset::Unambiguous { offset } => runs.push(Run {
            time: at(offset).to_zoned(tz.clone()),
            dst: None,
            scheduled: None,
        }),
        AmbiguousOffset::Gap { before, after } => {
            // The first instant of the new offset.
            let jump = tz.following(at(after)).next().unwrap().timestamp();
            if cronie(after.seconds() - before.seconds()) {
                run(jump, Dst::Moved);
            } else {
                skipped.push((jump, dt));
            }
        }
        AmbiguousOffset::Fold { before, after } => match s.dialect {
            Dialect::Quartz => run(at(after), Dst::Second),
            Dialect::Unix if !cronie(before.seconds() - after.seconds()) => {
                run(at(before), Dst::First);
                run(at(after), Dst::Second);
            }
            _ => run(at(before), Dst::First),
        },
    }
}

fn zone(s: &Schedule, name: Option<&str>, warnings: &mut Vec<Warning>) -> Result<TimeZone, Error> {
    let Some(name) = name else {
        return Ok(match s.dialect {
            Dialect::Aws => TimeZone::UTC,
            _ => local(TimeZone::try_system().ok(), warnings),
        });
    };
    let tz = tz::db().get(name).map_err(|_| {
        Error::new("unknown_tz", format!("Unknown time zone '{name}'."))
            .hint("Use an IANA name such as America/New_York, Europe/Berlin or UTC.")
    })?;
    let utc = tz.preceding(Timestamp::MAX).next().is_none()
        && tz.to_offset(Timestamp::UNIX_EPOCH) == Offset::UTC;
    if s.dialect == Dialect::Aws && !utc {
        warnings.push(Warning::new(
            "aws_rules_utc",
            format!(
                "EventBridge rules always run in UTC; only EventBridge Scheduler uses {}.",
                tz.iana_name().unwrap_or(name)
            ),
        ));
    }
    Ok(tz)
}

/// The computer's zone, or UTC when it has no IANA name to report.
fn local(system: Option<TimeZone>, warnings: &mut Vec<Warning>) -> TimeZone {
    match system {
        Some(tz) if tz.iana_name().is_some() => tz,
        _ => {
            warnings.push(Warning::new(
                "tz_fallback",
                "Couldn't find this computer's time zone, so times are in UTC. Pass --tz to choose one.",
            ));
            TimeZone::UTC
        }
    }
}

/// Reads `now`, a date, a civil date-time in `tz` or an RFC 3339 instant.
fn start(text: Option<&str>, tz: &TimeZone) -> Result<Zoned, Error> {
    let from = match text
        .map(str::trim)
        .filter(|t| !t.eq_ignore_ascii_case("now"))
    {
        None => Timestamp::now().to_zoned(tz.clone()),
        Some(t) => t
            .parse::<Timestamp>()
            .map(|ts| ts.to_zoned(tz.clone()))
            .or_else(|_| t.parse::<DateTime>()?.to_zoned(tz.clone()))
            .map_err(|_| {
                Error::new("bad_from", format!("Can't read the start time '{t}'."))
                    .hint("Use 2026-11-01, 2026-11-01T09:00 (in --tz) or 2026-11-01T09:00:00Z.")
            })?,
    };
    if !(1970..=LAST_YEAR).contains(&from.year()) {
        return Err(Error::new(
            "from_range",
            "The start time must be between 1970 and 2199.",
        ));
    }
    Ok(from)
}

/// The dates from `from` through 2199 that the schedule runs on.
fn days(s: &Schedule, from: Date) -> impl Iterator<Item = Date> + '_ {
    (from.year()..=LAST_YEAR)
        .filter(|&y| s.year.contains(y as u16))
        .flat_map(|y| s.month.iter().map(move |m| date(y, m as i8, 1)))
        .flat_map(|first| first.series(1.day()).take(first.days_in_month() as usize))
        .filter(move |&d| d >= from && s.runs_on(d))
}

/// The schedule's wall times on any day it runs, in order.
fn times(s: &Schedule) -> impl Iterator<Item = Time> + '_ {
    s.hour.iter().flat_map(move |h| {
        s.minute.iter().flat_map(move |m| {
            s.second
                .iter()
                .map(move |sec| Time::constant(h as i8, m as i8, sec as i8, 0))
        })
    })
}

fn never_runs(s: &Schedule, from: &Zoned) -> Error {
    let error =
        |reason: String| Error::new("never_runs", format!("This schedule never runs: {reason}."));
    if let Some(reason) = missing_day(s) {
        return error(reason)
            .hint("Pick a day that exists in every chosen month, or use L in Quartz or AWS.");
    }
    match s.year.iter().last() {
        Some(y) if (y as i16) < from.year() => error(format!(
            "the year field ends in {y}, before {}",
            from.date()
        )),
        _ => error(format!(
            "no time matches between {} and the end of {LAST_YEAR}",
            from.date()
        )),
    }
}

/// "day 30 doesn't occur in February", when no chosen day exists in any chosen month.
fn missing_day(s: &Schedule) -> Option<String> {
    if !matches!(s.day_rule, DayRule::DomOnly | DayRule::And) || !s.dom.specials.is_empty() {
        return None;
    }
    // 2000 is a leap year, so February has its 29th.
    let longest = |m: u16| date(2000, m as i8, 1).days_in_month() as u16;
    let days: Vec<u16> = s.dom.days.iter().collect();
    if s.month
        .iter()
        .any(|m| days.iter().any(|&d| d <= longest(m)))
    {
        return None;
    }
    let months = s
        .month
        .iter()
        .map(|m| MONTH_NAMES[m as usize - 1].to_string());
    let days_text = and_list(days.iter().map(u16::to_string));
    let months_text = and_list(months);
    Some(if days.len() == 1 {
        format!("day {days_text} doesn't occur in {months_text}")
    } else {
        format!("days {days_text} don't occur in {months_text}")
    })
}

/// "a", "a and b", "a, b and c".
fn and_list(items: impl Iterator<Item = String>) -> String {
    let mut items: Vec<String> = items.collect();
    let last = items.pop().unwrap_or_default();
    if items.is_empty() {
        last
    } else {
        format!("{} and {last}", items.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::parse;

    fn utc_runs(text: &str, count: usize) -> Vec<String> {
        let next = next(
            &parse(text, None).unwrap(),
            Some("UTC"),
            Some("2026-10-29T00:00:00Z"),
            count,
        )
        .unwrap();
        next.runs
            .iter()
            .map(|r| r.time.timestamp().to_string())
            .collect()
    }

    /// Runs as `time [dst] [scheduled]`, and the skipped wall times.
    fn runs_in(text: &str, tz: &str, from: &str, count: usize) -> (Vec<String>, Vec<String>) {
        let n = next(&parse(text, None).unwrap(), Some(tz), Some(from), count).unwrap();
        let runs = n.runs.iter().map(|r| {
            let mut line = r
                .time
                .timestamp()
                .display_with_offset(r.time.offset())
                .to_string();
            if let Some(dst) = r.dst {
                line += &format!(" {dst:?}");
            }
            if let Some(scheduled) = r.scheduled {
                line += &format!(" {scheduled}");
            }
            line
        });
        (
            runs.collect(),
            n.skipped.iter().map(DateTime::to_string).collect(),
        )
    }

    #[test]
    #[rustfmt::skip]
    fn follows_each_schedulers_dst_rules() {
        const NY: &str = "America/New_York";
        const SPRING: &str = "2027-03-13T12:00";
        const AUTUMN: &str = "2026-10-31T23:00";
        let none: [&str; 0] = [];
        // cronie runs a fixed-time job lost in the gap right after the jump...
        let (runs, skipped) = runs_in("30 2 * * *", NY, SPRING, 3);
        assert_eq!(runs, ["2027-03-14T03:00:00-04:00 Moved 2027-03-14T02:30:00", "2027-03-15T02:30:00-04:00", "2027-03-16T02:30:00-04:00"]);
        assert_eq!(skipped, none);
        // ...but skips a wildcard job's gap times.
        let (runs, skipped) = runs_in("*/15 2 * * *", NY, SPRING, 1);
        assert_eq!(runs, ["2027-03-15T02:00:00-04:00"]);
        assert_eq!(skipped, ["2027-03-14T02:00:00", "2027-03-14T02:15:00", "2027-03-14T02:30:00", "2027-03-14T02:45:00"]);
        // In a fold cronie runs a fixed-time job once and a wildcard job twice.
        assert_eq!(runs_in("30 1 * * *", NY, AUTUMN, 2).0, ["2026-11-01T01:30:00-04:00 First", "2026-11-02T01:30:00-05:00"]);
        assert_eq!(
            runs_in("0 * * * *", NY, AUTUMN, 4).0,
            ["2026-11-01T00:00:00-04:00", "2026-11-01T01:00:00-04:00 First", "2026-11-01T01:00:00-05:00 Second", "2026-11-01T02:00:00-05:00"]
        );
        // Quartz takes the second pass and skips gap times.
        assert_eq!(runs_in("0 30 1 * * ?", NY, AUTUMN, 2).0, ["2026-11-01T01:30:00-05:00 Second", "2026-11-02T01:30:00-05:00"]);
        assert_eq!(runs_in("0 30 2 * * ?", NY, SPRING, 1), (vec!["2027-03-15T02:30:00-04:00".into()], vec!["2027-03-14T02:30:00".into()]));
        // EventBridge takes the first pass and skips gap times.
        assert_eq!(runs_in("cron(30 1 * * ? *)", NY, AUTUMN, 2).0, ["2026-11-01T01:30:00-04:00 First", "2026-11-02T01:30:00-05:00"]);
        assert_eq!(runs_in("cron(30 2 * * ? *)", NY, SPRING, 1), (vec!["2027-03-15T02:30:00-04:00".into()], vec!["2027-03-14T02:30:00".into()]));
    }

    fn error(
        text: &str,
        tz: Option<&str>,
        from: Option<&str>,
    ) -> (&'static str, String, Option<String>) {
        let e = next(&parse(text, None).unwrap(), tz, from, 5).unwrap_err();
        (e.code, e.message, e.hint)
    }

    #[test]
    fn lists_runs_strictly_after_the_start() {
        assert_eq!(
            utc_runs("0 9 1 * MON", 4),
            [
                "2026-11-01T09:00:00Z",
                "2026-11-02T09:00:00Z",
                "2026-11-09T09:00:00Z",
                "2026-11-16T09:00:00Z"
            ]
        );
        assert_eq!(utc_runs("0 0 29 2 *", 1), ["2028-02-29T00:00:00Z"]);
        assert_eq!(utc_runs("@daily", 1), ["2026-10-30T00:00:00Z"]);
        assert_eq!(
            utc_runs("*/20 * * * * ?", 3),
            [
                "2026-10-29T00:00:20Z",
                "2026-10-29T00:00:40Z",
                "2026-10-29T00:01:00Z"
            ]
        );
    }

    #[test]
    fn reads_the_start_time_in_the_zone() {
        let s = parse("0 * * * *", None).unwrap();
        let from = |text| {
            next(&s, Some("America/New_York"), Some(text), 1)
                .unwrap()
                .from
                .to_string()
        };
        assert_eq!(
            from("2027-03-13T12:00"),
            "2027-03-13T12:00:00-05:00[America/New_York]"
        );
        assert_eq!(
            from("2027-03-13 12:00:30"),
            "2027-03-13T12:00:30-05:00[America/New_York]"
        );
        assert_eq!(
            from("2027-03-13"),
            "2027-03-13T00:00:00-05:00[America/New_York]"
        );
        assert_eq!(
            from("2027-03-13T17:00:00Z"),
            "2027-03-13T12:00:00-05:00[America/New_York]"
        );
        assert_eq!(
            from("2027-03-13T22:30+05:30"),
            "2027-03-13T12:00:00-05:00[America/New_York]"
        );
        // A time in the spring gap resolves later, one in the autumn fold earlier.
        assert_eq!(
            from("2027-03-14T02:30"),
            "2027-03-14T03:30:00-04:00[America/New_York]"
        );
        assert_eq!(
            from("2026-11-01T01:30"),
            "2026-11-01T01:30:00-04:00[America/New_York]"
        );
        assert!(next(&s, Some("UTC"), Some(" NOW "), 1).is_ok());
        assert!(next(&s, Some("UTC"), None, 1).is_ok());
    }

    #[test]
    fn names_the_zone_as_the_database_does() {
        let s = parse("0 * * * *", None).unwrap();
        let name = |tz| next(&s, Some(tz), Some("2026-01-01"), 1).unwrap().timezone;
        assert_eq!(name("america/new_york"), "America/New_York");
        assert_eq!(name("utc"), "UTC");
    }

    #[test]
    fn defaults_to_the_computers_zone_or_utc() {
        let mut warnings = vec![];
        let berlin = tz::db().get("Europe/Berlin").unwrap();
        assert_eq!(local(Some(berlin.clone()), &mut warnings), berlin);
        assert!(warnings.is_empty());
        let fixed = TimeZone::fixed(Offset::constant(3));
        assert_eq!(local(Some(fixed), &mut warnings), TimeZone::UTC);
        assert_eq!(local(None, &mut warnings), TimeZone::UTC);
        assert_eq!(warnings.len(), 2);
        assert_eq!(
            warnings[0].message,
            "Couldn't find this computer's time zone, so times are in UTC. Pass --tz to choose one."
        );

        let aws = parse("cron(0 18 ? * MON-FRI *)", None).unwrap();
        let n = next(&aws, None, Some("2026-10-29T00:00:00Z"), 3).unwrap();
        assert_eq!(n.timezone, "UTC");
        assert!(n.warnings.is_empty());
        assert_eq!(
            n.runs
                .iter()
                .map(|r| r.time.timestamp().to_string())
                .collect::<Vec<_>>(),
            [
                "2026-10-29T18:00:00Z",
                "2026-10-30T18:00:00Z",
                "2026-11-02T18:00:00Z"
            ]
        );
        assert!(
            next(&aws, Some("Etc/UTC"), None, 1)
                .unwrap()
                .warnings
                .is_empty()
        );
        let n = next(&aws, Some("America/New_York"), None, 1).unwrap();
        assert_eq!(n.warnings[0].code, "aws_rules_utc");
        assert_eq!(
            n.warnings[0].message,
            "EventBridge rules always run in UTC; only EventBridge Scheduler uses America/New_York."
        );
    }

    #[test]
    fn warns_when_fewer_runs_exist() {
        let s = parse("0 0 12 1 1 ? 2027-2028", None).unwrap();
        let n = next(&s, Some("UTC"), Some("2026-10-29"), 5).unwrap();
        assert_eq!(n.runs.len(), 2);
        assert_eq!(n.warnings[0].code, "few_runs");
        assert_eq!(
            n.warnings[0].message,
            "Only 2 of 5 runs exist before the end of 2028."
        );
    }

    #[test]
    #[rustfmt::skip]
    fn explains_why_a_schedule_never_runs_and_rejects_bad_input() {
        const DAY_HINT: &str = "Pick a day that exists in every chosen month, or use L in Quartz or AWS.";
        let from = Some("2026-10-29T00:00:00Z");
        let utc = Some("UTC");
        let cases = [
            ("0 0 30 2 *", utc, from, "never_runs", "This schedule never runs: day 30 doesn't occur in February.", Some(DAY_HINT)),
            ("0 0 0 31 2 ? *", utc, from, "never_runs", "This schedule never runs: day 31 doesn't occur in February.", Some(DAY_HINT)),
            ("0 0 31 4,6 *", utc, from, "never_runs", "This schedule never runs: day 31 doesn't occur in April and June.", Some(DAY_HINT)),
            ("0 0 30,31 2 *", utc, from, "never_runs", "This schedule never runs: days 30 and 31 don't occur in February.", Some(DAY_HINT)),
            ("0 0 12 1 1 ? 2020", utc, from, "never_runs", "This schedule never runs: the year field ends in 2020, before 2026-10-29.", None),
            ("0 0 12 ? 2 MON#5 2026-2027", utc, from, "never_runs", "This schedule never runs: no time matches between 2026-10-29 and the end of 2199.", None),
            ("@reboot", utc, from, "reboot", "@reboot runs once when cron starts, so it has no run times.", None),
            ("0 9 * * *", Some("Mars/Olympus"), from, "unknown_tz", "Unknown time zone 'Mars/Olympus'.", Some("Use an IANA name such as America/New_York, Europe/Berlin or UTC.")),
            ("0 9 * * *", utc, Some("tomorrow"), "bad_from", "Can't read the start time 'tomorrow'.", Some("Use 2026-11-01, 2026-11-01T09:00 (in --tz) or 2026-11-01T09:00:00Z.")),
            ("0 9 * * *", utc, Some("2200-01-01"), "from_range", "The start time must be between 1970 and 2199.", None),
            ("0 9 * * *", utc, Some("1969-12-31T23:00:00Z"), "from_range", "The start time must be between 1970 and 2199.", None),
        ];
        for (text, tz, from, code, message, hint) in cases {
            assert_eq!(error(text, tz, from), (code, message.to_string(), hint.map(String::from)), "{text}");
        }
    }
}
