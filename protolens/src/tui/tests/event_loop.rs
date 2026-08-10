// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `run_loop`'s dispatch pass (spec 0245 S1) and its sleep (spec 0263).

use std::convert::Infallible;
use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::Terminal;

use super::super::event::{AppEvent, InputPending};
use super::super::heat_worker::{HeatRequest, HeatWorkerHandle};
use super::super::terminal::{may_sleep_indefinitely, run_loop};
use super::super::tiered::Tier;
use super::super::App;
use super::support::*;

/// A `TestBackend` that counts the frames actually pushed to it.
///
/// `Terminal::draw` ends in exactly one `Backend::draw` call, so this
/// counter is the number of frames — the quantity spec 0245 S1 is about,
/// and one no test can otherwise see.
///
/// Its error type is `io::Error` rather than `TestBackend`'s own
/// `Infallible`, because `run_loop` requires `io::Error: From<B::Error>`
/// and no such conversion exists.
struct CountingBackend {
    inner: TestBackend,
    draws: usize,
}

impl CountingBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            inner: TestBackend::new(width, height),
            draws: 0,
        }
    }
}

fn never<T>(result: Result<T, Infallible>) -> io::Result<T> {
    Ok(result.unwrap_or_else(|e| match e {}))
}

impl Backend for CountingBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.draws += 1;
        never(self.inner.draw(content))
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        never(self.inner.hide_cursor())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        never(self.inner.show_cursor())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        never(self.inner.get_cursor_position())
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        never(self.inner.set_cursor_position(position))
    }

    fn clear(&mut self) -> io::Result<()> {
        never(self.inner.clear())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        never(self.inner.clear_region(clear_type))
    }

    fn size(&self) -> io::Result<Size> {
        never(self.inner.size())
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        never(self.inner.window_size())
    }

    fn flush(&mut self) -> io::Result<()> {
        never(self.inner.flush())
    }
}

fn key(code: KeyCode) -> AppEvent {
    AppEvent::Term(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

/// `:q` then Enter — the only way out of the TUI since spec 0236, and
/// so the only way to make `run_loop` return from a test.
fn quit_keys() -> Vec<AppEvent> {
    vec![
        key(KeyCode::Char(':')),
        key(KeyCode::Char('q')),
        key(KeyCode::Enter),
    ]
}

/// Drives `run_loop` over a pre-filled queue and reports the frames it
/// drew and what it left unconsumed. The sender is kept alive for the
/// whole call: `run_loop` treats a disconnected channel as a fatal loss
/// of input, which is right in production and noise here.
fn run_over(app: &mut App, events: Vec<AppEvent>) -> (usize, usize) {
    let (tx, rx) = mpsc::channel();
    for ev in events {
        tx.send(ev).expect("the receiver is alive");
    }
    let mut terminal = Terminal::new(CountingBackend::new(80, 24)).unwrap();
    let mut reader = None;
    run_loop(
        &mut terminal,
        app,
        &rx,
        &mut reader,
        &tx,
        &InputPending::default(),
    )
    .expect("`:q` must return cleanly");
    let left = rx.try_iter().count();
    (terminal.backend().draws, left)
}

/// Drives `run_loop` with an idle terminal, quitting from another thread
/// after `idle`, and reports the frames it drew.
///
/// The pane is deliberately tiny — one document row — so the fixture's
/// stops all sit off screen and every bake step is `Progressed`. That is
/// the path spec 0255 S6 defers, and the one a missing `bake_forces`
/// silences.
fn run_idle(app: &mut App, idle: Duration) -> usize {
    run_idle_after(app, idle, Vec::new())
}

/// [`run_idle`], with `prefix` sent ahead of the quit.
///
/// A test that leaves the search prompt open needs it: `quit_keys`
/// opens with `:`, which an open prompt takes as a character rather
/// than as a command, and the loop would then never return.
fn run_idle_after(app: &mut App, idle: Duration, prefix: Vec<AppEvent>) -> usize {
    let (tx, rx) = mpsc::channel();
    let quitter = {
        let tx = tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(idle);
            for ev in prefix.into_iter().chain(quit_keys()) {
                let _ = tx.send(ev);
            }
        })
    };
    let mut terminal = Terminal::new(CountingBackend::new(40, 3)).unwrap();
    let mut reader = None;
    run_loop(
        &mut terminal,
        app,
        &rx,
        &mut reader,
        &tx,
        &InputPending::default(),
    )
    .expect("`:q` must return cleanly");
    quitter.join().expect("the quitter must not panic");
    terminal.backend().draws
}

