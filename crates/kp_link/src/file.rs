//! File destination: same flow as the others, the bytes land in a file.
//! Also works for raw device nodes (e.g. `/dev/usb/lp0` for a USB-printer
//! cutter on the Raspberry Pi).

use std::fs::File;
use std::io::Write;

use crate::sink::Sink;

pub(crate) struct FileSink {
    file: File,
    path: String,
}

impl FileSink {
    /// Creates (or truncates) the file.
    pub(crate) fn create(path: &str) -> Result<Self, String> {
        let file = File::create(path)
            .map_err(|e| format!("Impossible de créer le fichier « {path} » : {e}"))?;
        Ok(Self {
            file,
            path: path.to_owned(),
        })
    }

    fn write_error(&self, e: &std::io::Error) -> String {
        format!("Erreur d'écriture dans le fichier « {} » : {e}", self.path)
    }
}

impl Sink for FileSink {
    fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.file.write_all(chunk).map_err(|e| self.write_error(&e))
    }

    fn finish(&mut self) -> Result<(), String> {
        self.file.flush().map_err(|e| self.write_error(&e))
    }

    /// The partial file is kept (what was written is what the cutter would
    /// have received).
    fn abort(&mut self) -> Result<(), String> {
        self.finish()
    }
}
