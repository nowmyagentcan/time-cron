use std::io::Write;
use std::process::ExitCode;
use std::sync::LazyLock;

use clap::{Args, Parser, Subcommand, ValueEnum};
use time_cron::error::Error;
use time_cron::schedule::Dialect;
use time_cron::{expr, report, text};

static VERSION: LazyLock<String> =
    LazyLock::new(|| format!("{} (tzdata {})", env!("CARGO_PKG_VERSION"), report::TZDATA));

const EXAMPLES: &str = "\x1b[1;4mExamples:\x1b[0m
  time-cron \"*/15 9-17 * * 1-5\"
  time-cron next \"0 9 * * 1-5\" --tz Europe/Berlin --count 10
  time-cron explain \"cron(0 18 ? * MON-FRI *)\"";

#[derive(Parser)]
#[command(
    name = "time-cron",
    bin_name = "time-cron",
    about = "Explain a cron schedule and see exactly when it runs.",
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
    /// Show the cause of an error
    #[arg(long, display_order = 10)]
    debug: bool,
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

#[derive(Clone, Copy, ValueEnum)]
enum DialectArg {
    Auto,
    Unix,
    Quartz,
    Aws,
}

fn count(s: &str) -> Result<usize, &'static str> {
    s.parse()
        .ok()
        .filter(|n| (1..=1000).contains(n))
        .ok_or("must be between 1 and 1000")
}

fn main() -> ExitCode {
    if std::env::args_os().len() == 1 {
        let e =
            Error::new("usage", "No expression given.").hint("Try: time-cron next \"0 9 * * 1-5\"");
        eprint!("{}", text::error(&e));
        return ExitCode::from(2);
    }
    let cli = Cli::parse();
    let (common, answer) = match &cli.command {
        Some(Command::Explain(c)) => (
            c,
            report::explain(&c.expr.join(" "), dialect(c)).map(|e| text::explained(&e)),
        ),
        Some(Command::Next(n)) => (&n.common, next(n)),
        None => (&cli.next.common, next(&cli.next)),
    };
    if common.debug {
        debug(common);
    }
    let answer = match answer {
        Ok(text) => text,
        Err(e) => {
            if common.debug {
                eprintln!("debug: {e:?}");
            }
            eprint!("{}", text::error(&e));
            return ExitCode::from(e.exit_code());
        }
    };
    match std::io::stdout().lock().write_all(answer.as_bytes()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprint!(
                "{}",
                text::error(&Error::new("io", format!("Can't write output: {e}.")))
            );
            ExitCode::from(3)
        }
    }
}

fn next(n: &NextArgs) -> Result<String, Error> {
    let expression = n.common.expr.join(" ");
    report::next(
        &expression,
        dialect(&n.common),
        n.tz.as_deref(),
        n.from.as_deref(),
        n.count,
    )
    .map(|u| text::upcoming(&u))
}

fn dialect(c: &Common) -> Option<Dialect> {
    match c.dialect {
        DialectArg::Auto => None,
        DialectArg::Unix => Some(Dialect::Unix),
        DialectArg::Quartz => Some(Dialect::Quartz),
        DialectArg::Aws => Some(Dialect::Aws),
    }
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
