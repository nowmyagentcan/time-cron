//! The answer to each command, shared by every face.

use jiff::civil::DateTime;
use jiff::fmt::temporal::DateTimePrinter;
use jiff::{Unit, Zoned};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::convert;
use crate::describe::{FieldInfo, describe, fields};
use crate::error::{Error, Warning};
use crate::expr::parse;
use crate::lint::lint;
use crate::runs::{self, Dst, Run};
use crate::schedule::{Dialect, Schedule};

/// The bundled IANA tzdata release, e.g. "2026f".
pub const TZDATA: &str = match jiff_tzdb::VERSION {
    Some(v) => v,
    None => "unknown",
};

#[derive(Debug, Serialize, JsonSchema)]
pub struct Explained {
    pub expression: String,
    pub dialect: Dialect,
    pub description: String,
    pub fields: Vec<FieldInfo>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Upcoming {
    pub expression: String,
    pub dialect: Dialect,
    pub description: String,
    pub timezone: String,
    pub tzdata: &'static str,
    #[serde(serialize_with = "rfc3339")]
    #[schemars(with = "String")]
    pub from: Zoned,
    #[schemars(with = "Vec<RunJson>")]
    pub runs: Vec<Run>,
    #[serde(serialize_with = "gap_skips")]
    #[schemars(with = "Vec<Skip>")]
    pub skipped: Vec<DateTime>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Converted {
    pub expression: String,
    pub dialect: Dialect,
    pub to: Dialect,
    pub results: Vec<String>,
    pub description: String,
    pub warnings: Vec<Warning>,
}

/// `{"error": {"code", "message", "hint"}}`, the JSON for every failure.
#[derive(Serialize)]
struct Failure<'a> {
    error: &'a Error,
}

/// The `Failure` JSON, for faces that send it as text.
pub fn failure(error: &Error) -> String {
    serde_json::to_string(&Failure { error }).expect("errors are plain data")
}

/// A run as JSON: `time` with the zone's offset, `utc`, and the DST note.
#[derive(Serialize, JsonSchema)]
struct RunJson {
    time: String,
    utc: String,
    dst: Option<Dst>,
    /// The wall time a run moved out of a DST gap was meant for.
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduled: Option<String>,
}

impl Serialize for Run {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        RunJson {
            time: rfc3339_text(&self.time),
            utc: self.time.timestamp().to_string(),
            dst: self.dst,
            scheduled: self.scheduled.map(|at| at.to_string()),
        }
        .serialize(s)
    }
}

/// A wall time the scheduler skips; `reason` is always "dst_gap".
#[derive(Serialize, JsonSchema)]
struct Skip {
    scheduled: String,
    reason: &'static str,
}

/// RFC 3339 to the second. An offset with seconds (Monrovia's -00:44:30 until 1972)
/// becomes the closest whole-minute offset and the wall time moves with it, so the
/// text still names the same instant (RFC 3339 §5.8, the 1937 Netherlands example).
fn rfc3339_text(z: &Zoned) -> String {
    let offset = z
        .offset()
        .round(Unit::Minute)
        .expect("tzdata offsets are in range");
    DateTimePrinter::new()
        .precision(Some(0))
        .timestamp_with_offset_to_string(&z.timestamp(), offset)
}

fn rfc3339<S: Serializer>(z: &Zoned, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&rfc3339_text(z))
}

fn gap_skips<S: Serializer>(skipped: &[DateTime], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(skipped.iter().map(|at| Skip {
        scheduled: at.to_string(),
        reason: "dst_gap",
    }))
}

/// The dialect to read: detected, or the one named.
#[derive(Clone, Copy, Default, Serialize, Deserialize, JsonSchema, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DialectArg {
    #[default]
    Auto,
    Unix,
    Quartz,
    Aws,
}

impl DialectArg {
    pub fn dialect(self) -> Option<Dialect> {
        match self {
            DialectArg::Auto => None,
            DialectArg::Unix => Some(Dialect::Unix),
            DialectArg::Quartz => Some(Dialect::Quartz),
            DialectArg::Aws => Some(Dialect::Aws),
        }
    }
}

