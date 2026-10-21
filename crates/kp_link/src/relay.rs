//! KI Plotter relay: the small service on the Raspberry Pi wired to the
//! cutter (crate `kp_relay`), and the client side used by the desktop app.
//!
//! Protocol — HTTP/1.1, one request per connection, `Connection: close`:
//! - `GET /status` → [`RelayStatus`] as JSON.
//! - `POST /job?…` ([`JobParams`]) with the raw HPGL/DMPL bytes as body →
//!   `200` and one JSON [`RelayEvent`] per line. In queue mode a single
//!   `queued` event (the job waits to be started on the relay's screen);
//!   in direct mode `progress` events until `done`. Any other status
//!   carries `{"error": "…"}` (e.g. `409` while another job is running).
//! - `POST /cancel` → the running job stops between two blocks.
//! - Discovery: a UDP datagram [`DISCOVER_MAGIC`] sent to port
//!   [`DEFAULT_PORT`] is answered with a [`RelayInfo`] JSON datagram.
//!
//! The relay also serves its own touch panel (`GET /`, `/api/…`), see the
//! `kp_relay` crate.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{Flow, JobInfo, PortInfo, SendOutcome};

/// TCP (HTTP) and UDP (discovery) port of the relay.
pub const DEFAULT_PORT: u16 = 7870;

/// Discovery request payload.
pub const DISCOVER_MAGIC: &[u8] = b"KI-PLOTTER-RELAY?";

/// A relay found on the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayInfo {
    pub name: String,
    /// Filled in by the client from the reply's source address.
    #[serde(default)]
    pub host: String,
    pub port: u16,
    pub version: String,
}

/// Outcome of the last job the relay ran.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LastJob {
    pub ok: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    pub bytes: usize,
    /// Address of the computer that sent it.
    pub client: String,
}

/// `GET /status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RelayStatus {
    pub name: String,
    pub version: String,
    pub busy: bool,
    pub sent: usize,
    pub total: usize,
    /// Where jobs go by default, e.g. "/dev/ttyUSB0"; empty if no cutter was found.
    pub device: String,
    /// Why no default device could be found, if so.
    pub device_error: Option<String>,
    /// Serial ports present on the relay.
    pub ports: Vec<PortInfo>,
    pub last: Option<LastJob>,
    /// "queue" (jobs wait for the relay's screen) or "direct".
    pub mode: String,
    /// Jobs waiting in the queue.
    pub queued: usize,
}

/// One line of the `POST /job` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RelayEvent {
    Progress {
        sent: usize,
        total: usize,
    },
    Done {
        ok: bool,
        cancelled: bool,
        error: Option<String>,
    },
    /// Queue mode: the job waits on the relay; `position` 1 = next.
    Queued {
        id: u64,
        position: usize,
    },
}

/// Per-job settings of `POST /job`. Empty, zero or `None` means "use the
/// relay's configuration".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobParams {
    /// `auto`, `/dev/ttyUSB0`, `/dev/usb/lp0`… (empty = relay default).
    pub device: String,
    pub baud: u32,
    pub flow: Option<Flow>,
    pub chunk_size: usize,
    pub chunk_delay_ms: u64,
    pub info: JobInfo,
}

fn push_num(q: &mut Vec<String>, key: &str, v: f64) {
    if v.is_finite() && v > 0.0 {
        q.push(format!("{key}={}", (v * 100.0).round() / 100.0));
    }
}

impl JobParams {
    pub fn to_query(&self) -> String {
        let mut q = vec![format!("device={}", encode_component(&self.device))];
        if self.baud > 0 {
            q.push(format!("baud={}", self.baud));
        }
        if let Some(f) = self.flow {
            q.push(format!("flow={}", flow_name(f)));
        }
        if self.chunk_size > 0 {
            q.push(format!("chunkSize={}", self.chunk_size));
        }
        if self.chunk_delay_ms > 0 {
            q.push(format!("chunkDelayMs={}", self.chunk_delay_ms));
        }
        let i = &self.info;
        if !i.name.is_empty() {
            q.push(format!("name={}", encode_component(&i.name)));
        }
        push_num(&mut q, "width", i.width_mm);
        push_num(&mut q, "height", i.height_mm);
        push_num(&mut q, "seconds", i.estimated_seconds);
        if let Some(s) = &i.settings {
            q.push(format!("settings={}", encode_component(&s.to_string())));
        }
        q.join("&")
    }

