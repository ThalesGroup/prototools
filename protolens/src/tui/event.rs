// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Event-driven main loop plumbing (spec 0152 G8) — a dedicated
//! input-reader thread and a small `AppEvent` enum, so `run_loop` waits
//! on one channel (a keypress, a mouse event, a worker progress
//! notification, or its own next deadline) rather than polling crossterm
//! for input on a schedule of its own.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event;
use crossterm::event::{Event, MouseEventKind};

/// How often the input-reader thread re-checks `stop` between
/// `event::poll` timeouts — bounds worst-case shutdown latency for
/// this thread without meaningfully affecting input latency (every
/// real keypress still wakes `poll` immediately).
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(200);

pub(super) enum AppEvent {
    Term(event::Event),
    HeatWorkerProgress,
}

/// Spec 0223 S1: how many terminal events the reader has sent that
/// `run_loop` has not yet taken off the channel. An `mpsc` receiver
/// cannot be peeked, so "is the user still typing" is answered by a
/// counter alongside the channel rather than by a length query on it.
///
/// **Only `AppEvent::Term` is counted.** The heat worker shares this
/// channel and emits progress continuously while a new viewport's cues
/// resolve — exactly during a scroll — so counting those would hold the
/// display monochrome for as long as the worker is busy, which is a
/// different and much longer condition than "the user is still
/// scrolling".
#[derive(Clone, Default)]
pub(super) struct InputPending(Arc<AtomicUsize>);

impl InputPending {
    /// Called by the reader thread immediately before `send`.
    fn note_sent(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Called by `run_loop` once per event taken off the channel,
    /// whichever of its receive paths produced it. Decrementing on a
    /// `HeatWorkerProgress` — which was never counted — would drive the
    /// counter below zero and wrap.
    pub(super) fn note_received(&self, ev: &AppEvent) {
        if matches!(ev, AppEvent::Term(_)) {
            self.0.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub(super) fn is_pending(&self) -> bool {
        self.0.load(Ordering::Relaxed) > 0
    }
}

/// Spec 0223 S2: a bare pointer move carries no user intent, and
/// `EnableMouseCapture` makes the terminal send one on essentially every
/// pixel the pointer crosses. Dropped here, before the channel, so a
/// hovering mouse cannot make the counter above look like a scroll.
///
/// Wheel (`ScrollUp`/`ScrollDown`) and `Drag` are deliberately not
/// filtered: a rolling wheel is precisely the flow this exists for, and
/// a drag is an active selection.
fn carries_intent(ev: &Event) -> bool {
    !matches!(ev, Event::Mouse(m) if m.kind == MouseEventKind::Moved)
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
    /// `pending` belongs to the *channel*, not to this thread: the
    /// Neovim handoff shuts the reader down and spawns a fresh one, and
    /// events the outgoing reader already sent are still in the queue
    /// waiting to be counted down. A counter owned per reader would let
    /// those decrements land on a new zero and wrap.
    pub(super) fn spawn(tx: mpsc::Sender<AppEvent>, pending: InputPending) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_pending = pending;
        let join = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match event::poll(INPUT_POLL_INTERVAL) {
                    Ok(false) => {} // the interval elapsed with no input
                    Ok(true) => {
                        if let Ok(ev) = event::read() {
                            if !carries_intent(&ev) {
                                continue;
                            }
                            // Counted *before* the send: the receiver may
                            // take it off the channel on another thread the
                            // instant it lands, and an increment that lost
                            // that race would underflow on the matching
                            // decrement.
                            thread_pending.note_sent();
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
        let handle = InputReaderHandle::spawn(tx, InputPending::default());
        let start = Instant::now();
        handle.shutdown();
        assert!(
            start.elapsed() < INPUT_POLL_INTERVAL * 3,
            "shutdown must join within a small bounded multiple of the poll interval"
        );
    }

    /// Spec 0223 S1's load-bearing distinction: the heat worker shares
    /// this channel, and counting its progress notifications would hold
    /// the screen monochrome for as long as the worker is busy — which
    /// silently turns the feature into "never highlight during a sweep"
    /// rather than "do not highlight a frame being scrolled past".
    #[test]
    fn a_terminal_event_raises_the_pending_count_and_a_worker_event_does_not() {
        let pending = InputPending::default();
        assert!(!pending.is_pending());

        pending.note_sent();
        assert!(pending.is_pending());

        // A worker event was never counted up, so it must not count
        // down: the counter is unsigned and a stray decrement wraps it
        // to `usize::MAX`, pinning the display monochrome forever.
        pending.note_received(&AppEvent::HeatWorkerProgress);
        assert!(pending.is_pending(), "a worker event must not decrement");

        pending.note_received(&AppEvent::Term(Event::FocusGained));
        assert!(!pending.is_pending());
    }

    /// Spec 0223 S2. `carries_intent` is the reader thread's filter;
    /// `event::read()` cannot be driven from a test, so the predicate is
    /// exercised directly.
    #[test]
    fn mouse_motion_never_enters_the_channel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent};

        let mouse = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })
        };

        assert!(!carries_intent(&mouse(MouseEventKind::Moved)));

        // A rolling wheel is precisely the flow this spec exists for,
        // and a drag is an active selection — neither may be dropped.
        assert!(carries_intent(&mouse(MouseEventKind::ScrollDown)));
        assert!(carries_intent(&mouse(MouseEventKind::ScrollUp)));
        assert!(carries_intent(&mouse(MouseEventKind::Drag(
            MouseButton::Left
        ))));
        assert!(carries_intent(&mouse(MouseEventKind::Down(
            MouseButton::Left
        ))));
        assert!(carries_intent(&Event::Key(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE
        ))));
    }
}
