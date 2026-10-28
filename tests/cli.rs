//! Runs the built binary the way people and agents do.

use std::io::Read;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const UTC: &str = "UTC";
const NY: &str = "America/New_York";
const START: &str = "2026-10-29T00:00:00Z";
const SPRING: &str = "2027-03-13T12:00";
const AUTUMN: &str = "2026-10-31T23:00";

fn time_cron(args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_time-cron"));
    cmd.args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .stdin(Stdio::null());
    cmd
}

/// Exit code, stdout and stderr, with marks folded to ASCII: a Windows console that isn't
/// set to UTF-8 gets the ASCII forms.
fn run(args: &[&str]) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = time_cron(args).output().unwrap();
    let text = |b: Vec<u8>| {
        String::from_utf8(b)
            .unwrap()
            .replace('✗', "x")
            .replace('·', "-")
            .replace('…', "...")
    };
    (status.code().unwrap(), text(stdout), text(stderr))
}

/// The `--json` answer, checked against the documented shape.
fn json(args: &[&str]) -> (i32, Value) {
    let args = [args, &["--json"]].concat();
    let out = time_cron(&args).output().unwrap();
    assert!(out.stderr.is_empty(), "{args:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.matches('\n').count(), 1, "one line: {stdout}");
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_shape(&value);
    (out.status.code().unwrap(), value)
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

fn assert_shape(v: &Value) {
    let strings = |v: &Value, names: &[&str]| names.iter().all(|n| v[n].is_string());
    let warnings = |v: &Value| {
        v["warnings"].as_array().unwrap().iter().all(|w| {
            let mut k = keys(w);
            k.sort();
            k == ["code", "message"] && strings(w, &["code", "message"])
        })
    };
    let mut k = keys(v);
    k.sort();
    match &k[..] {
        ["error"] => {
            let e = &v["error"];
            let mut k = keys(e);
            k.sort();
            assert_eq!(k, ["code", "hint", "message"]);
            assert!(strings(e, &["code", "message"]));
            assert!(e["hint"].is_string() || e["hint"].is_null());
        }
        ["description", "dialect", "expression", "fields", "warnings"] => {
            assert!(strings(v, &["description", "dialect", "expression"]));
            assert!(warnings(v));
            for f in v["fields"].as_array().unwrap() {
                let mut k = keys(f);
                k.sort();
                assert_eq!(k, ["meaning", "name", "text", "values"]);
                assert!(strings(f, &["meaning", "name", "text"]));
                let values = &f["values"];
                let day = f["name"] == "day_of_week";
                assert!(
                    values.is_null()
                        || values.as_array().unwrap().iter().all(|x| if day {
                            x.is_string()
                        } else {
                            x.is_u64()
                        })
                );
            }
        }
        [
            "description",
            "dialect",
            "expression",
            "from",
            "runs",
            "skipped",
            "timezone",
            "tzdata",
            "warnings",
        ] => {
            assert!(strings(
                v,
                &[
                    "description",
                    "dialect",
                    "expression",
                    "from",
                    "timezone",
                    "tzdata"
                ]
            ));
            assert!(warnings(v));
            for r in v["runs"].as_array().unwrap() {
                assert!(strings(r, &["time", "utc"]) && r["utc"].as_str().unwrap().ends_with('Z'));
                match r["dst"].as_str() {
                    Some("moved") => assert!(r["scheduled"].is_string()),
                    Some("first" | "second") | None => assert!(r.get("scheduled").is_none()),
                    Some(other) => panic!("dst {other}"),
                }
            }
            for s in v["skipped"].as_array().unwrap() {
                assert_eq!(s["reason"], "dst_gap");
                assert!(s["scheduled"].is_string());
            }
        }
        [
            "description",
            "dialect",
            "expression",
            "results",
            "to",
            "warnings",
        ] => {
            assert!(strings(v, &["description", "dialect", "expression", "to"]));
            assert!(warnings(v));
            let results = v["results"].as_array().unwrap();
            assert!((1..=2).contains(&results.len()) && results.iter().all(Value::is_string));
        }
        other => panic!("unexpected keys {other:?}"),
    }
}

fn codes(v: &Value) -> Vec<&str> {
    let warnings = v["warnings"].as_array().unwrap();
    warnings
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect()
}

#[test]
fn prints_the_version_with_the_tzdata_release() {
    for flag in ["--version", "-V"] {
        assert_eq!(
            run(&[flag]),
            (0, "time-cron 0.1.0 (tzdata 2026f)\n".into(), "".into())
        );
    }
}

#[test]
fn shows_examples_in_help() {
    for args in [
        &["--help"][..],
        &["-h"],
        &["next", "--help"],
        &["explain", "--help"],
        &["convert", "--help"],
    ] {
        let (code, out, _) = run(args);
        assert_eq!(code, 0);
        assert!(
            out.contains("Examples:\n  time-cron \"*/15 9-17 * * 1-5\"\n"),
            "{out}"
        );
        assert!(out.contains("  time-cron next \"0 0 12 ? * MON#2\" --json\n"));
        assert!(out.contains("  time-cron convert \"*/15 9-17 * * 1-5\" --to quartz"));
        assert!(!out.contains('\x1b'));
    }
    let (_, out, _) = run(&["--help"]);
    assert!(out.starts_with("Explain a cron schedule and see exactly when it runs.\n\nUsage: time-cron [COMMAND] [EXPR]...\n"));
    assert!(out.contains("--count <N>    Number of runs, 1-1000 [default: 5]"));
}

#[test]
fn detects_the_dialect() {
    for (expr, dialect) in [
        ("0 9 * * 1-5", "unix"),
        ("0 0 12 ? * MON#2", "quartz"),
        ("0 18 ? * MON-FRI *", "aws"),
        ("cron(0 18 ? * MON-FRI *)", "aws"),
        ("0 0 12 ? * MON 2030", "quartz"),
    ] {
        let (code, v) = json(&["explain", expr]);
        assert_eq!((code, v["dialect"].as_str()), (0, Some(dialect)), "{expr}");
        assert_eq!(v["expression"], expr);
    }
    let (code, v) = json(&["explain", "0 0 12 * * *"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &json!("ambiguous_dialect"))
    );
    let (code, v) = json(&["explain", "cron(0 18 ? * MON-FRI *)", "--dialect", "aws"]);
    assert_eq!((code, &v["dialect"]), (0, &json!("aws")));
}

#[test]
#[rustfmt::skip]
fn lists_next_runs_with_each_schedulers_dst_rules() {
    type Case<'a> = (&'a str, &'a str, &'a str, &'a str, &'a [&'a str], &'a [&'a str], &'a [&'a str]);
    // expression, zone ("" for the default), from, count, runs as "time [dst] [scheduled]",
    // skipped wall times, warning codes.
    let cases: &[Case] = &[
        ("0 9 1 * MON", UTC, START, "4", &["2026-11-01T09:00:00+00:00", "2026-11-02T09:00:00+00:00", "2026-11-09T09:00:00+00:00", "2026-11-16T09:00:00+00:00"], &[], &["dom_dow_or"]),
        ("0 9 */2 * MON", UTC, START, "4", &["2026-11-09T09:00:00+00:00", "2026-11-23T09:00:00+00:00", "2026-12-07T09:00:00+00:00", "2026-12-21T09:00:00+00:00"], &[], &["dom_dow_and"]),
        ("0 0 1,15 * FRI", UTC, START, "4", &["2026-10-30T00:00:00+00:00", "2026-11-01T00:00:00+00:00", "2026-11-06T00:00:00+00:00", "2026-11-13T00:00:00+00:00"], &[], &["dom_dow_or"]),
        ("0 0 * * 7", UTC, START, "1", &["2026-11-01T00:00:00+00:00"], &[], &[]),
        ("0 0 29 2 *", UTC, START, "1", &["2028-02-29T00:00:00+00:00"], &[], &["short_months"]),
        ("@daily", UTC, START, "1", &["2026-10-30T00:00:00+00:00"], &[], &[]),
        ("0,30 2 * * *", NY, SPRING, "3", &["2027-03-14T03:00:00-04:00 moved 2027-03-14T02:00:00", "2027-03-14T03:00:00-04:00 moved 2027-03-14T02:30:00", "2027-03-15T02:00:00-04:00"], &[], &["dst_gap", "dst_gap"]),
        ("*/15 2 * * *", NY, SPRING, "1", &["2027-03-15T02:00:00-04:00"], &["2027-03-14T02:00:00", "2027-03-14T02:15:00", "2027-03-14T02:30:00", "2027-03-14T02:45:00"], &["dst_gap", "dst_gap"]),
        ("30 1 * * *", NY, AUTUMN, "2", &["2026-11-01T01:30:00-04:00 first", "2026-11-02T01:30:00-05:00"], &[], &["dst_fold"]),
        ("*/30 1 * * *", NY, AUTUMN, "5", &["2026-11-01T01:00:00-04:00 first", "2026-11-01T01:30:00-04:00 first", "2026-11-01T01:00:00-05:00 second", "2026-11-01T01:30:00-05:00 second", "2026-11-02T01:00:00-05:00"], &[], &["dst_fold"]),
        ("0 * * * *", NY, AUTUMN, "4", &["2026-11-01T00:00:00-04:00", "2026-11-01T01:00:00-04:00 first", "2026-11-01T01:00:00-05:00 second", "2026-11-01T02:00:00-05:00"], &[], &["dst_fold", "dst_gap"]),
        ("0 30 1 * * ?", NY, AUTUMN, "2", &["2026-11-01T01:30:00-05:00 second", "2026-11-02T01:30:00-05:00"], &[], &["dst_fold"]),
        ("0 30 2 * * ?", NY, SPRING, "1", &["2027-03-15T02:30:00-04:00"], &["2027-03-14T02:30:00"], &["dst_gap", "dst_gap"]),
        ("cron(30 2 * * ? *)", NY, SPRING, "1", &["2027-03-15T02:30:00-04:00"], &["2027-03-14T02:30:00"], &["aws_rules_utc", "dst_gap", "dst_gap"]),
        ("cron(30 1 * * ? *)", NY, AUTUMN, "2", &["2026-11-01T01:30:00-04:00 first", "2026-11-02T01:30:00-05:00"], &[], &["aws_rules_utc", "dst_fold"]),
        ("0 0 * * *", "America/Havana", "2026-10-31T12:00", "2", &["2026-11-01T00:00:00-04:00", "2026-11-02T00:00:00-04:00"], &[], &[]),
        ("0 15 10 ? * 6L", UTC, START, "3", &["2026-10-30T10:15:00+00:00", "2026-11-27T10:15:00+00:00", "2026-12-25T10:15:00+00:00"], &[], &["dow_one_is_sunday"]),
        ("0 0 12 ? * MON#2", UTC, START, "3", &["2026-11-09T12:00:00+00:00", "2026-12-14T12:00:00+00:00", "2027-01-11T12:00:00+00:00"], &[], &[]),
        ("0 0 9 15W * ?", UTC, START, "3", &["2026-11-16T09:00:00+00:00", "2026-12-15T09:00:00+00:00", "2027-01-15T09:00:00+00:00"], &[], &[]),
        ("0 0 9 LW * ?", UTC, START, "3", &["2026-10-30T09:00:00+00:00", "2026-11-30T09:00:00+00:00", "2026-12-31T09:00:00+00:00"], &[], &[]),
        ("0 0 12 L-3 * ?", UTC, START, "3", &["2026-11-27T12:00:00+00:00", "2026-12-28T12:00:00+00:00", "2027-01-28T12:00:00+00:00"], &[], &[]),
        ("0 0 12 ? * L", UTC, START, "2", &["2026-10-31T12:00:00+00:00", "2026-11-07T12:00:00+00:00"], &[], &[]),
        ("0 0 12 ? * 1-5", UTC, START, "3", &["2026-10-29T12:00:00+00:00", "2026-11-01T12:00:00+00:00", "2026-11-02T12:00:00+00:00"], &[], &["dow_one_is_sunday"]),
        ("cron(0 18 ? * MON-FRI *)", "", START, "3", &["2026-10-29T18:00:00+00:00", "2026-10-30T18:00:00+00:00", "2026-11-02T18:00:00+00:00"], &[], &[]),
    ];
    for &(expr, tz, from, count, runs, skipped, warnings) in cases {
        let mut args = vec!["next", expr, "--from", from, "--count", count];
        if !tz.is_empty() {
            args.extend(["--tz", tz]);
        }
        let (code, v) = json(&args);
        assert_eq!(code, 0, "{expr}");
        let got: Vec<String> = v["runs"].as_array().unwrap().iter().map(|r| {
            [&r["time"], &r["dst"], &r["scheduled"]].iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")
        }).collect();
        assert_eq!(got, runs, "{expr}");
        let got: Vec<&str> = v["skipped"].as_array().unwrap().iter().map(|s| s["scheduled"].as_str().unwrap()).collect();
        assert_eq!(got, skipped, "{expr}");
        assert_eq!(codes(&v), warnings, "{expr}");
        assert_eq!(v["timezone"], if tz.is_empty() { UTC } else { tz }, "{expr}");
    }
    let (_, v) = json(&["next", "0 0 12 ? * 1-5", "--tz", UTC, "--from", START, "--count", "1"]);
    assert!(v["warnings"][0]["message"].as_str().unwrap().contains(" so 1-5 means Sunday through Thursday."));
}

#[test]
fn matches_the_documented_next_example() {
    let args = [
        "next",
        "30 2 * * *",
        "--tz",
        NY,
        "--from",
        SPRING,
        "--count",
        "3",
    ];
    let (code, v) = json(&args);
    let gap = "clocks jump from 02:00 to 03:00 in America/New_York. Cron runs 02:30 right after the jump.";
    let want = json!({
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
    });
    assert_eq!((code, v), (0, want));
    // The bare form is `next`, and the expression may come as separate words.
    let (code, v) = json(&[
        "30", "2", "*", "*", "*", "--tz", NY, "--from", SPRING, "--count", "3",
    ]);
    assert_eq!((code, &v["runs"][0]["dst"]), (0, &json!("moved")));
}

#[test]
fn prints_next_runs_as_text() {
    let args = [
        "next",
        "30 2 * * *",
        "--tz",
        NY,
        "--from",
        SPRING,
        "--count",
        "3",
    ];
    let gap = "clocks jump from 02:00 to 03:00 in America/New_York. Cron runs 02:30 right after the jump.";
    let want = format!(
        "At 02:30 every day.
America/New_York - tzdata 2026f - Unix cron rules
  Sun 2027-03-14 03:00 -04:00  moved from 02:30
  Mon 2027-03-15 02:30 -04:00
  Tue 2027-03-16 02:30 -04:00
! 2027-03-14: {gap}
! 2028-03-12: {gap}
"
    );
    assert_eq!(run(&args), (0, want, "".into()));
    let (code, out, _) = run(&["0 9 */2 * MON", "--tz", UTC, "--count", "1"]);
    assert_eq!(code, 0);
    assert!(out.starts_with(
        "At 09:00, on every 2nd day of the month (1, 3, 5, ...), only when that day is a Monday.\n"
    ));
}

#[test]
fn explains_each_field_as_text() {
    let want = "Every 15 minutes from 09:00 through 17:45, Monday through Friday.
Dialect: unix (5 fields)
  minute        */15  every 15 minutes
  hour          9-17  9 through 17
  day of month  *     any day
  month         *     any month
  day of week   1-5   Monday through Friday
";
    assert_eq!(
        run(&["explain", "*/15 9-17 * * 1-5"]),
        (0, want.into(), "".into())
    );
}

#[test]
fn matches_every_description_golden() {
    for (expr, want) in [
        ("0 9 * * 1-5", "At 09:00, Monday through Friday."),
        (
            "*/15 9-17 * * 1-5",
            "Every 15 minutes from 09:00 through 17:45, Monday through Friday.",
        ),
        ("30 2 * * *", "At 02:30 every day."),
        ("@daily", "At 00:00 every day."),
        ("@hourly", "At minute 0 of every hour."),
        ("0 * * * *", "At minute 0 of every hour."),
        ("* * * * *", "Every minute."),
        ("* 9 * * *", "Every minute from 09:00 through 09:59."),
        ("0 9,17 * * *", "At 09:00 and 17:00 every day."),
        ("5 4 * * sun", "At 04:05, on Sunday."),
        (
            "0 9 * * 1,3,5",
            "At 09:00, on Monday, Wednesday and Friday.",
        ),
        ("0 0 1 1 *", "At 00:00, on January 1."),
        (
            "0 0,12 1 */2 *",
            "At 00:00 and 12:00, on day 1 of the month, in January, March, May, July, September and November.",
        ),
        (
            "0 0 1,15 * FRI",
            "At 00:00, on day 1 and 15 of the month, or on Friday.",
        ),
        (
            "0 9 */2 * MON",
            "At 09:00, on every 2nd day of the month (1, 3, 5, …), only when that day is a Monday.",
        ),
        ("@reboot", "Once, when cron starts."),
        ("0 15 10 L * ?", "At 10:15, on the last day of the month."),
        (
            "0 0 12 ? * MON#2",
            "At 12:00, on the second Monday of the month.",
        ),
        (
            "0 15 10 ? * 6L",
            "At 10:15, on the last Friday of the month.",
        ),
        (
            "0 0 9 15W * ?",
            "At 09:00, on the weekday nearest day 15 of the month.",
        ),
        (
            "0 0 9 LW * ?",
            "At 09:00, on the last weekday of the month.",
        ),
        (
            "0 0 12 L-3 * ?",
            "At 12:00, 3 days before the last day of the month.",
        ),
        ("0 0 12 ? * L", "At 12:00, on Saturday."),
        (
            "0 0/5 14 * * ?",
            "Every 5 minutes from 14:00 through 14:55.",
        ),
        ("*/30 * * * * ?", "Every 30 seconds."),
        ("15 30 9 * * ?", "At 09:30:15 every day."),
        ("0 0 12 ? * 1-5", "At 12:00, Sunday through Thursday."),
        ("0 0 12 1 1 ? 2030", "At 12:00, on January 1, in 2030."),
        (
            "cron(0 18 ? * MON-FRI *)",
            "At 18:00, Monday through Friday.",
        ),
        ("cron(0/15 * * * ? *)", "Every 15 minutes."),
        ("cron(0 8 1 * ? *)", "At 08:00, on day 1 of the month."),
    ] {
        let (code, v) = json(&["explain", expr]);
        assert_eq!((code, v["description"].as_str()), (0, Some(want)), "{expr}");
    }
}

#[test]
#[rustfmt::skip]
fn matches_every_conversion_golden() {
    // Each result has passed the binary's own check: its first 100 UTC runs from
    // 1970 equal the source's.
    let cases: &[(&str, &str, &[&str], &[&str])] = &[
        ("0 9 * * 1-5", "quartz", &["0 0 9 ? * MON-FRI"], &[]),
        ("0 9 * * 1-5", "aws", &["cron(0 9 ? * MON-FRI *)"], &[]),
        ("*/15 9-17 * * 1-5", "quartz", &["0 0/15 9-17 ? * MON-FRI"], &[]),
        ("*/15 9-17 * * 1-5", "aws", &["cron(0/15 9-17 ? * MON-FRI *)"], &[]),
        ("0 0 * * 0", "quartz", &["0 0 0 ? * SUN"], &[]),
        ("@daily", "quartz", &["0 0 0 * * ?"], &[]),
        ("@daily", "unix", &["0 0 * * *"], &[]),
        ("0 0 1,15 * FRI", "quartz", &["0 0 0 1,15 * ?", "0 0 0 ? * FRI"], &["split"]),
        ("0 0 1,15 * FRI", "aws", &["cron(0 0 1,15 * ? *)", "cron(0 0 ? * FRI *)"], &["split"]),
        ("0 30 9 ? * 2-6", "unix", &["30 9 * * 1-5"], &[]),
        ("cron(0 18 ? * MON-FRI *)", "unix", &["0 18 * * 1-5"], &[]),
        ("cron(0 18 ? * MON-FRI *)", "quartz", &["0 0 18 ? * MON-FRI"], &[]),
        ("0 0 12 1 1 ? 2030", "aws", &["cron(0 12 1 1 ? 2030)"], &[]),
        ("0 0 12 ? * MON#2", "aws", &["cron(0 12 ? * 2#2 *)"], &[]),
        ("0 0 9 LW * ?", "aws", &["cron(0 9 LW * ? *)"], &["aws_undocumented"]),
    ];
    for &(expr, to, results, warnings) in cases {
        let (code, v) = json(&["convert", expr, "--to", to]);
        assert_eq!((code, &v["results"], codes(&v)), (0, &json!(results), warnings.to_vec()), "{expr} to {to}");
        assert_eq!((&v["expression"], &v["to"]), (&json!(expr), &json!(to)));
    }
    let (_, v) = json(&["convert", "0 0 1,15 * FRI", "--to", "aws"]);
    assert_eq!(v["warnings"][0]["message"], "AWS cron can't match day of month OR day of week in one expression, so this needs two rules or schedules.");
}

#[test]
fn prints_only_the_converted_expressions_on_stdout() {
    let (code, out, err) = run(&["convert", "0 0 1,15 * FRI", "--to", "quartz"]);
    assert_eq!((code, out.as_str()), (0, "0 0 0 1,15 * ?\n0 0 0 ? * FRI\n"));
    assert_eq!(
        err,
        "At 00:00, on day 1 and 15 of the month, or on Friday.
! Quartz can't match day of month OR day of week in one expression, so this needs two triggers.
"
    );
    // The source dialect can be named, and the words may come unquoted.
    let (code, out, _) = run(&[
        "convert",
        "0",
        "0",
        "18",
        "?",
        "*",
        "MON-FRI",
        "--dialect",
        "quartz",
        "--to",
        "aws",
    ]);
    assert_eq!((code, out.as_str()), (0, "cron(0 18 ? * MON-FRI *)\n"));
}

#[test]
#[rustfmt::skip]
fn reports_every_error_with_its_message_and_hint() {
    const TZ: &str = "Example: time-cron next \"0 9 * * *\" --tz Europe/Berlin";
    const FIXED: &str = "Replace it with a fixed value.";
    const QUESTION: &str = "Example: 0 0 9 ? * MON-FRI";
    const UNIX_ONLY: &str = "Pass --dialect quartz or aws, or list the days.";
    let long = format!("0 9 * * {}1", "1,".repeat(600));
    type Case<'a> = (&'a [&'a str], &'a str, &'a str, Option<&'a str>);
    let cases: &[Case] = &[
        (&["next", " "], "empty", "No cron expression given.", Some("Example: time-cron next \"0 9 * * 1-5\"")),
        (&["explain"], "empty", "No cron expression given.", Some("Example: time-cron next \"0 9 * * 1-5\"")),
        (&["next", &long], "too_long", "The expression is longer than 1024 characters.", None),
        (&["next", "0 9 * *\n1-5"], "multiline", "The expression must be on one line.", None),
        (&["next", "0", "9", "Cargo.toml", "Cargo.toml", "1-5"], "glob_expanded", "Your shell replaced * with file names.", Some("Quote the expression: time-cron next \"0 9 * * 1-5\"")),
        (&["next", "0 9 * *"], "field_count", "Expected 5 fields (Unix), 6 or 7 (Quartz) or cron(...) with 6 (AWS); got 4.", Some("Separate fields with spaces and quote the expression.")),
        (&["next", "0 9 * * 1-5", "--dialect", "quartz"], "dialect_field_count", "Quartz has 6 or 7 fields; got 5.", Some("Leave out --dialect to detect it.")),
        (&["explain", "0 0 12 * * *"], "ambiguous_dialect", "6 fields without '?' could be Quartz (seconds first) or AWS (year last).", Some("Put ? in day of month or day of week, or pass --dialect quartz or --dialect aws.")),
        (&["next", "rate(5 minutes)"], "rate_expression", "rate() expressions aren't cron.", Some("Use cron(...) or a cron expression.")),
        (&["next", "CRON_TZ=UTC 0 9 * * *"], "cron_tz_prefix", "Put the time zone in --tz instead of CRON_TZ=.", Some(TZ)),
        (&["explain", "@fortnightly"], "unknown_nickname", "Unknown nickname '@fortnightly'.", Some("Use @yearly, @annually, @monthly, @weekly, @daily, @midnight, @hourly or @reboot.")),
        (&["explain", "@daily", "--dialect", "aws"], "nickname_dialect", "Nicknames like @daily are Unix cron only.", Some("Drop --dialect, or write the fields out.")),
        (&["next", "0 0 12 ? * 0"], "out_of_range", "Day of week value 0 is out of range 1-7.", Some("Day 1 is Sunday and 7 is Saturday here.")),
        (&["next", "60 * * * *"], "out_of_range", "Minute value 60 is out of range 0-59.", None),
        (&["next", "0 0 1 FOO *"], "unknown_name", "Unknown month name 'FOO'.", Some("Use three-letter names: JAN-DEC, SUN-SAT.")),
        (&["next", "0 9-x * * *"], "bad_syntax", "Can't read '9-x' in the hour field.", Some("Use numbers, *, ranges (1-5), lists (1,3) and steps (*/15).")),
        (&["next", "*/0 * * * *"], "zero_step", "A step must be 1 or more.", None),
        (&["next", "0 0/60 * * * ?"], "step_too_large", "Step /60 is too large for the minute field (Quartz needs less than 60).", None),
        (&["next", "5/15 * * * *"], "lone_step", "Unix cron doesn't allow '5/15'.", Some("Write 5-59/15 or */15.")),
        (&["next", "0 17-9 * * *"], "backward_range", "The range 17-9 goes backwards.", Some("Write the smaller value first, or split it: 17-23,0-9.")),
        (&["next", "0 0 L * *"], "unix_special", "'L' isn't Unix cron syntax.", Some(UNIX_ONLY)),
        (&["next", "0~30 * * * *"], "random_value", "'~' picks a random time when cron loads the crontab, so the run times can't be predicted.", Some(FIXED)),
        (&["next", "H 9 * * *"], "jenkins_hash", "'H' is Jenkins syntax, which time-cron doesn't read.", Some(FIXED)),
        (&["next", "0 0 12 ? * ?"], "question_mark", "Quartz needs '?' in exactly one of day of month and day of week.", Some(QUESTION)),
        (&["next", "0 0 12 ? * 2,6L"], "l_in_list", "Quartz doesn't allow 'L' in a list of days of the week.", None),
        (&["next", "0 0 12 ? * MON#2,FRI#1"], "multiple_nth", "Only one '#' day is allowed, on its own.", None),
        (&["next", "0 0 12 1-5W * ?"], "w_needs_day", "'W' needs a single day of the month, like 15W.", None),
        (&["next", "0 0 12 L-31 * ?"], "l_offset", "L-31: the offset can be at most 30.", None),
        (&["next", "0 0 12 ? * MON#6"], "nth_range", "In MON#6, 6 must be 1-5.", None),
        (&["next", "cron(0 12 ? * 2/2 *)"], "aws_dow_step", "AWS cron doesn't allow '/' in day of week.", Some("List the days instead, e.g. MON,WED,FRI.")),
        (&["next", "0 0 12 1 1 ? 2200"], "year_range", "Year 2200 is out of range 1970-2199.", None),
        (&["next", "@reboot"], "reboot", "@reboot runs once when cron starts, so it has no run times.", None),
        (&["next", "0 0 30 2 *"], "never_runs", "This schedule never runs: day 30 doesn't occur in February.", Some("Pick a day that exists in every chosen month, or use L in Quartz or AWS.")),
        (&["explain", "0 0 31 2,4 *"], "never_runs", "This schedule never runs: day 31 doesn't occur in February and April.", Some("Pick a day that exists in every chosen month, or use L in Quartz or AWS.")),
        (&["next", "0 0 1 1 ? 2027", "--from", "2028-01-01"], "never_runs", "This schedule never runs: the year field ends in 2027, before 2028-01-01.", None),
        (&["next", "0 9 * * *", "--tz", "Mars/Olympus"], "unknown_tz", "Unknown time zone 'Mars/Olympus'.", Some("Use an IANA name such as America/New_York, Europe/Berlin or UTC.")),
        (&["next", "0 9 * * *", "--from", "tomorrow"], "bad_from", "Can't read the start time 'tomorrow'.", Some("Use 2026-11-01, 2026-11-01T09:00 (in --tz) or 2026-11-01T09:00:00Z.")),
        (&["next", "0 9 * * *", "--from", "2200-01-01"], "from_range", "The start time must be between 1970 and 2199.", None),
        (&["convert", "0 9 */2 * MON", "--to", "quartz"], "not_convertible", "Quartz can't express this: it needs both a day of month and a day of week to match, and Quartz and AWS can only use one.", Some("Use one day field and check the other inside the job.")),
        (&["convert", "0 0 12 ? * MON#2", "--to", "unix"], "not_convertible", "Unix cron can't express this: Unix cron has no L, W or #.", None),
        (&["convert", "15 0 9 * * ?", "--to", "unix"], "not_convertible", "Unix cron can't express this: Unix cron has no seconds field (this runs at second 15).", None),
        (&["convert", "15 0 9 * * ?", "--to", "aws"], "not_convertible", "AWS cron can't express this: AWS cron has no seconds field.", None),
        (&["convert", "0 0 12 1 1 ? 2030", "--to", "unix"], "not_convertible", "Unix cron can't express this: Unix cron has no year field.", None),
        (&["convert", "@reboot", "--to", "quartz"], "not_convertible", "Quartz can't express this: @reboot has no schedule.", None),
        (&["convert", "0 0 30 2 *", "--to", "aws"], "never_runs", "This schedule never runs: day 30 doesn't occur in February.", Some("Pick a day that exists in every chosen month, or use L in Quartz or AWS.")),
    ];
    for &(args, code, message, hint) in cases {
        let (exit, v) = json(args);
        assert_eq!((exit, v), (1, json!({"error": {"code": code, "message": message, "hint": hint}})), "{args:?}");
        let text = match hint {
            Some(hint) => format!("x {message}\n  {hint}\n"),
            None => format!("x {message}\n"),
        };
        assert_eq!(run(args), (1, "".into(), text), "{args:?}");
    }
}

#[test]
fn turns_away_bad_usage_with_exit_2() {
    for args in [
        &["next", "0 9 * * *", "--count", "0"][..],
        &["next", "0 9 * * *", "--count", "1001"],
        &["next", "0 9 * * *", "--count", "many"],
        &["next", "0 9 * * *", "--dialect", "spring"],
        &["next", "0 9 * * *", "--tz"],
        &["explain", "0 9 * * *", "--tz", UTC],
        &["0 9 * * *", "--frobnicate"],
        &["convert", "0 9 * * *"],
        &["convert", "0 9 * * *", "--to", "auto"],
    ] {
        let (code, out, err) = run(args);
        assert_eq!((code, out.as_str()), (2, ""), "{args:?}");
        assert!(err.starts_with("error: "), "{args:?}: {err}");
        assert!(err.contains("--help"), "{args:?}: {err}");
        // With --json the same complaint comes as the error object on stdout.
        let (code, v) = json(args);
        assert_eq!(
            (code, &v["error"]["code"]),
            (2, &json!("usage")),
            "{args:?}"
        );
        assert_eq!(v["error"]["hint"], "For more information, try '--help'.");
    }
    let (_, _, err) = run(&["next", "0 9 * * *", "--count", "0"]);
    assert!(err.contains("must be between 1 and 1000"), "{err}");
    let (_, v) = json(&["next", "0 9 * * *", "--count", "0"]);
    let message = "invalid value '0' for '--count <N>': must be between 1 and 1000";
    assert_eq!(v["error"]["message"], message);
    let (_, v) = json(&["convert", "0 9 * * *"]);
    let message = "the following required arguments were not provided: --to <D>";
    assert_eq!(v["error"]["message"], message);
    // Help and version stay text.
    assert!(run(&["--help", "--json"]).1.starts_with("Explain a cron"));
    // No expression at all, and no terminal to ask on.
    let want = "x No expression given.\n  Try: time-cron next \"0 9 * * 1-5\"\n";
    assert_eq!(run(&[]), (2, "".into(), want.into()));
}

#[test]
fn colours_only_when_asked_and_never_with_no_color() {
    let (_, out, _) = run(&["explain", "0 9 * * *"]);
    assert!(!out.contains('\x1b'));
    let forced = |no_color: &str| {
        let out = time_cron(&["explain", "0 9 * * *"])
            .env("CLICOLOR_FORCE", "1")
            .env("NO_COLOR", no_color)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    assert!(!forced("1").contains('\x1b'));
    if cfg!(not(windows)) {
        // Windows drops ANSI codes into console calls, which a pipe can't take.
        assert!(forced("").starts_with("\x1b[1mAt 09:00 every day.\x1b[0m\n"));
    }
}

#[test]
fn shows_the_parsed_fields_with_debug() {
    let (code, out, err) = run(&[
        "next",
        "0 0 12 ? * 6L",
        "--tz",
        UTC,
        "--debug",
        "--count",
        "1",
    ]);
    assert_eq!(code, 0);
    assert!(out.starts_with("At 12:00, on the last Friday of the month.\n"));
    assert!(err.contains("debug: hour [12]\n"), "{err}");
    let (code, _, err) = run(&["next", "0 0 30 2 *", "--debug"]);
    assert_eq!(code, 1);
    assert!(err.contains("debug: Error { code: \"never_runs\""), "{err}");
}

#[test]
fn ends_quietly_when_stdout_closes() {
    let mut child = time_cron(&["next", "* * * * *", "--count", "1000", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert_eq!((child.wait().unwrap().code(), err.as_str()), (Some(0), ""));
}
