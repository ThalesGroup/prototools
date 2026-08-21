// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The terminal as a device, and the loop that owns it.
//!
//! Everything between `main` handing over a built `App` and getting the
//! cooked terminal back: raw mode, mouse capture, the Kitty keyboard
//! protocol, `SIGTSTP` suspend/resume, the panic hook that has to undo
//! all of it, and `run_loop` itself.
//!
//! It is one file because those are one concern — every entry point here
//! either puts the terminal into a state or is responsible for taking it
//! back out — and because `run_loop` is a single state machine over
//! seven interdependent locals that reads the tuning constants at the
//! top.

use super::*;

/// How long `warm_up_heat_cues` (spec 0151 G8) waits, from its own
/// start, before drawing its first progress frame — avoids any flicker
/// for small/fast descriptor sets, where the whole pass is already
/// near-instant.
const WARMUP_FIRST_DRAW_DELAY: Duration = Duration::from_millis(300);

/// Minimum elapsed time between successive `warm_up_heat_cues` progress
/// redraws (spec 0151 G8) — time-based, not a fixed line-count interval,
/// so it stays responsive regardless of how expensive each individual
/// line turns out to be.
const WARMUP_REDRAW_INTERVAL: Duration = Duration::from_millis(300);

/// How long protolens may sit unattended before re-examining the
/// heat-cue activity byte (spec 0190 S7/G3). Bounds the activity dot's
/// staleness without requiring every path that drains the request queue
/// to emit an `AppEvent` — an event-maintained dot would freeze in
/// whatever state it was last drawn in, showing "busy" over an idle
/// worker.
///
/// Deliberately a flat interval rather than a timer armed per queue
/// event: queue events are far too frequent, and each one would
/// reschedule. The cost of a tick that changes nothing is two relaxed
/// atomic loads and a comparison — `run_loop` skips the frame entirely
/// (S8).
const ACTIVITY_TICK: Duration = Duration::from_millis(250);

/// Spec 0192 S3: the shortest interval between two frames drawn
/// *solely* because background scoring completed. Keystrokes, mouse
/// events, resizes and message/splash deadlines are never delayed by it
/// — only the repaint that shows a newly arrived cue is, and that
/// repaint is a visual refinement of a frame the user is already
/// looking at.
///
/// The worker notifies once per completed non-`Prefetch` request
/// (`heat_worker.rs`), and a fresh page queues one per unsettled
/// visible row, so without this a single keystroke costs a screenful of
/// full frames. At 50 ms a progressive fill costs at most 20 frames per
/// second, which is a refresh rate the user cannot distinguish from
/// immediate.
const HEAT_REPAINT_INTERVAL: Duration = Duration::from_millis(50);

/// Spec 0223 S4: how long after a monochrome frame the loop waits before
/// redrawing it in color. It is a settle interval, not a rate limit —
/// the recolor also requires the event queue to be empty, so a held-down
/// PageDown never reaches it, and the very first quiet moment does.
///
/// Sized against the input repeat rate rather than against the eye. A
/// terminal's key auto-repeat is on the order of 30 ms, so anything much
/// shorter would let a gap *between* two repeats trigger a recolor that
/// the next repeat immediately discards, paying the highlighting cost
/// for a frame the user never sees — the exact waste this avoids. Held
/// down, it means the color returns 50 ms after the key is released,
/// which reads as immediate.
const STYLES_SETTLE_INTERVAL: Duration = Duration::from_millis(50);

/// Spec 0255 S6: the shortest interval between two frames drawn *solely*
/// because the bake grew the document.
///
/// Ten times `HEAT_REPAINT_INTERVAL` because what it repaints is
/// smaller. A completed cue colors a row the user is reading; a bake
/// step changes the document's total height, which reaches only the
/// scrollbar thumb and the line-count footer — the rows on screen are
/// already final (spec 0249 S1's depth-first rule). One entry can be
/// tens of thousands of steps, so the alternative is tens of thousands
/// of frames redrawing identical text, which is what spec 0245 exists to
/// prevent.
const BAKE_REPAINT_INTERVAL: Duration = Duration::from_millis(500);

/// Spec 0192 S4: read-ahead steps guaranteed per main-loop iteration,
/// regardless of how busy the event channel is. Read-ahead is still
/// opportunistic — the `TryRecvError::Empty` arm in `run_loop` spends
/// as many steps as the user's idleness allows — but the guarantee is
/// what stops a steady event stream from starving it entirely. A burst
/// of `HeatWorkerProgress` events does exactly that: without the floor,
/// read-ahead ran twice across a forty-keystroke session, both times
/// before the first keystroke.
///
/// Small enough that the guaranteed work cannot itself add perceptible
/// latency to a keystroke: one step is a walk of a few rows plus at
/// most one `TieredBounded::upsert`.
const PREFETCH_STEPS_PER_ITERATION: usize = 8;

/// Spec 0245 S1b: how long one dispatch pass may keep taking events off
/// the queue before it stops and draws. Checked *after* each dispatch,
/// so one expensive event is always allowed to finish and the overshoot
/// is bounded by that event's own cost.
///
/// A time budget rather than an event count, because per-event dispatch
/// cost spans orders of magnitude — a clamped wheel pan is microseconds,
/// an override re-render is milliseconds — so a count that coalesced a
/// wheel burst usefully would stall the frame for tens of milliseconds
/// on a queue of expensive ones. What the user perceives is elapsed time
/// without a frame, so that is the unit.
///
/// At 8 ms a saturated queue still draws better than 60 Hz, while
/// sitting two to three orders of magnitude above one wheel dispatch —
/// so a real wheel burst coalesces completely and never reaches the
/// budget at all.
const DRAIN_BUDGET: Duration = Duration::from_millis(8);

