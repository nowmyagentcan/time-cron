//! The answer to each command, shared by every face.

use jiff::Zoned;
use jiff::civil::DateTime;

use crate::describe::{FieldInfo, describe, fields};
use crate::error::{Error, Warning};
use crate::expr::parse;
use crate::lint::lint;
use crate::runs::{self, Run};
use crate::schedule::Dialect;

/// The bundled IANA tzdata release, e.g. "2026f".
pub const TZDATA: &str = match jiff_tzdb::VERSION {
    Some(v) => v,
    None => "unknown",
};

#[derive(Debug)]
pub struct Explained {
    pub expression: String,
    pub dialect: Dialect,
    pub description: String,
    pub fields: Vec<FieldInfo>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug)]
pub struct Upcoming {
    pub expression: String,
    pub dialect: Dialect,
    pub description: String,
    pub timezone: String,
    pub tzdata: &'static str,
    pub from: Zoned,
    pub runs: Vec<Run>,
    pub skipped: Vec<DateTime>,
    pub warnings: Vec<Warning>,
}

pub fn explain(expression: &str, dialect: Option<Dialect>) -> Result<Explained, Error> {
    let s = parse(expression, dialect)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn keeps_the_expression_as_typed() {
        let e = explain("  @daily\n", None).unwrap();
        assert_eq!(e.expression, "@daily");
        assert_eq!(e.description, "At 00:00 every day.");
        assert_eq!(explain("", None).unwrap_err().code, "empty");
    }
}
