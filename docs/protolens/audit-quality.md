<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens codebase audit — quality findings

*written 2026-07-31, against `bc586dc`*

One of three companion audit documents:

- [audit-module-sizes.md](audit-module-sizes.md) — module sizes and
  proposed splits.
- [audit-duplication.md](audit-duplication.md) — factorization and
  deduplication opportunities.
- **this file** — everything else worth fixing, in priority order.

Items are numbered so they can be cited. Each was verified against the
code; line numbers will drift.

## The three to fix first

### 1. Quitting the TUI freezes on a large blob — HIGH

`heat_worker.rs:603-608`. `shutdown_inner` joins the worker thread, but
`score_all` has **no cancellation check**. On googleapis the join waits
for a full scoring pass to run to completion, so pressing `q` appears to
hang for several seconds with the terminal already handed back.

The fix is a cancellation flag polled inside `score_all`'s walk, not a
detached thread — the worker owns state the shutdown path needs to see
settled.

### 2. `std::env::set_var` in a multi-threaded process — MEDIUM-HIGH

`neovim.rs:151-159` calls `std::env::set_var` under a SAFETY comment
asserting no other threads are running. **That assertion is false**: the
heat worker thread is alive at that point. `set_var` is unsound with
concurrent readers, and Rust 2024 makes it `unsafe` for exactly this
reason.

The fix is three lines away and already in the file: pass the variable
through `Command::env` on the child, which is what `PROTOTEXT_PROTO_ROOT`
does at `:166`. No process-global mutation is needed at all.

### 3. A dead special case that hides an overflow — MEDIUM

`heat_cue.rs:346-377`. The doc comment describes the pre-spec-0216
world, and the packed special case at `:366-377` is now redundant:
`extract.rs:76-78` already handles packed runs. Worse, the dead branch
contains a `usize` overflow — `start + len.varint... as usize` wraps on
a crafted length, after which `:441` slices with the wrapped value and
panics.

Deleting the branch fixes the documentation drift and the overflow in
one change, and removes code rather than adding a guard.

The second audit pass **proved the branch dead** rather than suspecting
it: `decode.rs:913-916` now sets `packed_record_start` to the same
`raw_start[slot]` that `raw_range` begins at, so both arms return the
same range. See item 1.1 of
[audit-duplication.md](audit-duplication.md), which folds this into a
larger seven-site consolidation — do the two together.

## Correctness

### 4. An extension can render as the wrong type

`override_apply.rs:851` and `:853` narrow with `type_id as u32`. On
truncation the result is a *valid but different* type id, so the node
renders as the wrong extension silently — no error, no refusal, just
wrong output. Use a checked conversion and refuse.

### 5. `?` in the neovim handoff leaves the terminal broken

`neovim.rs:209` and `:214` propagate with `?` on paths where the
terminal is already out of raw mode and a foreground process group has
been set. An early return there leaves cooked mode and a dangling pgid.
These need cleanup on the error path, not `?`.

### 6. A non-protoc descriptor panics the jump-to-source

`neovim.rs:55` indexes `loc.span[0]`. `SourceCodeInfo.span` is only
guaranteed non-empty for descriptors protoc produced; any other producer
panics.

### 7. A dead reader is reported as a clean exit

`mod.rs:2583` maps `Err(Disconnected)` to `return Ok(())`. If the input
reader thread dies, protolens exits with status 0 and says nothing.
Whatever killed the reader is invisible to a script driving the tool.

### 8. An unreadable descriptor becomes an empty one

`decode.rs:240`: `read_descriptor_file(path).unwrap_or_default()`. A
permissions error, a truncated file and a genuinely empty descriptor set
are indistinguishable downstream; the user gets a document with no types
and no explanation.

### 8b. A probably-missed early-out costs a whole extra scoring pass

Found by the second duplication pass, and a defect rather than
duplication. `HeatCaches::window` (`heat_worker.rs:371-391`) answers
"does the cache cover this window?" in two steps — the `top_n` probe
*and* a fallback onto the `complete` slot, whose doc says the fallback
is what stops callers busy-looping. The worker's own re-check
(`heat_worker.rs:485-488`) has only the first step.

So a second request for the same range with a larger `end` — exactly
what `upgrade_active_override_to_complete` issues after
`recompute_override_candidates` — reports "not covered" and pays a full
second `score_all`, though `complete` already holds the answer.

Reasoned from the code, not reproduced. Fix the behavior before
unifying the two predicates; see item 2.1 of
[audit-duplication.md](audit-duplication.md).

### 9. The release profile has no overflow checks

`Cargo.toml`'s release profile sets `codegen-units = 1` and
`lto = "thin"` and nothing else. Items 3 and 4 above are both silent
wrap-arounds in release builds. Given that this tool parses untrusted
binary input, `overflow-checks = true` in release is worth its cost —
and it converts a class of silent-wrong-output bugs into loud ones.

