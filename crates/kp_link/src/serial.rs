//! Serial destination (USB-serial adapter such as the CH340 inside the KI-870A,
//! or a real COM port). 8N1, flow control as configured.

use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use serialport::{
    DataBits, FlowControl, Parity, SerialPort, SerialPortInfo, SerialPortType, StopBits,
};

use crate::sink::Sink;
use crate::{Flow, PortInfo, PortKind};

/// How long the cutter may refuse data (CTS held low, XOFF, driver queue not
/// moving) before the send fails with an explicit error instead of hanging.
/// It is also the port's write timeout, so a cancel requested while a block
/// is stuck is honoured after at most this long.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Poll interval while waiting for the driver's output queue to empty.
const DRAIN_POLL: Duration = Duration::from_millis(20);

impl From<Flow> for FlowControl {
    fn from(flow: Flow) -> Self {
        match flow {
            Flow::None => FlowControl::None,
            Flow::Hardware => FlowControl::Hardware,
            Flow::Software => FlowControl::Software,
        }
    }
}

pub(crate) fn port_info(p: SerialPortInfo) -> PortInfo {
    let mut info = PortInfo {
        name: p.port_name,
        kind: PortKind::Unknown,
        description: None,
        manufacturer: None,
        vid: None,
        pid: None,
    };
    match p.port_type {
        SerialPortType::UsbPort(usb) => {
            info.kind = PortKind::Usb;
            info.description = usb.product.and_then(|d| clean_product(&d, &info.name));
            info.manufacturer = usb.manufacturer.and_then(non_empty);
            info.vid = Some(usb.vid);
            info.pid = Some(usb.pid);
        }
        SerialPortType::BluetoothPort => info.kind = PortKind::Bluetooth,
        SerialPortType::PciPort => info.kind = PortKind::Pci,
        SerialPortType::Unknown => {}
    }
    info
}

/// Windows reports the friendly name, e.g. "USB-SERIAL CH340 (COM4)": drop the
/// redundant " (COM4)".
fn clean_product(product: &str, port: &str) -> Option<String> {
    let p = product.trim();
    let p = p
        .strip_suffix(')')
        .and_then(|s| s.strip_suffix(port))
        .and_then(|s| s.strip_suffix('('))
        .map_or(p, str::trim_end);
    non_empty(p.to_owned())
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn flow_label(flow: Flow) -> &'static str {
    match flow {
        Flow::None => "aucun",
        Flow::Hardware => "matériel RTS/CTS",
        Flow::Software => "logiciel XON/XOFF",
    }
}

pub(crate) struct SerialSink {
    port: Box<dyn SerialPort>,
    name: String,
    flow: Flow,
}

impl SerialSink {
    /// Opens `name` at `baud`, 8N1, with the given flow control, then asserts
    /// DTR and RTS (many CH340-based cutters ignore data until they see them).
    pub(crate) fn open(name: &str, baud: u32, flow: Flow) -> Result<Self, String> {
        if name.trim().is_empty() {
            return Err("Aucun port série n'est sélectionné.".into());
        }
        let port = serialport::new(name, baud)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .flow_control(flow.into())
            .timeout(STALL_TIMEOUT)
            .dtr_on_open(true)
            .open()
            .map_err(|e| open_error_message(name, &e))?;
        let mut sink = Self {
            port,
            name: name.to_owned(),
            flow,
        };
        // Best effort: some virtual ports (Bluetooth SPP...) reject modem
        // control lines, and with RTS/CTS the driver may own RTS already.
        let _ = sink.port.write_data_terminal_ready(true);
        let _ = sink.port.write_request_to_send(true);
        Ok(sink)
    }

    fn stall_message(&self) -> String {
        format!(
            "Le plotter n'accepte plus de données sur {} depuis {} s. \
             Vérifiez qu'il est allumé et en ligne (pas en pause), et que le contrôle de flux \
             ({}) correspond à son réglage.",
            self.name,
            STALL_TIMEOUT.as_secs(),
            flow_label(self.flow)
        )
    }

    fn write_error(&self, e: &io::Error) -> String {
        format!(
            "Erreur d'écriture sur le port {} (câble débranché ?) : {e}",
            self.name
        )
    }

    /// Waits until the driver's output queue is empty, failing if it stops
    /// moving for [`STALL_TIMEOUT`]. A plain `flush` (tcdrain on Linux) could
    /// block forever while the cutter holds CTS low.
    fn drain(&mut self) -> Result<(), String> {
        let mut last = None;
        let mut since = Instant::now();
        // An error here means "unsupported": fall through to the plain flush.
        while let Ok(queued) = self.port.bytes_to_write() {
            if queued == 0 {
                break;
            }
            if last != Some(queued) {
                last = Some(queued);
                since = Instant::now();
            } else if since.elapsed() >= STALL_TIMEOUT {
                return Err(self.stall_message());
            }
            thread::sleep(DRAIN_POLL);
        }
        // Best effort: the queue is empty, and some virtual COM drivers
        // reject FlushFileBuffers although every byte went out.
        let _ = self.port.flush();
        Ok(())
    }
}

