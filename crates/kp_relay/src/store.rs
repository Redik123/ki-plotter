//! Keeps the queue, the last job and the panel settings on disk, so a power
//! cut on the Pi loses nothing. Without a state directory everything stays
//! in memory (tests, quick runs on a PC).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::job::Job;

pub struct Store {
    dir: Option<PathBuf>,
}

/// Write then rename, so a power cut never leaves a half-written file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

impl Store {
    pub fn new(dir: Option<PathBuf>) -> Self {
        if let Some(d) = &dir
            && let Err(e) = fs::create_dir_all(d.join("queue"))
        {
            eprintln!("Dossier d'état {} inutilisable : {e}", d.display());
            return Self { dir: None };
        }
        Self { dir }
    }

    fn queue_dir(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join("queue"))
    }

    fn save_pair(dir: &Path, stem: &str, job: &Job) {
        let meta = serde_json::to_vec_pretty(job).unwrap_or_default();
        let res = write_atomic(&dir.join(format!("{stem}.plt")), &job.bytes)
            .and_then(|()| write_atomic(&dir.join(format!("{stem}.json")), &meta));
        if let Err(e) = res {
            eprintln!("Impossible d'enregistrer le travail {} : {e}", job.id);
        }
    }

    fn load_pair(dir: &Path, stem: &str) -> Option<Job> {
        let meta = fs::read(dir.join(format!("{stem}.json"))).ok()?;
        let bytes = fs::read(dir.join(format!("{stem}.plt"))).ok()?;
        let mut job = serde_json::from_slice::<Job>(&meta).ok()?;
        job.bytes = Arc::new(bytes);
        Some(job)
    }

    pub fn add_queued(&self, job: &Job) {
        if let Some(q) = self.queue_dir() {
            Self::save_pair(&q, &job.id.to_string(), job);
        }
    }

    pub fn remove_queued(&self, id: u64) {
        if let Some(q) = self.queue_dir() {
            let _ = fs::remove_file(q.join(format!("{id}.json")));
            let _ = fs::remove_file(q.join(format!("{id}.plt")));
        }
    }

    /// Queued jobs, oldest first.
    pub fn load_queue(&self) -> Vec<Job> {
        let Some(q) = self.queue_dir() else {
            return Vec::new();
        };
        let mut ids: Vec<u64> = fs::read_dir(&q)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.strip_suffix(".json")?.parse().ok()
            })
            .collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| Self::load_pair(&q, &id.to_string()))
            .collect()
    }

    pub fn save_last(&self, job: &Job) {
        if let Some(d) = &self.dir {
            Self::save_pair(d, "last", job);
        }
    }

    pub fn load_last(&self) -> Option<Job> {
        Self::load_pair(self.dir.as_ref()?, "last")
    }

    pub fn save_text(&self, file: &str, text: &str) -> Result<(), String> {
        let Some(d) = &self.dir else {
            return Ok(());
        };
        write_atomic(&d.join(file), text.as_bytes())
            .map_err(|e| format!("Impossible d'enregistrer les réglages : {e}"))
    }

    pub fn load_text(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.dir.as_ref()?.join(file)).ok()
    }
}
