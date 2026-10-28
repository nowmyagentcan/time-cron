//! Edge cases: calendar and zone corners, input limits, and conversions checked against the
//! run times they promise.

use std::process::{Command, Stdio};
use std::thread;

use serde_json::Value;

const UTC: &str = "UTC";
const NY: &str = "America/New_York";
const START: &str = "2026-10-29T00:00:00Z";
const SPRING: &str = "2027-03-13T12:00";
const AUTUMN: &str = "2026-10-31T23:00";

fn time_cron(args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_time-cron"));
    cmd.args(args).stdin(Stdio::null());
    cmd
}

/// Exit code and the `--json` answer.
fn json(args: &[&str]) -> (i32, Value) {
    let out = time_cron(&[args, &["--json"]].concat()).output().unwrap();
    let value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: {e}: {:?}", out.stdout));
    (out.status.code().unwrap(), value)
}

fn next(expr: &str, tz: &str, from: &str, count: &str) -> Value {
    let (code, v) = json(&["next", expr, "--tz", tz, "--from", from, "--count", count]);
    assert_eq!(code, 0, "{expr}: {v}");
    v
}

fn strings<'a>(v: &'a Value, list: &str, key: &str) -> Vec<&'a str> {
    let items = v[list].as_array().unwrap();
    items.iter().map(|x| x[key].as_str().unwrap()).collect()
}

fn codes(v: &Value) -> Vec<&str> {
    strings(v, "warnings", "code")
}

/// Runs as "time [dst] [scheduled]", plus the skipped wall times.
fn timeline(v: &Value) -> (Vec<String>, Vec<&str>) {
    let runs = v["runs"].as_array().unwrap().iter().map(|r| {
        let parts = [&r["time"], &r["dst"], &r["scheduled"]];
        parts
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    });
    (runs.collect(), strings(v, "skipped", "scheduled"))
}

#[test]
#[rustfmt::skip]
fn finds_runs_at_calendar_corners() {
    // expression, from, count, utc times
    let cases: &[(&str, &str, &str, &[&str])] = &[
        ("0 0 0 1W * ?", "2027-04-15T00:00:00Z", "1", &["2027-05-03T00:00:00Z"]),
        ("0 0 0 31W * ?", "2027-10-01T00:00:00Z", "1", &["2027-10-29T00:00:00Z"]),
        ("0 0 0 LW * ?", "2027-01-01T00:00:00Z", "3", &["2027-01-29T00:00:00Z", "2027-02-26T00:00:00Z", "2027-03-31T00:00:00Z"]),
        ("0 0 0 ? * MON#5", START, "3", &["2026-11-30T00:00:00Z", "2027-03-29T00:00:00Z", "2027-05-31T00:00:00Z"]),
        ("0 0 0 L * ?", "2100-02-01T00:00:00Z", "2", &["2100-02-28T00:00:00Z", "2100-03-31T00:00:00Z"]),
        ("0 0 0 29 2 ?", "2096-03-01T00:00:00Z", "1", &["2104-02-29T00:00:00Z"]),
        ("0 0 1 1 *", "1970-01-01T00:00:00Z", "1", &["1971-01-01T00:00:00Z"]),
        ("15 30 9 * * ?", "2026-10-29T09:30:15Z", "1", &["2026-10-30T09:30:15Z"]),
        ("0 0 * * FRI-SUN", START, "3", &["2026-10-30T00:00:00Z", "2026-10-31T00:00:00Z", "2026-11-01T00:00:00Z"]),
        ("0 0 * * 5-0", START, "3", &["2026-10-30T00:00:00Z", "2026-10-31T00:00:00Z", "2026-11-01T00:00:00Z"]),
        ("0 0 22-2 * * ?", START, "3", &["2026-10-29T01:00:00Z", "2026-10-29T02:00:00Z", "2026-10-29T22:00:00Z"]),
        ("0 0 0 ? * FRI-MON", START, "4", &["2026-10-30T00:00:00Z", "2026-10-31T00:00:00Z", "2026-11-01T00:00:00Z", "2026-11-02T00:00:00Z"]),
    ];
    for &(expr, from, count, utc) in cases {
        assert_eq!(strings(&next(expr, UTC, from, count), "runs", "utc"), utc, "{expr}");
    }
    let v = next("59 59 23 31 12 ? 2199", UTC, "2199-12-31T00:00:00Z", "2");
    assert_eq!((strings(&v, "runs", "utc"), codes(&v)), (vec!["2199-12-31T23:59:59Z"], vec!["few_runs"]));
}

