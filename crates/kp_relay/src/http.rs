//! Just enough HTTP/1.1 for the relay: one request per connection, bodies
//! sized by Content-Length, responses closed by `Connection: close`.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use serde::Serialize;

const MAX_HEAD: u64 = 16 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: Vec<u8>,
}

/// Why a request could not be read: an HTTP status to answer with, or
/// nothing to answer (the client went away).
#[derive(Debug)]
pub enum ReadError {
    Respond(u16, String),
    Gone,
}

pub fn read_request(r: &mut BufReader<TcpStream>, max_body: usize) -> Result<Request, ReadError> {
    let mut head = r.by_ref().take(MAX_HEAD);
    let mut line = String::new();
    if head.read_line(&mut line).map_err(|_| ReadError::Gone)? == 0 {
        return Err(ReadError::Gone);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(ReadError::Respond(400, "Requête invalide.".into()));
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (method, path, query) = (method.to_owned(), path.to_owned(), query.to_owned());

    let mut length = 0usize;
    loop {
        line.clear();
        if head.read_line(&mut line).map_err(|_| ReadError::Gone)? == 0 {
            return Err(ReadError::Gone);
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            length = v
                .trim()
                .parse()
                .map_err(|_| ReadError::Respond(400, "Content-Length invalide.".into()))?;
        }
    }
    if length > max_body {
        return Err(ReadError::Respond(
            413,
            format!(
                "Travail trop gros ({} Mo maximum).",
                max_body / (1024 * 1024)
            ),
        ));
    }
    let mut body = vec![0; length];
    // A short body means the sender cancelled (or lost the network) during
    // the upload: there is nothing to answer.
    r.read_exact(&mut body).map_err(|_| ReadError::Gone)?;
    Ok(Request {
        method,
        path,
        query,
        body,
    })
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

pub fn respond(s: &mut TcpStream, code: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {code} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        reason(code),
        body.len()
    )?;
    s.write_all(body)?;
    s.flush()
}

pub fn respond_json<T: Serialize>(s: &mut TcpStream, code: u16, value: &T) -> io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    respond(s, code, "application/json; charset=utf-8", &body)
}

pub fn respond_error(s: &mut TcpStream, code: u16, message: &str) -> io::Result<()> {
    respond_json(s, code, &serde_json::json!({ "error": message }))
}

/// Starts a response whose body runs until the connection closes.
pub fn start_stream(s: &mut TcpStream, content_type: &str) -> io::Result<()> {
    write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\n\
         Connection: close\r\n\r\n"
    )?;
    s.flush()
}