    /// Lenient: unknown keys and unparsable values are ignored.
    pub fn from_query(query: &str) -> Self {
        let mut p = Self::default();
        let num = |v: &str| {
            v.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite() && *x >= 0.0)
                .unwrap_or(0.0)
        };
        for (k, v) in parse_query(query) {
            match k.as_str() {
                "device" => p.device = v,
                "baud" => p.baud = v.parse().unwrap_or(0),
                "flow" => p.flow = parse_flow(&v),
                "chunkSize" => p.chunk_size = v.parse().unwrap_or(0),
                "chunkDelayMs" => p.chunk_delay_ms = v.parse().unwrap_or(0),
                "name" => p.info.name = v.chars().take(120).collect(),
                "width" => p.info.width_mm = num(&v),
                "height" => p.info.height_mm = num(&v),
                "seconds" => p.info.estimated_seconds = num(&v),
                "settings" => p.info.settings = serde_json::from_str(&v).ok(),
                _ => {}
            }
        }
        p
    }
}

pub fn flow_name(f: Flow) -> &'static str {
    match f {
        Flow::None => "none",
        Flow::Hardware => "hardware",
        Flow::Software => "software",
    }
}

pub fn parse_flow(s: &str) -> Option<Flow> {
    match s.trim().to_ascii_lowercase().as_str() {
        "none" | "aucun" => Some(Flow::None),
        "hardware" | "rtscts" | "rts/cts" => Some(Flow::Hardware),
        "software" | "xonxoff" | "xon/xoff" => Some(Flow::Software),
        _ => None,
    }
}

/// Percent-encodes everything but unreserved URL characters.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes `%XX` and `+`; invalid escapes are kept as is.
pub fn decode_component(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode_component(k), decode_component(v))
        })
        .collect()
}

// ---------------------------------------------------------------- client

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// Read timeout while streaming events, so a cancel is noticed quickly.
const POLL: Duration = Duration::from_millis(250);
/// The relay writes a line after every block; this long without one means
/// it is gone (power cut, Wi-Fi lost). Longer than the serial stall timeout.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(40);
const MAX_RESPONSE: u64 = 4 * 1024 * 1024;

/// Accepts "192.168.1.20", "ki-plotter.local", or a pasted
/// "http://host:port/" (its port wins over `port`).
fn split_host(host: &str, port: u16) -> (String, u16) {
    let h = host.trim();
    let h = h.strip_prefix("http://").unwrap_or(h);
    let h = h.split('/').next().unwrap_or(h);
    if let Some((name, p)) = h.rsplit_once(':')
        && !name.contains(':')
        && let Ok(p) = p.parse()
    {
        return (name.to_owned(), p);
    }
    (h.to_owned(), port)
}

fn unreachable(host: &str, port: u16) -> String {
    format!(
        "Impossible de joindre le relais {host}:{port}. Le Raspberry Pi est-il allumé, \
         sur le même réseau, et le service « ki-plotter-relay » démarré ?"
    )
}

fn lost(host: &str) -> String {
    format!(
        "La connexion avec le relais {host} a été coupée pendant l'envoi. \
         Le plotter a peut-être reçu une partie du travail."
    )
}

fn connect(host: &str, port: u16) -> Result<TcpStream, String> {
    if host.is_empty() {
        return Err("Indiquez l'adresse du Raspberry Pi (ou cliquez sur Rechercher).".into());
    }
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|_| format!("Adresse du relais inconnue : « {host} »."))?;
    for a in addrs {
        if let Ok(s) = TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            let _ = s.set_nodelay(true);
            let _ = s.set_read_timeout(Some(IO_TIMEOUT));
            let _ = s.set_write_timeout(Some(IO_TIMEOUT));
            return Ok(s);
        }
    }
    Err(unreachable(host, port))
}

fn write_head(
    s: &mut TcpStream,
    method: &str,
    path: &str,
    host: &str,
    len: usize,
) -> io::Result<()> {
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: KI Plotter\r\n\
         Content-Type: application/octet-stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
    )
}

/// Reads the status line and headers; returns the status code.
fn read_head(r: &mut BufReader<TcpStream>) -> io::Result<u16> {
    let mut line = String::new();
    r.read_line(&mut line)?;
    let code = line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "réponse HTTP invalide"))?;
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 || line.trim_end().is_empty() {
            return Ok(code);
        }
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// Turns a non-200 response into its French message.
fn error_of(r: &mut BufReader<TcpStream>, code: u16, host: &str) -> String {
    let mut body = Vec::new();
    let _ = r.take(MAX_RESPONSE).read_to_end(&mut body);
    serde_json::from_slice::<ErrorBody>(&body)
        .map(|e| e.error)
        .unwrap_or_else(|_| format!("Le relais {host} a répondu par une erreur ({code})."))
}

