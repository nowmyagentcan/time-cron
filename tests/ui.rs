//! The local page: the HTML itself, and `time-cron ui` answering over HTTP on 127.0.0.1.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

const PAGE: &str = include_str!("../src/ui.html");
const BIN: &str = env!("CARGO_BIN_EXE_time-cron");

#[test]
fn page_reaches_no_other_host() {
    let allowed = [
        "https://nowmyagentcan.com",
        "https://github.com/nowmyagentcan/time-cron",
        "https://x.com/chayp05",
        "http://www.w3.org/2000/svg",
    ];
    for (i, _) in PAGE.match_indices("://") {
        let start = PAGE[..i]
            .rfind(|c: char| !c.is_ascii_alphanumeric())
            .map_or(0, |p| p + 1);
        let url = &PAGE[start..];
        assert!(
            allowed.iter().any(|a| url.starts_with(a)),
            "unexpected URL: {}",
            &url[..url.len().min(60)]
        );
    }
    for loader in ["<script src", "<link rel=\"stylesheet\"", "@import", "url("] {
        assert!(!PAGE.contains(loader), "page loads a file: {loader}");
    }
}

#[test]
fn page_has_the_title_and_one_button() {
    assert!(PAGE.contains("<title>Explain a cron schedule</title>"));
    assert!(PAGE.contains("<button class=\"go\">Explain</button>"));
    assert!(PAGE.contains("fetch(\"/api\""));
}

