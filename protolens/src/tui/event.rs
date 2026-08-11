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
///
/// Spec 0263: this is the *fallback* now. On unix the reader blocks in
/// `poll(2)` with no timeout and is interrupted through a pipe, so it
/// performs no timed wakeup at all and stops the instant it is asked
/// to. The interval is what other platforms use, and what unix falls
/// back to if that setup ever fails (S11).
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(200);

pub(super) enum AppEvent {
    Term(event::Event),
    HeatWorkerProgress,
    /// Spec 0274 S9: a segment scan has an answer waiting. Uncounted by
    /// `InputPending` for the same reason the heat worker's is — it is
    /// the app talking to itself, not the user typing.
    SearchWorkerProgress,
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

/// Spec 0263 S5: the outcome of one wait between two drains.
enum Waited {
    /// The terminal may have something for us — go round and drain it.
    /// Also covers a resize, which reaches crossterm as a signal rather
    /// than as bytes on the terminal.
    MaybeInput,
    /// Shutdown was asked for, or the terminal went away. Either way
    /// this thread is done.
    Stop,
}

#[cfg(unix)]
mod untimed {
    use std::fs::OpenOptions;
    use std::io;
    use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};

    use nix::errno::Errno;
    use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
    use signal_hook::consts::SIGWINCH;
    use signal_hook::SigId;

    use super::Waited;

    /// The descriptor crossterm reads its events from (spec 0263 S8):
    /// stdin when stdin is a terminal, `/dev/tty` otherwise. crossterm
    /// makes exactly this choice in its own `tty_fd()`, and protolens
    /// has to make the same one — a blob can be handed to protolens on
    /// stdin, in which case fd 0 is a pipe that no terminal input will
    /// ever arrive on.
    enum Tty {
        Stdin,
        Owned(OwnedFd),
    }

    impl Tty {
        fn open() -> io::Result<Self> {
            // SAFETY: `isatty` only inspects the descriptor; it neither
            // reads from it nor takes ownership of it.
            if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
                Ok(Tty::Stdin)
            } else {
                // Opened read *and* write exactly as crossterm opens it.
                let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
                Ok(Tty::Owned(OwnedFd::from(tty)))
            }
        }

        fn borrow(&self) -> BorrowedFd<'_> {
            match self {
                // SAFETY: fd 0 outlives this thread, and the reader only
                // ever polls it — it is never closed here.
                Tty::Stdin => unsafe { BorrowedFd::borrow_raw(libc::STDIN_FILENO) },
                Tty::Owned(fd) => fd.as_fd(),
            }
        }
    }

    /// Spec 0263 S5: the reader's untimed wait. Blocks in `poll(2)` with
    /// no timeout on three descriptors, so an idle session performs no
    /// timed wakeup at all.
    pub(super) struct Wait {
        tty: Tty,
        /// S5a: crossterm learns about a resize through a `signal_hook`
        /// pipe of its own rather than through the terminal, so a wait
        /// on the terminal alone would sleep through every resize.
        winch: OwnedFd,
        stop: OwnedFd,
    }

    /// The other half of [`Wait`], kept by the handle: what makes the
    /// wait give up (spec 0263 S6).
    pub(super) struct Interrupt {
        stop: OwnedFd,
        winch: SigId,
    }

    impl Wait {
        /// Returns the wait and the interrupt that ends it.
        pub(super) fn new() -> io::Result<(Self, Interrupt)> {
            let tty = Tty::open()?;
            let (winch_rx, winch_tx) = nix::unistd::pipe()?;
            // Registrations for one signal chain, so crossterm's own
            // `SIGWINCH` pipe still fires and still produces the
            // `Event::Resize`; this one exists only to end the wait so
            // that the drain can collect it. A raw `sigaction` here
            // would *replace* crossterm's handler and stop resize
            // events reaching the app at all.
            let winch = signal_hook::low_level::pipe::register(SIGWINCH, winch_tx)?;
            let (stop_rx, stop_tx) = nix::unistd::pipe()?;
            Ok((
                Wait {
                    tty,
                    winch: winch_rx,
                    stop: stop_rx,
                },
                Interrupt {
                    stop: stop_tx,
                    winch,
                },
            ))
        }

        pub(super) fn wait(&self) -> Waited {
            let mut fds = [
                PollFd::new(self.tty.borrow(), PollFlags::POLLIN),
                PollFd::new(self.winch.as_fd(), PollFlags::POLLIN),
                PollFd::new(self.stop.as_fd(), PollFlags::POLLIN),
            ];
            match poll(&mut fds, PollTimeout::NONE) {
                Ok(_) => {}
                // A signal landed on this thread. Nothing was read and
                // nothing is owed, so going round simply re-blocks.
                Err(Errno::EINTR) => return Waited::MaybeInput,
                Err(_) => return Waited::Stop,
            }
            let revents = |fd: PollFd| fd.revents().unwrap_or_else(PollFlags::empty);
            if !revents(fds[2]).is_empty() {
                return Waited::Stop;
            }
            let broken = PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL;
            if revents(fds[0]).intersects(broken) {
                // The terminal is gone: there is nothing left to read
                // and polling it again would return at once, forever.
                return Waited::Stop;
            }
            if revents(fds[1]).contains(PollFlags::POLLIN) {
                // Drained, or the byte stays readable and every
                // subsequent `poll` returns immediately — a spin, not a
                // sleep. The contents carry nothing: the drain that
                // follows asks crossterm what actually happened.
                let mut sink = [0u8; 32];
                let _ = nix::unistd::read(self.winch.as_raw_fd(), &mut sink);
            }
            Waited::MaybeInput
        }
    }

    impl Interrupt {
        /// Spec 0263 S6. One byte is all [`Wait::wait`] looks for; if
        /// the pipe is full then an interrupt is already outstanding,
        /// which does just as well.
        pub(super) fn raise(&self) {
            let _ = nix::unistd::write(&self.stop, &[0u8]);
        }

        /// Undone after the thread is joined rather than leaked: the
        /// Neovim handoff spawns a fresh reader every time it returns,
        /// and each one registers a pipe of its own.
        pub(super) fn release(self) {
            signal_hook::low_level::unregister(self.winch);
        }
    }
}

