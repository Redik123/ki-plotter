//! The chunk loop shared by every destination.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::{SendOptions, SendOutcome};

/// An open connection to a destination. `pump` drives it; dropping it must
/// release the underlying resource (port, printer handle, file) even when
/// neither `finish` nor `abort` was called (error paths).
///
/// The future `Network` destination (Raspberry Pi relay) only needs a TCP
/// implementation of this trait.
pub(crate) trait Sink {
    /// Writes the whole block, or fails with a French, user-facing message.
    fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String>;
    /// Called once after the last block: flush / close the job cleanly.
    fn finish(&mut self) -> Result<(), String>;
    /// Called once when the user cancelled between two blocks.
    fn abort(&mut self) -> Result<(), String>;
}

/// Longest single sleep while pausing between blocks, so a cancel is seen quickly.
const CANCEL_POLL: Duration = Duration::from_millis(50);

pub(crate) fn pump<S: Sink + ?Sized>(
    sink: &mut S,
    data: &[u8],
    opts: &SendOptions,
    progress: &mut dyn FnMut(usize, usize),
    cancel: &AtomicBool,
) -> Result<SendOutcome, String> {
    let total = data.len();
    let chunk_size = if opts.chunk_size == 0 {
        SendOptions::DEFAULT_CHUNK_SIZE
    } else {
        opts.chunk_size
    };
    let delay = Duration::from_millis(opts.chunk_delay_ms);
    let cancelled = || cancel.load(Ordering::Acquire);

    let mut sent = 0;
    progress(sent, total);
    for chunk in data.chunks(chunk_size) {
        if cancelled() {
            sink.abort()?;
            return Ok(SendOutcome::Cancelled);
        }
        if let Err(err) = sink.write_chunk(chunk) {
            // A write that failed (typically timed out on a stalled port)
            // while the user was cancelling is reported as the cancel it is.
            // No `abort` here: the sink may be stuck; dropping it cleans up.
            return if cancelled() {
                Ok(SendOutcome::Cancelled)
            } else {
                Err(err)
            };
        }
        sent += chunk.len();
        progress(sent, total);
        if sent < total && !delay.is_zero() {
            pause(delay, cancel);
        }
    }
    sink.finish()?;
    Ok(SendOutcome::Completed)
}

/// Sleeps for `delay`, returning early if `cancel` becomes set.
fn pause(delay: Duration, cancel: &AtomicBool) {
    let end = Instant::now() + delay;
    loop {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        thread::sleep(left.min(CANCEL_POLL));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory sink recording what happened.
    #[derive(Default)]
    struct Mem {
        data: Vec<u8>,
        finished: bool,
        aborted: bool,
        fail_at: Option<usize>,
    }

    impl Sink for Mem {
        fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String> {
            if self.fail_at.is_some_and(|n| self.data.len() >= n) {
                return Err("boom".into());
            }
            self.data.extend_from_slice(chunk);
            Ok(())
        }
        fn finish(&mut self) -> Result<(), String> {
            self.finished = true;
            Ok(())
        }
        fn abort(&mut self) -> Result<(), String> {
            self.aborted = true;
            Ok(())
        }
    }

    fn opts(chunk_size: usize) -> SendOptions {
        SendOptions {
            chunk_size,
            chunk_delay_ms: 0,
            ..SendOptions::default()
        }
    }

    #[test]
    fn empty_job_reports_zero_and_completes() {
        let mut sink = Mem::default();
        let mut seen = Vec::new();
        let cancel = AtomicBool::new(false);
        let out = pump(
            &mut sink,
            &[],
            &opts(4),
            &mut |s, t| seen.push((s, t)),
            &cancel,
        );
        assert_eq!(out, Ok(SendOutcome::Completed));
        assert_eq!(seen, vec![(0, 0)]);
        assert!(sink.finished && !sink.aborted);
    }

    #[test]
    fn zero_chunk_size_uses_default() {
        let data = vec![7u8; 600];
        let mut sink = Mem::default();
        let mut seen = Vec::new();
        let cancel = AtomicBool::new(false);
        let out = pump(
            &mut sink,
            &data,
            &opts(0),
            &mut |s, _| seen.push(s),
            &cancel,
        );
        assert_eq!(out, Ok(SendOutcome::Completed));
        assert_eq!(seen, vec![0, 256, 512, 600]);
        assert_eq!(sink.data, data);
    }

    #[test]
    fn cancel_before_start_writes_nothing() {
        let mut sink = Mem::default();
        let cancel = AtomicBool::new(true);
        let out = pump(&mut sink, b"abcdef", &opts(2), &mut |_, _| {}, &cancel);
        assert_eq!(out, Ok(SendOutcome::Cancelled));
        assert!(sink.data.is_empty() && sink.aborted && !sink.finished);
    }

    #[test]
    fn cancel_after_last_chunk_still_completes() {
        let mut sink = Mem::default();
        let cancel = AtomicBool::new(false);
        let out = pump(
            &mut sink,
            b"abcd",
            &opts(2),
            &mut |s, t| {
                if s == t {
                    cancel.store(true, Ordering::Release);
                }
            },
            &cancel,
        );
        assert_eq!(out, Ok(SendOutcome::Completed));
        assert!(sink.finished && !sink.aborted);
    }

    #[test]
    fn write_error_is_returned_unless_cancelling() {
        let cancel = AtomicBool::new(false);
        let mut sink = Mem {
            fail_at: Some(2),
            ..Mem::default()
        };
        let out = pump(&mut sink, b"abcdef", &opts(2), &mut |_, _| {}, &cancel);
        assert_eq!(out, Err("boom".to_string()));
        assert!(!sink.finished);

        // The user cancels while a block is stuck, then the write times out.
        struct StuckThenCancelled<'a>(&'a AtomicBool);
        impl Sink for StuckThenCancelled<'_> {
            fn write_chunk(&mut self, _: &[u8]) -> Result<(), String> {
                self.0.store(true, Ordering::Release);
                Err("timeout".into())
            }
            fn finish(&mut self) -> Result<(), String> {
                unreachable!("finish after a failed write")
            }
            fn abort(&mut self) -> Result<(), String> {
                unreachable!("abort on a stuck sink")
            }
        }
        let cancel = AtomicBool::new(false);
        let mut sink = StuckThenCancelled(&cancel);
        let out = pump(&mut sink, b"abcdef", &opts(2), &mut |_, _| {}, &cancel);
        assert_eq!(out, Ok(SendOutcome::Cancelled));
    }

    #[test]
    fn pause_returns_early_on_cancel() {
        let cancel = AtomicBool::new(true);
        let start = Instant::now();
        pause(Duration::from_secs(5), &cancel);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
