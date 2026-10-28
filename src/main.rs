use std::io::{self, ErrorKind, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::LazyLock;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use time_cron::error::Error;
use time_cron::report::DialectArg;
use time_cron::runs::system_zone;
use time_cron::schedule::Dialect;
use time_cron::{expr, interactive, mcp, report, text};

static VERSION: LazyLock<String> =
    LazyLock::new(|| format!("{} (tzdata {})", env!("CARGO_PKG_VERSION"), report::TZDATA));

const EXAMPLES: &str = "\x1b[1;4mExamples:\x1b[0m
  time-cron \"*/15 9-17 * * 1-5\"
  time-cron next \"0 9 * * 1-5\" --tz Europe/Berlin --count 10
  time-cron next \"0 0 12 ? * MON#2\" --json
  time-cron explain \"cron(0 18 ? * MON-FRI *)\"
  time-cron convert \"*/15 9-17 * * 1-5\" --to quartz";

#[derive(Parser)]
#[command(
    name = "time-cron",
    bin_name = "time-cron",
    about = text::SENTENCE,
    version = VERSION.as_str(),
    override_usage = "time-cron [COMMAND] [EXPR]...",
    after_help = EXAMPLES,
    args_conflicts_with_subcommands = true,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    next: NextArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Say what each field means and flag likely mistakes
    #[command(after_help = EXAMPLES)]
    Explain(Common),
    /// List the next run times (the default command)
    #[command(after_help = EXAMPLES)]
    Next(NextArgs),
    /// Rewrite the expression for unix, quartz or aws
    #[command(after_help = EXAMPLES)]
    Convert(ConvertArgs),
    /// Run the MCP server on stdio
    Mcp,
}

#[derive(Args)]
struct ConvertArgs {
    #[command(flatten)]
    common: Common,
    /// Target dialect: unix, quartz or aws
    #[arg(long, value_name = "D", value_parser = target)]
    to: Dialect,
}

#[derive(Args)]
struct Common {
    /// The cron expression, quoted (separate words are joined with spaces)
    expr: Vec<String>,
    /// Auto, unix, quartz or aws
    #[arg(
        long,
        value_name = "D",
        default_value = "auto",
        hide_possible_values = true
    )]
    dialect: DialectArg,
    /// Print one JSON object
    #[arg(long, display_order = 10)]
    json: bool,
    /// Show the cause of an error
    #[arg(long, display_order = 10)]
    debug: bool,
}

impl Common {
    /// The words joined into one expression, unless the shell has swapped a `*` for file names.
    fn expression(&self) -> Result<String, Error> {
        if self.expr.len() > 1 && self.expr.iter().any(|w| Path::new(w).exists()) {
            return Err(
                Error::new("glob_expanded", "Your shell replaced * with file names.")
                    .hint("Quote the expression: time-cron next \"0 9 * * 1-5\""),
            );
        }
        Ok(self.expr.join(" "))
    }
}

#[derive(Args)]
struct NextArgs {
    #[command(flatten)]
    common: Common,
    /// IANA time zone [default: this computer's; UTC for aws]
    #[arg(long, value_name = "ZONE")]
    tz: Option<String>,
    /// Start after this time [default: now]
    #[arg(long, value_name = "TIME")]
    from: Option<String>,
    /// Number of runs, 1-1000
    #[arg(long, value_name = "N", default_value = "5", value_parser = count)]
    count: usize,
}

fn count(s: &str) -> Result<usize, &'static str> {
    s.parse()
        .ok()
        .filter(|n| (1..=1000).contains(n))
        .ok_or("must be between 1 and 1000")
}

fn target(s: &str) -> Result<Dialect, &'static str> {
    Dialect::from_name(s).ok_or("must be unix, quartz or aws")
}

