use crate::error::Error;
use crate::schedule::Dialect::{self, Aws, Quartz, Unix};

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

/// Checks the raw text, expands a nickname, strips `cron(...)` and works out the dialect
/// (`forced` when the user named one). Returns the dialect and the field texts; `@reboot`
/// is the only input that yields no fields.
pub fn read(text: &str, forced: Option<Dialect>) -> Result<(Dialect, Vec<&str>), Error> {
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
