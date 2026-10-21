//! Relay configuration: defaults, `key = value` file, settings saved from
//! the touch panel, command line.

use std::net::SocketAddr;
use std::path::PathBuf;

use kp_link::Flow;
use kp_link::relay::{DEFAULT_PORT, flow_name, parse_flow};
use serde::{Deserialize, Serialize};

/// What happens to a job received from a computer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// It waits in the queue until someone starts it on the relay's screen.
    #[default]
    Queue,
    /// It is cut right away.
    Direct,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Shown in KI Plotter's relay list and on the panel.
    pub name: String,
    pub listen: SocketAddr,
    pub mode: Mode,
    /// `auto`, `/dev/ttyUSB0`, `/dev/serial/by-id/…`, `/dev/usb/lp0`, or
    /// `file:<path>` (writes jobs to a file, for tests).
    pub device: String,
    /// Used when the computer sending the job does not give its own.
    pub baud: u32,
    pub flow: Flow,
    pub chunk_size: usize,
    pub chunk_delay_ms: u64,
    pub max_job_bytes: usize,
    /// Where queued jobs, the last job and panel settings are kept
    /// (`/var/lib/ki-plotter-relay` under systemd). None = memory only.
    pub state_dir: Option<PathBuf>,
    /// Settings given on the command line, re-applied after the panel's
    /// saved settings so the command line always wins.
    pub cli_overrides: Vec<(&'static str, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: default_name(),
            listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            mode: Mode::Queue,
            device: "auto".into(),
            baud: 9600,
            flow: Flow::Hardware,
            chunk_size: 256,
            chunk_delay_ms: 0,
            max_job_bytes: 64 * 1024 * 1024,
            state_dir: None,
            cli_overrides: Vec::new(),
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
            "mode" => {
                self.mode = match v.to_ascii_lowercase().as_str() {
                    "queue" | "file" | "attente" => Mode::Queue,
                    "direct" => Mode::Direct,
                    _ => return Err(bad()),
                }
            }
            "device" => self.device = v.to_owned(),
            "baud" => self.baud = v.parse().map_err(|_| bad())?,
            "flow" => self.flow = parse_flow(v).ok_or_else(bad)?,
            "chunk_size" => self.chunk_size = v.parse().map_err(|_| bad())?,
            "chunk_delay_ms" => self.chunk_delay_ms = v.parse().map_err(|_| bad())?,
            "max_job_mb" => {
                self.max_job_bytes = v.parse::<usize>().map_err(|_| bad())? * 1024 * 1024
            }
            "state_dir" => self.state_dir = (!v.is_empty()).then(|| PathBuf::from(v)),
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

    pub fn panel_settings(&self) -> PanelSettings {
        PanelSettings {
            name: self.name.clone(),
            mode: self.mode,
            device: self.device.clone(),
            baud: self.baud,
            flow: self.flow,
        }
    }

    pub fn apply(&mut self, s: &PanelSettings) {
        if !s.name.trim().is_empty() {
            self.name = s.name.trim().chars().take(60).collect();
        }
        self.mode = s.mode;
        self.device = if s.device.trim().is_empty() {
            "auto".into()
        } else {
            s.device.trim().to_owned()
        };
        if s.baud > 0 {
            self.baud = s.baud;
        }
        self.flow = s.flow;
    }
}

/// What the touch panel can change. Saved in `<state_dir>/settings.conf`,
/// which takes precedence over `/etc/ki-plotter-relay.conf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelSettings {
    pub name: String,
    pub mode: Mode,
    pub device: String,
    pub baud: u32,
    pub flow: Flow,
}

impl PanelSettings {
    pub fn to_conf(&self) -> String {
        format!(
            "# Réglages enregistrés depuis l'écran du relais (prioritaires sur /etc/ki-plotter-relay.conf)\n\
             name = {}\nmode = {}\ndevice = {}\nbaud = {}\nflow = {}\n",
            self.name.replace(['\n', '#'], " "),
            self.mode.name(),
            self.device.replace(['\n', '#'], ""),
            self.baud,
            flow_name(self.flow)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_and_overrides() {
        let mut c = Config::default();
        c.load_file(
            "# atelier\nname = Atelier  # commentaire\nlisten = 8000\ndevice=/dev/ttyUSB1\nflow = none\nbaud = 38400\n\nmax_job_mb = 8\nmode = direct\n",
        )
        .unwrap();
        assert_eq!(c.name, "Atelier");
        assert_eq!(c.listen, "0.0.0.0:8000".parse().unwrap());
        assert_eq!(c.device, "/dev/ttyUSB1");
        assert_eq!(c.flow, Flow::None);
        assert_eq!(c.baud, 38400);
        assert_eq!(c.mode, Mode::Direct);
        assert_eq!(c.max_job_bytes, 8 * 1024 * 1024);
        assert!(c.load_file("baud = vite").unwrap_err().contains("ligne 1"));
        assert!(
            c.load_file("couleur = rouge")
                .unwrap_err()
                .contains("inconnu")
        );
    }

    #[test]
    fn panel_settings_round_trip_through_the_file() {
        let mut c = Config::default();
        let s = PanelSettings {
            name: "Atelier # 2".into(),
            mode: Mode::Queue,
            device: String::new(),
            baud: 19200,
            flow: Flow::Software,
        };
        c.apply(&s);
        let mut back = Config::default();
        back.load_file(&c.panel_settings().to_conf()).unwrap();
        assert_eq!(back.name, "Atelier   2");
        assert_eq!(back.device, "auto");
        assert_eq!(back.baud, 19200);
        assert_eq!(back.flow, Flow::Software);
        assert_eq!(back.mode, Mode::Queue);
    }
}