/// The expression to read, as the page and MCP clients send it.
#[derive(Deserialize, JsonSchema)]
pub struct Source {
    /// A cron expression: Unix (5 fields), Quartz (6-7 fields, seconds first) or AWS cron(...).
    pub expression: String,
    /// auto detects the dialect.
    #[serde(default)]
    pub dialect: DialectArg,
}

#[derive(Deserialize, JsonSchema)]
pub struct NextArgs {
    #[serde(flatten)]
    pub source: Source,
    /// IANA time zone, e.g. America/New_York [default: this computer's; UTC for aws].
    pub timezone: Option<String>,
    /// Start after this time: now, 2026-11-01, 2026-11-01T09:00 (in the time zone) or 2026-11-01T09:00:00Z [default: now].
    pub from: Option<String>,
    /// Number of runs.
    #[serde(default = "five", deserialize_with = "count")]
    #[schemars(range(min = 1, max = 1000))]
    pub count: usize,
}

fn five() -> usize {
    5
}

/// 1-1000, refused in words rather than in Rust's type names.
fn count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    usize::deserialize(d)
        .ok()
        .filter(|n| (1..=1000).contains(n))
        .ok_or_else(|| serde::de::Error::custom("count must be a whole number between 1 and 1000"))
}

#[derive(Deserialize, JsonSchema)]
pub struct ConvertArgs {
    #[serde(flatten)]
    pub source: Source,
    /// The dialect to write.
    pub to: Dialect,
}

/// `{"op": "explain" | "next" | "convert", ...}`: a command as JSON.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Request {
    Explain(Source),
    Next(NextArgs),
    Convert(ConvertArgs),
}

fn json(answer: impl Serialize) -> Value {
    serde_json::to_value(answer).expect("answers are plain data")
}

/// The `--json` answer to a request from the page or an MCP client. A request that
/// can't be read fails with `bad_request`.
pub fn answer(request: Value) -> Result<Value, Error> {
    let bad = |why: &dyn std::fmt::Display| {
        Error::new("bad_request", format!("Can't read the request: {why}."))
    };
    Ok(
        match serde_json::from_value(request).map_err(|e| bad(&e))? {
            Request::Explain(s) => json(explain(&s.expression, s.dialect.dialect())?),
            Request::Next(n) => json(next(
                &n.source.expression,
                n.source.dialect.dialect(),
                n.timezone.as_deref(),
                n.from.as_deref(),
                n.count,
            )?),
            Request::Convert(c) => json(convert(
                &c.source.expression,
                c.source.dialect.dialect(),
                c.to,
            )?),
        },
    )
}

/// The parsed schedule, refused if it never runs: no scheduler would run it either.
fn runnable(expression: &str, dialect: Option<Dialect>) -> Result<Schedule, Error> {
    let s = parse(expression, dialect)?;
    if !s.reboot {
        runs::ever(&s, 1)?;
    }
    Ok(s)
}

pub fn explain(expression: &str, dialect: Option<Dialect>) -> Result<Explained, Error> {
    let s = runnable(expression, dialect)?;
    Ok(Explained {
        expression: expression.trim().into(),
        dialect: s.dialect,
        description: describe(&s),
        fields: fields(&s),
        warnings: lint(&s),
    })
}

pub fn next(
    expression: &str,
    dialect: Option<Dialect>,
    tz: Option<&str>,
    from: Option<&str>,
    count: usize,
) -> Result<Upcoming, Error> {
    let s = parse(expression, dialect)?;
    let n = runs::next(&s, tz, from, count)?;
    let mut warnings = lint(&s);
    warnings.extend(n.warnings);
    Ok(Upcoming {
        expression: expression.trim().into(),
        dialect: s.dialect,
        description: describe(&s),
        timezone: n.timezone,
        tzdata: TZDATA,
        from: n.from,
        runs: n.runs,
        skipped: n.skipped,
        warnings,
    })
}

