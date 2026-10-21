//! KI Plotter relay: runs on the Raspberry Pi wired to the cutter. The
//! desktop app builds the job (HPGL/DMPL) and posts it here; the relay
//! sends it to the cutter with `kp_link` and streams the progress back.
//! Protocol: see [`kp_link::relay`].

use std::io::{self, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kp_link::relay::{DISCOVER_MAGIC, JobParams, LastJob, RelayEvent, RelayInfo, RelayStatus};
use kp_link::{Destination, Flow, PortKind, SendOptions, SendOutcome};

pub mod config;
mod http;
mod page;

pub use config::Config;

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Shared {
    config: Config,
    /// TCP port actually bound (the config may say 0 in tests).
    port: u16,
    busy: AtomicBool,
    cancel: AtomicBool,
    progress: Mutex<(usize, usize)>,
    last: Mutex<Option<LastJob>>,
}

/// A running relay (threads keep serving until the process exits).
pub struct Relay {
    pub addr: SocketAddr,
}

/// Binds the HTTP port (and the discovery port, best effort) and serves in
/// background threads.
pub fn spawn(config: Config) -> io::Result<Relay> {
    let listener = TcpListener::bind(config.listen)?;
    let addr = listener.local_addr()?;
    let shared = Arc::new(Shared {
        config,
        port: addr.port(),
        busy: AtomicBool::new(false),
        cancel: AtomicBool::new(false),
        progress: Mutex::new((0, 0)),
        last: Mutex::new(None),
    });
    match UdpSocket::bind(SocketAddr::new(addr.ip(), addr.port())) {
        Ok(sock) => {
            let s = Arc::clone(&shared);
            thread::spawn(move || discovery_loop(&sock, &s));
        }
        Err(e) => eprintln!("Découverte automatique désactivée (UDP {addr}) : {e}"),
    }
    thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let s = Arc::clone(&shared);
                    thread::spawn(move || handle(&s, stream));
                }
                Err(e) => eprintln!("Connexion refusée : {e}"),
            }
        }
    });
    Ok(Relay { addr })
}

/// Serves forever.
pub fn run(config: Config) -> io::Result<()> {
    spawn(config)?;
    loop {
        thread::park();
    }
}

fn discovery_loop(sock: &UdpSocket, shared: &Shared) {
    let mut buf = [0u8; 256];
    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if &buf[..n] == DISCOVER_MAGIC => {
                let info = RelayInfo {
                    name: shared.config.name.clone(),
                    host: String::new(),
                    port: shared.port,
                    version: VERSION.into(),
                };
                if let Ok(reply) = serde_json::to_vec(&info) {
                    let _ = sock.send_to(&reply, from);
                }
            }
            Ok(_) => {}
            // e.g. ConnectionReset on Windows after an ICMP error: keep going.
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn status(&self) -> RelayStatus {
        let (sent, total) = *lock(&self.progress);
        let (device, device_error) =
            match resolve(&self.config.device, self.config.baud, self.config.flow) {
                Ok(d) => (describe(&d), None),
                Err(e) => (String::new(), Some(e)),
            };
        RelayStatus {
            name: self.config.name.clone(),
            version: VERSION.into(),
            busy: self.busy.load(Ordering::Acquire),
            sent,
            total,
            device,
            device_error,
            ports: kp_link::list_ports(),
            last: lock(&self.last).clone(),
        }
    }

    /// The job's destination: its own settings, else the configuration.
    fn destination(&self, p: &JobParams) -> Result<Destination, String> {
        let spec = if p.device.trim().is_empty() {
            self.config.device.as_str()
        } else {
            p.device.trim()
        };
        let baud = if p.baud > 0 { p.baud } else { self.config.baud };
        resolve(spec, baud, p.flow.unwrap_or(self.config.flow))
    }
}

/// `auto`, a tty path, a USB-printer node (`/dev/usb/lp0`) or `file:<path>`.
fn resolve(spec: &str, baud: u32, flow: Flow) -> Result<Destination, String> {
    let spec = spec.trim();
    if let Some(path) = spec.strip_prefix("file:") {
        return Ok(Destination::File { path: path.into() });
    }
    if spec.is_empty() || spec.eq_ignore_ascii_case("auto") {
        return auto_device(baud, flow);
    }
    if spec.starts_with("/dev/usb/lp") {
        return Ok(Destination::File { path: spec.into() });
    }
    Ok(Destination::Serial {
        port: spec.into(),
        baud,
        flow,
    })
}

/// First USB serial port, else a USB printer node.
fn auto_device(baud: u32, flow: Flow) -> Result<Destination, String> {
    if let Some(p) = kp_link::list_ports()
        .into_iter()
        .find(|p| p.kind == PortKind::Usb)
    {
        return Ok(Destination::Serial {
            port: p.name,
            baud,
            flow,
        });
    }
    for i in 0..4 {
        let lp = format!("/dev/usb/lp{i}");
        if Path::new(&lp).exists() {
            return Ok(Destination::File { path: lp });
        }
    }
    Err("Aucun plotter détecté sur le relais : branchez-le en USB et allumez-le.".into())
}

