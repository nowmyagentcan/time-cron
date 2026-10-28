//! The answer to each command, shared by every face.

use jiff::Zoned;
use jiff::civil::DateTime;
use serde::{Serialize, Serializer};

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

#[derive(Debug, Serialize)]
pub struct Explained {
    pub expression: String,
    pub dialect: Dialect,
    pub description: String,
    pub fields: Vec<FieldInfo>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Serialize)]
pub struct Upcoming {
    pub expression: String,
    pub dialect: Dialect,
    pub description: String,
    pub timezone: String,
    pub tzdata: &'static str,
    #[serde(serialize_with = "rfc3339")]
    pub from: Zoned,
    pub runs: Vec<Run>,
    #[serde(serialize_with = "gap_skips")]
    pub skipped: Vec<DateTime>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Serialize)]
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
pub struct Failure<'a> {
    pub error: &'a Error,
}

impl Serialize for Run {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Json {
            time: String,
            utc: String,
            dst: Option<Dst>,
            #[serde(skip_serializing_if = "Option::is_none")]
            scheduled: Option<String>,
        }
        Json {
            time: self.time.strftime(RFC3339).to_string(),
            utc: self.time.timestamp().to_string(),
            dst: self.dst,
            scheduled: self.scheduled.map(|at| at.to_string()),
        }
        .serialize(s)
    }
}

const RFC3339: &str = "%Y-%m-%dT%H:%M:%S%:z";

fn rfc3339<S: Serializer>(z: &Zoned, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&z.strftime(RFC3339))
}

fn gap_skips<S: Serializer>(skipped: &[DateTime], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(
        skipped
            .iter()
            .map(|at| serde_json::json!({"scheduled": at.to_string(), "reason": "dst_gap"})),
    )
}

/// The parsed schedule, refused if it never runs: no scheduler would run it either.
fn runnable(expression: &str, dialect: Option<Dialect>) -> Result<Schedule, Error> {
    let s = parse(expression, dialect)?;
    if !s.reboot {
        runs::next(&s, Some("UTC"), None, 1)?;
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
    let c = convert::convert(&s, to)?;
    Ok(Converted {
        expression: expression.trim().into(),
        dialect: s.dialect,
        to,
        results: c.results,
        description: describe(&s),
        warnings: c.warnings,
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
                "warnings": [{"code": "split", "message": "Quartz can't match day of month OR day of week in one expression, so this needs two triggers."}]
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
            explain("0 0 1 1 ? 2020", None).unwrap_err().code,
            "never_runs"
        );
    }
}