/// `time-cron ui` on a free port, stopped when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(args: &[&str]) -> Server {
        let mut child = Command::new(BIN)
            .arg("ui")
            .args(args)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let port = line
            .strip_prefix("Open http://127.0.0.1:")
            .and_then(|rest| rest.strip_suffix("  (Ctrl+C to stop)\n"))
            .unwrap_or_else(|| panic!("{line:?}"))
            .parse()
            .unwrap();
        Server { child, port }
    }

    /// Sends `head` (request line and headers, `{port}` filled in) and `body`; returns the
    /// status, the headers and the body.
    fn send(&self, head: &str, body: &[u8]) -> (u16, String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = head.replace("{port}", &self.port.to_string());
        stream
            .write_all(format!("{head}\r\n\r\n").as_bytes())
            .unwrap();
        stream.write_all(body).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        let status = headers[9..12].parse().unwrap();
        (status, headers.to_owned(), body.to_owned())
    }

    fn post(&self, request: &Value) -> (u16, Value) {
        let body = request.to_string();
        let head = format!(
            "POST /api HTTP/1.1\r\nHost: 127.0.0.1:{{port}}\r\nOrigin: http://127.0.0.1:{{port}}\r\nContent-Type: application/json\r\nContent-Length: {}",
            body.len()
        );
        let (status, _, body) = self.send(&head, body.as_bytes());
        (status, serde_json::from_str(&body).unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

/// The CLI's `--json` answer for the same input.
fn cli(args: &[&str]) -> Value {
    let out = Command::new(BIN).args(args).arg("--json").output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn serves_the_page_and_answers_like_the_cli() {
    let server = Server::start(&["--no-open"]);
    let (status, headers, body) = server.send("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}", b"");
    assert_eq!(status, 200);
    assert_eq!(body, PAGE);
    assert!(headers.contains("Content-Type: text/html; charset=utf-8\r\n"));
    assert!(headers.contains("Content-Security-Policy: default-src 'none';"));
    let next = json!({"op": "next", "expression": "30 2 * * *", "dialect": "auto", "timezone": "America/New_York", "from": "2027-03-13T12:00", "count": 3});
    let ny = ["--tz", "America/New_York", "--from", "2027-03-13T12:00"];
    assert_eq!(
        server.post(&next),
        (
            200,
            cli(&[&["next", "30 2 * * *", "--count", "3"][..], &ny].concat())
        )
    );
    for (request, args) in [
        (
            json!({"op": "explain", "expression": "0 0 12 ? * MON#2"}),
            &["explain", "0 0 12 ? * MON#2"][..],
        ),
        (
            json!({"op": "convert", "expression": "0 0 1,15 * FRI", "to": "quartz"}),
            &["convert", "0 0 1,15 * FRI", "--to", "quartz"],
        ),
        (
            json!({"op": "next", "expression": "cron(0 18 ? * MON-FRI *)", "from": "2026-10-29T00:00:00Z"}),
            &[
                "next",
                "cron(0 18 ? * MON-FRI *)",
                "--from",
                "2026-10-29T00:00:00Z",
            ],
        ),
    ] {
        assert_eq!(server.post(&request), (200, cli(args)), "{args:?}");
    }
    for (request, args) in [
        (
            json!({"op": "next", "expression": "0 0 30 2 *"}),
            &["next", "0 0 30 2 *"][..],
        ),
        (
            json!({"op": "next", "expression": "@daily", "timezone": "Mars/Base"}),
            &["next", "@daily", "--tz", "Mars/Base"],
        ),
        (
            json!({"op": "convert", "expression": "0 9 */2 * MON", "to": "quartz"}),
            &["convert", "0 9 */2 * MON", "--to", "quartz"],
        ),
        (json!({"op": "explain", "expression": ""}), &["explain", ""]),
    ] {
        assert_eq!(server.post(&request), (422, cli(args)), "{args:?}");
    }
    for request in [
        json!({"op": "next", "expression": "@daily", "count": 0}),
        json!({"op": "convert", "expression": "@daily"}),
        json!({"expression": "@daily"}),
        json!("@daily"),
    ] {
        let (status, error) = server.post(&request);
        assert_eq!(
            (status, &error["error"]["code"]),
            (400, &json!("bad_request")),
            "{request}"
        );
    }
    let head = "POST /api HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://LOCALHOST:{port}\r\nContent-Length: 5";
    let (status, _, body) = server.send(head, b"{nope");
    assert_eq!(status, 400);
    assert!(
        body.starts_with(r#"{"error":{"code":"bad_request","message":"Can't read the request: "#)
    );
}

#[test]
fn refuses_other_hosts_origins_routes_and_large_requests() {
    let server = Server::start(&["--no-open"]);
    let other = server.port.wrapping_add(1);
    for (head, status) in [
        ("GET / HTTP/1.1\r\nHost: localhost:{port}".to_owned(), 200),
        ("GET / HTTP/1.1".into(), 403),
        ("GET / HTTP/1.1\r\nHost: evil.example".into(), 403),
        ("GET / HTTP/1.1\r\nHost: evil.example:{port}".into(), 403),
        (format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{other}"), 403),
        (
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://evil.example".into(),
            403,
        ),
        (
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://127.0.0.1:{port}".into(),
            403,
        ),
        (
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: null".into(),
            403,
        ),
        ("GET /api HTTP/1.1\r\nHost: 127.0.0.1:{port}".into(), 404),
        (
            "GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1:{port}".into(),
            404,
        ),
        (
            "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0".into(),
            404,
        ),
        ("DELETE /api HTTP/1.1\r\nHost: 127.0.0.1:{port}".into(), 404),
    ] {
        assert_eq!(server.send(&head, b"").0, status, "{head}");
    }
    let big = vec![b' '; 16 * 1024 + 1];
    let head = format!(
        "POST /api HTTP/1.1\r\nHost: 127.0.0.1:{{port}}\r\nContent-Length: {}",
        big.len()
    );
    let (status, _, body) = server.send(&head, &big);
    assert_eq!(status, 413);
    assert_eq!(
        body,
        r#"{"error":{"code":"too_large","message":"The request is larger than 16 KB.","hint":null}}"#
    );
    let padded = format!(
        r#"{{"op":"explain","expression":"@daily"}}{}"#,
        " ".repeat(16 * 1024 - 38)
    );
    let head = format!(
        "POST /api HTTP/1.1\r\nHost: 127.0.0.1:{{port}}\r\nContent-Length: {}",
        padded.len()
    );
    assert_eq!(server.send(&head, padded.as_bytes()).0, 200);
    let head = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{{port}}\r\nX-Pad: {}",
        "a".repeat(17 * 1024)
    );
    assert_eq!(server.send(&head, b"").0, 413);
}

#[test]
fn reports_a_busy_port() {
    let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    let out = Command::new(BIN)
        .args(["ui", "--no-open", "--port", &port])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.contains(&format!("Can't start the page: port {port} is in use.\n")),
        "{err}"
    );
    assert!(err.ends_with("  Leave out --port to pick a free one.\n"));
}

/// A stand-in for xdg-open and open that writes down the address it was given.
#[cfg(unix)]
#[test]
fn opens_the_browser_unless_told_not_to() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("time-cron-browser-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let opened = dir.join("opened");
    for opener in ["xdg-open", "open"] {
        let script = dir.join(opener);
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$1\" > '{}'\n", opened.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let opened_url = || std::fs::read_to_string(&opened).ok();
    let start = |args: &[&str]| {
        let mut child = Command::new(BIN)
            .arg("ui")
            .args(args)
            .env("PATH", &path)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        child.kill().unwrap();
        child.wait().unwrap();
        line
    };
    start(&["--no-open"]);
    assert_eq!(opened_url(), None);
    let line = start(&[]);
    let url = line.split_whitespace().nth(1).unwrap();
    assert_eq!(opened_url().as_deref(), Some(format!("{url}\n").as_str()));
    std::fs::remove_dir_all(&dir).unwrap();
}