/// The fallback wait: crossterm's own timed poll, re-checking `stop`
/// once per interval (spec 0263 S11).
fn timed_wait() -> Waited {
    match event::poll(INPUT_POLL_INTERVAL) {
        // Either input arrived or the interval elapsed with none; the
        // caller's `stop` check and drain handle both.
        Ok(_) => Waited::MaybeInput,
        Err(_) => {
            // An error, not a timeout: `poll` returned at once and
            // consumed none of the interval. Treated as a bare "try
            // again" this pegs a core for as long as the condition
            // lasts, and the one that matters — a closed tty — never
            // clears. Sleeping the interval it did not spend keeps the
            // `stop` re-check on its usual cadence at one wakeup per
            // interval.
            thread::sleep(INPUT_POLL_INTERVAL);
            Waited::MaybeInput
        }
    }
}

/// How a drain ended.
enum Drained {
    /// crossterm has nothing further parsed — safe to block.
    Empty,
    /// crossterm could not say. Notably *not* "the terminal is quiet".
    Failed,
    /// `run_loop` has exited and taken the receiver with it.
    ChannelGone,
}

/// Spec 0263 S7: take everything crossterm has already parsed before
/// blocking on the descriptor.
///
/// crossterm reads up to a kilobyte from the terminal in one go and
/// parses every event it can out of that chunk into an internal queue,
/// returning one; while the rest sit in that queue the descriptor is
/// *not* readable. Blocking without draining first would strand them
/// until the next keystroke — for a paste or a wheel burst, that is
/// input the user watches go missing.
fn drain_parsed(pending: &InputPending, tx: &mpsc::Sender<AppEvent>) -> Drained {
    loop {
        match event::poll(Duration::ZERO) {
            Ok(true) => {}
            Ok(false) => return Drained::Empty,
            Err(_) => return Drained::Failed,
        }
        let Ok(ev) = event::read() else {
            return Drained::Failed;
        };
        if !carries_intent(&ev) {
            continue;
        }
        // Counted *before* the send: the receiver may take it off the
        // channel on another thread the instant it lands, and an
        // increment that lost that race would underflow on the matching
        // decrement.
        pending.note_sent();
        if tx.send(AppEvent::Term(ev)).is_err() {
            return Drained::ChannelGone; // run_loop already exited
        }
    }
}