#[test]
#[rustfmt::skip]
fn follows_half_hour_two_hour_and_whole_day_shifts() {
    let v = next("15,45 1 * * *", "Australia/Lord_Howe", "2027-04-03T12:00", "4");
    assert_eq!(timeline(&v).0, ["2027-04-04T01:15:00+11:00", "2027-04-04T01:45:00+11:00 first", "2027-04-05T01:15:00+10:30", "2027-04-05T01:45:00+10:30"]);
    let v = next("0 2 * * *", "Antarctica/Troll", "2027-03-27T12:00", "2");
    assert_eq!(timeline(&v).0, ["2027-03-28T03:00:00+02:00 moved 2027-03-28T02:00:00", "2027-03-29T02:00:00+02:00"]);
    // Samoa skipped 2011-12-30: a 24-hour gap is longer than cron's 3-hour catch-up.
    let v = next("0 12 * * *", "Pacific/Apia", "2011-12-29T00:00", "2");
    assert_eq!(timeline(&v), (vec!["2011-12-29T12:00:00-10:00".into(), "2011-12-31T12:00:00+14:00".into()], vec!["2011-12-30T12:00:00"]));
}

#[test]
fn reads_up_to_1024_bytes_and_plain_whitespace() {
    let list = format!("{}00", "0,".repeat(507));
    let longest = format!("  {list} 9 * * *\r\n");
    assert_eq!(longest.trim().len(), 1024);
    assert_eq!(json(&["explain", &longest]).0, 0);
    let (code, v) = json(&["explain", &format!("{list}0 9 * * *")]);
    assert_eq!((code, &v["error"]["code"]), (1, &"too_long".into()));
    let (code, v) = json(&["explain", "0\t9  *  * *\r\n"]);
    assert_eq!(
        (code, &v["description"]),
        (0, &"At 09:00 every day.".into())
    );
    let (code, v) = json(&["explain", "0 9 * * *\n0 10 * * *"]);
    assert_eq!((code, &v["error"]["code"]), (1, &"multiline".into()));
}

#[test]
fn warns_about_a_step_past_the_field() {
    let (_, v) = json(&["explain", "*/90 * * * *"]);
    assert_eq!(codes(&v), ["big_step"]);
}

#[test]
fn gives_the_same_answer_to_concurrent_runs() {
    let args = ["next", "*/15 2 * * *", "--tz", NY, "--from", SPRING];
    let answers: Vec<Value> = (0..16)
        .map(|_| thread::spawn(move || json(&args).1))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    assert!(answers.iter().all(|a| *a == answers[0]));
}

#[test]
fn converting_to_unix_keeps_the_dst_behaviour() {
    for (expr, tz, from) in [
        ("*/30 2 * * *", NY, SPRING),
        ("*/60 2 * * *", NY, SPRING),
        ("*/30 1 * * *", NY, AUTUMN),
        ("0 */12 * * *", "America/Santiago", "2027-09-04T12:00"),
    ] {
        let (code, v) = json(&["convert", expr, "--to", "unix"]);
        assert_eq!(code, 0, "{expr}: {v}");
        let result = v["results"][0].as_str().unwrap();
        assert_eq!(
            timeline(&next(result, tz, from, "6")),
            timeline(&next(expr, tz, from, "6")),
            "{expr} -> {result}"
        );
    }
}

#[test]
fn keeps_runs_in_time_order_across_a_fold_that_spans_midnight() {
    // Goose Bay, 1988-10-30: clocks went back from 00:01 to 22:01 the previous evening.
    let tz = "America/Goose_Bay";
    let v = next("*/30 * * * *", tz, "1988-10-29T23:45:00-02:00", "1");
    assert_eq!(strings(&v, "runs", "utc"), ["1988-10-30T02:00:00Z"]);
    let v = next("*/30 * * * *", tz, "1988-10-29T23:45:00-02:00", "8");
    let utc = strings(&v, "runs", "utc");
    assert!(utc.is_sorted(), "{utc:?}");
    let v = next("0 30 23 * * ?", tz, "1988-10-30T00:00:30-02:00", "1");
    assert_eq!(strings(&v, "runs", "utc"), ["1988-10-30T03:30:00Z"]);
}