### 10. `src/tui/structure.rs` has no tests

152 lines, zero tests. This is the spec 0216 shape layer — parent,
first-child, sibling, raw-range — and every method indexes the arena
**unchecked**. It is the module where an off-by-one is both most likely
and least visible, and it is the one module of its size in the codebase
with no coverage at all.

## Robustness and consistency

### 11. Two policies for the same line lookup

`mouse.rs:360` indexes `self.lines[line_idx]` directly;
`render.rs:411` does the same lookup with `.get().unwrap_or("")`. One of
the two is wrong. Given that mouse coordinates come from the terminal,
the mouse path is the one that should be tolerant.

### 12. `unreachable!` that is reachable

- `command_line.rs:469` — reachable through a command spelled in a way
  the parser accepts.
- `main.rs:227-229` — a `From` impl that panics for half its input
  domain. Item N of [audit-duplication.md](audit-duplication.md)
  proposes unifying the two format enums, which deletes this entirely.

### 13. `.expect()` on a per-frame path

`colorize.rs:138` and `:143`. A per-frame `.expect()` turns any
unexpected input into a panic with the terminal in raw mode, which is
the worst possible failure mode for a TUI.

### 14. An unguarded slice next to a guarded twin

`heat_worker.rs:378-381` slices without the bounds check its neighboring
near-identical block performs. Either the check is unnecessary in both
or it is necessary in both.

### 15. The heat range is derived two different ways

Seven sites, two idioms. `heat_cue.rs:387` and `mod.rs:1702` call
`heat_scored_range`; `heat_cue.rs:526`, `mod.rs:2036`,
`override_select.rs:356`, `:428` and `:481` inline
`extract::message_payload_range`. They agree today. Nothing makes them
agree tomorrow.

Promoted after the second pass: the cache is **written** keyed by one
spelling and **read back** by the other for the same node, so a
divergence would leave nodes permanently unsettled rather than merely
mis-drawn. Full treatment at item 1.1 of
[audit-duplication.md](audit-duplication.md).

### 16. `HELP_TEXT` is hand-maintained against four dispatchers

`mod.rs:559-719` — about 160 lines of key documentation with no
structural link to the four key-dispatch functions it describes. Adding
a binding and forgetting the help text produces no error.

Related: `mod.rs:190-191` documents adding an entry to `COMMANDS` as
"the only step needed". That is not true; other sites must be updated
too. The comment actively misleads.

### 17. `size_suffix` reports "0 MB"

`main.rs:253-258`. Any file under 1 MiB is announced as "0 MB".

### 18. Long functions

Beyond the module splits, these are the functions worth decomposing on
their own merit:

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
[audit-module-sizes.md](audit-module-sizes.md); the dispatchers
(`handle_key`, `handle_manage_key`, `handle_override_key`) are long
because they are flat matches, which is defensible — decompose them only
if a natural grouping exists.

## Stale comments

Seven comments describe machinery that no longer exists:

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
  unpacked heat ranges that spec 0216 removed (item 3 above).
- `override_apply.rs:2702-2704` — claims `fqdn_needs_dot_prefix` is
  shared with `render.rs`'s `render_override_pane`. It is not; its only
  users are `override_row_display`'s two branches.

## Clean results, recorded so the check is not repeated

The following were checked and found clean. Several contradict older
notes, which is why they are written down.

- **Zero `#[allow]` and zero `#[expect]` attributes** in the whole
  crate. No lint has been swept under a rug.
- `cargo check` and `cargo clippy` are clean workspace-wide.
- **Zero occurrences of `line_to_node` and `footer_line_to_node`.** Any
  note describing their unchecked `as u32` as a live defect is **stale**
  — spec 0210 deleted both.
- Also zero occurrences of `warm_initial_view`, `packed_record_siblings`,
  `arena_bytes_per_node`, `STRING_ALLOWANCE` and
  `PROTOLENS_NO_MEMORY_GUARD`. These names appear only in historical
  documents.
- **Zero `TODO`, `FIXME`, `XXX` or `HACK` markers.**
- No unbounded queues. `HeatRequestQueue` is bounded at 2048 with
  merge-on-push.
- No lock is held across expensive work.
- No questionable atomic ordering.
- No unexplained magic numbers.
- No live unsigned-underflow site (item 3's overflow is signed-width
  wraparound in dead code).

## Suggested order

1. Items **1**, **2**, **3** — the three at the top, in that order.
2. Item **9** (`overflow-checks`) — one line, and it protects items 4
   and 3.
3. Items **4**–**8**, the correctness set.
4. Item **10** — tests for `structure.rs`.
5. The stale comments — trivial, and they mislead every reader until
   removed.
6. Items **11**–**17** as opportunity allows.
