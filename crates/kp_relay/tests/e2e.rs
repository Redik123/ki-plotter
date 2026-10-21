//! The desktop side (`kp_link`) and the touch panel API talking to a real
//! relay on loopback, with the "cutter" being a file.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use kp_link::relay::{self, discover_at};
use kp_link::{Destination, Flow, JobInfo, SendOptions, SendOutcome, send};
use kp_relay::{Config, Mode};
use serde_json::Value;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("kp_relay_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn start_with(device: &str, mode: Mode, state: Option<PathBuf>) -> SocketAddr {
    let config = Config {
        name: "Atelier test".into(),
        listen: "127.0.0.1:0".parse().unwrap(),
        device: format!("file:{device}"),
        mode,
        state_dir: state,
        ..Config::default()
    };
    kp_relay::spawn(config).expect("relay").addr
}

fn start(device: &str) -> SocketAddr {
    start_with(device, Mode::Direct, None)
}

fn network(addr: SocketAddr) -> Destination {
    Destination::Network {
        host: "127.0.0.1".into(),
        port: addr.port(),
        device: String::new(),
        baud: 0,
        flow: None,
    }
}

fn job(len: usize) -> Vec<u8> {
    b"IN;PU0,0;PD400,0,400,400,0,400,0,0;"
        .iter()
        .copied()
        .cycle()
        .take(len)
        .collect()
}

fn named(name: &str) -> SendOptions {
    SendOptions {
        info: JobInfo {
            name: name.into(),
            width_mm: 120.0,
            height_mm: 40.0,
            estimated_seconds: 30.0,
            settings: None,
        },
        ..SendOptions::default()
    }
}

fn post(addr: SocketAddr, path: &str, body: &str) -> (u16, Value) {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let code = resp[9..12].parse().unwrap();
    let json = resp.split("\r\n\r\n").nth(1).unwrap_or("");
    (code, serde_json::from_str(json).unwrap_or(Value::Null))
}

fn get(addr: SocketAddr, path: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    resp
}