#[test]
fn converts_an_or_schedule_whose_day_of_month_never_occurs() {
    for to in ["quartz", "aws"] {
        let (code, v) = json(&["convert", "0 0 30 2 MON", "--to", to]);
        assert_eq!(code, 0, "{to}: {v}");
        // A result that never runs adds nothing; the others together give the source's runs.
        let mut runs = vec![];
        for r in v["results"].as_array().unwrap() {
            let r = r.as_str().unwrap();
            let (code, a) = json(&["next", r, "--tz", UTC, "--from", START, "--count", "4"]);
            if code == 0 {
                runs.extend(strings(&a, "runs", "utc").into_iter().map(String::from));
            }
        }
        runs.sort();
        let source = next("0 0 30 2 MON", UTC, START, "4");
        assert_eq!(runs[..4], strings(&source, "runs", "utc")[..], "{to}");
    }
}

#[test]
fn explains_and_converts_schedules_that_ended_in_the_past() {
    let (code, v) = json(&["explain", "0 0 12 1 1 ? 2020"]);
    assert_eq!(
        (code, &v["description"]),
        (0, &"At 12:00, on January 1, in 2020.".into())
    );
    let (code, v) = json(&["convert", "0 0 12 1 1 ? 2020", "--to", "aws"]);
    assert_eq!(
        (code, &v["results"][0]),
        (0, &"cron(0 12 1 1 ? 2020)".into())
    );
    let (code, v) = json(&["explain", "0 0 30 2 *"]);
    assert_eq!((code, &v["error"]["code"]), (1, &"never_runs".into()));
}

#[test]
fn writes_every_time_as_rfc_3339() {
    let v = next("0 0 * * *", "Africa/Monrovia", "1971-01-01", "1");
    let rfc3339 = |s: &str| {
        let offset = s.get(19..).unwrap_or_default();
        s.len() >= 20 && (offset == "Z" || (offset.len() == 6 && offset.as_bytes()[3] == b':'))
    };
    for t in [
        v["from"].as_str().unwrap(),
        v["runs"][0]["time"].as_str().unwrap(),
    ] {
        assert!(rfc3339(t), "{t}");
    }
}

#[test]
fn separates_fields_only_with_spaces_and_tabs() {
    for sep in ['\u{a0}', '\u{2028}', '\u{b}', '\u{c}', '\u{3000}'] {
        let (code, v) = json(&["explain", &format!("0{sep}9 * * *")]);
        assert_eq!(code, 1, "{:?}: {v}", sep);
    }
}

#[test]
fn does_not_call_a_step_of_one_too_big() {
    for expr in ["0 0 23/1 * * ?", "0 0 23-23/1 * * ?"] {
        let (_, v) = json(&["explain", expr]);
        assert!(!codes(&v).contains(&"big_step"), "{expr}: {v}");
    }
}

