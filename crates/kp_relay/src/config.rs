//! Relay configuration: defaults, `key = value` file, command line.

use std::net::SocketAddr;

use kp_link::Flow;
use kp_link::relay::{DEFAULT_PORT, parse_flow};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Shown in KI Plotter's relay list.
    pub name: String,
    pub listen: SocketAddr,
    /// `auto`, `/dev/ttyUSB0`, `/dev/serial/by-id/…`, `/dev/usb/lp0`, or
    /// `file:<path>` (writes jobs to a file, for tests).
    pub device: String,
    /// Used when the computer sending the job does not give its own.
    pub baud: u32,
    pub flow: Flow,
    pub chunk_size: usize,
    pub chunk_delay_ms: u64,
    pub max_job_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: default_name(),
            listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            device: "auto".into(),
            baud: 9600,
            flow: Flow::Hardware,
            chunk_size: 256,
            chunk_delay_ms: 0,
            max_job_bytes: 64 * 1024 * 1024,
        }
    }
}

/// The machine's hostname, e.g. "raspberrypi".
fn default_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "KI Plotter".into())
}

impl Config {
    /// Applies one setting; `key` as in the config file.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let v = value.trim();
        let bad = || format!("valeur invalide pour « {key} » : « {v} »");
        match key.trim() {
            "name" => self.name = v.to_owned(),
            "listen" => {
                self.listen = v
                    .parse()
                    .or_else(|_| {
                        v.parse::<u16>()
                            .map(|p| SocketAddr::from(([0, 0, 0, 0], p)))
                    })
                    .map_err(|_| bad())?;
            }
            "device" => self.device = v.to_owned(),
            "baud" => self.baud = v.parse().map_err(|_| bad())?,
            "flow" => self.flow = parse_flow(v).ok_or_else(bad)?,
            "chunk_size" => self.chunk_size = v.parse().map_err(|_| bad())?,
            "chunk_delay_ms" => self.chunk_delay_ms = v.parse().map_err(|_| bad())?,
            "max_job_mb" => {
                self.max_job_bytes = v.parse::<usize>().map_err(|_| bad())? * 1024 * 1024
            }
            other => return Err(format!("réglage inconnu : « {other} »")),
        }
        Ok(())
    }

    /// Reads a `key = value` file (`#` starts a comment).
    pub fn load_file(&mut self, text: &str) -> Result<(), String> {
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("ligne {} : « clé = valeur » attendu", n + 1))?;
            self.set(k, v)
                .map_err(|e| format!("ligne {} : {e}", n + 1))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_and_overrides() {
        let mut c = Config::default();
        c.load_file(
            "# atelier\nname = Atelier  # commentaire\nlisten = 8000\ndevice=/dev/ttyUSB1\nflow = none\nbaud = 38400\n\nmax_job_mb = 8\n",
        )
        .unwrap();
        assert_eq!(c.name, "Atelier");
        assert_eq!(c.listen, "0.0.0.0:8000".parse().unwrap());
        assert_eq!(c.device, "/dev/ttyUSB1");
        assert_eq!(c.flow, Flow::None);
        assert_eq!(c.baud, 38400);
        assert_eq!(c.max_job_bytes, 8 * 1024 * 1024);
        assert!(c.load_file("baud = vite").unwrap_err().contains("ligne 1"));
        assert!(
            c.load_file("couleur = rouge")
                .unwrap_err()
                .contains("inconnu")
        );
    }
}
