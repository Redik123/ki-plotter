//! Windows printer destination: the job goes to the spooler as a RAW document,
//! untouched by the driver (the KI-870A installed as a USB printer, usually
//! with the "Generic / Text Only" driver). Not available on other platforms.

use std::sync::atomic::AtomicBool;

use crate::{SendOptions, SendOutcome};

#[cfg(not(windows))]
pub(crate) fn list() -> Vec<String> {
    Vec::new()
}

#[cfg(not(windows))]
pub(crate) fn send(
    _name: &str,
    _data: &[u8],
    _opts: &SendOptions,
    _progress: &mut dyn FnMut(usize, usize),
    _cancel: &AtomicBool,
) -> Result<SendOutcome, String> {
    Err("Les imprimantes ne sont disponibles que sous Windows.".into())
}

#[cfg(windows)]
pub(crate) use win::list;

#[cfg(windows)]
pub(crate) fn send(
    name: &str,
    data: &[u8],
    opts: &SendOptions,
    progress: &mut dyn FnMut(usize, usize),
    cancel: &AtomicBool,
) -> Result<SendOutcome, String> {
    let mut sink = win::PrinterSink::open(name)?;
    crate::sink::pump(&mut sink, data, opts, progress, cancel)
}

#[cfg(windows)]
mod win {
    use windows::Win32::Graphics::Printing::{
        AbortPrinter, ClosePrinter, DOC_INFO_1W, EndDocPrinter, EndPagePrinter, EnumPrintersW,
        OpenPrinterW, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE,
        PRINTER_INFO_4W, StartDocPrinterW, StartPagePrinter, WritePrinter,
    };
    use windows::core::{Error, HRESULT, PCWSTR, PWSTR};

    use crate::sink::Sink;

    const ERROR_INVALID_PRINTER_NAME: u32 = 1801;
    const ERROR_INVALID_DATATYPE: u32 = 1804;

    /// Name shown in the Windows print queue.
    const JOB_NAME: &str = "KI Plotter";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn is(e: &Error, win32: u32) -> bool {
        e.code() == HRESULT::from_win32(win32)
    }

