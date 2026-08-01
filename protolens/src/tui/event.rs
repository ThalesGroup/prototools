// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Event-driven main loop plumbing (spec 0152 G8) — a dedicated
//! input-reader thread and a small `AppEvent` enum, so `run_loop` waits
//! on one channel (a keypress, a mouse event, a worker progress
//! notification, or its own next deadline) rather than polling crossterm
//! for input on a schedule of its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event;

/// How often the input-reader thread re-checks `stop` between
/// `event::poll` timeouts — bounds worst-case shutdown latency for
/// this thread without meaningfully affecting input latency (every
/// real keypress still wakes `poll` immediately).
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(200);

pub(super) enum AppEvent {
    Term(event::Event),
    HeatWorkerProgress,
}

/// Owns the input-reader thread's join handle and shutdown flag (spec
/// 0152 G8/G9). Holds no unsafe/`'static`-reference data — its
/// `shutdown()` is joined purely for deterministic, leak-free
/// teardown, not for memory safety.
pub(super) struct InputReaderHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl InputReaderHandle {
    pub(super) fn spawn(tx: mpsc::Sender<AppEvent>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let join = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match event::poll(INPUT_POLL_INTERVAL) {
                    Ok(false) => {} // the interval elapsed with no input
                    Ok(true) => {
                        if let Ok(ev) = event::read() {
                            if tx.send(AppEvent::Term(ev)).is_err() {
                                break; // receiver gone — run_loop already exited
                            }
                        }
                    }
                    Err(_) => {
                        // An error, not a timeout: `poll` returned at
                        // once and consumed none of the interval.
                        // Treated as a bare "try again" this pegs a
                        // core for as long as the condition lasts, and
                        // the one that matters — a closed tty — never
                        // clears. Sleeping the interval it did not
                        // spend keeps the `stop` re-check on its usual
                        // cadence at one wakeup per interval.
                        thread::sleep(INPUT_POLL_INTERVAL);
                    }
                }
            }
        });
        InputReaderHandle {
            stop,
            join: Some(join),
        }
    }

    pub(super) fn shutdown(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// `InputReaderHandle::shutdown` must join promptly (spec 0152
    /// test plan) — this doesn't exercise real event delivery (no
    /// terminal/crossterm test double available in this scope), just
    /// the 200ms-bounded (`INPUT_POLL_INTERVAL`) spawn/shutdown round
    /// trip: the worst case is one full poll cycle before the thread
    /// re-checks `stop`.
    #[test]
    fn spawn_and_shutdown_round_trip_within_a_bounded_timeout() {
        let (tx, _rx) = mpsc::channel::<AppEvent>();
        let handle = InputReaderHandle::spawn(tx);
        let start = Instant::now();
        handle.shutdown();
        assert!(
            start.elapsed() < INPUT_POLL_INTERVAL * 3,
            "shutdown must join within a small bounded multiple of the poll interval"
        );
    }
}
