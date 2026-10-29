//! The no-argument mode: each typed line is an expression or a slash command.

use std::io::{BufRead, Write};

use crate::error::Error;
use crate::report::{self, TZDATA};
use crate::runs::find_zone;
use crate::schedule::Dialect;
use crate::text::{self, BOLD, DIM, RESET, SENTENCE};

/// Shows the banner, then answers each line from `input` until `/quit` or the end of input.
/// Times start in `zone`; `unicode` false swaps box and marks for ASCII.
pub fn run(
    mut input: impl BufRead,
    out: &mut impl Write,
    zone: String,
    unicode: bool,
) -> Result<(), Error> {
    let mut send = |text: String| {
        let text = if unicode { text } else { text::ascii(&text) };
        out.write_all(text.as_bytes())
            .and_then(|()| out.flush())
            .map_err(|e| Error::new("io", format!("Can't write output: {e}.")))
    };
    let mut session = Session {
        zone,
        dialect: None,
        last: None,
    };
    send(banner(&session.zone))?;
    let mut line = String::new();
    loop {
        send(format!("{BOLD}›{RESET} "))?;
        line.clear();
        let read = input.read_line(&mut line);
        if read.map_err(|e| Error::new("io", format!("Can't read input: {e}.")))? == 0 {
            return send("\n".into());
        }
        match line.trim() {
            "/quit" => return Ok(()),
            "" => {}
            typed => send(session.answer(typed) + "\n")?,
        }
    }
}

struct Session {
    zone: String,
    dialect: Option<Dialect>,
    /// The last expression typed, for `/to`.
    last: Option<String>,
}

impl Session {
    fn answer(&mut self, typed: &str) -> String {
        self.reply(typed).unwrap_or_else(|e| {
            // Here the dialect is a command, not the --dialect flag.
            let e = match e.code {
                "dialect_field_count" => e.hint("Try /dialect auto."),
                _ => e,
            };
            indent(&text::error(&e))
        })
    }

    fn reply(&mut self, typed: &str) -> Result<String, Error> {
        let (command, arg) = typed
            .split_once(' ')
            .map_or((typed, ""), |(c, a)| (c, a.trim()));
        match command {
            "/tz" => {
                if !arg.is_empty() {
                    let tz = find_zone(arg)?;
                    self.zone = tz.iana_name().unwrap_or(arg).into();
                }
                Ok(format!("  Times in {}\n", self.zone))
            }
            "/dialect" => {
                if !arg.is_empty() {
                    self.dialect = match arg.to_ascii_lowercase().as_str() {
                        "auto" => None,
                        _ => Some(dialect(arg, "auto, unix, quartz or aws")?),
                    };
                }
                Ok(format!("  Dialect: {}\n", self.dialect_name()))
            }
            "/to" => {
                let Some(last) = &self.last else {
                    let to = if arg.is_empty() { "quartz" } else { arg };
                    return Err(usage(
                        "Type an expression first.",
                        format!("Then /to {to} converts it."),
                    ));
                };
                let to = dialect(arg, "unix, quartz or aws")?;
                let c = report::convert(last, self.dialect, to)?;
                let results: String = c.results.iter().map(|r| format!("  {r}\n")).collect();
                Ok(results + &indent(&text::warnings(&c.warnings)))
            }
            "/help" => Ok(format!(
                "  Type a cron expression to see what it means and its next 5 runs.
  /tz ZONE      time zone, e.g. /tz America/New_York (now {})
  /dialect D    auto, unix, quartz or aws (now {})
  /to D         convert the last expression to unix, quartz or aws
  /quit         leave (Ctrl+C works too)
",
                self.zone,
                self.dialect_name()
            )),
            _ if command.starts_with('/') => {
                Err(usage(format!("Unknown command '{command}'."), "Try /help."))
            }
            _ => {
                self.last = Some(typed.into());
                let u = report::next(typed, self.dialect, Some(&self.zone), None, 5)?;
                Ok(format!(
                    "  {BOLD}{}{RESET}\n{}{}",
                    u.description,
                    text::runs(&u),
                    indent(&text::warnings(&u.warnings))
                ))
            }
        }
    }

    fn dialect_name(&self) -> &'static str {
        self.dialect.map_or("auto", Dialect::name)
    }
}

fn dialect(name: &str, choices: &str) -> Result<Dialect, Error> {
    Dialect::from_name(name).ok_or_else(|| {
        usage(
            format!("Unknown dialect '{name}'."),
            format!("Use {choices}."),
        )
    })
}

fn usage(message: impl Into<String>, hint: impl Into<String>) -> Error {
    Error::new("usage", message).hint(hint)
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}\n")).collect()
}