/// Drain any input events already queued in the terminal's input buffer
/// before disabling raw mode.
///
/// `EnableMouseCapture` always turns on any-motion reporting (crossterm
/// gives no way to opt out short of hand-rolling the escape sequences — see
/// the comment on `handle_mouse`'s `Moved` guard), so a mouse move happening
/// in the split second between the app's last `event::read()` and raw mode
/// being disabled here would otherwise sit unread in the pty's input queue.
/// Once cooked-mode echo comes back on, the tty driver echoes those queued
/// bytes straight to the screen as raw escape-sequence garbage (e.g.
/// `^[[<35;60;17M`) that the shell then needs an Enter/Ctrl-C to clear.
/// Reading them here, while raw mode (no echo) is still active, discards
/// them silently instead.
fn drain_pending_input() {
    let deadline = Instant::now() + Duration::from_millis(60);
    while Instant::now() < deadline {
        match term_event::poll(Duration::from_millis(15)) {
            Ok(true) => {
                let _ = term_event::read();
            }
            _ => break,
        }
    }
}

/// Set by `push_keyboard_enhancement` once it has actually pushed Kitty
/// keyboard-protocol enhancement flags, so `pop_keyboard_enhancement`
/// knows whether the matching pop is needed — `restore_terminal`/
/// `suspend` are free functions with no `App` to carry this as ordinary
/// state, and popping when nothing was pushed would either do nothing
/// (unsupported terminals just ignore the unknown escape sequence) or,
/// worse, pop a flag set some other way. Single-threaded (one terminal,
/// one event loop), so `Ordering::Relaxed` is enough.
static KITTY_KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// Push `DISAMBIGUATE_ESCAPE_CODES` (Kitty keyboard protocol) if the
/// terminal supports it — without it, legacy terminal escape sequences
/// carry no modifier parameter for printable keys, so Shift-Space is
/// reported identically to plain Space (unlike arrow/function keys,
/// which already carry one, e.g. `ESC [1;2A` for Shift-Up).
/// `supports_keyboard_enhancement` queries the terminal and
/// blocks briefly waiting for its response — fine here since it only
/// ever runs before the main event loop starts (`run`) or during a
/// suspend/resume cycle (`suspend`), never concurrently with
/// `event::read`/`poll`. On terminals that don't support it this is a
/// no-op: `handle_manage_key`'s guarded `Char(' ') if SHIFT` arm simply
/// never fires there.
fn push_keyboard_enhancement() -> io::Result<()> {
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        KITTY_KEYBOARD_ENHANCED.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Undo `push_keyboard_enhancement`, if it actually pushed anything.
fn pop_keyboard_enhancement() {
    if KITTY_KEYBOARD_ENHANCED.swap(false, Ordering::Relaxed) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
}

/// Restore the terminal to its normal (cooked, main-screen, no mouse
/// capture, visible cursor) state — shared by `run`'s own cleanup and the
/// panic hook below, so a panic mid-session doesn't leave the user's
/// terminal unusable.
///
/// The body is the exact inverse of the setup sequence in `run` and in
/// `enable_raw_mode_and_reenter` — mouse capture and the alternate screen
/// undone first, then the keyboard enhancement, then raw mode — so that
/// each undo runs in the state its counterpart was issued in.
/// `drain_pending_input` is the exception, and comes first: it has to read
/// while raw mode is still on.
///
/// `Show` is here rather than at the call sites because `Terminal::draw`
/// leaves the hardware cursor hidden unless the frame set a position, so
/// *any* path out of the TUI owes the shell a visible cursor — including
/// the panic hook, which has no `Terminal` to ask.
pub(super) fn restore_terminal() {
    drain_pending_input();
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
    pop_keyboard_enhancement();
    let _ = disable_raw_mode();
}

/// `Ctrl-Z` suspend (spec 0113 D31): leave the terminal in the same clean
/// state a normal exit would (mirroring `restore_terminal`/the panic
/// hook), raise `SIGTSTP` on this process, and — once `fg` sends
/// `SIGCONT` and execution resumes right here — re-enter the alternate
/// screen/mouse-capture/raw-mode trio and force a full redraw, since the
/// terminal's actual contents are unknown after a suspend/resume cycle
/// (another program may have used the same terminal in between).
#[cfg(unix)]
fn suspend<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    restore_terminal();
    // SAFETY: raising a signal on our own process is always sound.
    unsafe {
        libc::raise(libc::SIGTSTP);
    }
    enable_raw_mode_and_reenter(terminal)
}

/// Re-enter raw mode/keyboard-enhancement/alternate-screen/mouse-capture
/// and force a full redraw — the shared tail of `suspend()`'s own
/// resume path (spec 0113 D31) and `neovim::open_editor`'s Neovim-
/// handoff resume path (spec 0144 G5): both leave the terminal via
/// `restore_terminal()`, hand control to something else, then need this
/// exact same re-entry sequence once control returns.
#[cfg(unix)]
pub(super) fn enable_raw_mode_and_reenter<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    enable_raw_mode()?;
    push_keyboard_enhancement()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    terminal.clear()?;
    Ok(())
}

/// Where a panic on a thread other than the UI thread is left for
/// `run_loop` to find. Holds the *first* one; a later panic on another
/// background thread is usually the first one's consequence.
static BACKGROUND_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// The one line of a panic worth putting in a message row: what was said
/// and where. The default hook's backtrace has nowhere to go here — the
/// alternate screen is still up and belongs to the document.
fn describe_panic(info: &std::panic::PanicHookInfo<'_>) -> String {
    let what = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(no message)");
    let thread = std::thread::current().name().unwrap_or("?").to_string();
    match info.location() {
        Some(loc) => format!("thread '{thread}' panicked at {loc}: {what}"),
        None => format!("thread '{thread}' panicked: {what}"),
    }
}