pub fn convert(
    expression: &str,
    dialect: Option<Dialect>,
    to: Dialect,
) -> Result<Converted, Error> {
    let s = runnable(expression, dialect)?;
    let (results, warnings) = convert::convert(&s, to)?;
    Ok(Converted {
        expression: expression.trim().into(),
        dialect: s.dialect,
        to,
        results,
        description: describe(&s),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn puts_static_warnings_before_run_time_ones() {
        let u = next(
            "cron(0 2 ? * 1-5 *)",
            None,
            Some("America/New_York"),
            Some("2027-03-01"),
            1,
        )
        .unwrap();
        let codes: Vec<_> = u.warnings.iter().map(|w| w.code).collect();
        assert_eq!(codes, ["dow_one_is_sunday", "aws_rules_utc", "dst_gap"]);
        assert_eq!((u.dialect, u.tzdata), (Dialect::Aws, "2026f"));
    }

    #[test]
    fn serializes_next_like_the_documented_example() {
        let u = next(
            "30 2 * * *",
            None,
            Some("America/New_York"),
            Some("2027-03-13T12:00"),
            3,
        )
        .unwrap();
        let gap = "clocks jump from 02:00 to 03:00 in America/New_York. Cron runs 02:30 right after the jump.";
        assert_eq!(
            serde_json::to_value(&u).unwrap(),
            json!({
                "expression": "30 2 * * *",
                "dialect": "unix",
                "description": "At 02:30 every day.",
                "timezone": "America/New_York",
                "tzdata": "2026f",
                "from": "2027-03-13T12:00:00-05:00",
                "runs": [
                    {"time": "2027-03-14T03:00:00-04:00", "utc": "2027-03-14T07:00:00Z", "dst": "moved", "scheduled": "2027-03-14T02:30:00"},
                    {"time": "2027-03-15T02:30:00-04:00", "utc": "2027-03-15T06:30:00Z", "dst": null},
                    {"time": "2027-03-16T02:30:00-04:00", "utc": "2027-03-16T06:30:00Z", "dst": null}
                ],
                "skipped": [],
                "warnings": [
                    {"code": "dst_gap", "message": format!("2027-03-14: {gap}")},
                    {"code": "dst_gap", "message": format!("2028-03-12: {gap}")}
                ]
            })
        );
        let u = next(
            "0 30 2 * * ?",
            None,
            Some("America/New_York"),
            Some("2027-03-13T12:00"),
            1,
        );
        assert_eq!(
            serde_json::to_value(u.unwrap()).unwrap()["skipped"],
            json!([{"scheduled": "2027-03-14T02:30:00", "reason": "dst_gap"}])
        );
    }

    #[test]
    fn serializes_explain_and_errors_like_the_documented_examples() {
        let e = explain("*/15 9-17 * * 1-5", None).unwrap();
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({
                "expression": "*/15 9-17 * * 1-5",
                "dialect": "unix",
                "description": "Every 15 minutes from 09:00 through 17:45, Monday through Friday.",
                "fields": [
                    {"name": "minute", "text": "*/15", "values": [0, 15, 30, 45], "meaning": "every 15 minutes"},
                    {"name": "hour", "text": "9-17", "values": [9, 10, 11, 12, 13, 14, 15, 16, 17], "meaning": "9 through 17"},
                    {"name": "day_of_month", "text": "*", "values": null, "meaning": "any day"},
                    {"name": "month", "text": "*", "values": null, "meaning": "any month"},
                    {"name": "day_of_week", "text": "1-5", "values": ["MON", "TUE", "WED", "THU", "FRI"], "meaning": "Monday through Friday"}
                ],
                "warnings": []
            })
        );
        let error = explain("rate(5 minutes)", None).unwrap_err();
        assert_eq!(
            serde_json::to_value(Failure { error: &error }).unwrap(),
            json!({"error": {"code": "rate_expression", "message": "rate() expressions aren't cron.", "hint": "Use cron(...) or a cron expression."}})
        );
        let error = explain(&"1 ".repeat(600), None).unwrap_err();
        assert_eq!(
            serde_json::to_value(Failure { error: &error }).unwrap()["error"]["hint"],
            json!(null)
        );
    }

    #[test]
    fn serializes_convert_like_the_documented_example() {
        let c = convert(" 0 0 1,15 * FRI ", None, Dialect::Quartz).unwrap();
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({
                "expression": "0 0 1,15 * FRI",
                "dialect": "unix",
                "to": "quartz",
                "results": ["0 0 0 1,15 * ?", "0 0 0 ? * FRI"],
                "description": "At 00:00, on day 1 and 15 of the month, or on Friday.",
                "warnings": [{"code": "split", "message": "Quartz can't match day of month OR day of week in one expression, so this needs two triggers."},
                    {"code": "dst_rules", "message": "DST rules differ: Quartz skips times in a gap and runs a repeated time once, the second time, while Unix cron runs a time in a gap right after the jump and a repeated time once, the first time, when clocks change by up to 3 hours."}]
            })
        );
        let never = convert("0 0 30 2 *", None, Dialect::Quartz).unwrap_err();
        assert_eq!(never.code, "never_runs");
    }

    #[test]
    fn keeps_the_expression_as_typed() {
        let e = explain("  @daily\n", None).unwrap();
        assert_eq!(e.expression, "@daily");
        assert_eq!(e.description, "At 00:00 every day.");
        assert_eq!(explain("", None).unwrap_err().code, "empty");
        assert_eq!(
            explain("@reboot", None).unwrap().description,
            "Once, when cron starts."
        );
        assert_eq!(explain("0 0 30 2 *", None).unwrap_err().code, "never_runs");
        assert_eq!(
            explain("0 0 1 1 ? 2020", None).unwrap().description,
            "At 00:00, on January 1, in 2020."
        );
    }

    #[test]
    fn answers_json_requests_like_the_commands() {
        let next = answer(
            json!({"op": "next", "expression": "30 2 * * *", "timezone": "America/New_York", "from": "2027-03-13T12:00", "count": 1}),
        );
        assert_eq!(next.unwrap()["runs"][0]["dst"], "moved");
        let quartz = answer(
            json!({"op": "convert", "expression": "0 9 * * 1-5", "to": "quartz", "dialect": "unix"}),
        );
        assert_eq!(quartz.unwrap()["results"], json!(["0 0 9 ? * MON-FRI"]));
        let explained = answer(json!({"op": "explain", "expression": "@daily"})).unwrap();
        assert_eq!(explained["description"], "At 00:00 every day.");
        let failed = |request| answer(request).unwrap_err();
        assert_eq!(
            failed(json!({"op": "explain", "expression": "0 0 30 2 *"})).code,
            "never_runs"
        );
        for (request, why) in [
            (json!({"op": "explain"}), "missing field `expression`"),
            (
                json!({"op": "next", "expression": "@daily", "count": 1001}),
                "count must be a whole number between 1 and 1000",
            ),
            (
                json!({"op": "next", "expression": "@daily", "count": 2.5}),
                "count must be a whole number between 1 and 1000",
            ),
            (
                json!({"op": "next", "expression": "@daily", "count": "3"}),
                "count must be a whole number between 1 and 1000",
            ),
            (
                json!({"op": "convert", "expression": "@daily", "to": "cron"}),
                "unknown variant `cron`, expected one of `unix`, `quartz`, `aws`",
            ),
            (
                json!({"op": "list", "expression": "@daily"}),
                "unknown variant `list`, expected one of `explain`, `next`, `convert`",
            ),
            (json!({"expression": "@daily"}), "missing field `op`"),
        ] {
            let e = failed(request);
            assert_eq!(
                (e.code, e.message),
                ("bad_request", format!("Can't read the request: {why}."))
            );
        }
    }
}
