//! Talks to `time-cron mcp` over stdio the way MCP clients do.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_time-cron");

/// Sends each message, closes stdin, and returns the responses by id and the exit code.
fn session(messages: &[Value]) -> (Vec<Value>, i32) {
    let mut child = Command::new(BIN)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for m in messages {
        writeln!(stdin, "{m}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut responses: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    responses.sort_by_key(|r| r["id"].as_u64());
    (responses, out.status.code().unwrap())
}

/// The CLI's `--json` answer for the same input.
fn cli(args: &[&str]) -> Value {
    let out = Command::new(BIN).args(args).arg("--json").output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn call(id: u64, tool: &str, arguments: Value) -> Value {
    request(
        id,
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
    )
}

const INSTRUCTIONS: &str = "Explain cron expressions, list their next run times in any time zone with each scheduler's daylight-saving rules, and convert between Unix cron, Quartz and AWS EventBridge. Use these tools instead of computing cron schedules yourself.";

/// The three tools with the documented descriptions, arguments and hints.
fn check_tools(list: &Value) {
    let tools = list["result"]["tools"].as_array().unwrap();
    let tool = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap();
    assert_eq!(tools.len(), 3);
    for (name, start, required, properties) in [
        (
            "explain_cron",
            "Explain a cron expression in plain English and flag likely mistakes. Reads Unix cron (5 fields)",
            &["expression"][..],
            &["dialect", "expression"][..],
        ),
        (
            "next_cron_runs",
            "List the next run times of a cron expression in an IANA time zone, following the scheduler's own daylight-saving rules (cronie for Unix, Quartz, EventBridge Scheduler).",
            &["expression"],
            &["count", "dialect", "expression", "from", "timezone"],
        ),
        (
            "convert_cron",
            "Convert a cron expression between Unix cron, Quartz and AWS EventBridge. Returns one expression, two when",
            &["expression", "to"],
            &["dialect", "expression", "to"],
        ),
    ] {
        let t = tool(name);
        assert!(
            t["description"].as_str().unwrap().starts_with(start),
            "{name}"
        );
        assert_eq!(
            t["annotations"],
            json!({"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false})
        );
        let input = &t["inputSchema"];
        assert_eq!(input["required"], json!(required), "{name}");
        let mut names: Vec<_> = input["properties"].as_object().unwrap().keys().collect();
        names.sort();
        assert_eq!(names, properties, "{name}");
        assert_eq!(input["properties"]["dialect"]["default"], "auto");
        assert_eq!(t["outputSchema"]["type"], "object");
    }
    let count = &tool("next_cron_runs")["inputSchema"]["properties"]["count"];
    assert_eq!(
        (&count["minimum"], &count["maximum"], &count["default"]),
        (&json!(1), &json!(1000), &json!(5))
    );
    let defs = &tool("convert_cron")["inputSchema"]["$defs"];
    assert_eq!(defs["Dialect"]["enum"], json!(["unix", "quartz", "aws"]));
    assert_eq!(
        defs["DialectArg"]["enum"],
        json!(["auto", "unix", "quartz", "aws"])
    );
    let runs = &tool("next_cron_runs")["outputSchema"];
    assert_eq!(
        runs["required"],
        json!([
            "expression",
            "dialect",
            "description",
            "timezone",
            "tzdata",
            "from",
            "runs",
            "skipped",
            "warnings"
        ])
    );
}

/// A successful call: structured content equal to the CLI's JSON, and the same JSON as text.
fn check_answer(response: &Value, cli_args: &[&str]) {
    let result = &response["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"], cli(cli_args), "{cli_args:?}");
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);
}

/// A failed call: an error result whose text is the CLI's JSON error object.
fn check_error(response: &Value, code: &str, cli_args: Option<&[&str]>) {
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert!(result.get("structuredContent").is_none());
    let error: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], code);
    if let Some(args) = cli_args {
        assert_eq!(error, cli(args));
    }
}

#[test]
fn answers_after_an_initialize_handshake() {
    let next = json!({"expression": "30 2 * * *", "timezone": "America/New_York", "from": "2027-03-13T12:00", "count": 3});
    let (responses, code) = session(&[
        request(
            1,
            "initialize",
            json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0.0"}}),
        ),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        request(2, "tools/list", json!({})),
        call(3, "next_cron_runs", next),
        call(
            4,
            "explain_cron",
            json!({"expression": "*/15 9-17 * * 1-5"}),
        ),
        call(
            5,
            "convert_cron",
            json!({"expression": "0 0 1,15 * FRI", "to": "aws"}),
        ),
        call(6, "next_cron_runs", json!({"expression": "0 0 30 2 *"})),
        call(
            7,
            "convert_cron",
            json!({"expression": "0 9 */2 * MON", "to": "quartz"}),
        ),
        call(
            8,
            "explain_cron",
            json!({"expression": "0 0 12 * * *", "dialect": "unix"}),
        ),
        call(
            9,
            "next_cron_runs",
            json!({"expression": "@daily", "count": 0}),
        ),
        call(10, "convert_cron", json!({"expression": "@daily"})),
        call(
            11,
            "explain_cron",
            json!({"expression": "0 9 * * *", "dialect": "cron"}),
        ),
    ]);
    assert_eq!(code, 0);
    assert_eq!(responses.len(), 11);
    let init = &responses[0]["result"];
    assert_eq!(init["protocolVersion"], "2025-11-25");
    assert_eq!(
        init["serverInfo"],
        json!({"name": "time-cron", "version": "0.1.0"})
    );
    assert_eq!(init["instructions"], INSTRUCTIONS);
    assert_eq!(init["capabilities"]["tools"], json!({}));
    check_tools(&responses[1]);
    let ny = [
        "--tz",
        "America/New_York",
        "--from",
        "2027-03-13T12:00",
        "--count",
        "3",
    ];
    check_answer(&responses[2], &[&["next", "30 2 * * *"][..], &ny].concat());
    assert_eq!(
        responses[2]["result"]["structuredContent"]["runs"][0]["dst"],
        "moved"
    );
    check_answer(&responses[3], &["explain", "*/15 9-17 * * 1-5"]);
    check_answer(&responses[4], &["convert", "0 0 1,15 * FRI", "--to", "aws"]);
    check_error(&responses[5], "never_runs", Some(&["next", "0 0 30 2 *"]));
    check_error(
        &responses[6],
        "not_convertible",
        Some(&["convert", "0 9 */2 * MON", "--to", "quartz"]),
    );
    check_error(
        &responses[7],
        "dialect_field_count",
        Some(&["explain", "0 0 12 * * *", "--dialect", "unix"]),
    );
    for response in &responses[8..] {
        check_error(response, "bad_request", None);
    }
}

#[test]
fn answers_after_server_discover() {
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1.0.0"},
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let explain = json!({"_meta": meta, "name": "explain_cron", "arguments": {"expression": "0 0 12 ? * MON#2"}});
    let (responses, code) = session(&[
        request(1, "server/discover", json!({"_meta": meta})),
        request(2, "tools/list", json!({"_meta": meta})),
        request(3, "tools/call", explain),
    ]);
    assert_eq!(code, 0);
    let discover = &responses[0]["result"];
    let versions = discover["supportedVersions"].as_array().unwrap();
    assert!(versions.contains(&json!("2026-07-28")) && versions.contains(&json!("2025-11-25")));
    assert_eq!(
        discover["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "time-cron"
    );
    assert_eq!(discover["instructions"], INSTRUCTIONS);
    check_tools(&responses[1]);
    check_answer(&responses[2], &["explain", "0 0 12 ? * MON#2"]);
}

#[test]
fn exits_quietly_when_stdin_closes() {
    assert_eq!(session(&[]), (vec![], 0));
}