/// Take whatever the panic hook recorded, if anything.
fn take_background_panic() -> Option<String> {
    BACKGROUND_PANIC
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// Run the interactive TUI loop against a real terminal.
pub fn run(app: &mut App) -> io::Result<()> {
    // A panic mid-session (e.g. an indexing bug) would otherwise unwind
    // straight out of this function, skipping the cleanup below and
    // leaving the terminal stuck in raw/alt-screen/mouse-capture mode.
    // Restore it first, then hand off to the default panic printer.
    //
    // Flaw C4: installed *before* the first fallible call, not after
    // terminal setup — otherwise a panic during setup unwinds with raw
    // mode already on and no hook installed to undo it.
    //
    // A panic hook is process-global, so this one runs on whichever
    // thread panicked — including the heat worker and the sweep shards it
    // spawns. Only the UI thread owns the terminal, and only its panic is
    // actually unwinding towards the cleanup below; restoring the
    // terminal for anyone else would drop a live session out of the
    // alternate screen and then print over what replaced it. So the two
    // cases are separated by thread id.
    let ui_thread = std::thread::current().id();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui_thread {
            restore_terminal();
            default_hook(info);
            return;
        }
        let mut slot = BACKGROUND_PANIC.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(describe_panic(info));
        }
    }));

    // Terminal setup, up to and including the `Terminal` itself. Every
    // line after the first is fallible *with raw mode already on*, and
    // `terminal` does not exist yet, so this window cannot be covered by
    // the cleanup block at the end of this function — it gets its own
    // captured `Result` instead.
    let setup = (|| -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
        enable_raw_mode()?;
        push_keyboard_enhancement()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        Terminal::new(CrosstermBackend::new(stdout))
    })();
    let mut terminal = match setup {
        Ok(terminal) => terminal,
        Err(e) => {
            restore_terminal();
            let _ = std::panic::take_hook();
            return Err(e);
        }
    };

    let (tx, rx) = mpsc::channel();
    // `Option`-wrapped so `run_loop` can `take()` it and shut it down
    // around the Neovim handoff below — see `run_loop`'s own doc comment
    // on that block for why.
    // Spec 0223 S1: owned here, beside the channel it describes, so that
    // the reader respawn inside `run_loop` inherits the same counter.
    let input_pending = event::InputPending::default();
    let mut input_reader = Some(event::InputReaderHandle::spawn(
        tx.clone(),
        input_pending.clone(),
    ));

    // Flaw C4: everything fallible from here to `run_loop` lives inside
    // this closure, whose `Result` is *captured* rather than propagated.
    // A `?` in here returns from the closure, so the cleanup block below
    // is reached on every path by construction rather than by review —
    // a bare `terminal.size()?` or `warm_up_heat_cues(..)?` would return
    // straight out of `run`, leaving the terminal in raw/alternate-
    // screen/mouse-capture mode for whatever ran next. Any future `?`
    // added to this region is covered for free, which is the point.
    let result = (|| -> io::Result<()> {
        // Safe upper bound (spec 0151 G6/G8): the real, render-computed
        // `override_list_height` (set by the override pane's own first
        // render, `render.rs`) is always <= the raw terminal height, since
        // it's an inner-area height net of borders/header rows. Setting it
        // eagerly here means G6's cross-population cap isn't stuck at `1`
        // (its `App::new` default) for the warm-up pass or any ordinary
        // browsing before the user's first `t` press.
        app.override_list_height = terminal.size()?.height.max(1) as usize;

        // Spec 0152 G1: spawned only when a scoring graph is loaded — with
        // no graph, `app.heat_worker` stays `None` for the whole session,
        // and every fork that checks `heat_worker.is_some()` falls through
        // to the existing synchronous logic. Spawned *before* `warm_up_
        // heat_cues` below so that its initial-viewport pass can push
        // requests onto the worker's queue and return immediately
        // instead of scoring synchronously on this thread — warming up
        // with no worker to hand work off to costs a multi-second black
        // screen at startup against a large scoring graph.
        if let Some(graph) = &app.ctx.graph {
            // Spec 0180 S2: an owning handle, not a `&'static` copied out of the
            // mapping. Each `Arc::clone` below is a refcount bump, and it is what
            // makes both spawns below independent of when `App` drops.
            let graph = Arc::clone(graph);
            // Spec 0216 S28: a refcount bump, not a copy of the whole
            // blob, since the worker can share ownership of it.
            let blob = Arc::clone(&app.blob);

            // Spec 0168: no second, detached "root-type" thread here.
            // `decode::decode` resolves the type before rendering, so by
            // the time `App` exists the document is already what it is —
            // nothing re-scores the blob and re-renders the whole
            // document through the splice machinery underneath a reader
            // who has already started browsing.
            // Spec 0217 S6: the workers sweep while the main thread is
            // drawing, so they get the budget less the one thread the
            // main loop is already spending — never less than 1, which
            // is the un-sharded sweep this has always been.
            //
            // Spec 0250 S1: this is now a thread count as well as a
            // fan-out width — that many speculative queries may walk at
            // once — which is why it must stay a per-core budget rather
            // than becoming, say, the queue depth.
            let worker_jobs = app.sweep_jobs.saturating_sub(1).max(1);
            app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
                Arc::clone(&app.heat_caches),
                graph,
                blob,
                tx.clone(),
                worker_jobs,
            ));
        }

        // Spec 0274 S9: from here on there is a loop draining `rx`, so a
        // segment scan has somewhere to report to and may run off the
        // main thread. Unconditional — unlike the heat worker it needs
        // no graph, only a listener.
        app.search_progress = Some(tx.clone());

        warm_up_heat_cues(&mut terminal, app)?;

        run_loop(
            &mut terminal,
            app,
            &rx,
            &mut input_reader,
            &tx,
            &input_pending,
        )
    })();

    // Spec 0152 G9: both threads joined, unconditionally (see "Shutdown
    // and safety"). Not load-bearing for *memory* safety — the worker
    // owns an `Arc<LoadedGraph>` (spec 0180 S2), so it cannot observe an
    // unmapped page whether or not this runs. Joining is what stops the
    // worker writing to the terminal after `restore_terminal` below, and
    // what keeps the process from lingering on a background sweep the
    // user has already quit.
    if let Some(worker) = app.heat_worker.take() {
        worker.shutdown();
    }
    if let Some(reader) = input_reader.take() {
        reader.shutdown();
    }
    let _ = std::panic::take_hook();
    restore_terminal();

    result
}