#[test]
fn takes_debug_on_every_command() {
    let out = time_cron(&["mcp", "--debug"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = time_cron(&["ui", "--debug", "--port", "70000"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stderr).contains("'--debug'"));
}

/// A field of a random Unix expression: its text and the values it matches.
fn random_field(seed: &mut u64, lo: u16, hi: u16) -> (String, Vec<u16>) {
    let mut roll = |n: u16| {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed % u64::from(n)) as u16
    };
    let span = hi - lo + 1;
    let a = lo + roll(span);
    let b = a + roll(hi - a + 1);
    let step = 1 + roll(span);
    match roll(6) {
        0 => ("*".into(), (lo..=hi).collect()),
        1 => (
            format!("*/{step}"),
            (lo..=hi).step_by(step.into()).collect(),
        ),
        2 => (a.to_string(), vec![a]),
        3 => (format!("{a}-{b}"), (a..=b).collect()),
        4 => (
            format!("{a}-{b}/{step}"),
            (a..=b).step_by(step.into()).collect(),
        ),
        _ => (format!("{a},{b}"), vec![a, b]),
    }
}

/// Generated Unix schedules agree with a day-by-day reading of cronie's rules in UTC.
#[test]
fn matches_an_independent_reading_of_unix_cron() {
    let mut seed = 0x5eed_c0de_u64;
    for _ in 0..120 {
        let [minute, hour, dom, month, dow] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)]
            .map(|(lo, hi)| random_field(&mut seed, lo, hi));
        let expr = [&minute, &hour, &dom, &month, &dow]
            .map(|f| f.0.as_str())
            .join(" ");
        let day_rule_and = dom.0.starts_with('*') || dow.0.starts_with('*');
        let mut want = vec![];
        let mut day = jiff::civil::date(2026, 10, 29);
        while want.len() < 20 && day.year() < 2200 {
            let wd = day.weekday().to_sunday_zero_offset() as u16;
            let on_dom = dom.1.contains(&(day.day() as u16));
            let on_dow = dow.1.iter().any(|d| d % 7 == wd);
            let on_day = if day_rule_and {
                on_dom && on_dow
            } else {
                on_dom || on_dow
            };
            if month.1.contains(&(day.month() as u16)) && on_day {
                let mut times: Vec<(u16, u16)> = hour
                    .1
                    .iter()
                    .flat_map(|h| minute.1.iter().map(move |m| (*h, *m)))
                    .collect();
                times.sort();
                times.dedup();
                for (h, m) in times {
                    if (day, h, m) != (jiff::civil::date(2026, 10, 29), 0, 0) && want.len() < 20 {
                        want.push(format!("{day}T{h:02}:{m:02}:00Z"));
                    }
                }
            }
            day = day.tomorrow().unwrap();
        }
        let (code, v) = json(&["next", &expr, "--tz", UTC, "--from", START, "--count", "20"]);
        if want.is_empty() {
            assert_eq!(
                (code, &v["error"]["code"]),
                (1, &"never_runs".into()),
                "{expr}"
            );
        } else {
            assert_eq!(code, 0, "{expr}: {v}");
            assert_eq!(strings(&v, "runs", "utc"), want, "{expr}");
        }
    }
}

#[test]
fn lists_the_same_first_runs_whatever_the_count() {
    for (expr, tz, from) in [
        ("*/30 1 * * *", NY, AUTUMN),
        ("30 1 * * *", NY, AUTUMN),
        ("0 0/20 1 * * ?", NY, AUTUMN),
        ("cron(30 1 * * ? *)", NY, AUTUMN),
        ("*/15 2 * * *", NY, SPRING),
        ("0 15 1 * * ?", "Australia/Lord_Howe", "2027-04-03T12:00"),
        (
            "*/30 * * * *",
            "America/Goose_Bay",
            "1988-10-29T23:00:00-02:00",
        ),
        ("0 */6 * * *", "America/Santiago", "2027-04-03T12:00"),
    ] {
        let all = next(expr, tz, from, "20");
        let utc = strings(&all, "runs", "utc");
        assert!(utc.is_sorted(), "{expr}: {utc:?}");
        for count in [1, 2, 3, 5, 8] {
            let some = next(expr, tz, from, &count.to_string());
            assert_eq!(
                some["runs"].as_array().unwrap()[..],
                all["runs"].as_array().unwrap()[..count],
                "{expr} --count {count}"
            );
        }
    }
}

#[test]
fn every_time_names_the_same_instant_as_its_utc() {
    let instant = |s: &str| {
        s.parse::<jiff::Timestamp>()
            .unwrap_or_else(|e| panic!("{s}: {e}"))
    };
    for from in ["1971-01-01", "1971-06-01T00:00:00Z", "1972-01-06T12:00"] {
        let v = next("0 0 * * *", "Africa/Monrovia", from, "3");
        for r in v["runs"].as_array().unwrap() {
            let (time, utc) = (r["time"].as_str().unwrap(), r["utc"].as_str().unwrap());
            assert_eq!(instant(time), instant(utc), "{from}: {time} vs {utc}");
        }
    }
    let v = next("0 0 * * *", "Africa/Monrovia", "1971-06-01T00:00:00Z", "1");
    assert_eq!(
        instant(v["from"].as_str().unwrap()),
        instant("1971-06-01T00:00:00Z")
    );
}

#[test]
fn warns_that_dst_handling_changes_across_dialects() {
    for (expr, to) in [
        ("0 0/30 2 * * ?", "unix"),
        ("30 2 * * *", "quartz"),
        ("cron(30 2 * * ? *)", "unix"),
    ] {
        let (code, v) = json(&["convert", expr, "--to", to]);
        assert_eq!(code, 0, "{expr}: {v}");
        assert!(!codes(&v).is_empty(), "{expr} -> {to}: {v}");
    }
}