/// Spec 0255 S6, and the bug that shipped with it: narrowing the loop's
/// deadline wakes it up, but only `redraw` draws. Without the
/// `bake_forces` term the bake ran to completion behind a frame that
/// still showed the bounded document's line count.
///
/// Asserted against a control on the same fixture and the same wait, so
/// it cannot pass on a frame some other timer happened to owe.
#[test]
fn a_progressing_bake_forces_a_repaint() {
    let idle = Duration::from_millis(700);

    let quiet = {
        let (mut app, _) = repeated_message_fixture();
        app.splash = false;
        run_idle(&mut app, idle)
    };
    // Also spec 0263's "an event wakes the sleeping loop": drawing once
    // and then returning only on the quit is what a loop blocked in
    // `rx.recv()` does, and the only thing that can end that block is an
    // event from another thread.
    assert_eq!(quiet, 1, "an idle loop with no bake draws once and waits");

    let (mut app, _) = repeated_message_fixture();
    app.splash = false;
    let root = app.first_node;
    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");
    assert!(!app.bake_queue.is_empty(), "there is a debt to pay");

    let draws = run_idle(&mut app, idle);

    assert!(
        app.auto_folded.is_empty(),
        "the bake finished: {:?}",
        app.auto_folded
    );
    assert!(
        draws > quiet,
        "the bake must buy a frame the quiet loop does not: {draws}"
    );
}

/// Spec 0245 S1. Every event already on the queue is dispatched before
/// the loop draws, so a burst costs one frame rather than one each.
///
/// The count is exact, not a bound: `redraw` starts `true`, so the loop
/// draws its opening frame, receives, drains the whole queue, and then
/// returns on the quit without drawing again. One event per frame would
/// make it one plus the number of events that are not the last.
#[test]
fn a_burst_of_queued_events_is_one_dispatch_pass_and_one_frame() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.splash = false;

    let mut events: Vec<AppEvent> = (0..5).map(|_| key(KeyCode::Down)).collect();
    events.extend(quit_keys());
    let (draws, left) = run_over(&mut app, events);

    assert_eq!(draws, 1, "eight queued events must cost one frame");
    assert_eq!(left, 0, "and all of them must have been dispatched");
    assert_eq!(app.cursor, 5, "five `Down`s, dispatched in order");
}

/// Spec 0263 S2. Six inputs, and each one alone must veto the untimed
/// receive — a term dropped by accident is invisible in every ordinary
/// session and shows up only as a screen that stays wrong until the
/// user touches something.
#[test]
fn may_sleep_indefinitely_only_when_nothing_is_owed() {
    assert!(
        may_sleep_indefinitely(None, false, false, false, false, true),
        "with nothing owed the loop must be allowed to sleep"
    );

    let vetoes: [(&str, bool); 6] = [
        ("a message or the splash is due to expire", {
            let due = Some(Instant::now() + Duration::from_secs(1));
            may_sleep_indefinitely(due, false, false, false, false, true)
        }),
        ("a heat completion is owed a repaint", {
            may_sleep_indefinitely(None, true, false, false, false, true)
        }),
        ("the monochrome frame is owed its color back", {
            may_sleep_indefinitely(None, false, true, false, false, true)
        }),
        ("a bake grew the document off screen", {
            may_sleep_indefinitely(None, false, false, true, false, true)
        }),
        ("a bake changed a row on screen", {
            may_sleep_indefinitely(None, false, false, false, true, true)
        }),
        ("the heat worker has not settled", {
            may_sleep_indefinitely(None, false, false, false, false, false)
        }),
    ];
    for (why, slept) in vetoes {
        assert!(!slept, "the loop must stay on a timer while {why}");
    }
}