/// One-time warm-up pass (spec 0151 G8) priming heat cues for the
/// initial viewport before the first `run_loop` iteration. With the
/// spec-0152 worker already spawned by the time this runs, each
/// `heat_cue_for` call below either hits the cache or pushes a request
/// onto the worker's queue and returns immediately — this loop scores
/// nothing itself, which is what keeps startup against a large scoring
/// graph from being a multi-second black screen. Still runs while heat
/// cues are hidden (`i`): the background fetch/cache is worth priming
/// regardless of whether a cue is currently shown — only
/// `heat_cue_for`'s return value is suppressed while hidden, not the
/// underlying work.
pub(super) fn warm_up_heat_cues<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    if app.ctx.graph.is_none() {
        return Ok(());
    }
    let rows = terminal.size()?.height as usize;
    let lines: Vec<usize> = app
        .visible_window(0, rows)
        .into_iter()
        .map(|(line, _)| line)
        .collect();
    let start = Instant::now();
    let mut last_draw = start;
    for (i, &line_idx) in lines.iter().enumerate() {
        app.heat_cue_for(line_idx); // populates the caches; return value unused here
        let now = Instant::now();
        if now.duration_since(start) > WARMUP_FIRST_DRAW_DELAY
            && now.duration_since(last_draw) > WARMUP_REDRAW_INTERVAL
        {
            app.message = format!(
                "Computing inference cues for the initial view: {}/{} lines scored...",
                i + 1,
                lines.len()
            );
            terminal.draw(|frame| app.render(frame))?;
            last_draw = now;
        }
    }
    if Instant::now().duration_since(start) > WARMUP_FIRST_DRAW_DELAY {
        app.message.clear();
    }
    Ok(())
}

/// Spec 0286 S6: called after each dispatched *input* event, because an
/// input that was not a pan ends the gesture the wall's cue reports.
///
/// Only key and mouse events, deliberately: a heat result or a resize is
/// not the reader doing something else. Putting a lit cue out is a
/// change on screen, so it takes back spec 0245 S2's "nothing happened"
/// verdict — which the handler above reached without knowing this was
/// coming.
/// Every pane's wall, and not just the focused one's: focus can change
/// during the very event being settled, and a pan aimed at one pane is
/// something other than a pan as far as the other two are concerned.
/// `|` rather than `||` — being asked is what ends a gesture, so no wall
/// may be skipped.
fn settle_edge_resistance(app: &mut App) {
    let put_a_cue_out = app.scroll_resistance.settle()
        | app.override_resistance.settle()
        | app.manage_resistance.settle();
    if put_a_cue_out {
        app.event_changed_nothing = false;
    }
}

/// Dispatch one received event.
///
/// Returns the `redraw_why` tag for an event that asks for a frame, or
/// `None` for one that does not. A key `Release` is dispatched to
/// nothing (see below) yet still asks for a frame, exactly as it did
/// when `run_loop` decided this from the event's shape alone.
pub(super) fn dispatch_event(app: &mut App, ev: &event::AppEvent) -> Option<&'static str> {
    match ev {
        // Some Kitty-protocol-aware terminals report a `Release` event
        // for a keystroke in addition to `Press`, even though this app
        // only requests `DISAMBIGUATE_ESCAPE_CODES` (not
        // `REPORT_EVENT_TYPES`). A `Release` event dispatched through
        // `handle_key` after its matching `Press` already changed focus
        // (e.g. `t`/`Esc`/`Tab` closing the override pane) would land in
        // the *new* focus's handler instead of being a no-op —
        // surfacing as a keypress "leaking" into both panes. Ignore
        // anything but `Press`/`Repeat`.
        event::AppEvent::Term(Event::Key(key)) => {
            if key.kind != KeyEventKind::Release {
                let dispatched_at = Instant::now();
                app.handle_key(*key);
                settle_edge_resistance(app);
                trace::trace!(
                    "key {:?} us={}",
                    key.code,
                    dispatched_at.elapsed().as_micros()
                );
            }
            Some("key")
        }
        event::AppEvent::Term(Event::Mouse(mouse)) => {
            let dispatched_at = Instant::now();
            app.handle_mouse(*mouse);
            settle_edge_resistance(app);
            trace::trace!(
                "mouse {:?} col={} row={} mods={:?} us={}",
                mouse.kind,
                mouse.column,
                mouse.row,
                mouse.modifiers,
                dispatched_at.elapsed().as_micros()
            );
            Some("mouse")
        }
        event::AppEvent::Term(_) => Some("term"),
        // Spec 0224 S1: O(1). Terminal events share this channel in FIFO
        // order, so anything done here is paid by the next keystroke
        // behind it. Spec 0192 S3: it forces no frame of its own — it
        // only owes one within `HEAT_REPAINT_INTERVAL`.
        event::AppEvent::HeatWorkerProgress => {
            app.poll_pending_override_work();
            None
        }
        // Spec 0274 S9: the wake-up alone. The answer is collected by
        // the idle arm's `search_sweep_step`, which is where every other
        // step of a sweep happens too — doing it here would put a
        // sweep's progress in two places and give the drain a reason to
        // stop.
        event::AppEvent::SearchWorkerProgress => None,
    }
}