    /// Local and connected (network) printer queues.
    pub(crate) fn list() -> Vec<String> {
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        // u64 storage keeps the PRINTER_INFO_4W array pointer-aligned.
        let mut buf: Vec<u64> = Vec::new();
        // The required size can grow between the two calls (printer added).
        for _ in 0..4 {
            let mut needed = 0u32;
            let mut returned = 0u32;
            let byte_len = buf.len() * size_of::<u64>();
            let bytes = (byte_len > 0).then(|| {
                // SAFETY: same allocation viewed as bytes; u8 has no alignment needs.
                unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), byte_len) }
            });
            // SAFETY: valid out-pointers; the buffer (if any) is writable for its length.
            let res = unsafe {
                EnumPrintersW(flags, PCWSTR::null(), 4, bytes, &mut needed, &mut returned)
            };
            match res {
                Ok(()) => return collect(&buf, returned),
                Err(_) if needed as usize > byte_len => {
                    buf = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
                }
                Err(_) => return Vec::new(),
            }
        }
        Vec::new()
    }

    fn collect(buf: &[u64], returned: u32) -> Vec<String> {
        let max = std::mem::size_of_val(buf) / size_of::<PRINTER_INFO_4W>();
        let count = (returned as usize).min(max);
        // SAFETY: EnumPrintersW filled `returned` PRINTER_INFO_4W at the start
        // of the (aligned) buffer; the strings they point to live in it too.
        let infos =
            unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<PRINTER_INFO_4W>(), count) };
        infos
            .iter()
            .filter(|i| !i.pPrinterName.is_null())
            // SAFETY: non-null, NUL-terminated string inside `buf`.
            .filter_map(|i| unsafe { i.pPrinterName.to_string() }.ok())
            .filter(|n| !n.is_empty())
            .collect()
    }

    /// An open RAW print job. Dropping it without `finish` discards the job.
    pub(crate) struct PrinterSink {
        handle: PRINTER_HANDLE,
        name: String,
        doc_open: bool,
        page_open: bool,
    }

    impl PrinterSink {
        pub(crate) fn open(name: &str) -> Result<Self, String> {
            if name.trim().is_empty() {
                return Err("Aucune imprimante n'est sélectionnée.".into());
            }
            let wname = wide(name);
            let mut handle = PRINTER_HANDLE::default();
            // SAFETY: NUL-terminated name, valid out-pointer; default access
            // (PRINTER_ACCESS_USE) is enough to print.
            unsafe { OpenPrinterW(PCWSTR(wname.as_ptr()), &mut handle, None) }.map_err(|e| {
                if is(&e, ERROR_INVALID_PRINTER_NAME) {
                    format!("L'imprimante « {name} » est introuvable.")
                } else {
                    format!(
                        "Impossible d'ouvrir l'imprimante « {name} » : {}",
                        e.message()
                    )
                }
            })?;
            // From here on Drop closes the handle.
            let mut sink = Self {
                handle,
                name: name.to_owned(),
                doc_open: false,
                page_open: false,
            };

            let mut doc_name = wide(JOB_NAME);
            let mut datatype = wide("RAW");
            let info = DOC_INFO_1W {
                pDocName: PWSTR(doc_name.as_mut_ptr()),
                pOutputFile: PWSTR::null(),
                pDatatype: PWSTR(datatype.as_mut_ptr()),
            };
            // SAFETY: open handle; `info` and its strings outlive the call.
            let job = unsafe { StartDocPrinterW(sink.handle, 1, &info) };
            if job == 0 {
                let e = Error::from_thread();
                return Err(if is(&e, ERROR_INVALID_DATATYPE) {
                    format!(
                        "L'imprimante « {name} » n'accepte pas les données brutes (RAW). \
                         Installez-la avec le pilote « Generic / Text Only »."
                    )
                } else {
                    format!(
                        "L'imprimante « {name} » refuse le travail : {}",
                        e.message()
                    )
                });
            }
            sink.doc_open = true;

            // SAFETY: open handle with a started document.
            if !unsafe { StartPagePrinter(sink.handle) }.as_bool() {
                return Err(sink.error("refuse le travail"));
            }
            sink.page_open = true;
            Ok(sink)
        }

        /// Message for the Win32 error that just happened.
        fn error(&self, what: &str) -> String {
            let e = Error::from_thread();
            format!("L'imprimante « {} » {what} : {}", self.name, e.message())
        }
    }

    impl Sink for PrinterSink {
        /// Handed to the spooler: returns as soon as the bytes are spooled,
        /// not when the cutter has received them.
        fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String> {
            let mut rest = chunk;
            while !rest.is_empty() {
                let len = u32::try_from(rest.len()).unwrap_or(u32::MAX);
                let mut written = 0u32;
                // SAFETY: open page; `rest` is readable for `len` bytes.
                let ok =
                    unsafe { WritePrinter(self.handle, rest.as_ptr().cast(), len, &mut written) }
                        .as_bool();
                if !ok {
                    return Err(self.error("n'accepte plus de données"));
                }
                if written == 0 {
                    return Err(format!(
                        "L'imprimante « {} » n'accepte plus de données.",
                        self.name
                    ));
                }
                rest = rest.get(written as usize..).unwrap_or_default();
            }
            Ok(())
        }

        fn finish(&mut self) -> Result<(), String> {
            if self.page_open {
                self.page_open = false;
                // SAFETY: open page on an open handle.
                if !unsafe { EndPagePrinter(self.handle) }.as_bool() {
                    return Err(self.error("n'a pas pu terminer le travail"));
                }
            }
            if self.doc_open {
                self.doc_open = false;
                // SAFETY: open document on an open handle.
                if !unsafe { EndDocPrinter(self.handle) }.as_bool() {
                    return Err(self.error("n'a pas pu terminer le travail"));
                }
            }
            Ok(())
        }

        /// AbortPrinter deletes the spooled job, so the part not yet passed to
        /// the cutter is dropped (with EndDocPrinter the spooler would keep
        /// feeding the partial job). If the queue does not spool (AbortPrinter
        /// fails), the partial job is closed normally instead.
        fn abort(&mut self) -> Result<(), String> {
            if !self.doc_open {
                return Ok(());
            }
            // SAFETY: open handle with a started document.
            if unsafe { AbortPrinter(self.handle) }.as_bool() {
                self.page_open = false;
                self.doc_open = false;
                return Ok(());
            }
            // The partial job is acceptable after a cancel: ignore errors.
            let _ = self.finish();
            Ok(())
        }
    }

    impl Drop for PrinterSink {
        fn drop(&mut self) {
            // Error paths: discard a half-written job rather than print it.
            if self.doc_open {
                let _ = self.abort();
            }
            // SAFETY: the handle came from a successful OpenPrinterW and is
            // closed exactly once, here.
            let _ = unsafe { ClosePrinter(self.handle) };
        }
    }
}