impl Sink for SerialSink {
    fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String> {
        let mut rest = chunk;
        while !rest.is_empty() {
            match self.port.write(rest) {
                // Windows reports a write timeout as a short (here empty) write;
                // each attempt waits up to STALL_TIMEOUT for the cutter.
                Ok(0) => return Err(self.stall_message()),
                Ok(n) => rest = rest.get(n..).unwrap_or_default(),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // POSIX: poll() timed out.
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    return Err(self.stall_message());
                }
                Err(e) => return Err(self.write_error(&e)),
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        self.drain()
    }

    /// Stops sending and lets what the driver already queued go out. The
    /// cutter still cuts whatever is in its own buffer: a cancel cannot stop
    /// the blade immediately.
    fn abort(&mut self) -> Result<(), String> {
        self.drain()
    }
}

/// Why opening a port failed, as far as we can tell.
#[derive(Debug, PartialEq, Eq)]
enum OpenFailure {
    NotFound,
    Busy,
    /// Unix only: the user may not open the tty (not in group `dialout`).
    #[cfg_attr(windows, allow(dead_code))]
    PermissionDenied,
    Other,
}

fn open_error_message(port: &str, err: &serialport::Error) -> String {
    match classify_open_error(port, err) {
        OpenFailure::NotFound => {
            format!("Le port {port} est introuvable. Le plotter est-il branché et allumé ?")
        }
        OpenFailure::Busy => format!(
            "Le port {port} est déjà utilisé par un autre programme (SignMaster est-il ouvert ?)."
        ),
        OpenFailure::PermissionDenied => format!(
            "Accès refusé au port {port}. Sous Linux, l'utilisateur doit faire partie du groupe « dialout »."
        ),
        OpenFailure::Other => format!("Erreur sur le port {port} : {}", err.description),
    }
}

/// serialport maps "not found" and "access denied" (= port already open
/// elsewhere) to the same `NoDevice` kind on Windows, with only a localized
/// message to tell them apart. Re-open the device once ourselves to get the
/// real Win32 error code.
#[cfg(windows)]
fn classify_open_error(port: &str, err: &serialport::Error) -> OpenFailure {
    use serialport::ErrorKind as K;
    use std::os::windows::fs::OpenOptionsExt;

    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_PATH_NOT_FOUND: i32 = 3;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_INVALID_NAME: i32 = 123;

    if !matches!(
        err.kind,
        K::NoDevice | K::Io(io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied)
    ) {
        return OpenFailure::Other;
    }
    let path = if port.starts_with('\\') {
        port.to_owned()
    } else {
        format!(r"\\.\{port}")
    };
    // Exclusive, OPEN_EXISTING, like serialport does. If it now succeeds the
    // port was freed in between: report the original error as is.
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
    {
        Ok(_) => OpenFailure::Other,
        Err(e) => match e.raw_os_error() {
            Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_NAME) => {
                OpenFailure::NotFound
            }
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION) => OpenFailure::Busy,
            _ => OpenFailure::Other,
        },
    }
}

/// On POSIX serialport keeps errno's meaning: ENOENT → NotFound, EBUSY or a
/// held flock → NoDevice, EACCES → PermissionDenied.
#[cfg(not(windows))]
fn classify_open_error(_port: &str, err: &serialport::Error) -> OpenFailure {
    use serialport::ErrorKind as K;
    match err.kind {
        K::Io(io::ErrorKind::NotFound) => OpenFailure::NotFound,
        K::NoDevice => OpenFailure::Busy,
        K::Io(io::ErrorKind::PermissionDenied) => OpenFailure::PermissionDenied,
        _ => OpenFailure::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_name_suffix_is_dropped() {
        assert_eq!(
            clean_product("USB-SERIAL CH340 (COM4)", "COM4").as_deref(),
            Some("USB-SERIAL CH340")
        );
        assert_eq!(
            clean_product("USB-SERIAL CH340 (COM4)", "COM5").as_deref(),
            Some("USB-SERIAL CH340 (COM4)")
        );
        assert_eq!(clean_product("  ", "COM4"), None);
    }

    #[cfg(windows)]
    #[test]
    fn missing_windows_port_is_not_found() {
        let err = serialport::Error::new(serialport::ErrorKind::NoDevice, "x");
        assert_eq!(classify_open_error("COM250", &err), OpenFailure::NotFound);
    }
}