/// Spec 0245 S1c: whether the event just dispatched armed something
/// that takes the terminal away from `run_loop`'s draw.
///
/// A drain must stop here and leave the rest of the queue in the
/// channel. This is a correctness rule, not a tuning knob: dispatching
/// a keystroke after the user asked to suspend would deliver it to a
/// screen that is no longer ours.
/// Spec 0263 S2: whether the loop may block until an event arrives
/// instead of waking on `ACTIVITY_TICK` — that is, whether anything at
/// all is still owed a frame that no event will ask for.
///
/// A free function over its inputs so that the rule can be tested
/// without a running loop, which is the only practical way to cover a
/// condition whose failure mode is a screen that stays wrong until the
/// user touches something.
///
/// `activity_settled` is the caller's conjunction of the two activity
/// windows, the value the dot was last *drawn* with, and a fresh probe
/// of the worker. All four are needed: the first three say the dot on
/// screen is dark and has been for two windows, and the fourth says the
/// worker will not light it again on its own.
///
/// `bake_visible` cannot in fact be set at the one call site — the arm
/// that raises it leaves the receive loop immediately, and the frame it
/// forces clears it again. It is a term here anyway, because the
/// argument for leaving it out is a property of a control flow three
/// screens away and the cost of keeping it is one `&&`.
pub(super) fn may_sleep_indefinitely(
    ui_deadline: Option<Instant>,
    heat_dirty: bool,
    styles_stale: bool,
    bake_dirty: bool,
    bake_visible: bool,
    activity_settled: bool,
) -> bool {
    ui_deadline.is_none()
        && !heat_dirty
        && !styles_stale
        && !bake_dirty
        && !bake_visible
        && activity_settled
}

pub(super) fn control_transfer_pending(app: &App) -> bool {
    #[cfg(unix)]
    let editor = app.pending_editor_open.is_some();
    #[cfg(not(unix))]
    let editor = false;
    app.should_quit || app.should_suspend || editor
}