fn main() -> ExitCode {
    if std::env::args_os().len() == 1 {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            let (stdin, mut stdout) = (io::stdin().lock(), anstream::stdout());
            return served(interactive::run(
                stdin,
                &mut stdout,
                system_zone(),
                text::unicode(),
            ));
        }
        let e =
            Error::new("usage", "No expression given.").hint("Try: time-cron next \"0 9 * * 1-5\"");
        fail(&e);
        return ExitCode::from(2);
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // Agents asking for JSON get clap's complaint as the JSON error object.
        Err(e) if e.use_stderr() && std::env::args_os().any(|a| a == "--json") => {
            let text = e.render().to_string();
            let first = text.split("\n\n").next().unwrap_or_default();
            let message = first.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut error = Error::new("usage", message.trim_start_matches("error: "));
            error.hint = text.lines().rfind(|l| !l.is_empty()).map(String::from);
            return emit(&json(&report::Failure { error: &error }), "", 2);
        }
        Err(e) => e.exit(),
    };
    let (common, answer) = match &cli.command {
        Some(Command::Mcp) => return served(mcp::serve()),
        Some(Command::Explain(c)) => (
            c,
            c.expression()
                .and_then(|x| report::explain(&x, dialect(c)))
                .map(|e| show(c, &e, text::explained)),
        ),
        Some(Command::Next(n)) => (&n.common, next(n)),
        Some(Command::Convert(c)) => (&c.common, convert(c)),
        None => (&cli.next.common, next(&cli.next)),
    };
    if common.debug {
        debug(common);
    }
    let (out, notes, code) = match answer {
        Ok((out, notes)) => (out, notes, 0),
        Err(error) => {
            if common.debug {
                eprintln!("debug: {error:?}");
            }
            if common.json {
                let out = json(&report::Failure { error: &error });
                (out, String::new(), error.exit_code())
            } else {
                fail(&error);
                (String::new(), String::new(), error.exit_code())
            }
        }
    };
    emit(&out, &notes, code)
}

/// The exit of a face that runs until its input ends.
fn served(result: Result<(), Error>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            fail(&e);
            ExitCode::from(e.exit_code())
        }
    }
}

/// Writes the answer to stdout and its notes to stderr; a closed stdout ends quietly.
fn emit(out: &str, notes: &str, code: u8) -> ExitCode {
    match anstream::stdout().lock().write_all(out.as_bytes()) {
        Ok(()) => {
            anstream::eprint!("{notes}");
            ExitCode::from(code)
        }
        Err(e) if e.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            fail(&Error::new("io", format!("Can't write output: {e}.")));
            ExitCode::from(3)
        }
    }
}

fn next(n: &NextArgs) -> Result<(String, String), Error> {
    let expression = n.common.expression()?;
    report::next(
        &expression,
        dialect(&n.common),
        n.tz.as_deref(),
        n.from.as_deref(),
        n.count,
    )
    .map(|u| show(&n.common, &u, text::upcoming))
}

/// The results on stdout; the description and warnings on stderr, unless `--json`.
fn convert(c: &ConvertArgs) -> Result<(String, String), Error> {
    let expression = c.common.expression()?;
    let answer = report::convert(&expression, dialect(&c.common), c.to)?;
    Ok(if c.common.json {
        (json(&answer), String::new())
    } else {
        (
            people(text::converted(&answer)),
            people(text::notes(&answer)),
        )
    })
}

/// The answer as `--json` or as text, for stdout, and nothing for stderr.
fn show<T: Serialize>(c: &Common, answer: &T, text: fn(&T) -> String) -> (String, String) {
    let out = if c.json {
        json(answer)
    } else {
        people(text(answer))
    };
    (out, String::new())
}

fn json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("answers are plain data") + "\n"
}

/// Text for a person, in ASCII when the console can't show Unicode.
fn people(text: String) -> String {
    if text::unicode() {
        text
    } else {
        text::ascii(&text)
    }
}

fn fail(e: &Error) {
    anstream::eprint!("{}", people(text::error(e)));
}

fn dialect(c: &Common) -> Option<Dialect> {
    c.dialect.dialect()
}

/// The parsed field sets, for `--debug`.
fn debug(c: &Common) {
    let Ok(s) = expr::parse(&c.expr.join(" "), dialect(c)) else {
        return;
    };
    for f in &s.fields {
        eprintln!("debug: {} {:?}", f.kind.key(), s.values(f.kind));
    }
    eprintln!(
        "debug: day of month specials {:?}, day of week {:?}, day rule {:?}, fixed time {}",
        s.dom.specials, s.dow, s.day_rule, s.fixed_time
    );
}

#[cfg(test)]
mod tests {
    use jiff::{civil::date, tz::Offset};

    #[test]
    fn bundles_tzdata_2026f() {
        assert_eq!(jiff_tzdb::VERSION, Some("2026f"));
        // 2026f keeps Cuba on -04 after 2026-11-01; 2026e moved it to -05.
        let havana = date(2026, 11, 2)
            .at(0, 0, 0, 0)
            .in_tz("America/Havana")
            .unwrap();
        assert_eq!(havana.offset(), Offset::constant(-4));
    }

    #[test]
    fn checks_the_command_line() {
        use clap::CommandFactory;
        super::Cli::command().debug_assert();
    }
}