/// Spec 0263 S3, and the reason the last term is `heat_activity()`
/// rather than the far more obvious "the request queue is empty".
///
/// A worker that has taken a request off the queue and is walking it is
/// in exactly this state, and its `Prefetch` completions deliberately
/// send no event (spec 0164 G10) — so a loop that fell asleep here
/// would strand every cue that walk resolves until the user touched
/// something.
#[test]
fn a_worker_still_in_flight_keeps_the_loop_on_a_timer() {
    let mut app = sibling_leaves_app(&["f0: 0", "f1: 1"]);
    let worker = HeatWorkerHandle::stub_for_test();
    worker.push(
        HeatRequest {
            range: 0..8,
            current_key: None,
            start: 0,
            end: 8,
            tier: Tier::Prefetch,
        },
        Tier::Prefetch,
    );
    let admitted = worker.admit_next_range();
    app.heat_worker = Some(worker);

    assert_eq!(admitted, Some(0), "the request must have been taken");
    let worker = app.heat_worker.as_ref().unwrap();
    assert_eq!(worker.queue_len(), 0, "and the queue left empty");
    assert!(
        app.heat_activity().is_some(),
        "yet the walk it started is still live"
    );

    assert!(
        !may_sleep_indefinitely(None, false, false, false, false, false),
        "an in-flight walk must keep the loop on a timer"
    );
}

/// Spec 0263 G2, and the regression this spec most plausibly causes: a
/// message dismisses itself on a deadline that no event announces, so
/// sleeping through it would leave a stale notice on screen for as long
/// as the user left the terminal alone.
///
/// The deadline is planted directly, alongside the `last_message_seen`
/// that stops the first `render` re-arming it, so the test waits a
/// fraction of a second rather than the whole `MESSAGE_TIMEOUT`.
#[test]
fn an_expiring_message_still_dismisses_itself_with_no_events() {
    let mut app = sibling_leaves_app(&["f0: 0", "f1: 1"]);
    app.splash = false;
    app.message = "waiting to expire".to_string();
    app.last_message_seen = app.message.clone();
    app.message_deadline = Some(Instant::now() + Duration::from_millis(200));

    run_idle(&mut app, Duration::from_millis(700));

    assert!(
        app.message.is_empty(),
        "the message must expire with no event to ask for it: {:?}",
        app.message
    );
}

/// Spec 0272 S3. A sweep's answer is owed a frame, and until spec 0263
/// the activity tick delivered it by accident: `may_sleep_indefinitely`
/// has no search term, so once the sleep became untimed the loop parked
/// with `search_dirty` set and the prompt kept the color of a question
/// the sweep had already answered. What the user saw was a pattern that
/// stayed yellow until they pressed a key to ask why.
///
/// Asserted against a control that types nothing and waits the same
/// time, so it cannot pass on a frame some other term happened to owe.
/// Both runs get the same `Esc`, which the searching one needs to close
/// its prompt before `quit_keys` can be read as a command.
#[test]
fn a_settled_search_forces_a_repaint_with_no_events() {
    let idle = Duration::from_millis(700);
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let prefix = || vec![key(KeyCode::Esc)];

    let quiet = {
        let mut app = sibling_leaves_app(&refs);
        app.splash = false;
        run_idle_after(&mut app, idle, prefix())
    };

    let mut app = sibling_leaves_app(&refs);
    app.splash = false;
    for c in "/zzz".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert!(
        app.search_sweep.is_some(),
        "typing a pattern must leave a sweep for the loop to run"
    );

    let draws = run_idle_after(&mut app, idle, prefix());

    assert!(
        draws > quiet,
        "the settled miss must buy a frame the quiet loop does not: {draws} vs {quiet}"
    );
}

/// Spec 0245 S1c. A control transfer ends the pass where it happens and
/// leaves the rest of the queue in the channel — dispatching a keystroke
/// after the user asked to quit would deliver it to a screen that is no
/// longer ours.
#[test]
fn a_drain_stops_at_a_control_transfer() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.splash = false;

    let mut events = quit_keys();
    events.push(key(KeyCode::Down));
    events.push(key(KeyCode::Down));
    let (_draws, left) = run_over(&mut app, events);

    assert_eq!(left, 2, "the events behind the quit stay unconsumed");
    assert_eq!(app.cursor, 0, "and so change nothing");
}