fn request<T: DeserializeOwned>(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
) -> Result<T, String> {
    let (host, port) = split_host(host, port);
    let mut s = connect(&host, port)?;
    write_head(&mut s, method, path, &host, 0).map_err(|_| unreachable(&host, port))?;
    let mut r = BufReader::new(s);
    let code = read_head(&mut r).map_err(|_| unreachable(&host, port))?;
    if code != 200 {
        return Err(error_of(&mut r, code, &host));
    }
    let mut body = Vec::new();
    r.take(MAX_RESPONSE)
        .read_to_end(&mut body)
        .map_err(|_| lost(&host))?;
    serde_json::from_slice(&body).map_err(|_| {
        format!("« {host}:{port} » ne ressemble pas à un relais KI Plotter (réponse inattendue).")
    })
}

/// Asks a relay how it is doing (and which cutter it sees).
pub fn status(host: &str, port: u16) -> Result<RelayStatus, String> {
    request(host, port, "GET", "/status")
}

#[derive(Deserialize)]
struct Ack {}

/// Asks a relay to stop its running job between two blocks.
pub fn cancel(host: &str, port: u16) -> Result<(), String> {
    request::<Ack>(host, port, "POST", "/cancel").map(|_| ())
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// Sends a job through a relay: upload, then follow the relay's progress
/// (the bytes reach the cutter at serial speed, on the Pi).
pub(crate) fn send(
    host: &str,
    port: u16,
    params: &JobParams,
    data: &[u8],
    progress: &mut dyn FnMut(usize, usize),
    cancel_flag: &AtomicBool,
) -> Result<SendOutcome, String> {
    let (host, port) = split_host(host, port);
    let cancelled = || cancel_flag.load(Ordering::Acquire);
    let mut s = connect(&host, port)?;
    let path = format!("/job?{}", params.to_query());
    write_head(&mut s, "POST", &path, &host, data.len()).map_err(|_| unreachable(&host, port))?;
    progress(0, data.len());
    // Uploading over the LAN is quick. Cancelling here drops the connection:
    // the relay sees a short body and never starts the job.
    for chunk in data.chunks(64 * 1024) {
        if cancelled() {
            return Ok(SendOutcome::Cancelled);
        }
        s.write_all(chunk).map_err(|_| lost(&host))?;
    }
    let _ = s.flush();

    let mut r = BufReader::new(s);
    let code = read_head(&mut r).map_err(|_| lost(&host))?;
    if code != 200 {
        return Err(error_of(&mut r, code, &host));
    }
    let _ = r.get_ref().set_read_timeout(Some(POLL));
    let mut line = Vec::new();
    let mut heard = Instant::now();
    let mut cancel_sent = false;
    loop {
        // Checked every turn: while the job runs, lines keep arriving and the
        // read never times out.
        if cancelled() && !cancel_sent {
            cancel_sent = true;
            cancel(&host, port)?;
        }
        match r.read_until(b'\n', &mut line) {
            Ok(0) => return Err(lost(&host)),
            Ok(_) if line.last() == Some(&b'\n') => {
                heard = Instant::now();
                let event = serde_json::from_slice::<RelayEvent>(line.trim_ascii());
                line.clear();
                match event {
                    Ok(RelayEvent::Progress { sent, total }) => progress(sent, total),
                    Ok(RelayEvent::Done { ok: true, .. }) => return Ok(SendOutcome::Completed),
                    Ok(RelayEvent::Done {
                        cancelled: true, ..
                    }) => return Ok(SendOutcome::Cancelled),
                    Ok(RelayEvent::Queued { position, .. }) => {
                        return Ok(SendOutcome::Queued { position });
                    }
                    Ok(RelayEvent::Done { error, .. }) => {
                        return Err(error
                            .unwrap_or_else(|| "Le relais n'a pas pu envoyer le travail.".into()));
                    }
                    // Unknown line from a newer relay: ignore it.
                    Err(_) => {}
                }
            }
            // A partial line without newline: EOF follows, or more data.
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // `read_until` keeps the bytes read before a timeout in `line`.
            Err(e) if is_timeout(&e) => {
                if heard.elapsed() > SILENCE_TIMEOUT {
                    return Err(format!(
                        "Le relais {host} ne donne plus de nouvelles depuis {} s.",
                        SILENCE_TIMEOUT.as_secs()
                    ));
                }
            }
            Err(_) => return Err(lost(&host)),
        }
    }
}

// ---------------------------------------------------------------- discovery

/// Broadcasts a discovery request on every IPv4 network of this computer
/// and collects the relays that answer within `timeout`.
pub fn discover(timeout: Duration) -> Vec<RelayInfo> {
    let mut targets: Vec<SocketAddr> = vec![(Ipv4Addr::BROADCAST, DEFAULT_PORT).into()];
    // 255.255.255.255 only leaves through one interface on Windows: also send
    // to each network's own broadcast address (Wi-Fi, Ethernet, VPN…).
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for i in ifaces {
            if let if_addrs::IfAddr::V4(v4) = i.addr
                && !v4.ip.is_loopback()
            {
                let bcast = Ipv4Addr::from(u32::from(v4.ip) | !u32::from(v4.netmask));
                let t = SocketAddr::from((bcast, DEFAULT_PORT));
                if !targets.contains(&t) {
                    targets.push(t);
                }
            }
        }
    }
    discover_at(&targets, timeout)
}

