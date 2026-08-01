<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens codebase audit — quality findings

*written 2026-07-31 against `bc586dc`; second pass 2026-08-01*

One of three companion audit documents:

- [audit-module-sizes.md](audit-module-sizes.md) — module sizes and
  proposed splits.
- [audit-duplication.md](audit-duplication.md) — factorization and
  deduplication opportunities.
- **this file** — everything else worth fixing, in priority order.

Items are numbered so they can be cited. Each was verified against the
code; line numbers will drift. Where a finding was reasoned from the
code rather than reproduced, it says so.

The second pass swept four areas the first pass had only skimmed:
untrusted-input handling and the `unsafe` blocks, concurrency and
terminal lifecycle, the batch/CLI path, and test coverage. It found
enough that the document is now ordered by **what the user sees when it
goes wrong** rather than by subsystem.

## A. Silently wrong output — ALL FIXED 2026-08-01

The worst class in this codebase, because protolens' entire job is to
tell you what some bytes mean. A crash is recoverable; a plausible-
looking wrong answer is not.

Every item below is fixed; each carries a short note on what was done.
The descriptions are kept because they say what the failure *looked
like*, which is what a future reader needs to recognize a relapse.

### A1. One bad byte turns a text file into an empty document — HIGH

`blob.rs:93` calls `encode_text_to_binary_into(&text, &mut buf)`. That
function **has no return type**: on invalid UTF-8 it returns at
`prototext-core/src/serialize/encode_text/mod.rs:190-193`, leaving the
output buffer exactly as it found it.

So a `#@ prototext` file containing a single non-UTF-8 byte opens as a
zero-length blob — no error, no warning, just an empty document. The
comment at that site (`// The text is always valid ASCII`) states an
assumption the caller does not enforce.

**Fixed** by a UTF-8 check in `Blob::load`, ahead of the encode, that
returns `io::ErrorKind::InvalidData` naming the offending byte offset.
That is the system boundary and it keeps the change inside one crate;
changing the encoder's public signature would have reached
prototext-core for no extra benefit. Guarded by
`a_text_blob_with_one_bad_byte_is_an_error_not_an_empty_document`, which
also pins that `--assume-binary` still takes the other producer.

### A2. Batch export to stdout never flushes — HIGH

`main.rs:556-562` and `:579-585` both `write_all` to `std::io::stdout()`
and return `ExitCode::SUCCESS`. Neither flushes. The **only** `flush` in
the crate is in the TUI teardown.

Rust's runtime does flush stdout at exit, but it discards the error. So
`protolens export ... > /full/disk` writes a truncated stream and exits
`0`. A script piping protolens into a build cannot tell.

**Fixed**: both paths now go through one `write_stdout` helper that locks
stdout, writes and flushes, with either error mapped to
`ExitCode::FAILURE`.

### A3. `App::new`'s own refusals are invisible in batch

`main.rs:510`. Spec 0221 routes refused overrides to stderr and makes
the export fail — but the check sits **inside** the
`if let Some(overrides_path)` block. Overrides can also arrive from the
document's own inference, and a refusal on that path in batch mode still
produces a clean exit and a file. This is 0221's defect on the other
branch.

**Fixed**: the check moved out of that block and now runs on every
export. The message names `--load-overrides <path>` when there was one
and says plain `override` otherwise.

### A4. An extension can render as the wrong type

`override_apply.rs:851` and `:853` narrow with `type_id as u32`. On
truncation the result is a *valid but different* type id, so the node
renders as the wrong extension silently — no error, no refusal, just
wrong output.

**Fixed**: `u32::try_from(...).ok()?`. The enclosing function returns
`Option<String>`, so refusing simply leaves the node untyped.

### A5. Wire-derived field numbers are narrowed unchecked

Three sites take a field number parsed from an untrusted varint and
narrow it:

- `extract.rs:102-106` — `field_number as u32` is fed to `write_tag` to
  reconstruct the END_GROUP tag whose length is then subtracted from
  the range. A truncating value produces a different-length tag, so an
  exported group is cut at the wrong byte.
- `decode.rs:1097` — `as i32`.
- `export_descriptor.rs:82` — `f.number` is a `u64` written straight
  into `FieldDescriptorProto.number` as `i32`, **with no validation**,
  and that descriptor goes to disk for another tool to consume.

**Fixed, and the premise turned out to be half wrong.** Field numbers
above 2^29 are already refused *once*, and it has been that way since
spec 0212: `NodeSpan::field_number` is a `u32`, and a tag whose field
number does not fit never reaches a span — the sink reports it through
`Sink::malformed` instead (`render_text/sink.rs`, on the field itself).