pub(super) fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    rx: &mpsc::Receiver<event::AppEvent>,
    input_reader: &mut Option<event::InputReaderHandle>,
    tx: &mpsc::Sender<event::AppEvent>,
    input_pending: &event::InputPending,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    // Spec 0190 S8: an activity tick that changes nothing must not cost
    // a frame. The gate has to sit *above* `terminal.draw` — skipping
    // work inside `render` would not work, because ratatui resets the
    // back buffer every frame, so a span that is not re-emitted is
    // erased rather than preserved.
    let mut redraw = true;
    // `PROTOLENS_TRACE` only: which of the three gate clauses below
    // forced this frame. Without it a trace full of back-to-back draws
    // says how much they cost but not who asked for them.
    let mut redraw_why = "initial";
    // Spec 0191 S3: a two-window sliding maximum of the activity level.
    // `Tier` is `Ord` (`Prefetch < Visible < User`) and `Option<T>`
    // orders `None < Some(_)`, so "highest seen" is just `max`. One
    // window is one iteration of this loop; the dot shows
    // `max(previous, current)`, so a rise lands on the very next frame
    // while a fall has to be confirmed by two consecutive quiet windows.
    //
    // Both windows are *reset*, never merely accumulated. An earlier
    // draft kept a single high-water mark reset only at a draw, which
    // deadlocked: with the mark stuck high and the dot already showing
    // that level, the gate below saw no difference, so no draw
    // happened, so the mark was never reset — the dot stayed lit
    // forever after a read-ahead wave finished.
    let mut activity_window: Option<tiered::Tier> = None;
    let mut activity_prev_window: Option<tiered::Tier> = None;
    // Spec 0192 S3: a completed heat request marks the frame dirty
    // instead of forcing one. `last_heat_frame` is stamped by *any*
    // draw, not only a heat-driven one — a frame the user just caused
    // has already shown whatever cues had arrived, so the interval
    // should run from it.
    let mut heat_dirty = false;
    let mut last_heat_frame = Instant::now();
    // Spec 0223 S4: set whenever the frame just drawn was the
    // monochrome one, and cleared by the colored frame that replaces
    // it. This is what *guarantees* G2 — that the frame the user reads
    // is in color — rather than leaving it to emerge from the gate
    // below. As the gate stands every terminal event forces its own
    // frame, so the last queued event is followed by a draw that
    // samples an empty counter and colors itself; but that argument is
    // a property of four `*_forces` clauses that will keep changing,
    // and its failure mode is a screen that stays gray until the user
    // touches something. The flag makes the recolor owed rather than
    // incidental.
    let mut styles_stale = false;
    let mut last_mono_frame = Instant::now();
    // Spec 0255 S6: the bake owes a frame the way a completed cue does —
    // the document got taller, so the footer's line count and the
    // scrollbar are wrong until something redraws them. Nothing else
    // will ask.
    let mut bake_dirty = false;
    let mut last_bake_frame = Instant::now();
    // Spec 0249 S8: and a bake step that expanded a stop the user was
    // looking at owes that frame immediately instead — the rows it
    // replaced are the ones being read, not the footer's total.
    let mut bake_visible = false;
    // Spec 0257 S3/S4 reversed spec 0255 S2's assignment here: the flag
    // is now set by `App::new`, from the budget `main` chose for the
    // *startup* render, because that render has to be bounded too and
    // the pass inside `App::new` must already see the flag. The rule it
    // stood for is unchanged — only a session with a loop to bake in is
    // bounded, and a headless export still passes no budget.
    // Not asserted here: the event-loop tests drive `run_loop` over
    // fixtures built unbounded on purpose.
    loop {
        // A background thread died (see the hook in `run`). Say so, and
        // let go of the worker: its thread is gone, so every request
        // still queued and every one pushed from here on would go
        // unanswered and leave the cue at `[?]` forever. Dropped, the
        // cue path falls back to computing on this thread — slower, but
        // it answers.
        if let Some(panic) = take_background_panic() {
            app.message = format!("background thread failed, cues are now inline: {panic}");
            if let Some(worker) = app.heat_worker.take() {
                worker.shutdown();
            }
            redraw = true;
        }
        if redraw {
            app.activity_shown = activity_prev_window.max(activity_window);
            // Spec 0223 S1/S3: sampled here rather than read from inside
            // `render`, which keeps the counter out of `App` and lets the
            // render tests set the flag directly.
            app.input_pending = input_pending.is_pending();
            let drawn_at = Instant::now();
            terminal.draw(|frame| app.render(frame))?;
            styles_stale = app.input_pending;
            if styles_stale {
                last_mono_frame = drawn_at;
            }
            heat_dirty = false;
            last_heat_frame = drawn_at;
            bake_dirty = false;
            bake_visible = false;
            last_bake_frame = drawn_at;
            trace::trace!("draw {redraw_why} us={}", drawn_at.elapsed().as_micros());
            // Sample again immediately after the draw. `render` is
            // itself a producer — `heat_cue_for` pushes a `Visible`
            // request per unsettled visible row — so the requests this
            // draw just queued belong to the current window. Sampling
            // only before the draw is what made the dot emit one dark
            // frame per completed request (G4).
            activity_window = activity_window.max(app.heat_activity());
        }
        // While a status message is pending auto-dismissal
        // (`message_deadline`, `track_message_timeout`) or the splash
        // screen hasn't yet auto-dismissed (`splash_deadline`), the
        // receive below must wake by that deadline, so that the next
        // `render()` — which is what actually clears an expired message
        // or splash — runs even with no further event. Spec 0152 G8:
        // the input-reader thread owns the `event::poll`/`event::read()`
        // pair and forwards through `rx` alongside the worker thread's
        // own progress notifications, so this loop sleeps until there's
        // a reason to wake instead of polling on a fixed schedule.
        //
        // Spec 0280 S11 adds a third of exactly the same shape: a
        // pointer resting on an annotated type name has earned its score
        // box at `hover_deadline`, and `track_hover_dwell` — like
        // `track_message_timeout` — is a `render()` step, so the frame
        // that opens the box is the frame this deadline buys. Nothing
        // arms it but the pointer landing on such a name, so an untouched
        // mouse leaves spec 0263's idle guarantee exactly as measured.
        let splash_deadline = app.splash.then_some(app.splash_deadline);
        let ui_deadline = [app.message_deadline, splash_deadline, app.hover_deadline]
            .into_iter()
            .flatten()
            .min();
        // Spec 0190 S7: the activity tick is a third candidate deadline
        // alongside those two, and unlike them it is always present.
        //
        // It is still computed unconditionally, because the background
        // steps below yield on it. What it no longer does is keep the
        // loop awake forever: spec 0263 S1 makes the *sleep* itself
        // untimed once nothing is owed, so `deadline` from here on is
        // the schedule for work in progress rather than for idleness.
        let activity_deadline = Instant::now() + ACTIVITY_TICK;
        let mut deadline = ui_deadline.map_or(activity_deadline, |d| d.min(activity_deadline));
        // Spec 0192 S3: a deferred heat repaint is a fourth candidate
        // deadline. Without it, a completion arriving inside the
        // interval would be shown only when some unrelated event next
        // woke the loop — the same class of self-deadlock spec 0191 S3
        // hit, and the activity tick would merely bound it at 250 ms
        // rather than remove it.
        if heat_dirty {
            deadline = deadline.min(last_heat_frame + HEAT_REPAINT_INTERVAL);
        }
        // Spec 0223 S4: and a deferred recolor is a fifth, for the same
        // reason — the frame that puts the color back is owed to the
        // user with no event of its own to ask for it.
        if styles_stale {
            deadline = deadline.min(last_mono_frame + STYLES_SETTLE_INTERVAL);
        }
        // Spec 0255 S6: and a bake's growth is a sixth. Without the
        // clause the last frame of a bake waits for an unrelated event —
        // the self-deadlock spec 0191 S3 and spec 0192 S3 each hit in
        // turn, and the activity tick would only bound it at 250 ms
        // rather than remove it.
        if bake_dirty {
            deadline = deadline.min(last_bake_frame + BAKE_REPAINT_INTERVAL);
        }
        // Spec 0192 S4: read-ahead's guaranteed slice, taken before the
        // receive loop below rather than only in its `Empty` arm.
        for _ in 0..PREFETCH_STEPS_PER_ITERATION {
            if matches!(app.prefetch_step(), PrefetchStep::Idle) {
                break;
            }
        }
        // Spec 0164 G7: interleave one unit of inline prefetch work
        // with a non-blocking channel check on every idle-wait
        // iteration, so read-ahead never holds the thread for more
        // than a single `TieredBounded::upsert` push before yielding
        // to a pending event. Falls back to the deadline-aware receive
        // above only once `prefetch_step` reports `Idle` (walk
        // exhausted or capacity-rejected).
        let received = loop {
            match rx.try_recv() {
                // Spec 0223 S2: no arm here for a bare mouse-move. The
                // reader thread now drops one before it reaches the
                // channel, which both keeps it from starving read-ahead
                // (its reason for being discarded here) and keeps a
                // hovering pointer from looking like pending input.
                Ok(ev) => break Some(ev),
                // Four background jobs share this arm, in a fixed order
                // (spec 0255 S5, spec 0256 S2). Each does one unit and
                // yields, both to a pending event and to the deadline
                // computed above — an expiring message, the splash's
                // auto-dismiss, a due repaint, the activity tick — none
                // of which has an event to announce it, and this loop is
                // the only thing between them and the frame that honors
                // them. Without the check a worker holds the thread
                // until it runs dry and all of them are simply late.
                // Breaking with no event is exactly the timeout case the
                // sleep below produces, and the `*_forces` tests just
                // past the loop already know what to do with it.
                Err(mpsc::TryRecvError::Empty) => {
                    // Spec 0235 S3: first, because a sweep exists only
                    // while a pattern is being typed and the user is
                    // waiting on its answer. The other two are not
                    // waited on by anyone.
                    match app.search_sweep_step() {
                        SweepStep::Progressed => {
                            if Instant::now() >= deadline {
                                break None;
                            }
                            continue;
                        }
                        // Spec 0274 S10: a segment scan is in flight on
                        // a worker thread. The other three jobs stand
                        // aside for it — the bake would have to stop it
                        // to write the document, and the two heat jobs
                        // would take the core it is running on — and
                        // this pass falls through to the sleep, which
                        // its report will end.
                        SweepStep::Waiting => {}
                        SweepStep::Idle => {
                            // Spec 0256 S2: second, and ahead of the
                            // bake because the bake is what grows the
                            // replacement document. Draining after it
                            // would hold the old document's text alive
                            // next to the new one and raise peak
                            // memory; draining first keeps peak where
                            // it was. Sets no dirty flag and breaks
                            // with no event: nothing on screen depends
                            // on it, so unlike the bake it does not
                            // even owe a deferred repaint.
                            if app.discard_step() {
                                if Instant::now() >= deadline {
                                    break None;
                                }
                                continue;
                            }
                            // Spec 0343 B6 stage 2: one trie chunk,
                            // between discard and bake.  Deliberately
                            // does NOT `continue` — one idle pass does
                            // one chunk *and* one bake step, so the
                            // bake is not starved while the trie builds
                            // and the document stops growing on screen.
                            // The chunk is sized to leave the bake step
                            // within the frame budget (B6).
                            app.shadow_step();
                            // Spec 0255 S5: third — ahead of read-ahead,
                            // and not because it is the more deserving
                            // of the two. Read-ahead cannot make
                            // progress between two bake steps at all:
                            // each splice bumps `structural_version`,
                            // which supersedes the read-ahead wave, so
                            // interleaving them buys a full re-walk and
                            // three lock acquisitions per bake step and
                            // issues heat requests for rows the next
                            // bake step will move — spec 0252's waste,
                            // reintroduced. The queue is finite, so this
                            // defers read-ahead until the structure
                            // holds still, which is the only state it
                            // can work in.
                            match app.bake_step() {
                                // Spec 0249 S8: the rows on screen just
                                // changed under the reader, so this one
                                // does not wait for
                                // `BAKE_REPAINT_INTERVAL` or for
                                // anything else. Leaving with no event
                                // is the timeout case, and the draw at
                                // the head of the next iteration is the
                                // point.
                                BakeStep::Visible => {
                                    bake_visible = true;
                                    break None;
                                }
                                BakeStep::Progressed => {
                                    bake_dirty = true;
                                    if Instant::now() >= deadline {
                                        break None;
                                    }
                                    continue;
                                }
                                BakeStep::Idle => {}
                            }
                            if matches!(app.prefetch_step(), PrefetchStep::Progressed) {
                                if Instant::now() >= deadline {
                                    break None;
                                }
                                continue;
                            }
                            // Spec 0274 S10: collecting a verdict
                            // reports `Idle` so that the three jobs
                            // above each get their step — but it is a
                            // yield, not an answer, and the frozen
                            // queue still owes segments that nothing
                            // else will ever wake this loop to hand
                            // out. Placed after them so that the yield
                            // is honored and before the sleep so that
                            // it cannot become an untimed one.
                            if app.search_segment_pending() {
                                continue;
                            }
                            // Spec 0277 S9: fifth and last, because it
                            // is the only one of the five that nothing
                            // on screen is waiting for — the sweep has
                            // already said *where* the match is, and
                            // this only says how many there are.
                            if matches!(app.search_tally_step(), SweepStep::Progressed) {
                                if Instant::now() >= deadline {
                                    break None;
                                }
                                continue;
                            }
                        }
                    }
                    // Nothing left to do, so this is the one place the
                    // loop genuinely sleeps — and therefore (spec 0263
                    // S1) the one place that decides between sleeping on
                    // a timer and sleeping until something happens.
                    //
                    // The decision is made *here* rather than where
                    // `deadline` is computed so that the arms above keep
                    // yielding on the tick. Reaching this line means all
                    // four background steps reported `Idle` in this
                    // pass, so there is no such work left to yield from.
                    //
                    // Spec 0235 S5's frame is owed before any of that is
                    // considered. The step that settles a sweep's answer
                    // sets `search_dirty` and still reports
                    // `Progressed`, so the *next* pass is the first to
                    // arrive here — and until spec 0263 made this sleep
                    // untimed, the activity tick happened to deliver
                    // that frame within 250 ms. It no longer does: the
                    // prompt would sit yellow on a pattern the sweep had
                    // already ruled out until some unrelated event woke
                    // the loop, which in practice meant until the user
                    // pressed a key to ask why. Breaking rather than
                    // shortening `deadline`, because there is nothing
                    // left to wait for. `take_search_dirty` past the
                    // loop is what clears it, so this fires once.
                    if app.search_frame_owed() {
                        break None;
                    }
                    let activity_settled = activity_prev_window.is_none()
                        && activity_window.is_none()
                        && app.activity_shown.is_none()
                        && app.heat_activity().is_none();
                    if may_sleep_indefinitely(
                        ui_deadline,
                        heat_dirty,
                        styles_stale,
                        bake_dirty,
                        bake_visible,
                        activity_settled,
                    ) {
                        // No timer anywhere in the process now: the heat
                        // workers are on their condvar and the input
                        // reader is in an untimed `poll`. A `RecvError`
                        // is impossible while this loop holds `tx`, and
                        // is diagnosed by the `try_recv` at the head of
                        // the next iteration if it ever becomes so.
                        break rx.recv().ok();
                    }
                    let timeout = deadline.saturating_duration_since(Instant::now());
                    break rx.recv_timeout(timeout).ok(); // timeout elapsed => None
                }
                // Cannot fire as the code stands: this loop holds `tx`
                // for the Neovim handoff's reader respawn below, so a
                // sender outlives every receive here. Reported as an
                // error rather than as `Ok(())` all the same — if that
                // invariant is ever broken the session has lost its
                // input, and exiting 0 in silence is the one answer that
                // is certainly wrong.
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "the input channel disconnected",
                    ));
                }
            }
        };
        // Spec 0223 S1: the one decrement site. The loop above has two
        // receiving arms — the `try_recv` at its head and the
        // `recv_timeout` in the `Idle` case — and both funnel into this
        // binding, so counting here cannot miss one or count one twice.
        // `note_received` ignores a `HeatWorkerProgress`, which was
        // never counted up.
        if let Some(ev) = &received {
            input_pending.note_received(ev);
        }
        // Spec 0190 S8, one of the reasons to draw. Sampled ahead of the
        // dispatch below because nothing in it can change the activity
        // byte without also having produced the event that already
        // forces a redraw.
        //
        // Spec 0191 S4: it compares the debounced value,
        // not a fresh probe. A fresh probe forces a frame on every
        // trough between two requests; folding them into the sliding
        // maximum keeps high-frequency toggling from costing main-thread
        // frames during exactly the period when the worker wants the
        // CPU.
        activity_window = activity_window.max(app.heat_activity());
        // Spec 0245 S1: dispatch everything already queued, then draw
        // once. A burst of input the loop absorbs comfortably costs one
        // frame, not one frame each — and since `note_received` runs for
        // every event the pass takes, the counter is back at zero by the
        // time that frame samples it, so the frame is in color. Spec
        // 0223's monochrome now means the machine is genuinely behind
        // the input, which is what it was meant to mean.
        //
        // Dispatch order is untouched: coalescing removes frames, never
        // events. A pane or mode change mid-burst is therefore not a
        // reason to stop — the resulting state is identical, and the
        // user generated every event in the burst against the pre-burst
        // frame anyway.
        let mut event_forces = false;
        let mut event_why = "term";
        let drain_started = Instant::now();
        let mut current = received;
        while let Some(ev) = &current {
            // Spec 0192 S3: a completed heat request is the one event
            // that does not force its own frame — it only owes one
            // within `HEAT_REPAINT_INTERVAL`. Since spec 0224 that
            // deadline is the *whole* of how the answer reaches the
            // screen: the dispatch no longer touches `heat_states`, and
            // the frame this flag buys re-reads the cache for every
            // unsettled row it draws.
            if matches!(ev, event::AppEvent::HeatWorkerProgress) {
                heat_dirty = true;
            }
            // Spec 0245 S2: cleared before every dispatch, so the
            // default is to redraw; only a pan that hit its bound sets
            // it.
            app.event_changed_nothing = false;
            if let Some(why) = dispatch_event(app, ev) {
                if !app.event_changed_nothing {
                    event_forces = true;
                    event_why = why;
                }
            }
            // Spec 0245 S1c, then S1b, then S1a: stop at a control
            // transfer, stop once the pass has spent its budget, and
            // otherwise take only what is *already* queued. The last is
            // what keeps the drain from chasing a producer that outpaces
            // it and never drawing at all.
            if control_transfer_pending(app) || drain_started.elapsed() >= DRAIN_BUDGET {
                break;
            }
            current = match rx.try_recv() {
                Ok(ev) => {
                    input_pending.note_received(&ev);
                    Some(ev)
                }
                // A disconnect is diagnosed by the receive at the head
                // of the next iteration, which is the one place that
                // knows how to report it.
                Err(_) => None,
            };
        }
        let heat_forces = heat_dirty && Instant::now() >= last_heat_frame + HEAT_REPAINT_INTERVAL;
        // Spec 0255 S6, and the half of it the deadline clause above
        // cannot do on its own: waking for the repaint is not drawing
        // it. Without this the bake's frame is owed forever and arrives
        // only when some unrelated event happens to force one — the
        // document grows by millions of lines and the footer keeps
        // reporting the first screenful.
        let bake_forces = bake_visible
            || (bake_dirty && Instant::now() >= last_bake_frame + BAKE_REPAINT_INTERVAL);
        // Spec 0223 S4/G2. Gated on the queue being *empty*: recoloring
        // a viewport the user is still scrolling past would pay the
        // highlighting cost for a frame nobody stops on, which is the
        // whole thing this spec exists to avoid.
        let styles_force = styles_stale
            && !input_pending.is_pending()
            && Instant::now() >= last_mono_frame + STYLES_SETTLE_INTERVAL;
        let deadline_forces = ui_deadline.is_some_and(|d| Instant::now() >= d);
        let activity_forces = activity_prev_window.max(activity_window) != app.activity_shown;
        // Spec 0235 S5: a sweep draws when its *answer* changes, not
        // when it does work — it steps hundreds of times a second and a
        // frame each would be the very stall this is meant to avoid.
        let search_forces = app.take_search_dirty();
        redraw = event_forces
            || heat_forces
            || bake_forces
            || styles_force
            || deadline_forces
            || activity_forces
            || search_forces;
        redraw_why = if event_forces {
            event_why
        } else if heat_forces {
            "heat"
        } else if bake_forces {
            if bake_visible {
                "bake-visible"
            } else {
                "bake"
            }
        } else if styles_force {
            "styles"
        } else if deadline_forces {
            "deadline"
        } else if search_forces {
            "search"
        } else {
            "activity"
        };
        // Close the window. Everything above sampled into
        // `activity_window`; from here it is history, and the next
        // iteration starts clean. This is the reset the earlier
        // single-high-water-mark draft lacked.
        activity_prev_window = activity_window;
        activity_window = None;
        if app.should_quit {
            return Ok(());
        }
        #[cfg(unix)]
        if app.should_suspend {
            app.should_suspend = false;
            suspend(terminal)?;
        }
        #[cfg(unix)]
        if let Some(req) = app.pending_editor_open.take() {
            // `open_editor` backgrounds this process's own process group
            // relative to the terminal (`tcsetpgrp(io::stdin(),
            // nvim_pgid)`) for as long as Neovim owns the foreground.
            // The input-reader thread (spec 0152 G8) is otherwise
            // permanently blocked in `event::read()` on that same stdin
            // — a background process's read from its controlling
            // terminal draws SIGTTIN, whose default disposition stops
            // the *whole process* (every thread, not just this one),
            // with nothing left to `SIGCONT` it back. Shutting it down
            // before the handoff and respawning a fresh one right after
            // `open_editor` reclaims the terminal is what keeps anything
            // else from touching stdin while Neovim has it.
            if let Some(reader) = input_reader.take() {
                reader.shutdown();
            }
            neovim::open_editor(terminal, app, req)?;
            *input_reader = Some(event::InputReaderHandle::spawn(
                tx.clone(),
                input_pending.clone(),
            ));
        }
    }
}
