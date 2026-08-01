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

## A. Silently wrong output

The worst class in this codebase, because protolens' entire job is to
tell you what some bytes mean. A crash is recoverable; a plausible-
looking wrong answer is not.

### A1. One bad byte turns a text file into an empty document — HIGH

`blob.rs:93` calls `encode_text_to_binary_into(&text, &mut buf)`. That
function **has no return type**: on invalid UTF-8 it returns at
`prototext-core/src/serialize/encode_text/mod.rs:190-193`, leaving the
output buffer exactly as it found it.

So a `#@ prototext` file containing a single non-UTF-8 byte opens as a
zero-length blob — no error, no warning, just an empty document. The
comment at that site (`// The text is always valid ASCII`) states an
assumption the caller does not enforce.

The fix is a return type on the encoder, or a UTF-8 check in `Blob::load`
that reports. Either way the empty result must stop being indis-
tinguishable from an empty file.

### A2. Batch export to stdout never flushes — HIGH

`main.rs:556-562` and `:579-585` both `write_all` to `std::io::stdout()`
and return `ExitCode::SUCCESS`. Neither flushes. The **only** `flush` in
the crate is in the TUI teardown.

Rust's runtime does flush stdout at exit, but it discards the error. So
`protolens export ... > /full/disk` writes a truncated stream and exits
`0`. A script piping protolens into a build cannot tell.

Explicit `flush()` on both paths, with the error mapped to
`ExitCode::FAILURE`, is a four-line fix.

### A3. `App::new`'s own refusals are invisible in batch

`main.rs:510`. Spec 0221 routes refused overrides to stderr and makes
the export fail — but the check sits **inside** the
`if let Some(overrides_path)` block. Overrides can also arrive from the
document's own inference, and a refusal on that path in batch mode still
produces a clean exit and a file. This is 0221's defect on the other
branch.

### A4. An extension can render as the wrong type

`override_apply.rs:851` and `:853` narrow with `type_id as u32`. On
truncation the result is a *valid but different* type id, so the node
renders as the wrong extension silently — no error, no refusal, just
wrong output. Use a checked conversion and refuse.

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

Field numbers above 2^29 are already invalid protobuf. Refusing them
once, at decode, would retire all three.

### A6. An unreadable descriptor becomes an empty one

`decode.rs:240`: `read_descriptor_file(path).unwrap_or_default()`. A
permissions error, a truncated file and a genuinely empty descriptor set
are indistinguishable downstream; the user gets a document with no types
and no explanation.

### A7. A dead reader is reported as a clean exit

`mod.rs:2583` maps `Err(Disconnected)` to `return Ok(())`. If the input
reader thread dies, protolens exits with status 0 and says nothing.

Compounding it: that arm is currently **unreachable**, because
`run_loop` holds its own clone of the sender for the whole loop. The
disconnect it is meant to catch cannot fire.

## B. The terminal or the session is left broken

### B1. Quitting the TUI freezes on a large blob — HIGH

`heat_worker.rs:603-608`. `shutdown_inner` joins the worker thread, but
`score_all` has **no cancellation check**. On googleapis the join waits
for a full scoring pass to run to completion, so pressing `q` appears to
hang for several seconds with the terminal already handed back.

The fix is a cancellation flag polled inside `score_all`'s walk, not a
detached thread — the worker owns state the shutdown path needs to see
settled.

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

### B4. `?` in the neovim handoff leaves the terminal broken

`neovim.rs:209` and `:214` propagate with `?` on paths where the
terminal is already out of raw mode and a foreground process group has
been set. An early return there leaves cooked mode and a dangling pgid.
These need cleanup on the error path, not `?`.

### B5. `std::env::set_var` in a multi-threaded process

`neovim.rs:151-159` calls `std::env::set_var` under a SAFETY comment
asserting no other threads are running. **That assertion is false**: the
heat worker thread is alive at that point — the handoff shuts down the
*input reader*, never the worker. `set_var` is unsound with concurrent
readers, and Rust 2024 makes it `unsafe` for exactly this reason.

The fix is three lines away and already in the file: pass the variable
through `Command::env` on the child, which is what `PROTOTEXT_PROTO_ROOT`
does at `:166`. No process-global mutation is needed at all.

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

### B7. No signal handling at all

SIGTERM, SIGHUP and SIGINT are unhandled. A `kill` or a closed terminal
emulator leaves raw mode and the alternate screen behind.

## C. Panics on hostile or unusual input

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

### C2. Half a range is clamped

`extract.rs:179-180`:

```rust
let end = r.end.min(lines.len());
format!("{PROTOTEXT_HEADER}{}", dedent(&lines[r.start..end]))
```

`end` is clamped, `start` is not. A `start` past `lines.len()`, or past
the clamped `end`, panics in the slice. If the clamp is needed at all it
is needed on both ends.

### C3. `lines[0]` with no guard

`decode.rs:1455-1458` indexes `lines[0]` on the wrapper path. Every
current caller produces a non-empty `lines`, but nothing states or
checks it.

### C4. A non-protoc descriptor panics the jump-to-source

`neovim.rs:55` indexes `loc.span[0]`. `SourceCodeInfo.span` is only
guaranteed non-empty for descriptors protoc produced; any other producer
panics.

### C5. `.expect()` on a per-frame path

`colorize.rs:138` and `:143`. A per-frame `.expect()` turns any
unexpected input into a panic with the terminal in raw mode, which is
the worst possible failure mode for a TUI — and now also means no
cursor afterward (B2).

### C6. `unreachable!` that is reachable

- `command_line.rs:469` — reachable through a command spelled in a way
  the parser accepts.