So two of the three sites were reading a value already bounded, and only
their `u64` types hid it. Those are now `u32` end to end
(`register_wrapper`, `synthetic_wrapper_name`, `ResolvedField::number`,
`resolve_export_fields`'s grouping), which makes the narrowing to
`FieldDescriptorProto`'s `i32` exact by construction rather than by
luck, with the invariant cited at each cast.

`extract.rs` was the real one: `message_payload_range` takes raw bytes
and a range and re-parses the tag itself, so it has no span to lean on.
It now requires `wfield_oor` to be `None` — the parser's own report that
the field number is neither 0 nor at least 2^29 — before rebuilding the
END_GROUP tag, and falls through to the tag-only strip otherwise.

### A6. An unreadable descriptor becomes an empty one

`decode.rs:240`: `read_descriptor_file(path).unwrap_or_default()`. A
permissions error, a truncated file and a genuinely empty descriptor set
are indistinguishable downstream.

The consequence is narrower than first written — this is
`descriptor_sha256`, not the pool load (which already uses `?`). But it
is still silently wrong output: a `:save` whose descriptor set has been
removed or chmod'ed since startup writes the hash of *nothing* into the
overrides file, and the `:restore` that reads it back then warns about a
mismatch that never happened.

**Fixed**: `descriptor_sha256` and `target_hashes` are fallible;
`run_save_overrides` reports through the status line and writes nothing,
`load_overrides` propagates.

Worth knowing before touching this again: 17 test fixtures write a
descriptor set to a temp path, load it, and `remove_file` it
immediately. Two of them (`type_as` and the MessageSet-expansion one)
back tests that hash the descriptor set, so they now keep the file. The
forgiving `unwrap_or_default` was the only reason that idiom worked for
them.

### A7. A dead reader is reported as a clean exit

`mod.rs:2583` maps `Err(Disconnected)` to `return Ok(())`. If the input
reader thread dies, protolens exits with status 0 and says nothing.

Compounding it: that arm is currently **unreachable**, because
`run_loop` holds its own clone of the sender for the whole loop. The
disconnect it is meant to catch cannot fire.

**Fixed as far as it goes**: the arm returns an `io::Error` instead of
`Ok(())`, so the exit is no longer silent and no longer 0.

The unreachability is *not* fixed, and should not be: `run_loop` needs
`tx` to respawn the input reader after the Neovim handoff, so a sender
provably outlives every receive in the loop. Detecting a reader thread
that has died is a different feature — the channel cannot tell you.
(`rx.recv_timeout(...).ok()` on the idle path folds `Disconnected` into
the timeout for the same reason, and was left alone.)

## B. The terminal or the session is left broken

B1–B6 are fixed (2026-08-01); **B7 is not**, and is a design question
rather than a defect to patch — see its own note. The descriptions are
kept because they say what the failure *looked like*, which is what makes
a regression recognizable.

### B1. Quitting the TUI freezes on a large blob — HIGH

`heat_worker.rs:603-608`. `shutdown_inner` joins the worker thread, but
`score_all` has **no cancellation check**. On googleapis the join waits
for a full scoring pass to run to completion, so pressing `q` appears to
hang for several seconds with the terminal already handed back.

The fix is a cancellation flag polled inside `score_all`'s walk, not a
detached thread — the worker owns state the shutdown path needs to see
settled.

**Fixed.** `score_subset` gained a `cancel: Option<&AtomicBool>`, polled
once per wire field at every recursion level in `score_message_multi`.
The poll returns the buffer length, which is what makes the unwind
immediate: every enclosing loop already stops at `pos == buflen`.
`HeatRequestQueue` republishes its mutex-guarded `stop` as an
`AtomicBool` — a duplicate, not a replacement, because moving `stop` out
of the lock would open a lost-wakeup window against the condvar in
`pop_blocking`. `signal_stop` raises the atomic *before* taking the lock,
so a worker mid-sweep starts unwinding while the quitting thread is still
acquiring.

A cancelled sweep returns a partial ranking, and the worker discards it
rather than writing it to the cache — the cache outlives the thread, and
a partial ranking there is indistinguishable from a real one. Nothing is
reported back through the library: the caller raised the flag, so it
already knows.

Granularity is one wire field, not one part, so `--jobs 1` — the
un-sharded path, and the slowest sweep — is cancelled just as promptly as
a 24-part one.

### B2. After any panic the shell has no visible cursor — HIGH

`restore_terminal` (`mod.rs:2227-2232`) does not issue `Show`. The last
`Terminal::draw()` hides the hardware cursor unless the render callback
sets a position, and only the command row does that — so on a normal
frame the cursor is hidden when the panic hook runs, and it stays hidden
for the life of the terminal.

The codebase already knows this: `suspend` (`mod.rs:2246-2255`) calls
`restore_terminal()` and then `terminal.show_cursor()`, under a comment
explaining exactly this failure. The `Show` belongs inside
`restore_terminal`, where all three callers get it.

**Fixed**, and the three `terminal.show_cursor()` calls that were working
around it are gone with it — `suspend`, `run`'s cleanup and
`neovim::open_editor` each called it on the line after
`restore_terminal()`. Checked against ratatui 0.30 before removing them:
`Terminal::draw` calls `hide_cursor` unconditionally when the frame set
no position, and the `hidden_cursor` flag it maintains is read nowhere
outside its own tests — so nothing was depending on ratatui's
bookkeeping being updated, only on the escape sequence being written.

### B3. A worker panic tears the terminal out from under a live UI — HIGH

The hook installed at `mod.rs:2291-2295` is **process-global**. A panic
on the heat-worker thread — or on a sweep shard — runs
`restore_terminal()` while the main loop is still drawing into an
alternate screen that has just been left.

The aftermath is worse than the moment: `app.heat_worker` stays `Some`
forever, so every cue renders `[?]` (`heat_cue.rs:418`), `in_flight` is
never cleared so the activity indicator stays lit, and
`let _ = join.join()` discards the panic. The session degrades silently
into one that answers nothing.

A hook that checks `thread::current().id()` against the UI thread, and
otherwise records the failure for the main loop to report, is the
minimum.

**Fixed**, as described: `run` captures its own thread id when it
installs the hook, and a panic from anywhere else goes into a
`BACKGROUND_PANIC` slot instead of touching the terminal. The first
panic wins; a second background panic is usually the first one's
consequence (a sweep shard's panic is re-raised on the worker thread by
`resume_unwind`, so one failure records twice).

`run_loop` drains the slot at the top of each iteration, puts the message
in the message row, and — this is the part that fixes the *aftermath* —
drops `app.heat_worker`. The handle's thread is dead, so every queued
request and every future one would go unanswered and leave the cue at
`[?]` forever; without the handle, `heat_cue_for` falls back to its
synchronous path, which is slower but answers.

### B4. `?` in the neovim handoff leaves the terminal broken

`neovim.rs:209` and `:214` propagate with `?` on paths where the
terminal is already out of raw mode and a foreground process group has
been set. An early return there leaves cooked mode and a dangling pgid.
These need cleanup on the error path, not `?`.

**Fixed.** Both now reclaim the terminal (`tcsetpgrp` back to our own
pgid), report through `app.message`, re-enter raw mode and return
`Ok(())` — the shape the "cannot launch nvim" arm three lines up already
used. The `waitpid` reclaim is unconditional and runs *before* the result
is examined, since a failed wait hands the terminal back no more than a
successful one does. A failed `SIGCONT` also forgets the instance
(`EditorState::NotRunning`, socket removed), so the next `v` spawns a
fresh Neovim instead of signalling the same corpse.

Moving the `killpg` out of the `match` arm is what made this possible at
all: the arm borrows `app.editor_state`, so nothing inside it can write
to `app`.

### B5. `std::env::set_var` in a multi-threaded process

`neovim.rs:151-159` calls `std::env::set_var` under a SAFETY comment
asserting no other threads are running. **That assertion is false**: the
heat worker thread is alive at that point — the handoff shuts down the
*input reader*, never the worker. `set_var` is unsound with concurrent
readers, and Rust 2024 makes it `unsafe` for exactly this reason.

The fix is three lines away and already in the file: pass the variable
through `Command::env` on the child, which is what `PROTOTEXT_PROTO_ROOT`
does at `:166`. No process-global mutation is needed at all.

**Fixed**, and more cheaply than that: not even `Command::env` is needed.
Nothing downstream reads `PROTOLENS_NVIM_CONFIG` — grep the workspace and
the only reader is the `-u` flag two lines below, five words from where
the variable was set. It is now a local, and the whole `unsafe` block is
gone.

### B6. Setup and teardown are not inverses

`restore_terminal` pops the keyboard-enhancement flag *before*
`LeaveAlternateScreen`, though setup pushes it *before*
`EnterAlternateScreen`; and it calls `disable_raw_mode` before
`DisableMouseCapture`, so the mouse-disable escape is written in cooked
mode. Neither is known to misbehave, but a pair that is not symmetric
has to be re-verified by hand every time either side changes.

Related: the cleanup at the end of `run` is a straight-line block, not a
guard, and `InputReaderHandle` has no `Drop`. The closure trick at
`:2332` covers the fallible region — but only that region.

**Fixed** for the ordering: `restore_terminal` is now the exact reverse
of setup — `DisableMouseCapture`, `LeaveAlternateScreen`, `Show`, then
the keyboard-enhancement pop, then `disable_raw_mode`. `drain_pending_
input` stays first, and is the one deliberate exception: it has to read
while raw mode is still on.

**Not done**: the "related" half. The cleanup block is still
straight-line and `InputReaderHandle` still has no `Drop`. Both are only
reachable by a panic, which the hook already covers for the terminal
itself; making them guards is a separate change with its own drop-order
questions.

### B7. No signal handling at all — NOT FIXED

SIGTERM, SIGHUP and SIGINT are unhandled. A `kill` or a closed terminal
emulator leaves raw mode and the alternate screen behind.

Left open deliberately, and all three matter. Crossterm's raw mode clears
`ISIG`, so the *terminal driver* no longer turns the INTR character into
a signal and Ctrl-C inside protolens arrives as a key event — but that
governs only how the signal would be generated, not whether it can be
delivered. `kill -INT`, a `kill(2)` from any process and a shell's
`kill %1` all still deliver SIGINT with its default terminating
disposition, leaving raw mode and the alternate screen behind exactly as
SIGTERM would. Raw mode changes the *likelihood*, not the exposure.

Handling them is not a patch: a handler runs in a context where almost
nothing it would want to call is legal, so the honest shapes are a
self-pipe woken by the event loop, or `signal_hook`'s flag plus a poll,
and either is a new dependency or a new thread plus a decision about what
a half-finished `:save` should do. That is a design, and it belongs in a
spec rather than in a fix pass.

The file already contains the proof that dispositions are load-bearing
here: `neovim.rs` installs `SIGTTOU` → `SIG_IGN` for the process's
lifetime, because the default would have stopped protolens mid-handoff.

## C. Panics on hostile or unusual input

All of C1–C9 are fixed (2026-08-01); each item carries a note below.
C9 turned up a live wrap nothing else had: `structure.rs`'s
`last_child`/`prev_sibling` used `bool::then_some`, whose argument is
evaluated *whether or not the condition holds*, so `0 - 1` ran on the
root slot and on every childless node. Harmless in a wrapping build —
the value is discarded — but it is the exact reason to turn the checks
on.

### C1. A dead special case that hides an overflow — MEDIUM

`heat_cue.rs:346-377`. The doc comment describes the pre-spec-0216
world, and the packed special case at `:366-377` is now redundant:
`extract.rs:76-78` already handles packed runs. Worse, the dead branch
contains a `usize` overflow — `start + len.varint... as usize` wraps on
a crafted length, after which `:441` slices with the wrapped value and
panics.

Deleting the branch fixes the documentation drift and the overflow in
one change, and removes code rather than adding a guard.

The second pass **proved the branch dead** rather than suspecting it:
`decode.rs:913-916` now sets `packed_record_start` to the same
`raw_start[slot]` that `raw_range` begins at, so both arms return the
same range. See item 1.1 of
[audit-duplication.md](audit-duplication.md), which folds this into a
larger seven-site consolidation — do the two together.

**Fixed.** The branch is deleted; `heat_scored_range` is now the single
call to `extract::message_payload_range`. The end comes from the
arena's `raw_end`, a real offset into the blob, rather than from
`start + length`, so there is nothing left to overflow. The seven-site
consolidation is still open and now has one fewer site.

### C2. Half a range is clamped

`extract.rs:179-180`:

```rust
let end = r.end.min(lines.len());
format!("{PROTOTEXT_HEADER}{}", dedent(&lines[r.start..end]))
```

`end` is clamped, `start` is not. A `start` past `lines.len()`, or past
the clamped `end`, panics in the slice. If the clamp is needed at all it
is needed on both ends.

**Fixed.** `start` is clamped to the already-clamped `end`, so an
overrunning range yields an empty extract rather than a panic.

### C3. `lines[0]` with no guard

`decode.rs:1455-1458` indexes `lines[0]` on the wrapper path. Every
current caller produces a non-empty `lines`, but nothing states or
checks it.

**Fixed.** `lines.first()`, threaded through the existing
`and_then`: a render that produced no text has no synthetic field name
to patch.

### C4. A non-protoc descriptor panics the jump-to-source

`neovim.rs:55` indexes `loc.span[0]`. `SourceCodeInfo.span` is only
guaranteed non-empty for descriptors protoc produced; any other producer
panics.

**Fixed.** A `[line, col, ..]` slice pattern, falling through to the
same `(1, 1)` a missing `Location` already gives. The function's doc
comment now says so.

### C5. `.expect()` on a per-frame path

`colorize.rs:138` and `:143`. A per-frame `.expect()` turns any
unexpected input into a panic with the terminal in raw mode, which is
the worst possible failure mode for a TUI — and now also means no
cursor afterward (B2).

**Fixed.** A refused start returns no hints at all; a mid-stream
failure keeps the hints collected so far and stops. Either way the
document renders, monochrome from that point on. `config()`'s
`.expect()` on the compiled query is untouched — that one is a build-
time constant, not input.

### C6. `unreachable!` that is reachable

- `command_line.rs:469` — reachable through a command spelled in a way
  the parser accepts.
- `main.rs:227-229` — a `From` impl that panics for half its input
  domain. [audit-duplication.md](audit-duplication.md) proposes unifying
  the two format enums, which deletes this entirely.

**Fixed, both.** `run_command`'s fallthrough sets `message` to "command
not implemented"; `COMMANDS`'s doc comment, which claimed registering a
name was the only step, now says an arm is still needed. The `From`
impl became `ExtractFormatArg::extract_format`, returning `Option` — a
total function over the whole enum. Unifying the two enums is still
[audit-duplication.md](audit-duplication.md)'s to do; this just removes
the trap in the meantime.

### C7. Two policies for the same line lookup

`mouse.rs:360` indexes `self.lines[line_idx]` directly; `render.rs:411`
does the same lookup with `.get().unwrap_or("")`. One of the two is
wrong. Given that mouse coordinates come from the terminal, the mouse
path is the one that should be tolerant.

**Fixed.** The mouse path adopts the draw path's `.get().unwrap_or("")`;
a row `lines` does not hold misses the fold-marker test.

### C8. An unguarded slice next to a guarded twin

`heat_worker.rs:378-381` slices without the bounds check its neighboring
near-identical block performs. Either the check is unnecessary in both
or it is necessary in both.

**Fixed.** The `top_n` arm clamps `start` to `end` as the `complete`
arm already did — `len() >= end` constrains only where the window ends.

### C9. The release profile has no overflow checks

`Cargo.toml`'s release profile sets `codegen-units = 1` and
`lto = "thin"` and nothing else. Items C1, A4 and A5 are all silent
wrap-arounds in release builds. Given that this tool parses untrusted
binary input, `overflow-checks = true` in release is worth its cost —
and it converts a class of silent-wrong-output bugs into loud ones.

**Fixed**, workspace-wide, and measured before being kept.

Cost: `protolens --descriptor-set googleapis.desc -j 4 googleapis.desc
exit` — spec 0198's startup target, on the whole 25.6 MB descriptor set
so the scoring walk dominates — goes **4.01 s → 4.45 s, about +11%**,
pinned to the four-E-core cluster under `bin/bench`'s lock. That is the
worst case the tool has; it buys a trap on every arithmetic wrap in the
walk, the decoder and the tui.

Turning it on immediately found one: `structure.rs`'s `last_child` and
`prev_sibling` computed `block.end - 1` / `idx - 1` through
`bool::then_some`, **whose argument is evaluated eagerly**, so both ran
on the empty block and on slot 0 — 335 of protolens's tests panicked.
The value was discarded in both cases, so the behavior was never wrong;
that is precisely why nothing had noticed. Both are now `then(|| ..)`.
The two neighboring `then_some`s add rather than subtract and cannot
wrap on a real slot index, so they stand.

## D. Scheduling and per-frame cost

D1–D4 are fixed (2026-08-01); each item carries a note below.

### D1. A probably-missed early-out costs a whole extra scoring pass

`HeatCaches::window` (`heat_worker.rs:371-391`) answers "does the cache
cover this window?" in two steps — the `top_n` probe *and* a fallback
onto the `complete` slot, whose doc says the fallback is what stops
callers busy-looping. The worker's own re-check (`heat_worker.rs:485-488`)
has only the first step.

So a second request for the same range with a larger `end` — exactly
what `upgrade_active_override_to_complete` issues after
`recompute_override_candidates` — reports "not covered" and pays a full
second `score_all`, though `complete` already holds the answer.

Reasoned from the code, not reproduced. Fix the behavior before
unifying the two predicates; see item 2.1 of
[audit-duplication.md](audit-duplication.md).

**Fixed, and reproduced on the way.** The worker's re-check is now
`c.window(start, req.start, req.end, req.tier).is_some()` — the
readers' own predicate rather than a restatement of half of it, which
resolves item 2.1 for this pair as well. `heat_caches_worker_round_trip`
gained a third push with a wider `end` than `top_n` holds; against the
old check it fails with `left: 2, right: 1`, one whole extra sweep.

### D2. A clone of the whole result inside the shared mutex

`tiered.rs:188-194`. `peek` clones the entry's value — for the heat
caches, the entire `top_n` vector — and does it while the caller holds
the `HeatCaches` mutex, on the per-frame render path. The clone is
needed because the guard cannot outlive the call; returning a small
projection, or the length the callers actually test, would not be.

**Fixed.** `TieredBounded::peek_with(key, tier, f)` does the same
promoting read and hands `f` a borrow; `peek` is now `peek_with(.., V::
clone)`, so no caller loses anything. The two per-frame readers in
`heat_cue.rs` take two `Copy` fields instead of the candidate list, and
`HeatCaches::window` copies the window rather than all of `top_n`.

### D3. The prefetch inner loop is unbounded

It ignores `ui_deadline` once entered, so a slow step can overrun the
frame budget it exists to respect.

**Fixed.** The `Progressed` arm breaks out with no event once
`Instant::now() >= deadline` — the same "timeout elapsed" outcome the
`Idle` arm's `recv_timeout` produces, which the `*_forces` tests just
past the loop already handle. `deadline` is the minimum of all four
candidates, so the bound is the activity tick (250 ms) even when
nothing else is pending, and the outer loop re-enters read-ahead
immediately when no reason to draw was found.

### D4. A poll error becomes a hot spin

`event.rs:44` treats a `poll` error as "try again" with no backoff. A
persistent error (a closed tty) spins a core.

**Fixed.** `unwrap_or(false)` became a three-arm `match`; the `Err` arm
sleeps `INPUT_POLL_INTERVAL` — the interval `poll` returned without
spending — so the `stop` re-check keeps its cadence at one wakeup per
interval instead of pegging a core.

## E. The overrides file

Everything here is about `:save`/`:restore` and `--load-overrides`, and
all of it is data the user expects to survive.

E1 and E3–E7 are fixed (2026-08-01); E2 was already fixed, by spec 0221
S6, before this audit was written.

### E1. Every write is truncate-in-place

There is no write-to-temp-and-rename anywhere. An interrupted `:save`
destroys the collection it was saving.

**Fixed.** `run_save_overrides` now goes through a `write_atomically`
helper: a sibling temp file (a *sibling*, because `rename` is only
atomic within one filesystem and nothing says the target shares one
with `/tmp`), `sync_all` before the rename so the rename cannot become
durable ahead of the bytes it points at, and `remove_file` on every
error path. Scoped to the overrides file, which is the one write whose
contents exist nowhere else — an export can simply be re-run.

### E2. `:save` persists what `:restore` just dropped

`retain_resolvable` (`override_pane.rs:483-495`) silently drops entries
that no longer resolve, returning only a count nobody surfaces. Save
after restore and the dropped entries are gone from the file too.

**Already fixed** — this item was stale when it was written. Spec 0221
S6 gave the count a channel: `load_overrides` turns a non-zero drop into
one more of the warnings both callers already print. The drop itself is
still a drop, but it is no longer silent.

### E3. `from_yaml` does not re-establish the collection's invariants

`override_pane.rs:639` builds `OverrideEntry` values straight from the
YAML. Nothing re-establishes "at most one active entry per origin", so a
hand-merged file loads into a state the rest of the code assumes cannot
exist — and the node quietly resolves to raw.

**Fixed.** New `OverrideCollection::enforce_single_active`, called from
`load_overrides` and reported through the same warnings list as the drop
above — a repair the user did not ask for is one they should hear about.
It keeps the *first active* entry of each origin, matching what
`toggle_active_cascading` already does for runs it does not own, and
never promotes an entry the file marked inactive. It is O(n) rather than
O(n²) because the collection is sorted by origin, so entries sharing one
are a contiguous run.

The repair lives at `load_overrides` rather than in `from_yaml` as the
item suggested: `from_yaml` is a parser, and the invariant matters at
the point the collection becomes `self.overrides`. Ordering against
`retain_resolvable` is immaterial — resolvability is a function of the
origin alone, so entries sharing an origin are always kept or dropped
together.

### E4. `version` is written and ignored

`YamlFile.version` (`:506`) is serialized as `1` at `:625` and never
read. The forward-compatibility hook exists but is not wired up, which
is worse than not having it: a future format change has no way to
report itself.

**Fixed.** `YAML_FORMAT_VERSION`, written by `to_yaml` and checked by
`from_yaml`. The check needed E5's restructuring to be worth anything:
`YamlFile` is now generic in its entry type, so `from_yaml` reads the
envelope with the entries still uninterpreted (`serde_norway::Value`)
and a version mismatch is reported *as one* rather than as whatever the
untagged match made of a format this build does not know.

### E5. `#[serde(untagged)]` destroys position information

A malformed entry produces an error that names neither the entry nor
the line. `r#type` also has no `#[serde(default)]`.

**Fixed.** `from_yaml` deserializes `overrides` as `Vec<Value>` and
converts one entry at a time, so the diagnostic names the entry index —
"overrides file entry 2 is malformed". The line number is still lost
(that is `untagged`'s buffering, and unavoidable without abandoning it),
but the index is the actionable part in a file of a hundred entries.
`#[serde(default)]` added to all three `r#type` fields: an `Option`
field is still a *required key* to serde without it, so an entry
carrying only a display name failed to match any variant at all.

### E6. A hash mismatch is only a warning

Even for `--format=descriptor-binary`, where the output is a schema
another tool will consume.

**Fixed.** `load_overrides` now returns an `OverrideLoad` — the warnings
plus a `hash_mismatch` flag — instead of a bare warning list, because
the two callers do not agree on how serious a mismatch is. It stays a
warning interactively and for the two document formats, which export the
document's own bytes whatever the overrides were written against. For
`--format=descriptor-*` it is fatal and no output is produced: that
output is a schema assembled from exactly those overrides, so a
collection written for a different blob describes neither, and its
consumer has no way to notice.

### E7. Late validation

`--format=descriptor-*` requires `--load-overrides`, and that check
(`main.rs:526-532`) runs *after* the full startup — about 3.5 s on
googleapis — rather than at argument parse.

**Fixed.** Moved to just after `Cli::parse()`, beside the existing
`--type requires --descriptor-set` check. Nothing about the pairing
depends on the blob, the descriptor set or the tree. The regression test
probes it with a nonexistent `--descriptor-set`: reaching the load at
all would produce *that* diagnostic instead.

## F. Test coverage

580 tests, of which 538 run by default (19 `#[ignore]`d, 23 integration).
Coverage is good overall; these are the holes.

### F1. `src/tui/structure.rs` has no tests

152 lines, zero tests. This is the spec 0216 shape layer — parent,
first-child, sibling, raw-range — and every method indexes the arena
**unchecked**. It is the module where an off-by-one is both most likely
and least visible, and it is the one module of its size in the codebase
with no coverage at all.

**Fixed.** `src/tui/tests/structure.rs` runs five property tests over
five real fixtures. Parent and child agree in both directions and the
ordinal is the position the child was fetched by; `first_child`/
`last_child` are `nth_child` at the two ends; the sibling steps are
inverses and stop at the edges of the child block rather than walking
into the neighboring parent's, which in level order sits in the very
next slots; `nth_child` past the end is `None`; `doc_next` enumerates
the same nodes in the same order as the recursive walk, and stops
rather than cycling. The root terminates every climb by being its own
arena parent (spec 0216 S8).

### F2. The sharded sweep's thread path is untested

`sweep.rs:174-243` — `ranked_with`'s cursor pull, the per-part
partition, the join — has no test. This is spec 0218's core and the
place a work-stealing bug would hide.

**Fixed.** The un-sharded single-thread path is the reference, and every
thread count must reproduce its ranking *exactly*. That is the real
statement: which thread draws which part is nondeterministic, so a part
drawn twice or never drawn changes the ranking rather than crashing, and
no timing assertion would see it. A second test pins the fixture to more
parts than a thread can hold at once, so a future `partition_roots`
change cannot quietly reduce the first test to comparing the un-sharded
path with itself; a third shows a raised `cancel` reaching every shard
rather than hanging.

### F3. Three tests report green while skipping

Three `theme.rs` tests self-skip on a truecolor terminal, and the nvim
test self-skips when nvim is *installed*. The ANSI-16 palette is
therefore untested on any modern development machine, and the reported
result says otherwise. A skip must be visible as a skip.

**Fixed.** The skipping was a consequence of reading the answer from the
environment. `style_for`/`heat_style`/`heat_suffix_style` each split
into a `*_in(…, rgb: bool)` helper, so both palettes are reachable from
a test with no environment mutation and nothing to skip; one env test
remains, covering `COLORTERM` into `supports_rgb`, which cannot skip.
The nvim test names a binary that is not on `PATH`, through a
`#[cfg(test)]` thread-local, instead of hoping the machine has no
Neovim.

### F4. Exit codes are asserted only as success/failure

`tests/batch_export.rs` checks `success()` / `!success()`. A panic
(status 101) passes every negative test. Assert the code.

**Fixed.** One `assert_refused` helper, and all nine negative sites
converted. Every deliberate diagnostic path in `main.rs` returns
`ExitCode::FAILURE`, so the assertion is `status.code() == Some(1)` — a
panic (101) or a signal (no code at all) now fails the test that is
supposed to be watching for exactly that.

### F5. Uncovered edges

`window_nodes` sortedness and its version guard (`window_nodes_version`
has zero test references); `ThemeKind::System`, resolved by two
hand-written match arms guarding seven downstream `unreachable!`s; the
three primitive-keyword lists in `colorize.rs`; a zero-size terminal;
deeply-nested documents; `Blob::load`'s error arms. One real fixed crash
has its regression test only in an `#[ignore]`d test that depends on
`/tmp/pdb.desc`.

**Fixed**, item by item.

- **`window_nodes`.** Two tests. The first pins the cache's answers to
  the descent's for the same lines and asserts the stored vector is
  ascending — out of order the binary search does not lie, it stops
  hitting, and the cache silently becomes the whole-document descent it
  exists to avoid. The second records a deliberately *wrong* entry and
  bumps `structural_version` by hand, because the guard turns out to be
  unreachable through any public mutator: every path that bumps the
  version ends in `clamp_pan_offset`, which re-records the cache at the
  new version. A test routed through a real fold or splice tests the
  redraw, not the guard.
- **`ThemeKind::System`.** `resolve_system`'s own output is pushed
  through all seven guarded functions, at both color depths.
- **The primitive-keyword lists.** They are in `decode.rs`, not
  `colorize.rs` — this entry named the wrong file. The three
  (`primitive_type_for_keyword`'s arms,
  `primitive_keywords_for_wire_type`'s slices,
  `ALL_PRIMITIVE_KEYWORDS`) are now held against each other, and each
  keyword's wire type is checked against protobuf's rule rather than
  against a copy of the table. `colorize.rs` had its own version of the
  same problem — `SyntaxRole`, `RECOGNIZED_NAMES` and the highlight
  index have to agree, and `RECOGNIZED_NAMES` has to agree with the
  grammar's `highlights.scm` in another directory — so that is covered
  too, along with the two roles (`punctuation.delimiter` and the plain
  `punctuation.bracket`) that had no test at all.
- **A zero-size terminal.** Drawn at `0x0`, `1x1`, `2x1`, `1x2`, `3x3`,
  `0x24` and `80x0`, in all three pane layouts, with the keys that
  measure the page they are scrolling pressed at each.
- **Deeply-nested documents.** A self-recursive schema, nested to the
  deepest depth `build_arena` accepts (`MAX_WIRE_DEPTH - 3`), opened,
  drawn, descended and folded. Real documents run to about a dozen
  levels; everything between there and the cap was untried.
- **`Blob::load`'s error arms.** A missing path is `NotFound` on both
  producers — the kind is what tells a typo from an unreadable file —
  and an empty file is an empty document rather than an error.
- **The `/tmp/pdb.desc` crash.** The merge that panicked and the offset
  that fed it a bad patch were already covered; the *sequence* was not.
  `Down`, `t`, `Enter` now runs through `handle_key` on an in-repo
  fixture, and the document is checked to still account for every line
  afterwards.

### F6. Long functions

Beyond the module splits, these are worth decomposing on their own
merit:

| lines | function |
|---:|---|
| 446 | `render` |
| 416 | `handle_key` |
| 380 | `handle_manage_key` |
| 365 | `main` |
| 269 | `render_node_as` |
| 254 | `run_loop` |
| 231 | `App::new` |
| 225 | `handle_mouse` |
| 221 | `splice_override` |
| 209 | `handle_override_key` |

`render` is already covered by
[audit-module-sizes.md](audit-module-sizes.md); the dispatchers are long
because they are flat matches, which is defensible — decompose them only
if a natural grouping exists.

**Assessed; one split made, the rest declined.** Two things this table
does not show, both of which change the verdict.

The first is that these counts are of *lines*, not of statements, and
these functions are unusually heavily commented — 24% to 50% of each:

| function | lines | comment | code |
|---|---:|---:|---:|
| `render` | 446 | 137 | 291 |
| `handle_key` | 398 | 143 | 226 |
| `main` | 396 | 124 | 252 |
| `run_loop` | 254 | 129 | 125 |
| `App::new` | 213 | 52 | 161 |
| `handle_mouse` | 213 | 89 | 113 |

`run_loop` is half prose. Splitting it would not remove those lines, it
would distribute them, and each fragment would have to re-establish the
context its comments assume.

The second is that length is a proxy for the thing actually worth
avoiding, which is a function doing several separable jobs. On that test
the dispatchers pass: each arm binds one key and calls one method, the
arms share no state, and there is no grouping in them that is not
already the grouping the four separate dispatchers provide
(`handle_key`, `handle_override_key`, `handle_manage_key`,
`handle_mouse` are the modes). `run_loop` passes for the opposite
reason — it is one state machine over seven interdependent locals
(`redraw`, `redraw_why`, two activity windows, `heat_dirty`,
`last_heat_frame`, `deadline`), and any extraction would take most of
them as arguments and hand most of them back.

`main` was the exception and is split. It was startup (a fixed sequence
that runs for every invocation) followed by a `match` on the subcommand
whose `Export` arm was 130 self-contained lines — a different job, on an
`App` that is already built. That arm is now `run_export`, and the
`--output`-or-stdout block it contained twice, verbatim, is
`write_output`. `main` goes from 396 lines (252 of code) to 278 (171).

`render`, `render_node_as`, `App::new` and `splice_override` are left
alone: they are long the way a render pass is long, and
[audit-module-sizes.md](audit-module-sizes.md) already owns the first.

## G. Documentation drift

### G1. `HELP_TEXT` is hand-maintained against four dispatchers

`mod.rs:559-719` — about 160 lines of key documentation with no
structural link to the four key-dispatch functions it describes. Adding
a binding and forgetting the help text produces no error.

Related: `mod.rs:190-191` documents adding an entry to `COMMANDS` as
"the only step needed". That is not true; other sites must be updated
too. The comment actively misleads.

**Fixed.** The drift was real and had already happened. Four bindings
and one command were live and undocumented:

- `v` — jump to the FQDN under focus's `.proto` declaration in Neovim,
  from any pane (spec 0144 G1).
- `f` in the management pane — edit the highlighted entry's display-name
  override (spec 0119 G4).
- `Ctrl-A` / `Ctrl-E` — the caret's line-start/line-end aliases.
- `Ctrl-B`/`Ctrl-F` and `Alt-B`/`Alt-F` — the command line's character
  and word motions.
- `:proto-root <dir>`, and `:quit`. Neither appeared in the help; `:`
  itself, which is what opens the command line from any pane, did not
  either.

Generating `HELP_TEXT` from the match arms was declined for the reason
its own doc comment gives: it is phrased for a reader, in an order the
dispatchers do not have, and several entries (the `x` chord, the drag
selection, `Shift+wheel`) describe behavior that is not one arm.

The link is made by a test instead. `tests/help_text.rs` reads the six
dispatcher sources back with `include_str!`, extracts every
`KeyCode::Char('x')` literal, and requires each to appear in `HELP_TEXT`
**as a token in its own right** rather than as a letter inside a word —
which is the distinction that matters, since `v` occurs in "available"
and in "move" and a substring search called it documented. Five
characters are exempt as chord components (`gg`, `zc`, `zC`, `xb`,
`xp`), each named with its chord in the test. A second test holds
`COMMANDS` against the help, and a third caps the help's line width,
since the modal cuts rather than wraps.

The check is weaker than generation — it says a key is *mentioned*, not
that what is written about it is true — but mention is the half that
drifts silently. Verified to bite by deleting the `v` entry.

`COMMANDS`' doc comment now says the registry is the source of truth for
the name and nothing else, and names both other sites.

### G2. `size_suffix` reports "0 MB"

`main.rs:253-258`. Any file under 1 MiB is announced as "0 MB".

**Fixed.** The unit is now chosen to fit — bytes, KB, or MB — because
the number is the whole point of the suffix: it exists to say what the
wait the message announces is proportional to, and a flat "0 MB" reads
as either an empty file or a bug while telling the user neither. A
descriptor set of a few hundred kilobytes is entirely ordinary, so this
was the common case, not an edge one. `size_suffix_picks_a_unit_the_
number_survives` pins every boundary, and that an unreadable path still
yields no suffix at all rather than a "(0 bytes)" claim about a file
nobody could stat.

### G3. Stale comments

Eight comments describe machinery that no longer exists:

- `decode.rs:590` — mentions `local_tree` (deleted by spec 0216).
- `mod.rs:2574-2578` — describes spec 0203 compaction (dissolved by
  0216).
- `override_apply.rs:2092` — compaction.
- `src/tui/tests/support.rs:17` — compaction.
- `render_cache.rs:7` and `:40` — both cite
  `override_pane::CandidateCache`, a type deleted by commit `234dcc8`.
  These two are the *only* references left in the crate, which also
  makes the standing "should the two MRU caches share a generic?"
  question moot: there is no second cache. See item 2.2 of
  [audit-duplication.md](audit-duplication.md).
- `heat_cue.rs:368-369` — asserts a difference between the packed and
  unpacked heat ranges that spec 0216 removed (item C1).
- `override_apply.rs:2702-2704` — claims `fqdn_needs_dot_prefix` is
  shared with `render.rs`'s `render_override_pane`. It is not; its only
  users are `override_row_display`'s two branches.
- `encode_text/mod.rs:189` — "The text is always valid ASCII", stated
  as fact next to the branch that silently handles it not being (A1).

**Fixed**, with one already gone and the rest rewritten rather than
deleted, since each sat where a reader would ask the question it was
answering.

`heat_cue.rs` was fixed by C1 and needed nothing. The three compaction
notes (`decode.rs`, `override_apply.rs`, `tests/support.rs`) and
`mod.rs`'s read-ahead `Idle` arm now state the property that holds — the
arena is immutable, a retype allocates no slot, nothing is renumbered —
instead of narrating the machinery that used to make it false. The
`support.rs` one also had to say why `Shape` survives its own
justification: a raw index would now be stable, but these assertions are
about what the reader is shown, which an index does not say.

`render_cache.rs`'s two `CandidateCache` citations are replaced by a
note that this is the crate's only byte-bounded cache and that
`tiered::TieredBounded` is not a second one, which is what makes the
standing "should they share a generic?" question moot at the place
someone would ask it.

`fqdn_needs_dot_prefix` now names its real second caller,
`override_select.rs`'s `override_row_display`, one indirection below
what it claimed.

`encode_text_to_binary_into`'s comment now says what is true and what
follows from it: rendered prototext is ASCII, so the claim holds for
this crate's own output; it is not a guarantee about the argument, and
this signature cannot report a violation, so a non-UTF-8 input appends
nothing and is indistinguishable from an empty document — which makes
validation the caller's job, as `Blob::load` now does.

## Clean results, recorded so the check is not repeated

The following were checked and found clean. Several contradict older
notes, which is why they are written down.

**The `unsafe` blocks are sound.** All six live in `blob.rs` (`:283-284`,
`:288-291`, `:296-301`, `:309-321`, `:328-340`, `:349-358`) and were
audited individually. The `HEADROOM`/page-size sufficiency precondition
that makes the pointer arithmetic in-bounds is **provable** from
`prefix.len() <= 11`, not merely assumed. No mapping is leaked.

**Concurrency is disciplined.** The crate has exactly two mutexes, and
they are **never held simultaneously** — verified at every acquisition
site, so there is no deadlock to find. Every atomic ordering is
justified. `pop_blocking` is a proper condvar wait, not a spin.
`HeatRequestQueue` is bounded at 2048 with merge-on-push; there are no
unbounded queues, no lock held across expensive work.

**The strongest invariant enforcement in the crate is already there.**
`arena_gap` runs on **every** fixture, on every `decode()` under
`cfg(test)` — spec 0216's superset property cannot regress unnoticed.
`verify_repair` defaults **true**, so every splice checks its line
counts. There are exactly three compile-time size assertions and **all
three are in production modules**, so a plain `cargo build` catches a
layout change.

**The panic hook is installed before the first fallible call**
(`mod.rs:2291`, ahead of `enable_raw_mode`), so a panic during terminal
setup is covered. Its problems (B2, B3) are elsewhere.

Also clean:

- **Zero `#[allow]` and zero `#[expect]` attributes** in the whole
  crate. No lint has been swept under a rug.
- `cargo check` and `cargo clippy` are clean workspace-wide.
- **Zero `TODO`, `FIXME`, `XXX` or `HACK` markers.**
- No attacker-controlled `with_capacity`. Production resource handling
  (files, child processes) is clean.
- No unexplained magic numbers.
- **Zero occurrences of `line_to_node` and `footer_line_to_node`.** Any
  note describing their unchecked `as u32` as a live defect is **stale**
  — spec 0210 deleted both.
- Also zero occurrences of `warm_initial_view`, `packed_record_siblings`,
  `arena_bytes_per_node`, `STRING_ALLOWANCE` and
  `PROTOLENS_NO_MEMORY_GUARD`. These names appear only in historical
  documents.

## Suggested order

1. **A1, A2, B2** — three small fixes for three findings that are
   invisible to the user until they have already cost something.
2. **B1, B5, C1** — the three the first pass led with, in that order.
3. **C9** (`overflow-checks`) — one line, and it protects C1, A4 and A5.
4. **A3**–**A7**, then **B3** — the correctness set.
5. **F1** and **F2** — tests for the two untested load-bearing modules.
6. The stale comments (G3) — trivial, and they mislead every reader
   until removed.
7. **D**, **E** and the rest of **F**/**G** as opportunity allows.
   Within E, **E1** first: it is the only one that loses data.