/// Discovery against explicit addresses (also used by the tests on loopback).
pub fn discover_at(targets: &[SocketAddr], timeout: Duration) -> Vec<RelayInfo> {
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return Vec::new();
    };
    let _ = sock.set_broadcast(true);
    for t in targets {
        let _ = sock.send_to(DISCOVER_MAGIC, t);
    }
    let end = Instant::now() + timeout;
    let mut found: Vec<RelayInfo> = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let _ = sock.set_read_timeout(Some(left.max(Duration::from_millis(1))));
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let Ok(mut info) = serde_json::from_slice::<RelayInfo>(&buf[..n]) else {
                    continue;
                };
                info.host = match from.ip() {
                    IpAddr::V4(ip) => ip.to_string(),
                    IpAddr::V6(ip) => ip.to_string(),
                };
                if !found
                    .iter()
                    .any(|f| f.host == info.host && f.port == info.port)
                {
                    found.push(info);
                }
            }
            Err(e) if is_timeout(&e) => break,
            // Windows reports an ICMP "port unreachable" from an earlier
            // datagram as ConnectionReset on the next receive: keep listening.
            Err(_) => {}
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_round_trip() {
        let p = JobParams {
            device: "/dev/serial/by-id/usb-1a86 CH340".into(),
            baud: 38400,
            flow: Some(Flow::None),
            chunk_size: 128,
            chunk_delay_ms: 5,
            info: JobInfo {
                name: "Logo « boulangerie » & co".into(),
                width_mm: 230.5,
                height_mm: 80.0,
                estimated_seconds: 42.25,
                settings: Some(serde_json::json!({"machine": {"protocol": "hpgl", "flipY": true}})),
            },
        };
        let q = p.to_query();
        assert!(
            !q.contains(' ') && !q.contains("/dev") && !q.contains('{'),
            "{q}"
        );
        assert_eq!(JobParams::from_query(&q), p);
        assert_eq!(
            JobParams::from_query("device=&baud=x&flow=zz&junk"),
            JobParams::default()
        );
    }

    #[test]
    fn decode_is_lenient() {
        assert_eq!(decode_component("a%2Fb+c"), "a/b c");
        assert_eq!(decode_component("100%"), "100%");
        assert_eq!(decode_component("%zz%4"), "%zz%4");
    }

    #[test]
    fn host_forms() {
        assert_eq!(
            split_host(" 192.168.1.20 ", 7870),
            ("192.168.1.20".into(), 7870)
        );
        assert_eq!(
            split_host("http://ki-plotter.local:8080/", 7870),
            ("ki-plotter.local".into(), 8080)
        );
        assert_eq!(split_host("pi:abc", 7870), ("pi:abc".into(), 7870));
    }

    #[test]
    fn events_are_tagged() {
        let line = serde_json::to_string(&RelayEvent::Progress { sent: 3, total: 9 }).unwrap();
        assert_eq!(line, r#"{"type":"progress","sent":3,"total":9}"#);
        let done: RelayEvent =
            serde_json::from_str(r#"{"type":"done","ok":false,"cancelled":false,"error":"x"}"#)
                .unwrap();
        assert_eq!(
            done,
            RelayEvent::Done {
                ok: false,
                cancelled: false,
                error: Some("x".into())
            }
        );
    }

    #[test]
    fn unreachable_relay_is_a_clear_error() {
        // Nothing listens on this loopback port.
        let err = status("127.0.0.1", 1).unwrap_err();
        assert!(err.contains("Impossible de joindre"), "{err}");
        assert!(status("", 7870).unwrap_err().contains("adresse"));
    }

    #[test]
    fn discovery_without_relay_finds_nothing() {
        let t: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert!(discover_at(&[t], Duration::from_millis(100)).is_empty());
    }
}
