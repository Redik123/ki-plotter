//! KI Plotter relay: runs on the Raspberry Pi wired to the cutter. The
//! desktop app builds the job (HPGL/DMPL) and posts it here. In queue mode
//! the job waits until someone starts it on the relay's touch panel; in
//! direct mode it is cut right away and the progress streams back.
//! Protocol: see [`kp_link::relay`]; panel API: `/api/…` below.
//!
//! One worker thread does all the cutting, one job at a time. HTTP threads
//! only change the shared [`State`] and wake it.

use std::collections::VecDeque;
use std::io::{self, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kp_link::relay::{DISCOVER_MAGIC, JobParams, LastJob, RelayEvent, RelayInfo, RelayStatus};
use kp_link::{Destination, Flow, PortInfo, PortKind, SendOptions, SendOutcome};
use serde::Serialize;

pub mod config;
mod http;
mod job;
mod store;

pub use config::{Config, Mode, PanelSettings};
use job::{Job, JobView};
use store::Store;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PANEL: &str = include_str!("panel.html");
const SETTINGS_FILE: &str = "settings.conf";
const PC_SETTINGS_FILE: &str = "pc-settings.json";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the worker is asked to cut next.
enum Request {
    Queued(u64),
    Recut,
    Test,
}

struct Running {
    job: Job,
    sent: usize,
    total: usize,
}

#[derive(Clone)]
struct Finished {
    job: Job,
    /// Unix time it ended.
    at: u64,
    ok: bool,
    cancelled: bool,
    error: Option<String>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    queue: VecDeque<Job>,
    current: Option<Running>,
    /// Taken by the worker; while set, nothing else may start.
    request: Option<Request>,
    /// Last finished job, test cuts included (what the panel reports).
    recent: Option<Finished>,
    /// Last real job, for "recut".
    last_job: Option<Job>,
    /// The desktop's settings from its last job, for the relay's test cut.
    pc_settings: Option<serde_json::Value>,
}

impl State {
    fn busy(&self) -> bool {
        self.current.is_some() || self.request.is_some()
    }

    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

struct Engine {
    config: Mutex<Config>,
    /// TCP port actually bound (the config may say 0 in tests).
    port: u16,
    state: Mutex<State>,
    wake: Condvar,
    cancel: AtomicBool,
    store: Store,
}

type ApiError = (u16, String);

fn busy_error() -> ApiError {
    (
        409,
        "Une découpe est déjà en cours. Attendez qu'elle finisse ou arrêtez-la.".into(),
    )
}

/// A running relay (threads keep serving until the process exits).
pub struct Relay {
    pub addr: SocketAddr,
}

/// Binds the HTTP port (and the discovery port, best effort), restores the
/// saved queue and settings, and serves in background threads.
pub fn spawn(mut config: Config) -> io::Result<Relay> {
    let store = Store::new(config.state_dir.clone());
    if let Some(text) = store.load_text(SETTINGS_FILE)
        && let Err(e) = config.load_file(&text)
    {
        eprintln!("Réglages de l'écran ignorés ({SETTINGS_FILE}) : {e}");
    }
    for (k, v) in config.cli_overrides.clone() {
        let _ = config.set(k, &v);
    }
    let listener = TcpListener::bind(config.listen)?;
    let addr = listener.local_addr()?;

    let mut state = State {
        queue: store.load_queue().into(),
        last_job: store.load_last(),
        pc_settings: store
            .load_text(PC_SETTINGS_FILE)
            .and_then(|t| serde_json::from_str(&t).ok()),
        ..State::default()
    };
    state.next_id = state
        .queue
        .iter()
        .chain(state.last_job.iter())
        .map(|j| j.id)
        .max()
        .unwrap_or(0);
    if !state.queue.is_empty() {
        eprintln!("{} travail(s) en attente retrouvé(s).", state.queue.len());
    }

    let engine = Arc::new(Engine {
        config: Mutex::new(config),
        port: addr.port(),
        state: Mutex::new(state),
        wake: Condvar::new(),
        cancel: AtomicBool::new(false),
        store,
    });
    match UdpSocket::bind(SocketAddr::new(addr.ip(), addr.port())) {
        Ok(sock) => {
            let e = Arc::clone(&engine);
            thread::spawn(move || discovery_loop(&sock, &e));
        }
        Err(err) => eprintln!("Découverte automatique désactivée (UDP {addr}) : {err}"),
    }
    let e = Arc::clone(&engine);
    thread::spawn(move || e.worker());
    thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let e = Arc::clone(&engine);
                    thread::spawn(move || handle(&e, stream));
                }
                Err(err) => eprintln!("Connexion refusée : {err}"),
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

fn discovery_loop(sock: &UdpSocket, engine: &Engine) {
    let mut buf = [0u8; 256];
    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if &buf[..n] == DISCOVER_MAGIC => {
                let info = RelayInfo {
                    name: lock(&engine.config).name.clone(),
                    host: String::new(),
                    port: engine.port,
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
    Err("Aucun plotter détecté : branchez-le en USB sur le relais et allumez-le.".into())
}

fn describe(d: &Destination) -> String {
    match d {
        Destination::Serial { port, .. } => port.clone(),
        Destination::File { path } => path.clone(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- engine

impl Engine {
    fn wait_for_job(&self) -> Job {
        let mut st = lock(&self.state);
        loop {
            if let Some(req) = st.request.take() {
                let job = match req {
                    Request::Queued(id) => st
                        .queue
                        .iter()
                        .position(|j| j.id == id)
                        .and_then(|p| st.queue.remove(p)),
                    Request::Recut => st.last_job.clone().map(|mut j| {
                        j.id = st.take_id();
                        j.received = now();
                        j
                    }),
                    Request::Test => match job::test_cut(st.pc_settings.as_ref()) {
                        Ok(mut j) => {
                            j.id = st.take_id();
                            j.received = now();
                            Some(j)
                        }
                        Err(e) => {
                            eprintln!("Coupe test impossible : {e}");
                            st.recent = Some(Finished {
                                job: Job {
                                    name: "Coupe test 25 mm".into(),
                                    test: true,
                                    received: now(),
                                    ..Job::default()
                                },
                                at: now(),
                                ok: false,
                                cancelled: false,
                                error: Some(e),
                            });
                            None
                        }
                    },
                };
                if let Some(job) = job {
                    // Set under the same lock as `request.take()`: never "not busy" in between.
                    st.current = Some(Running {
                        total: job.bytes.len(),
                        sent: 0,
                        job: job.clone(),
                    });
                    return job;
                }
                continue;
            }
            st = self.wake.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn worker(&self) {
        loop {
            let job = self.wait_for_job();
            self.store.remove_queued(job.id);
            self.cancel.store(false, Ordering::Release);
            let (ok, cancelled, error) = match self.cut(&job) {
                Ok(SendOutcome::Completed | SendOutcome::Queued { .. }) => (true, false, None),
                Ok(SendOutcome::Cancelled) => (false, true, None),
                Err(e) => (false, false, Some(e)),
            };
            match &error {
                Some(e) => eprintln!("Échec de « {} » : {e}", job.name),
                None if cancelled => eprintln!("« {} » arrêté.", job.name),
                None => eprintln!("« {} » découpé.", job.name),
            }
            if !job.test {
                self.store.save_last(&job);
            }
            let mut st = lock(&self.state);
            st.current = None;
            if !job.test {
                st.last_job = Some(job.clone());
            }
            st.recent = Some(Finished {
                job,
                at: now(),
                ok,
                cancelled,
                error,
            });
        }
    }

    fn cut(&self, job: &Job) -> Result<SendOutcome, String> {
        let cfg = lock(&self.config).clone();
        let spec = if job.device.trim().is_empty() {
            cfg.device.as_str()
        } else {
            job.device.as_str()
        };
        let baud = if job.baud > 0 { job.baud } else { cfg.baud };
        let dest = resolve(spec, baud, job.flow.unwrap_or(cfg.flow))?;
        // A sender that gives its block size also gives its pause (0 is
        // omitted from the query), so both come from the same side.
        let (chunk_size, chunk_delay_ms) = if job.chunk_size > 0 {
            (job.chunk_size, job.chunk_delay_ms)
        } else {
            (cfg.chunk_size, cfg.chunk_delay_ms)
        };
        let opts = SendOptions {
            chunk_size,
            chunk_delay_ms,
            ..SendOptions::default()
        };
        eprintln!(
            "Découpe de « {} » ({} octets) → {}",
            job.name,
            job.bytes.len(),
            describe(&dest)
        );
        kp_link::send(
            &dest,
            &job.bytes,
            &opts,
            &mut |sent, total| {
                if let Some(r) = &mut lock(&self.state).current {
                    r.sent = sent;
                    r.total = total;
                }
            },
            &self.cancel,
        )
    }

    fn submit(&self, req: Request) -> Result<(), ApiError> {
        let mut st = lock(&self.state);
        if st.busy() {
            return Err(busy_error());
        }
        match &req {
            Request::Queued(id) if !st.queue.iter().any(|j| j.id == *id) => {
                return Err((404, "Ce travail n'est plus dans la file.".into()));
            }
            Request::Recut if st.last_job.is_none() => {
                return Err((404, "Aucun travail à recouper pour l'instant.".into()));
            }
            _ => {}
        }
        st.request = Some(req);
        self.wake.notify_all();
        Ok(())
    }

    /// Adds a job to the queue; with `run`, also starts it (direct mode),
    /// atomically: refused if something is already cutting.
    fn enqueue(&self, mut job: Job, run: bool) -> Result<(u64, usize), ApiError> {
        let mut st = lock(&self.state);
        if run && st.busy() {
            return Err(busy_error());
        }
        job.id = st.take_id();
        let id = job.id;
        self.store.add_queued(&job);
        st.queue.push_back(job);
        let position = st.queue.len();
        if run {
            st.request = Some(Request::Queued(id));
            self.wake.notify_all();
        }
        Ok((id, position))
    }

    fn delete(&self, id: u64) -> Result<(), ApiError> {
        let mut st = lock(&self.state);
        let pos = st
            .queue
            .iter()
            .position(|j| j.id == id)
            .ok_or((404, "Ce travail n'est plus dans la file.".to_string()))?;
        st.queue.remove(pos);
        self.store.remove_queued(id);
        Ok(())
    }

    fn cancel(&self) {
        if lock(&self.state).current.is_some() {
            self.cancel.store(true, Ordering::Release);
        }
    }

    fn save_settings(&self, s: &PanelSettings) -> Result<(), ApiError> {
        let mut cfg = lock(&self.config);
        cfg.apply(s);
        self.store
            .save_text(SETTINGS_FILE, &cfg.panel_settings().to_conf())
            .map_err(|e| (500, e))?;
        eprintln!(
            "Réglages modifiés depuis l'écran : mode {}, plotter {}, {} bauds, flux {:?}",
            cfg.mode.name(),
            cfg.device,
            cfg.baud,
            cfg.flow
        );
        Ok(())
    }

    fn default_device(&self) -> (String, Option<String>) {
        let cfg = lock(&self.config).clone();
        match resolve(&cfg.device, cfg.baud, cfg.flow) {
            Ok(d) => (describe(&d), None),
            Err(e) => (String::new(), Some(e)),
        }
    }

    /// `GET /status`, for the desktop app.
    fn status(&self) -> RelayStatus {
        let (device, device_error) = self.default_device();
        let cfg = lock(&self.config).clone();
        let st = lock(&self.state);
        let (sent, total) = st.current.as_ref().map_or((0, 0), |r| (r.sent, r.total));
        RelayStatus {
            name: cfg.name,
            version: VERSION.into(),
            busy: st.busy(),
            sent,
            total,
            device,
            device_error,
            ports: kp_link::list_ports(),
            last: st.recent.as_ref().map(|f| LastJob {
                ok: f.ok,
                cancelled: f.cancelled,
                error: f.error.clone(),
                bytes: f.job.bytes.len(),
                client: f.job.client.clone(),
            }),
            mode: cfg.mode.name().into(),
            queued: st.queue.len(),
        }
    }

    /// `GET /api/state`, for the touch panel.
    fn panel_state(&self) -> PanelState {
        let (device, device_error) = self.default_device();
        let cfg = lock(&self.config).clone();
        let st = lock(&self.state);
        PanelState {
            name: cfg.name.clone(),
            version: VERSION,
            settings: cfg.panel_settings(),
            device,
            device_error,
            ports: kp_link::list_ports(),
            busy: st.busy(),
            current: st.current.as_ref().map(|r| RunningView {
                job: r.job.view(),
                sent: r.sent,
                total: r.total,
            }),
            queue: st.queue.iter().map(Job::view).collect(),
            recent: st.recent.as_ref().map(|f| FinishedView {
                job: f.job.view(),
                finished_at: f.at,
                ok: f.ok,
                cancelled: f.cancelled,
                error: f.error.clone(),
            }),
            recut: st.last_job.as_ref().map(Job::view),
            now: now(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunningView {
    #[serde(flatten)]
    job: JobView,
    sent: usize,
    total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FinishedView {
    #[serde(flatten)]
    job: JobView,
    finished_at: u64,
    ok: bool,
    cancelled: bool,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelState {
    name: String,
    version: &'static str,
    settings: PanelSettings,
    device: String,
    device_error: Option<String>,
    ports: Vec<PortInfo>,
    busy: bool,
    current: Option<RunningView>,
    queue: Vec<JobView>,
    recent: Option<FinishedView>,
    /// The job "recut" would cut again.
    recut: Option<JobView>,
    now: u64,
}

// ---------------------------------------------------------------- HTTP

fn query_id(query: &str) -> Option<u64> {
    kp_link::relay::parse_query(query)
        .into_iter()
        .find(|(k, _)| k == "id")
        .and_then(|(_, v)| v.parse().ok())
}

fn ack(out: &mut TcpStream, r: Result<(), ApiError>) -> io::Result<()> {
    match r {
        Ok(()) => http::respond_json(out, 200, &serde_json::json!({ "ok": true })),
        Err((code, msg)) => http::respond_error(out, code, &msg),
    }
}

fn handle(engine: &Engine, stream: TcpStream) {
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
    let max = lock(&engine.config).max_job_bytes;
    let req = match http::read_request(&mut BufReader::new(read_half), max) {
        Ok(r) => r,
        Err(http::ReadError::Respond(code, msg)) => {
            let _ = http::respond_error(&mut out, code, &msg);
            return;
        }
        Err(http::ReadError::Gone) => return,
    };
    let o = &mut out;
    let _ = match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/" | "/index.html") => {
            http::respond(o, 200, "text/html; charset=utf-8", PANEL.as_bytes())
        }
        ("GET", "/status") => http::respond_json(o, 200, &engine.status()),
        ("GET", "/api/state") => http::respond_json(o, 200, &engine.panel_state()),
        ("POST", "/job") => {
            receive_job(engine, o, &req, &peer);
            Ok(())
        }
        ("POST", "/cancel" | "/api/cancel") => {
            engine.cancel();
            ack(o, Ok(()))
        }
        ("POST", "/api/start") => {
            let id =
                query_id(&req.query).or_else(|| lock(&engine.state).queue.front().map(|j| j.id));
            match id {
                Some(id) => ack(o, engine.submit(Request::Queued(id))),
                None => ack(o, Err((404, "Aucun travail en attente.".into()))),
            }
        }
        ("POST", "/api/delete") => match query_id(&req.query) {
            Some(id) => ack(o, engine.delete(id)),
            None => ack(o, Err((400, "Travail non précisé.".into()))),
        },
        ("POST", "/api/recut") => ack(o, engine.submit(Request::Recut)),
        ("POST", "/api/test") => ack(o, engine.submit(Request::Test)),
        ("POST", "/api/settings") => match serde_json::from_slice::<PanelSettings>(&req.body) {
            Ok(s) => ack(o, engine.save_settings(&s)),
            Err(_) => ack(o, Err((400, "Réglages invalides.".into()))),
        },
        (_, "/" | "/status" | "/job" | "/cancel") => {
            http::respond_error(o, 405, "Méthode non autorisée.")
        }
        (_, p) if p.starts_with("/api/") => http::respond_error(o, 405, "Méthode non autorisée."),
        _ => http::respond_error(o, 404, "Introuvable."),
    };
}

fn write_event(out: &mut TcpStream, e: &RelayEvent) -> io::Result<()> {
    let mut line = serde_json::to_vec(e).unwrap_or_default();
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

fn receive_job(engine: &Engine, out: &mut TcpStream, req: &http::Request, peer: &str) {
    let p = JobParams::from_query(&req.query);
    if let Some(s) = &p.info.settings {
        let _ = engine.store.save_text(PC_SETTINGS_FILE, &s.to_string());
        lock(&engine.state).pc_settings = Some(s.clone());
    }
    let job = Job {
        name: if p.info.name.trim().is_empty() {
            format!("Travail de {peer}")
        } else {
            p.info.name.trim().to_owned()
        },
        width_mm: p.info.width_mm,
        height_mm: p.info.height_mm,
        estimated_seconds: p.info.estimated_seconds,
        received: now(),
        client: peer.to_owned(),
        test: false,
        device: p.device,
        baud: p.baud,
        flow: p.flow,
        chunk_size: p.chunk_size,
        chunk_delay_ms: p.chunk_delay_ms,
        bytes: Arc::new(req.body.clone()),
        id: 0,
    };
    let name = job.name.clone();
    let total = job.bytes.len();
    let mode = lock(&engine.config).mode;
    if mode == Mode::Queue {
        let Ok((id, position)) = engine.enqueue(job, false) else {
            return;
        };
        eprintln!("« {name} » reçu de {peer}, en attente (position {position}).");
        if http::start_stream(out, "application/x-ndjson").is_ok() {
            let _ = write_event(out, &RelayEvent::Queued { id, position });
        }
        return;
    }

    // Direct mode: cut now and stream the progress until it ends.
    let id = match engine.enqueue(job, true) {
        Ok((id, _)) => id,
        Err((code, msg)) => {
            let _ = http::respond_error(out, code, &msg);
            return;
        }
    };
    eprintln!("« {name} » reçu de {peer}, découpe directe.");
    if http::start_stream(out, "application/x-ndjson").is_err() {
        // The job goes on even if the sender stopped listening.
        return;
    }
    let mut last_sent = usize::MAX;
    loop {
        thread::sleep(Duration::from_millis(100));
        let event = {
            let st = lock(&engine.state);
            match (&st.current, &st.recent) {
                (Some(r), _) if r.job.id == id => {
                    (r.sent != last_sent).then_some(RelayEvent::Progress {
                        sent: r.sent,
                        total: r.total,
                    })
                }
                (_, Some(f)) if f.job.id == id => Some(RelayEvent::Done {
                    ok: f.ok,
                    cancelled: f.cancelled,
                    error: f.error.clone(),
                }),
                // Deleted on the panel before the worker picked it up.
                _ if st.request.is_none() && !st.queue.iter().any(|j| j.id == id) => {
                    Some(RelayEvent::Done {
                        ok: false,
                        cancelled: true,
                        error: None,
                    })
                }
                // Still waiting for the worker to pick it up.
                _ => None,
            }
        };
        let Some(event) = event else { continue };
        let done = matches!(event, RelayEvent::Done { .. });
        if let RelayEvent::Progress { sent, .. } = event {
            last_sent = sent;
        }
        // A short job can finish between two polls: report 100 % before "done".
        if let RelayEvent::Done { ok: true, .. } = event
            && last_sent != total
            && write_event(out, &RelayEvent::Progress { sent: total, total }).is_err()
        {
            return;
        }
        if write_event(out, &event).is_err() || done {
            return;
        }
    }
}
