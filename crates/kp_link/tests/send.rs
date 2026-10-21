use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use kp_link::{Destination, Flow, SendOptions, SendOutcome, list_ports, list_printers, send};

/// Fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("kp_link_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
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

fn job(len: usize) -> Vec<u8> {
    // Something HPGL-looking, not a multiple of the chunk size.
    b"IN;PU0,0;PD100,0,100,100,0,100,0,0;"
        .iter()
        .copied()
        .cycle()
        .take(len)
        .collect()
}

#[test]
fn file_destination_end_to_end() {
    let dir = TempDir::new("file");
    let path = dir.file("job.plt");
    let data = job(1000);
    let opts = SendOptions {
        chunk_size: 64,
        chunk_delay_ms: 1,
    };
    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();

    let out = send(
        &Destination::File { path: path.clone() },
        &data,
        &opts,
        &mut |sent, total| seen.push((sent, total)),
        &cancel,
    );

    assert_eq!(out, Ok(SendOutcome::Completed));
    assert_eq!(seen.first(), Some(&(0, data.len())));
    assert_eq!(seen.last(), Some(&(data.len(), data.len())));
    assert_eq!(seen.len(), 1 + data.len().div_ceil(64));
    assert!(seen.windows(2).all(|w| w[0].0 < w[1].0));
    assert!(seen.iter().all(|&(_, total)| total == data.len()));
    assert_eq!(std::fs::read(&path).expect("read back"), data);
}

#[test]
fn cancel_from_progress_stops_after_first_chunk() {
    let dir = TempDir::new("cancel");
    let path = dir.file("job.plt");
    let data = job(1000);
    let opts = SendOptions {
        chunk_size: 100,
        chunk_delay_ms: 0,
    };
    let cancel = AtomicBool::new(false);
    let mut calls = 0;

    let out = send(
        &Destination::File { path: path.clone() },
        &data,
        &opts,
        &mut |sent, _| {
            calls += 1;
            if sent > 0 {
                cancel.store(true, Ordering::Release);
            }
        },
        &cancel,
    );

    assert_eq!(out, Ok(SendOutcome::Cancelled));
    assert_eq!(calls, 2, "progress at 0, then after the first chunk only");
    let written = std::fs::read(&path).expect("read back");
    assert!(written.len() < data.len());
    assert_eq!(written, data[..100]);
}

#[test]
fn file_in_missing_directory_is_a_french_error() {
    let dir = TempDir::new("missing");
    let path = dir.file("nope/job.plt");
    let err = send(
        &Destination::File { path },
        b"IN;",
        &SendOptions::default(),
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .expect_err("parent directory does not exist");
    assert!(err.starts_with("Impossible de créer le fichier"), "{err}");
}

#[test]
fn destination_json_round_trips() {
    let serial = r#"{"kind":"serial","port":"COM4","baud":9600,"flow":"hardware"}"#;
    let dest: Destination = serde_json::from_str(serial).expect("parse serial");
    assert_eq!(
        dest,
        Destination::Serial {
            port: "COM4".into(),
            baud: 9600,
            flow: Flow::Hardware,
        }
    );
    assert_eq!(serde_json::to_string(&dest).expect("serialize"), serial);

    let file = r#"{"kind":"file","path":"C:/x.plt"}"#;
    let dest: Destination = serde_json::from_str(file).expect("parse file");
    assert_eq!(
        dest,
        Destination::File {
            path: "C:/x.plt".into()
        }
    );
    assert_eq!(serde_json::to_string(&dest).expect("serialize"), file);

    let printer = r#"{"kind":"printer","name":"KI-870A"}"#;
    let dest: Destination = serde_json::from_str(printer).expect("parse printer");
    assert_eq!(serde_json::to_string(&dest).expect("serialize"), printer);

    for flow in ["none", "hardware", "software"] {
        let json = format!("\"{flow}\"");
        let f: Flow = serde_json::from_str(&json).expect("parse flow");
        assert_eq!(serde_json::to_string(&f).expect("serialize"), json);
    }
}

#[test]
fn send_options_json() {
    let o: SendOptions =
        serde_json::from_str(r#"{"chunkSize":512,"chunkDelayMs":10}"#).expect("parse");
    assert_eq!(
        o,
        SendOptions {
            chunk_size: 512,
            chunk_delay_ms: 10
        }
    );
    let d: SendOptions = serde_json::from_str("{}").expect("parse defaults");
    assert_eq!(d, SendOptions::default());
    assert_eq!(d.chunk_size, 256);
    assert_eq!(d.chunk_delay_ms, 0);
}

#[test]
fn listing_does_not_panic() {
    let ports = list_ports();
    let printers = list_printers();
    for p in &ports {
        println!(
            "port {p:?} -> {}",
            serde_json::to_string(p).unwrap_or_default()
        );
    }
    println!("printers: {printers:?}");
}

#[cfg(windows)]
#[test]
fn missing_serial_port_is_introuvable() {
    let dest = Destination::Serial {
        port: "COM250".into(),
        baud: 9600,
        flow: Flow::Hardware,
    };
    let mut calls = 0;
    let err = send(
        &dest,
        b"IN;",
        &SendOptions::default(),
        &mut |_, _| calls += 1,
        &AtomicBool::new(false),
    )
    .expect_err("COM250 does not exist");
    assert_eq!(
        err,
        "Le port COM250 est introuvable. Le plotter est-il branché et allumé ?"
    );
    assert_eq!(calls, 0, "no progress when the port cannot be opened");
}

#[cfg(windows)]
#[test]
fn missing_printer_is_an_error() {
    let err = send(
        &Destination::Printer {
            name: "KI Plotter - imprimante inexistante".into(),
        },
        b"IN;",
        &SendOptions::default(),
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .expect_err("no such printer");
    assert!(err.contains("KI Plotter - imprimante inexistante"), "{err}");
    println!("{err}");
}

#[cfg(not(windows))]
#[test]
fn printers_are_windows_only() {
    let err = send(
        &Destination::Printer { name: "x".into() },
        b"IN;",
        &SendOptions::default(),
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .expect_err("no printers here");
    assert_eq!(err, "Les imprimantes ne sont disponibles que sous Windows.");
    assert!(list_printers().is_empty());
}
