//! The local page on 127.0.0.1: `GET /` serves it and `POST /api` answers it.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::error::Error;
use crate::report::{self, failure};

const PAGE: &str = include_str!("ui.html");
/// The most a request may send in headers, and again in its body.
const LIMIT: u64 = 16 * 1024;
/// The page reaches nothing but its own server.
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; connect-src 'self'; base-uri 'none'; form-action 'none'";

type Response = (u16, &'static str, String);

/// Serves on 127.0.0.1:`port` (0 picks a free port) until the process is stopped, and opens
/// the page in the default browser when `open` is set.
pub fn serve(port: u16, open: bool) -> Result<(), Error> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|e| {
        if e.kind() == io::ErrorKind::AddrInUse {
            Error::new(
                "port_in_use",
                format!("Can't start the page: port {port} is in use."),
            )
            .hint("Leave out --port to pick a free one.")
        } else {
            Error::new("io", format!("Can't start the page: {e}."))
        }
    })?;
    let port = listener.local_addr().map_or(port, |a| a.port());
    let url = format!("http://127.0.0.1:{port}");
    // With stdout closed nobody learns the address, so end quietly as other commands do.
    match writeln!(io::stdout(), "Open {url}  (Ctrl+C to stop)") {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
        Err(e) => return Err(Error::new("io", format!("Can't start the page: {e}."))),
        Ok(()) => {}
    }
    if open {
        browse(url);
    }
    for stream in listener.incoming().flatten() {
        // A client that hangs up or stalls loses only its own connection.
        thread::spawn(move || handle(&stream, port));
    }
    Ok(())
}

/// Asks the OS to open `url`; when it can't, the printed address is enough.
fn browse(url: String) {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Some openers wait for the browser to close, so they run beside the server.
    thread::spawn(move || command.status());
}

/// One request, one response, then the connection closes.
fn handle(stream: &TcpStream, port: u16) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let (status, kind, body) = respond(&mut BufReader::new(stream.take(LIMIT)), port)?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Content Too Large",
        _ => "Unprocessable Content",
    };
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: {CSP}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.shutdown(Shutdown::Write)?;
    // Read what the client still sends, so closing doesn't reset the connection before it
    // has read the answer.
    io::copy(&mut stream.take(1 << 20), &mut io::sink()).map(drop)
}

fn respond(reader: &mut BufReader<io::Take<&TcpStream>>, port: u16) -> io::Result<Response> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut words = line.split_whitespace();
    let route = format!(
        "{} {}",
        words.next().unwrap_or(""),
        words.next().unwrap_or("")
    );
    let (mut host, mut origin, mut length) = (None, None, None);
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(match reader.get_ref().limit() {
                0 => error(
                    413,
                    "too_large",
                    "The request headers are larger than 16 KB.",
                ),
                _ => error(400, "bad_request", "The request ended early."),
            });
        }
        if line.trim_end_matches(['\r', '\n']).is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Ok(error(400, "bad_request", "Can't read a request header."));
        };
        let field = match name.to_ascii_lowercase().as_str() {
            "host" => &mut host,
            "origin" => &mut origin,
            "content-length" => &mut length,
            _ => continue,
        };
        // A second value could disagree with the first one checked (RFC 9112 §3.2, §6.3).
        if field.replace(value.trim().to_owned()).is_some() {
            return Ok(error(400, "bad_request", "A request header is repeated."));
        }
    }
    let local = |scheme: &str, value: &str| {
        ["127.0.0.1", "localhost"]
            .iter()
            .any(|h| value.eq_ignore_ascii_case(&format!("{scheme}{h}:{port}")))
    };
    if !host.is_some_and(|h| local("", &h)) || origin.is_some_and(|o| !local("http://", &o)) {
        return Ok(error(
            403,
            "forbidden",
            "Only this computer's page can use time-cron ui.",
        ));
    }
    match route.as_str() {
        "GET /" => Ok((200, "text/html; charset=utf-8", PAGE.into())),
        "POST /api" => {
            let Some(length) = length.map_or(Some(0), |l| l.parse::<u64>().ok()) else {
                return Ok(error(400, "bad_request", "Can't read the request length."));
            };
            if length > LIMIT {
                return Ok(error(413, "too_large", "The request is larger than 16 KB."));
            }
            let buffered = reader.buffer().len() as u64;
            reader.get_mut().set_limit(length.saturating_sub(buffered));
            let mut body = Vec::new();
            reader.take(length).read_to_end(&mut body)?;
            if (body.len() as u64) < length {
                return Ok(error(400, "bad_request", "The request ended early."));
            }
            let answer = serde_json::from_slice::<Value>(&body)
                .map_err(|e| Error::new("bad_request", format!("Can't read the request: {e}.")))
                .and_then(report::answer);
            Ok(match answer {
                Ok(answer) => (200, JSON, answer.to_string()),
                Err(e) if e.code == "bad_request" => (400, JSON, failure(&e)),
                Err(e) => (422, JSON, failure(&e)),
            })
        }
        _ => Ok(error(404, "not_found", "There's nothing here.")),
    }
}

const JSON: &str = "application/json";

fn error(status: u16, code: &'static str, message: &str) -> Response {
    (status, JSON, failure(&Error::new(code, message)))
}