fn panel(addr: SocketAddr) -> Value {
    let resp = get(addr, "/api/state");
    serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

/// Waits until the relay is idle again, then returns its panel state.
fn wait_idle(addr: SocketAddr) -> Value {
    let start = Instant::now();
    loop {
        let st = panel(addr);
        if st["busy"] == false || start.elapsed() > Duration::from_secs(10) {
            return st;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn job_goes_through_the_relay() {
    let dir = TempDir::new("job");
    let out = dir.file("cutter.plt");
    let addr = start(&out);

    let st = relay::status("127.0.0.1", addr.port()).unwrap();
    assert_eq!(st.name, "Atelier test");
    assert_eq!(st.device, out);
    assert_eq!(st.mode, "direct");
    assert!(!st.busy);

    let data = job(5000);
    let mut seen = Vec::new();
    let opts = SendOptions {
        chunk_size: 512,
        chunk_delay_ms: 1,
        ..SendOptions::default()
    };
    let res = send(
        &network(addr),
        &data,
        &opts,
        &mut |s, t| seen.push((s, t)),
        &AtomicBool::new(false),
    );
    assert_eq!(res, Ok(SendOutcome::Completed));
    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert_eq!(seen.first(), Some(&(0, data.len())));
    assert_eq!(seen.last(), Some(&(data.len(), data.len())));
    assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "{seen:?}");

    let last = relay::status("127.0.0.1", addr.port())
        .unwrap()
        .last
        .unwrap();
    assert!(last.ok && last.bytes == data.len() && last.client == "127.0.0.1");
}

#[test]
fn busy_relay_refuses_and_cancel_stops() {
    let dir = TempDir::new("busy");
    let addr = start(&dir.file("cutter.plt"));
    // ~3 s at 16 bytes every 10 ms.
    let slow = SendOptions {
        chunk_size: 16,
        chunk_delay_ms: 10,
        ..SendOptions::default()
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&cancel);
    let dest = network(addr);
    let first = thread::spawn(move || send(&dest, &job(5000), &slow, &mut |_, _| {}, &c));
    thread::sleep(Duration::from_millis(400));

    let second = send(
        &network(addr),
        b"IN;",
        &SendOptions::default(),
        &mut |_, _| {},
        &AtomicBool::new(false),
    );
    assert!(second.unwrap_err().contains("déjà"));
    assert!(relay::status("127.0.0.1", addr.port()).unwrap().busy);

    cancel.store(true, Ordering::Release);
    assert_eq!(first.join().unwrap(), Ok(SendOutcome::Cancelled));
    let st = relay::status("127.0.0.1", addr.port()).unwrap();
    assert!(!st.busy && st.last.unwrap().cancelled);
}

#[test]
fn relay_errors_reach_the_desktop() {
    let dir = TempDir::new("err");
    let addr = start(&dir.file("cutter.plt"));
    let dest = Destination::Network {
        host: "127.0.0.1".into(),
        port: addr.port(),
        device: format!("file:{}", dir.file("pas/de/dossier/x.plt")),
        baud: 9600,
        flow: Some(Flow::None),
    };
    let err = send(
        &dest,
        b"IN;",
        &SendOptions::default(),
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(err.contains("Impossible de créer le fichier"), "{err}");
}

#[test]
fn discovery_and_panel_page() {
    let dir = TempDir::new("disc");
    let addr = start(&dir.file("cutter.plt"));
    let found = discover_at(&[addr], Duration::from_millis(500));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].name, "Atelier test");
    assert_eq!(found[0].host, "127.0.0.1");
    assert_eq!(found[0].port, addr.port());

    let page = get(addr, "/");
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("Coupe test") && page.contains("/api/state"));
    assert!(get(addr, "/nope").starts_with("HTTP/1.1 404"));
    assert!(get(addr, "/api/start").starts_with("HTTP/1.1 405"));
}

#[test]
fn queue_start_delete_recut_and_test() {
    let dir = TempDir::new("queue");
    let out = dir.file("cutter.plt");
    let addr = start_with(&out, Mode::Queue, None);
    let flag = AtomicBool::new(false);

    let a = job(900);
    let b = job(1300);
    assert_eq!(
        send(&network(addr), &a, &named("Logo A"), &mut |_, _| {}, &flag),
        Ok(SendOutcome::Queued { position: 1 })
    );
    assert_eq!(
        send(&network(addr), &b, &named("Logo B"), &mut |_, _| {}, &flag),
        Ok(SendOutcome::Queued { position: 2 })
    );
    // Nothing is cut until someone starts it on the panel.
    assert!(std::fs::metadata(&out).is_err());
    let st = panel(addr);
    assert_eq!(st["queue"].as_array().unwrap().len(), 2);
    assert_eq!(st["queue"][0]["name"], "Logo A");
    assert_eq!(st["queue"][0]["widthMm"], 120.0);
    let id_b = st["queue"][1]["id"].as_u64().unwrap();
    assert_eq!(relay::status("127.0.0.1", addr.port()).unwrap().queued, 2);

    // Start the next one (no id): A.
    assert_eq!(post(addr, "/api/start", "").0, 200);
    let st = wait_idle(addr);
    assert_eq!(std::fs::read(&out).unwrap(), a);
    assert_eq!(st["recent"]["name"], "Logo A");
    assert_eq!(st["recent"]["ok"], true);
    assert_eq!(st["recut"]["name"], "Logo A");
    assert_eq!(st["queue"].as_array().unwrap().len(), 1);

    // Delete B, then a start on it is refused.
    assert_eq!(post(addr, &format!("/api/delete?id={id_b}"), "").0, 200);
    assert_eq!(post(addr, &format!("/api/start?id={id_b}"), "").0, 404);
    assert_eq!(post(addr, "/api/start", "").0, 404);

    // Recut A.
    std::fs::remove_file(&out).unwrap();
    assert_eq!(post(addr, "/api/recut", "").0, 200);
    wait_idle(addr);
    assert_eq!(std::fs::read(&out).unwrap(), a);

    // Test cut, with no settings from the PC yet: HPGL defaults.
    assert_eq!(post(addr, "/api/test", "").0, 200);
    let st = wait_idle(addr);
    assert!(std::fs::read_to_string(&out).unwrap().starts_with("IN;"));
    assert_eq!(st["recent"]["test"], true);
    // A test cut never replaces the job "recut" would cut.
    assert_eq!(st["recut"]["name"], "Logo A");

    // A job carrying the PC's settings makes the next test cut use them.
    let mut opts = named("DMPL");
    opts.info.settings = Some(serde_json::json!({"machine": {"protocol": "dmpl"}}));
    send(&network(addr), b"x", &opts, &mut |_, _| {}, &flag).unwrap();
    assert_eq!(post(addr, "/api/test", "").0, 200);
    wait_idle(addr);
    assert!(std::fs::read_to_string(&out).unwrap().starts_with(";:"));
}

#[test]
fn queue_and_settings_survive_a_restart() {
    let dir = TempDir::new("persist");
    let state = dir.0.join("state");
    let out = dir.file("cutter.plt");
    let addr = start_with(&out, Mode::Queue, Some(state.clone()));
    let flag = AtomicBool::new(false);
    for name in ["Un", "Deux"] {
        send(
            &network(addr),
            &job(700),
            &named(name),
            &mut |_, _| {},
            &flag,
        )
        .unwrap();
    }
    let settings =
        r#"{"name":"Atelier","mode":"direct","device":"auto","baud":38400,"flow":"none"}"#;
    assert_eq!(post(addr, "/api/settings", settings).0, 200);
    assert_eq!(post(addr, "/api/settings", "pas du json").0, 400);

    // "Power cut": a new relay on the same state directory.
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        state_dir: Some(state),
        // The command line still wins over what the panel saved.
        cli_overrides: vec![("device", format!("file:{out}"))],
        ..Config::default()
    };
    let again = kp_relay::spawn(config).unwrap().addr;
    let st = panel(again);
    let names: Vec<&str> = st["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Un", "Deux"]);
    assert_eq!(st["name"], "Atelier");
    assert_eq!(st["settings"]["mode"], "direct");
    assert_eq!(st["settings"]["baud"], 38400);
    assert_eq!(st["settings"]["device"], format!("file:{out}"));
    // New ids continue after the restored ones.
    send(
        &network(again),
        b"IN;",
        &named("Trois"),
        &mut |_, _| {},
        &flag,
    )
    .unwrap();
    let st = wait_idle(again);
    let max_queued = st["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["id"].as_u64().unwrap())
        .max()
        .unwrap();
    assert!(st["recent"]["id"].as_u64().unwrap() > max_queued);
}