fn describe(d: &Destination) -> String {
    match d {
        Destination::Serial { port, .. } => port.clone(),
        Destination::File { path } => path.clone(),
        _ => String::new(),
    }
}

fn handle(shared: &Shared, stream: TcpStream) {
    let peer = stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut out = stream;
    let req = match http::read_request(&mut BufReader::new(read_half), shared.config.max_job_bytes)
    {
        Ok(r) => r,
        Err(http::ReadError::Respond(code, msg)) => {
            let _ = http::respond_error(&mut out, code, &msg);
            return;
        }
        Err(http::ReadError::Gone) => return,
    };
    let _ = match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/status") => http::respond_json(&mut out, 200, &shared.status()),
        ("GET", "/") => http::respond(
            &mut out,
            200,
            "text/html; charset=utf-8",
            page::render(&shared.status()).as_bytes(),
        ),
        ("POST", "/job") => {
            run_job(shared, &mut out, &req, &peer);
            Ok(())
        }
        ("POST", "/cancel") => {
            if shared.busy.load(Ordering::Acquire) {
                shared.cancel.store(true, Ordering::Release);
                eprintln!("Annulation demandée par {peer}");
            }
            http::respond_json(&mut out, 200, &serde_json::json!({}))
        }
        (_, "/" | "/status" | "/job" | "/cancel") => {
            http::respond_error(&mut out, 405, "Méthode non autorisée.")
        }
        _ => http::respond_error(&mut out, 404, "Introuvable."),
    };
}

fn write_event(out: &mut TcpStream, e: &RelayEvent) -> io::Result<()> {
    let mut line = serde_json::to_vec(e).unwrap_or_default();
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

/// Clears `busy` when the job ends, however it ends.
struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn run_job(shared: &Shared, out: &mut TcpStream, req: &http::Request, peer: &str) {
    if shared.busy.swap(true, Ordering::AcqRel) {
        let _ = http::respond_error(
            out,
            409,
            "Le plotter est déjà en train de recevoir un travail. Réessayez quand il aura fini.",
        );
        return;
    }
    let _busy = BusyGuard(&shared.busy);
    shared.cancel.store(false, Ordering::Release);
    let params = JobParams::from_query(&req.query);
    let total = req.body.len();
    let record = |ok: bool, cancelled: bool, error: Option<String>| {
        *lock(&shared.last) = Some(LastJob {
            ok,
            cancelled,
            error,
            bytes: total,
            client: peer.to_owned(),
        });
    };
    let dest = match shared.destination(&params) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Travail refusé ({peer}) : {e}");
            record(false, false, Some(e.clone()));
            let _ = http::respond_error(out, 503, &e);
            return;
        }
    };
    // If we can't even answer, the sender is gone: don't cut behind its back.
    if http::start_stream(out, "application/x-ndjson").is_err() {
        return;
    }
    eprintln!(
        "Travail de {total} octets depuis {peer} → {}",
        describe(&dest)
    );
    *lock(&shared.progress) = (0, total);
    // A sender that gives its block size also gives its pause (0 is omitted
    // from the query), so both come from the same side.
    let opts = SendOptions {
        chunk_size: if params.chunk_size > 0 {
            params.chunk_size
        } else {
            shared.config.chunk_size
        },
        chunk_delay_ms: if params.chunk_size > 0 {
            params.chunk_delay_ms
        } else {
            shared.config.chunk_delay_ms
        },
    };
    // The job goes on even if the sender stops listening (PC asleep, Wi-Fi):
    // progress lines are best effort.
    let mut client_ok = true;
    let mut last_line: Option<Instant> = None;
    let result = kp_link::send(
        &dest,
        &req.body,
        &opts,
        &mut |sent, total| {
            *lock(&shared.progress) = (sent, total);
            let due = last_line.is_none_or(|t| t.elapsed() >= Duration::from_millis(100));
            if client_ok && (due || sent == total) {
                last_line = Some(Instant::now());
                client_ok = write_event(out, &RelayEvent::Progress { sent, total }).is_ok();
            }
        },
        &shared.cancel,
    );
    let (ok, cancelled, error) = match result {
        Ok(SendOutcome::Completed) => (true, false, None),
        Ok(SendOutcome::Cancelled) => (false, true, None),
        Err(e) => (false, false, Some(e)),
    };
    match &error {
        Some(e) => eprintln!("Échec : {e}"),
        None if cancelled => eprintln!("Travail annulé."),
        None => eprintln!("Travail envoyé."),
    }
    record(ok, cancelled, error.clone());
    if client_ok {
        let _ = write_event(
            out,
            &RelayEvent::Done {
                ok,
                cancelled,
                error,
            },
        );
    }
}