- `main.rs:227-229` — a `From` impl that panics for half its input
  domain. [audit-duplication.md](audit-duplication.md) proposes unifying
  the two format enums, which deletes this entirely.

### C7. Two policies for the same line lookup

`mouse.rs:360` indexes `self.lines[line_idx]` directly; `render.rs:411`
does the same lookup with `.get().unwrap_or("")`. One of the two is
wrong. Given that mouse coordinates come from the terminal, the mouse
path is the one that should be tolerant.

### C8. An unguarded slice next to a guarded twin

`heat_worker.rs:378-381` slices without the bounds check its neighboring
near-identical block performs. Either the check is unnecessary in both
or it is necessary in both.

### C9. The release profile has no overflow checks

`Cargo.toml`'s release profile sets `codegen-units = 1` and
`lto = "thin"` and nothing else. Items C1, A4 and A5 are all silent
wrap-arounds in release builds. Given that this tool parses untrusted
binary input, `overflow-checks = true` in release is worth its cost —
and it converts a class of silent-wrong-output bugs into loud ones.

## D. Scheduling and per-frame cost

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

### D2. A clone of the whole result inside the shared mutex

`tiered.rs:188-194`. `peek` clones the entry's value — for the heat
caches, the entire `top_n` vector — and does it while the caller holds
the `HeatCaches` mutex, on the per-frame render path. The clone is
needed because the guard cannot outlive the call; returning a small
projection, or the length the callers actually test, would not be.

### D3. The prefetch inner loop is unbounded

It ignores `ui_deadline` once entered, so a slow step can overrun the
frame budget it exists to respect.

### D4. A poll error becomes a hot spin

`event.rs:44` treats a `poll` error as "try again" with no backoff. A
persistent error (a closed tty) spins a core.

## E. The overrides file

Everything here is about `:save`/`:restore` and `--load-overrides`, and
all of it is data the user expects to survive.

### E1. Every write is truncate-in-place

There is no write-to-temp-and-rename anywhere. An interrupted `:save`
destroys the collection it was saving.

### E2. `:save` persists what `:restore` just dropped

`retain_resolvable` (`override_pane.rs:483-495`) silently drops entries
that no longer resolve, returning only a count nobody surfaces. Save
after restore and the dropped entries are gone from the file too.

### E3. `from_yaml` does not re-establish the collection's invariants

`override_pane.rs:639` builds `OverrideEntry` values straight from the
YAML. Nothing re-establishes "at most one active entry per origin", so a
hand-merged file loads into a state the rest of the code assumes cannot
exist — and the node quietly resolves to raw.

### E4. `version` is written and ignored

`YamlFile.version` (`:506`) is serialized as `1` at `:625` and never
read. The forward-compatibility hook exists but is not wired up, which
is worse than not having it: a future format change has no way to
report itself.

### E5. `#[serde(untagged)]` destroys position information

A malformed entry produces an error that names neither the entry nor
the line. `r#type` also has no `#[serde(default)]`.

### E6. A hash mismatch is only a warning

Even for `--format=descriptor-binary`, where the output is a schema
another tool will consume.

### E7. Late validation

`--format=descriptor-*` requires `--load-overrides`, and that check
(`main.rs:526-532`) runs *after* the full startup — about 3.5 s on
googleapis — rather than at argument parse.

## F. Test coverage

580 tests, of which 538 run by default (19 `#[ignore]`d, 23 integration).
Coverage is good overall; these are the holes.

### F1. `src/tui/structure.rs` has no tests

152 lines, zero tests. This is the spec 0216 shape layer — parent,
first-child, sibling, raw-range — and every method indexes the arena
**unchecked**. It is the module where an off-by-one is both most likely
and least visible, and it is the one module of its size in the codebase
with no coverage at all.

### F2. The sharded sweep's thread path is untested

`sweep.rs:174-243` — `ranked_with`'s cursor pull, the per-part
partition, the join — has no test. This is spec 0218's core and the
place a work-stealing bug would hide.

### F3. Three tests report green while skipping

Three `theme.rs` tests self-skip on a truecolor terminal, and the nvim
test self-skips when nvim is *installed*. The ANSI-16 palette is
therefore untested on any modern development machine, and the reported
result says otherwise. A skip must be visible as a skip.

### F4. Exit codes are asserted only as success/failure

`tests/batch_export.rs` checks `success()` / `!success()`. A panic
(status 101) passes every negative test. Assert the code.

### F5. Uncovered edges

`window_nodes` sortedness and its version guard (`window_nodes_version`
has zero test references); `ThemeKind::System`, resolved by two
hand-written match arms guarding seven downstream `unreachable!`s; the
three primitive-keyword lists in `colorize.rs`; a zero-size terminal;
deeply-nested documents; `Blob::load`'s error arms. One real fixed crash
has its regression test only in an `#[ignore]`d test that depends on
`/tmp/pdb.desc`.

### F6. Long functions

Beyond the module splits, these are worth decomposing on their own
merit:

| lines | function |
|---:|---|
| 467 | `render` |
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

## G. Documentation drift

### G1. `HELP_TEXT` is hand-maintained against four dispatchers

`mod.rs:559-719` — about 160 lines of key documentation with no
structural link to the four key-dispatch functions it describes. Adding
a binding and forgetting the help text produces no error.

Related: `mod.rs:190-191` documents adding an entry to `COMMANDS` as
"the only step needed". That is not true; other sites must be updated
too. The comment actively misleads.

### G2. `size_suffix` reports "0 MB"

`main.rs:253-258`. Any file under 1 MiB is announced as "0 MB".

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