/// The reader thread's body, shared by both waits (spec 0263 S5).
fn read_loop(
    stop: &AtomicBool,
    pending: &InputPending,
    tx: &mpsc::Sender<AppEvent>,
    mut wait: impl FnMut() -> Waited,
) {
    while !stop.load(Ordering::Relaxed) {
        match drain_parsed(pending, tx) {
            Drained::Empty => {}
            Drained::ChannelGone => return,
            Drained::Failed => {
                // Blocking in the untimed wait now would be a spin
                // rather than a sleep: the descriptor stays readable
                // precisely because the events on it could not be
                // collected, so `poll(2)` would return immediately and
                // forever. Backing off by the fallback's own interval
                // costs the same five wakeups a second the timed loop
                // always cost, lets a transient failure clear, and
                // cannot peg a core.
                thread::sleep(INPUT_POLL_INTERVAL);
                continue;
            }
        }
        if matches!(wait(), Waited::Stop) {
            return;
        }
    }
}

/// Owns the input-reader thread's join handle and shutdown flag (spec
/// 0152 G8/G9). Holds no unsafe/`'static`-reference data — its
/// `shutdown()` is joined purely for deterministic, leak-free
/// teardown, not for memory safety.
pub(super) struct InputReaderHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    /// `None` when the untimed wait could not be set up and the thread
    /// fell back to the timed one (spec 0263 S11), in which case the
    /// `stop` flag above is the only way to reach it.
    #[cfg(unix)]
    interrupt: Option<untimed::Interrupt>,
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

        // Spec 0263 S11: a session that wakes five times a second is a
        // power problem, but a reader that cannot be stopped is a broken
        // Neovim handoff — so a setup failure falls back rather than
        // aborting.
        #[cfg(unix)]
        let (wait, interrupt) = match untimed::Wait::new() {
            Ok((wait, interrupt)) => (Some(wait), Some(interrupt)),
            Err(_) => (None, None),
        };

        let join = thread::spawn(move || {
            // Spec 0264 S7: this thread inherited whatever mask the main
            // thread is under. It has no reason to sit on a fast core —
            // since spec 0263 it is blocked in `poll(2)` with no timed
            // wakeups — and every reason not to hold one.
            crate::affinity::widen();
            #[cfg(unix)]
            if let Some(wait) = wait {
                read_loop(&thread_stop, &pending, &tx, || wait.wait());
                return;
            }
            read_loop(&thread_stop, &pending, &tx, timed_wait);
        });

        InputReaderHandle {
            stop,
            join: Some(join),
            #[cfg(unix)]
            interrupt,
        }
    }

    /// Test-only: whether this reader got the untimed wait or fell back
    /// to the timed one (spec 0263 S11). The fallback is reachable in
    /// ordinary test environments — a build sandbox has a `/dev/tty` but
    /// no controlling terminal behind it — and the two have different
    /// shutdown bounds, so a test that cares must ask.
    #[cfg(test)]
    pub(super) fn is_untimed(&self) -> bool {
        #[cfg(unix)]
        {
            self.interrupt.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    pub(super) fn shutdown(mut self) {
        // The flag first, so that a reader already past its wait — one
        // busy draining, say — stops at the top of the next iteration
        // without needing the byte at all.
        self.stop.store(true, Ordering::Relaxed);
        #[cfg(unix)]
        if let Some(interrupt) = &self.interrupt {
            interrupt.raise();
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        // Only once the thread is gone: the registration keeps the write
        // end of a pipe the thread is polling the read end of.
        #[cfg(unix)]
        if let Some(interrupt) = self.interrupt.take() {
            interrupt.release();
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
    /// the spawn/shutdown round trip.
    ///
    /// Spec 0263 S6 tightens the bound: the reader is interrupted
    /// through a pipe rather than waited out, so shutdown no longer
    /// costs up to a poll interval. That is the direct evidence for S6
    /// and, through it, for G3.
    ///
    /// The bound is conditional because the fallback is genuinely
    /// reachable here (S11): a build sandbox has a `/dev/tty` with no
    /// controlling terminal behind it, and a reader that fell back is
    /// entitled to its old, slower answer. Asking which one this is
    /// keeps the fast bound an assertion rather than a coin flip.
    #[test]
    fn the_reader_stops_at_once_when_asked() {
        let (tx, _rx) = mpsc::channel::<AppEvent>();
        let handle = InputReaderHandle::spawn(tx, InputPending::default());
        let untimed = handle.is_untimed();
        let start = Instant::now();
        handle.shutdown();
        let elapsed = start.elapsed();
        if untimed {
            assert!(
                elapsed < INPUT_POLL_INTERVAL / 4,
                "an interrupted reader must not wait out anything: {elapsed:?}"
            );
        } else {
            assert!(
                elapsed < INPUT_POLL_INTERVAL * 3,
                "the fallback must still join within a small multiple of \
                 the poll interval: {elapsed:?}"
            );
        }
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

    /// Spec 0263 S7's direct evidence, and S5's: a burst written to the
    /// terminal in one `write` must arrive in full with no following
    /// keystroke to flush it.
    ///
    /// crossterm reads up to a kilobyte in one go and parses every event
    /// out of it into an internal queue, returning one — so after that
    /// read the descriptor is *not* readable while the rest sit in the
    /// queue. A reader that blocked in `poll(2)` without draining first
    /// would deliver one character of a paste and strand the other
    /// seven, which is why this is a test and not a comment.
    ///
    /// It needs a real terminal, and the terminal a process reads is a
    /// property of the process, not of a thread — so the reader runs in
    /// a re-exec of this same test binary with its stdin bound to a pty.
    /// That also makes it the one test that certainly exercises the
    /// untimed path (S8: stdin is a tty, so that is what gets polled),
    /// which is why it asserts on that too.
    #[cfg(unix)]
    #[test]
    fn the_reader_delivers_every_event_of_a_chunk() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        if std::env::var_os(CHUNK_CHILD).is_some() {
            chunk_child();
            return;
        }

        let pty = nix::pty::openpty(None, None).expect("a pty pair");
        let child = Command::new(std::env::current_exe().expect("this test binary"))
            .args([
                "--exact",
                "--nocapture",
                "tui::event::tests::the_reader_delivers_every_event_of_a_chunk",
            ])
            .env(CHUNK_CHILD, "1")
            .stdin(Stdio::from(pty.slave))
            .stdout(Stdio::piped())
            .spawn()
            .expect("re-exec must succeed");

        // One `write`, so the whole burst reaches crossterm in a single
        // read and every event but the first comes out of its queue
        // rather than off the descriptor. Written before any handshake
        // on purpose: the pty buffers, so there is no race to lose.
        let mut master = std::fs::File::from(pty.master);
        master
            .write_all(CHUNK_BURST)
            .expect("the burst must be sent");
        master.flush().expect("and reach the pty");

        let out = child.wait_with_output().expect("the child must finish");
        let text = String::from_utf8_lossy(&out.stdout);
        let expected = format!("EVENTS {} UNTIMED true", CHUNK_BURST.len());
        assert!(
            text.contains(&expected),
            "expected `{expected}` in the child's output:\n{text}"
        );
    }

    /// Set on the re-exec above so the child takes the other branch.
    #[cfg(unix)]
    const CHUNK_CHILD: &str = "PROTOLENS_READER_CHUNK_CHILD";

    /// Eight one-byte keys: eight events out of one read, and no escape
    /// sequence whose parse could be ambiguous about how many that is.
    #[cfg(unix)]
    const CHUNK_BURST: &[u8] = b"abcdefgh";

    /// The re-exec'd half: read the burst through a real
    /// `InputReaderHandle` and report what arrived.
    ///
    /// Bounded rather than blocking, because the failure being tested
    /// for is precisely a reader that delivers some of the burst and
    /// then waits forever for input that is not coming.
    #[cfg(unix)]
    fn chunk_child() {
        use std::io::Write;

        // Without raw mode the line discipline holds the burst until a
        // newline, which would test the terminal rather than the reader.
        crossterm::terminal::enable_raw_mode().expect("the pty must go raw");
        let (tx, rx) = mpsc::channel::<AppEvent>();
        let handle = InputReaderHandle::spawn(tx, InputPending::default());

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut seen = 0usize;
        while seen < CHUNK_BURST.len() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(left) {
                Ok(AppEvent::Term(_)) => seen += 1,
                Ok(AppEvent::HeatWorkerProgress | AppEvent::SearchWorkerProgress) => {}
                Err(_) => break,
            }
        }

        let untimed = handle.is_untimed();
        handle.shutdown();
        let _ = crossterm::terminal::disable_raw_mode();
        println!("EVENTS {seen} UNTIMED {untimed}");
        let _ = std::io::stdout().flush();
    }
}
