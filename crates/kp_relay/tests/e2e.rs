//! The desktop side (`kp_link`) talking to a real relay on loopback, with
//! the "cutter" being a file.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use kp_link::relay::{self, discover_at};
use kp_link::{Destination, Flow, SendOptions, SendOutcome, send};
use kp_relay::Config;

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

fn start(device: &str) -> SocketAddr {
    let config = Config {
        name: "Atelier test".into(),
        listen: "127.0.0.1:0".parse().unwrap(),
        device: format!("file:{device}"),
        ..Config::default()
    };
    kp_relay::spawn(config).expect("relay").addr
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

#[test]
fn job_goes_through_the_relay() {
    let dir = TempDir::new("job");
    let out = dir.file("cutter.plt");
    let addr = start(&out);

    let st = relay::status("127.0.0.1", addr.port()).unwrap();
    assert_eq!(st.name, "Atelier test");
    assert_eq!(st.device, out);
    assert!(!st.busy);

    let data = job(5000);
    let mut seen = Vec::new();
    let opts = SendOptions {
        chunk_size: 512,
        chunk_delay_ms: 1,
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
fn discovery_and_status_page() {
    let dir = TempDir::new("disc");
    let addr = start(&dir.file("cutter.plt"));
    let found = discover_at(&[addr], Duration::from_millis(500));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].name, "Atelier test");
    assert_eq!(found[0].host, "127.0.0.1");
    assert_eq!(found[0].port, addr.port());

    let mut s = TcpStream::connect(addr).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut page = String::new();
    s.read_to_string(&mut page).unwrap();
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("Atelier test") && page.contains("Prêt"));

    let mut s = TcpStream::connect(addr).unwrap();
    s.write_all(b"GET /nope HTTP/1.1\r\n\r\n").unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 404"));
}
