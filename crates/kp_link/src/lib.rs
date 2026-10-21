//! Output destinations for cut jobs. The job itself (HPGL/DMPL bytes) is
//! built elsewhere; this crate only moves bytes to the cutter.
//!
//! Every destination is driven by the same chunk loop (see [`send`]): the
//! bytes go out in blocks of [`SendOptions::chunk_size`], progress is reported
//! after each block and cancellation is checked between blocks.
//!
//! No Tauri dependency on purpose: the same crate runs inside the small
//! relay service on the Raspberry Pi wired to the cutter (Linux), see
//! [`relay`]. Windows-only code (winspool printers) is gated with `cfg(windows)`.

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

mod file;
mod natural;
mod printer;
pub mod relay;
mod serial;
mod sink;

pub use serial::STALL_TIMEOUT;

/// Where a job is sent. Mirrors `Destination` in `src/model/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Destination {
    /// Serial port (`COM4` on Windows, `/dev/ttyUSB0` on Linux), 8N1.
    Serial { port: String, baud: u32, flow: Flow },
    /// Windows printer queue, job sent as RAW bytes (cutter installed as a
    /// USB printer, typically with the "Generic / Text Only" driver).
    Printer { name: String },
    /// Plain file (also usable for a raw device node such as `/dev/usb/lp0`).
    File { path: String },
    /// KI Plotter relay on the LAN (Raspberry Pi wired to the cutter).
    /// `device` empty or "auto", `baud` 0 and no `flow` fall back to the
    /// relay's own configuration.
    Network {
        host: String,
        #[serde(default = "relay_port")]
        port: u16,
        #[serde(default)]
        device: String,
        #[serde(default)]
        baud: u32,
        #[serde(default)]
        flow: Option<Flow>,
    },
}

/// Serial flow control. Mirrors `Flow` in `src/model/types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flow {
    #[default]
    None,
    /// RTS/CTS: the cutter holds CTS low while its buffer is full.
    Hardware,
    /// XON/XOFF.
    Software,
}

/// How the bytes are paced, and what the job is (shown by the relay).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SendOptions {
    /// Bytes per block (0 is treated as the default, 256).
    pub chunk_size: usize,
    /// Pause between two blocks, in milliseconds (none after the last one).
    pub chunk_delay_ms: u64,
    /// Only used by the network destination.
    pub info: JobInfo,
}

impl SendOptions {
    pub const DEFAULT_CHUNK_SIZE: usize = 256;
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
            chunk_delay_ms: 0,
            info: JobInfo::default(),
        }
    }
}

/// Description of a job, for the relay's screen (queue, history) and its
/// own test cut.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobInfo {
    pub name: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub estimated_seconds: f64,
    /// The desktop's `{cut, machine}` settings, so a test cut started on
    /// the relay matches what the desktop would send.
    pub settings: Option<serde_json::Value>,
}

/// How a [`send`] call ended (errors are the `Err` side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendOutcome {
    /// Every byte was handed to the destination.
    Completed,
    /// Stopped early because `cancel` was set.
    Cancelled,
    /// Network only: the relay keeps the job in its queue until someone
    /// starts it on the relay's screen. `position` is 1 for the next one.
    Queued { position: usize },
}

/// Kind of serial port, as far as the OS can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortKind {
    Usb,
    Bluetooth,
    Pci,
    Unknown,
}

/// One serial port. Mirrors `PortInfo` in `src/model/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub name: String,
    pub kind: PortKind,
    /// USB product name (e.g. "USB-SERIAL CH340"), when known.
    pub description: Option<String>,
    pub manufacturer: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
}

/// Serial ports present on this machine, sorted naturally (COM2 before COM10).
/// Never fails: returns an empty list if enumeration is not possible.
pub fn list_ports() -> Vec<PortInfo> {
    // serialport's Linux sysfs scan `expect`s /sys/class/tty; never let that
    // take the caller down.
    let ports = match std::panic::catch_unwind(serialport::available_ports) {
        Ok(Ok(ports)) => ports,
        _ => return Vec::new(),
    };
    let mut out: Vec<PortInfo> = ports.into_iter().map(serial::port_info).collect();
    out.sort_by(|a, b| natural::cmp(&a.name, &b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Printer queues (local and connected) on Windows, sorted naturally.
/// Always empty on other platforms; never fails.
pub fn list_printers() -> Vec<String> {
    let mut names = printer::list();
    names.sort_by(|a, b| natural::cmp(a, b));
    names.dedup();
    names
}

/// Sends `data` to `dest`. Blocking: run it on a worker thread.
///
/// * `progress(sent, total)` is called once with `sent == 0` after the
///   destination has been opened, then after every block. `sent` only grows
///   and ends at `total` when the job completes.
/// * `cancel` is checked before each block (and during the pause between
///   blocks). Once set, no further block is written and
///   [`SendOutcome::Cancelled`] is returned. A block already being written is
///   finished first; on a stalled serial port this can take up to
///   [`STALL_TIMEOUT`].
///
/// What "sent" means depends on the destination: for a serial port the bytes
/// were accepted by the OS driver (the cutter has its own buffer and keeps
/// cutting what it already received, even after a cancel); for a printer they
/// were handed to the Windows spooler; for a file they were written.
pub fn send(
    dest: &Destination,
    data: &[u8],
    opts: &SendOptions,
    progress: &mut dyn FnMut(usize, usize),
    cancel: &AtomicBool,
) -> Result<SendOutcome, String> {
    match dest {
        Destination::Serial { port, baud, flow } => {
            let mut sink = serial::SerialSink::open(port, *baud, *flow)?;
            sink::pump(&mut sink, data, opts, progress, cancel)
        }
        Destination::Printer { name } => printer::send(name, data, opts, progress, cancel),
        Destination::File { path } => {
            let mut sink = file::FileSink::create(path)?;
            sink::pump(&mut sink, data, opts, progress, cancel)
        }
        // Not the chunk loop: the upload is instant, the progress that
        // matters is the relay's own serial send, which it streams back.
        Destination::Network {
            host,
            port,
            device,
            baud,
            flow,
        } => {
            let params = relay::JobParams {
                device: device.clone(),
                baud: *baud,
                flow: *flow,
                chunk_size: opts.chunk_size,
                chunk_delay_ms: opts.chunk_delay_ms,
                info: opts.info.clone(),
            };
            relay::send(host, *port, &params, data, progress, cancel)
        }
    }
}

const fn relay_port() -> u16 {
    relay::DEFAULT_PORT
}