/// The box with name and sentence, the commands and the zone in use.
fn banner(zone: &str) -> String {
    // The sentence is the longer line, so it sets the width.
    let version = format!("{:<1$}", env!("CARGO_PKG_VERSION"), SENTENCE.len() - 10);
    let bar = "─".repeat(SENTENCE.len() + 4);
    format!(
        "{DIM}╭{bar}╮{RESET}
{DIM}│{RESET} {BOLD}✻ time-cron{RESET} {version} {DIM}│{RESET}
{DIM}│{RESET}   {SENTENCE} {DIM}│{RESET}
{DIM}╰{bar}╯{RESET}
  {DIM}Type a cron expression · /tz /dialect /to · /help · /quit{RESET}
  {DIM}Times in {zone} (tzdata {TZDATA}){RESET}

"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(input: &str, unicode: bool) -> String {
        let mut out = vec![];
        run(input.as_bytes(), &mut out, "Europe/Berlin".into(), unicode).unwrap();
        anstream::adapter::strip_str(&String::from_utf8(out).unwrap()).to_string()
    }

    /// The answers after the banner, one string per prompt.
    fn answers(input: &str) -> Vec<String> {
        let out = session(input, true);
        let (_, after) = out.split_once("\n\n› ").unwrap();
        after.split("› ").map(String::from).collect()
    }

    #[test]
    fn opens_with_the_banner() {
        let banner = "╭─────────────────────────────────────────────────────────╮
│ ✻ time-cron 0.1.0                                       │
│   Explain a cron schedule and see exactly when it runs. │
╰─────────────────────────────────────────────────────────╯
  Type a cron expression · /tz /dialect /to · /help · /quit
  Times in Europe/Berlin (tzdata 2026f)

› ";
        assert_eq!(session("/quit\n", true), banner);
        assert!(session("", false).starts_with(
            "+---------------------------------------------------------+
| * time-cron 0.1.0                                       |"
        ));
        // The end of input leaves the shell prompt on a new line.
        assert!(session("", true).ends_with("› \n"));
    }

    #[test]
    fn answers_expressions_and_errors() {
        let got = answers("0 0 12 1 1 ? 2199\n\n0 0 30 2 *\n/quit\nnot read\n");
        assert_eq!(
            got,
            [
                "  At 12:00, on January 1, in 2199.
  Tue 2199-01-01 12:00 +01:00
  ! Only 1 of 5 runs exist before the end of 2199.

",
                "",
                "  ✗ This schedule never runs: day 30 doesn't occur in February.
    Pick a day that exists in every chosen month, or use L in Quartz or AWS.

",
                "",
            ]
        );
    }

    #[test]
    fn changes_zone_and_dialect_and_converts() {
        let got = answers(
            "/to quartz\n/tz america/new_york\n/tz Mars/Olympus\n/dialect aws\n0 9 * * 1-5\n0 12 1 1 ? 2199\n/to quartz\n/to cobol\n/dialect\n/dialect auto\n/frob\n",
        );
        assert_eq!(
            got,
            [
                "  ✗ Type an expression first.\n    Then /to quartz converts it.\n\n",
                "  Times in America/New_York\n\n",
                "  ✗ Unknown time zone 'Mars/Olympus'.\n    Use an IANA name such as America/New_York, Europe/Berlin or UTC.\n\n",
                "  Dialect: aws\n\n",
                "  ✗ AWS cron has 6 fields; got 5.\n    Try /dialect auto.\n\n",
                "  At 12:00, on January 1, in 2199.
  Tue 2199-01-01 12:00 -05:00
  ! EventBridge rules always run in UTC; only EventBridge Scheduler uses America/New_York.
  ! Only 1 of 5 runs exist before the end of 2199.

",
                "  0 0 12 1 1 ? 2199\n  ! DST rules differ: Quartz skips times in a gap and runs a repeated time once, the second time, while EventBridge Scheduler skips times in a gap and runs a repeated time once, the first time.\n\n",
                "  ✗ Unknown dialect 'cobol'.\n    Use unix, quartz or aws.\n\n",
                "  Dialect: aws\n\n",
                "  Dialect: auto\n\n",
                "  ✗ Unknown command '/frob'.\n    Try /help.\n\n",
                "\n",
            ]
        );
    }

    #[test]
    fn lists_the_commands_with_the_current_settings() {
        let got = answers("/help\n/quit\n");
        assert_eq!(
            got[0],
            "  Type a cron expression to see what it means and its next 5 runs.
  /tz ZONE      time zone, e.g. /tz America/New_York (now Europe/Berlin)
  /dialect D    auto, unix, quartz or aws (now auto)
  /to D         convert the last expression to unix, quartz or aws
  /quit         leave (Ctrl+C works too)

"
        );
    }
}
