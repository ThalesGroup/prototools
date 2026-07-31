<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens rendering — implementation worklist

*frozen 2026-07-26*

> **This document is a closed handoff, kept only because later specs
> cite its `W*` item numbers.** Phases 2 through 8 below were never
> updated, and most of their items were overtaken by specs 0180 through
> 0218 rather than worked through in order — W15's
> `rebuild_visible_rows` and W25's `TreeNode` shrink, for instance, both
> describe machinery specs 0210 and 0216 deleted. Do not start an item
> from this file. For what is still open read the audit table at the top
> of [rendering-flaws.md](rendering-flaws.md).

**Phase 1 is closed as of 2026-07-26.** W1 obsoleted by spec 0185, W2 and
W3 done, W4 done via spec 0180, W26 half 1 done via spec 0171 with half 2
superseded by spec 0174, W27 done, W28 moot, W29/W30 done via spec 0172.
Each was re-verified against the code on that date rather than trusted
from this document — several had been fixed as a side effect of a spec
that never mentioned the item.

## How to use this document

This is the **implementation handoff** for the rendering review. The two
analysis documents state the *why*:

- [rendering-flaws.md](rendering-flaws.md) — correctness bugs (C*), perf
  cliffs (P*), architectural smells (A*), doc drift (D*).
- [rendering-scaling-roadmap.md](rendering-scaling-roadmap.md) — scaling
  proposals (S*), with per-item shape, invariant impact and risk.

Two companion reviews of the libraries protolens decodes and scores
with feed items into this list as well. Their IDs are namespaced below as
`decode C*/P*` and `scoring C*/P*` to keep them distinct from the
rendering review's own:

- [../prototext/decode-flaws.md](../prototext/decode-flaws.md) —
  `prototext-core` decode and sinks.
- [../scoring-flaws.md](../scoring-flaws.md) — `prototext-graph`'s
  `score_all`.

This document states the *what, in what order, and how you know it
worked*. Every item below is self-contained: it names the files, the
change, the test that proves it, and what must land first. Read the
referenced flaw/roadmap entry before starting an item — the worklist
deliberately does not repeat the argument, only the action.

**Work items are numbered W0–W32 and are in dependency order.** Do not
reorder without checking `Blocked by`. Items within the same phase that
list no dependency on each other may be done in any order or in
parallel. Numbering is append-only: W25–W32 were added after the first
draft and sit in their dependency-correct phase, not at the end.

### Ground rules

- Build and test with `--release`. This project never compiles
  `debug_assert!` (this is itself the subject of W2).
- Test invocation for this crate is `cargo test --release --bin
  protolens` — `protolens` has a `[[bin]]` target and **no** `src/lib.rs`,
  so `--lib` does not work.
- Each work item is one commit. Reference the flaw ID (and spec path,
  where one exists) in the commit body. No `#` characters in commit
  messages.
- When an item implements a spec, set that spec's `Status: implemented`
  and fill `Implemented in:` with the date.
- Run `reuse lint` before each commit; it must be clean.
- If a lint run produces errors, group them by root cause and present
  **one group at a time** for approval before fixing. Do not batch.

### Decisions required before starting

These are not for the implementer to decide.

**All decisions are settled. Nothing below blocks the start of work.**

D-a through D-f were answered on **2026-07-25**; D-g and D-h were
**deferred** the same day. All eight are recorded struck-through below,
with the consequence already folded into the affected items. They are
kept rather than deleted so the implementer can see *why* an item is
scoped the way it is.

| # | Decision | Affects |
|---|---|---|
| ~~**D-a**~~ | ~~Spec 0168 (root type before decode)?~~ **Implement.** | W6 is in. **W4 is dropped entirely** — 0168 deletes the spawn site, which deletes both of its defects |
| ~~**D-b**~~ | ~~Spec 0169 (budget elision run)?~~ **Suspended.** The requirement is only that an elided region *renders as* `...`; it does **not** need to be a navigable node. That subset is carved out into **spec 0170**. | W22 now implements [0170](../specs/0170-prototext-core-render-budget-truncation-as-ellipsis.md), is `prototext-core`-only, is no longer blocked by W5, and drops from medium to low risk. **No `NodeSpan::is_elision` field is added** |
| ~~**D-c**~~ | ~~Spec 0162 (arena reclamation)?~~ **Yes, eventually — may be deferred.** | W23 stays in Phase 8 but is **no longer a blocker for W24**. See the consequence note in W23 |
| ~~**D-d**~~ | ~~Spec 0165 (heat-cue pool sizing) in or out of scope?~~ **In.** `--heat-cue-stats` in particular is wanted. | **W20 becomes "implement 0165"**. W9 is unchanged — its two parts are this review's findings, are not in 0165, and land first |
| ~~**D-e**~~ | ~~Is the 24.5 MB class a real target?~~ **Yes.** Opening `googleapis.desc` (24.5 MB) is a required capability. | all of **Phase 8** is in scope, not conditional |
| ~~**D-f**~~ | ~~Do W16, knowing W24 deletes rather than accelerates it?~~ **No — skip it.** | **W16 is dropped.** S2 is resolved by W24 instead. W17 is unaffected and still in |
| ~~**D-g**~~ | ~~Scoring-graph format version bump for a per-enum open/closed bit?~~ **Answered, 2026-07-26 — no format change needed.** Spec [0176](../specs/0176-open-enums-have-no-range.md): an open enum has no range, so it emits `type: int32` in `reproto` and the bit has nothing to qualify. | W30's interim is superseded, and its precision loss is recovered rather than accepted: a closed enum keeps its full range. See [Deferred](#deferred-out-of-scope-for-this-worklist) item 2 |
| ~~**D-h**~~ | ~~Widen the `u16` 65 535-root ceiling, or correct the design doc?~~ **Answered, 2026-07-26 — widen it.** Spec [0179](../specs/0179-active-entry-field-widths.md): `ActiveEntry::entries` is `SmallVec<[u32; 4]>`, measured allocation-neutral because the inline capacity did not change. **No format change** — the index is runtime-only. | W29's `assert!`→`Err` conversion stands and the check is kept, now against `u32::MAX`. See [Deferred](#deferred-out-of-scope-for-this-worklist) item 1 |

**Three items are struck through and must not be implemented: W4**
(superseded by W6), **W16** (superseded by W24), and **spec 0169**
(suspended in favor of 0170). They are retained so that a reader tracing
a flaw or roadmap ID to its work item lands somewhere that explains where
it went, rather than nowhere.

### Deferred (out of scope for this worklist)

**Both are now closed** (2026-07-26) and are kept below only so that the
reasoning is not rediscovered from scratch. They were orthogonal to
everything above — no item in this worklist waited on them, and both were
rescoped so that the work they touch is self-contained without them.

Neither needed a format bump, which retires the batching advice that used to
stand here ("take D-h first, then D-g, because a format bump is worth
batching").

**1. ~~D-h — the 65 535-root ceiling.~~ Closed 2026-07-26 — spec
[0179](../specs/0179-active-entry-field-widths.md), with no format change.**

`ActiveEntry::entries` held `u16` indices, so `score_all` asserted
`graph.roots.len() <= u16::MAX`, while [../schema-match.md](../schema-match.md)
states a "100,000+ FDPs" target. Both could not be right, and the
contradiction was live rather than theoretical: googleapis alone compiles to
**49 255 roots**, 75% of the old ceiling. Resolved by widening, not by
correcting the doc.

The measurement this deferral asked for was made, and it moved the answer.
The concern was that `u32` "costs memory in the hottest structure in the
scoring walk" — true of `size_of`, false of what matters. The load-bearing
number is the *inline capacity*, and holding it at 4 makes every spill
decision bit-identical to the `u16` version, so the widening is
allocation-neutral. `SmallVec<[u32; 2]>` keeps `size_of` unchanged and looks
like the frugal choice for exactly that reason; measured, it costs **+21.9%
allocations**.

W29 had already converted the panic into a clean `Err`, so the failure mode
in the meantime was a legible error message rather than an abort — which is
what made this deferrable rather than merely postponed. That check is kept,
now against `u32::MAX`.

**2. ~~D-g — a per-enum open/closed bit in the compiled graph.~~ Closed
2026-07-26 — spec [0176](../specs/0176-open-enums-have-no-range.md), with no
format change.**

The framing was wrong, and the wrongness is worth keeping on the record. It
asked how to *qualify* an open enum's range, so every answer was a new bit
somewhere and therefore a format bump. But **an open enum has no range**:
every 32-bit value is legal. `type: int32` says precisely that, is already a
kind the builder and the walk both understand, and leaves standing the one
check that should survive (C5's 32-bit gap veto). The whole change is four
lines in `reproto/src/reproto/phases.py`.

Two corrections it also forces:

- W30's interim was **not** the correctness half at the cost of precision. It
  gave up a closed enum's discriminating power *and* did not actually reach the
  `prototext` CLI (see the correction under W30), so it bought less than it
  cost. Spec 0176 recovers both: closed enums keep their full range, open enums
  have none.
- Generalizing: when a deferral's scope is "add a bit to the format", check
  first whether the thing being qualified should exist at all. Deleting the
  range was cheaper than describing it.

What genuinely remained was narrower and was *not* D-g: whether the two
surviving range **vetoes** — bool, and closed enum — should be vetoes rather
than penalties, since neither value is impossible on the wire. Tracked as C12 in
[../scoring-flaws.md](../scoring-flaws.md), and closed the same day by spec
[0178](../specs/0178-out-of-range-is-a-penalty-not-a-veto.md): both are
penalties now, and `strict_ranges`/`--relax-ranges` are gone.

---

## Phase 0 — Baseline

Nothing downstream can be evaluated without this. Several later items are
explicitly gated on measurements.

### W0. Promote the profiling harness and record a committed baseline

**Fixes:** the measurement gates in S6, S10(3), S11, and the roadmap's
final "re-measure" step; also doc drift in the harness's own header.

**Files:** `protolens/src/tui/tests/profiling.rs`,
`docs/protolens/rendering-scaling-roadmap.md`.

**Change.**

1. Fix the header comment: the invocation is `cargo test --release -p
   protolens --bin protolens tui::tests::profiling -- --ignored
   --nocapture`. It currently says `--lib`, which cannot work — there is
   no `src/lib.rs`.
2. Remove the "throwaway — not meant to stay in the tree" disclaimer and
   the two one-off diagnostics that have served their purpose
   (`diagnose_pdb_max_children_per_parent` and any other hypothesis-
   specific probe). Keep the harness itself.
3. The fixtures (`/tmp/pdb.desc`, `/tmp/db3.desc`) are not in the repo
   and every test already skips gracefully when absent. Document at the
   top of the file what each fixture is and how to obtain or regenerate
   it, so the numbers are reproducible by someone who doesn't have them.
4. Add timings for the steps the roadmap's cost table names but the
   harness does not currently measure: `DescriptorContext::load`,
   `decode()`, `App::new`, `resolve_root_winner_fqdn` (needed by W6),
   one override commit, and one `t`-then-`Down` sequence.
5. Record the results as a dated table in the roadmap, replacing the
   current indicative one, with the machine and the fixture sizes named.

**Proving test.** The harness runs clean on a machine with no fixtures
(all skips) and produces the full table on one with them.

**Blocked by:** nothing.

**Risk:** none. No production code touched.

---

## Phase 1 — Crashes and silent corruption

These are the items that fail *in front of a user*, on ordinary actions.
None depends on any other; do them first and in any order.

W26–W29 come from the two library reviews rather than from the rendering
review. They belong here and not in a separate track because their
failure modes are protolens's failure modes: a `SIGSEGV` in
`prototext-core` kills the TUI just as dead as one in `protolens`, and
W26 in particular fires on `googleapis.desc`, the input class D-e
confirmed as required.

### W1. ~~Repair the seam node's `doc_prev` before preview truncation~~ — **OBSOLETE 2026-07-26, via spec 0185**

[Spec 0185](../specs/0185-the-preview-is-an-overlay.md) deleted the
preview's splice, and with it the watermark truncation this item patches.
There is no seam and no `Err` path left to make consistent —
`preview_override_highlight` renders into an overlay and does not touch
the tree. Nothing to do; [C1](rendering-flaws.md) is closed by deletion
rather than by fix.

**Do not port this fix to the committed splice path by analogy.** That
path overwrites `tree[after].doc_prev` unconditionally on its own success
path, which is where it always was correct; the bug was specific to
truncation-then-maybe-fail.

**Original item, for the record:**

**Fixes:** [C1](rendering-flaws.md).

**Files:** `protolens/src/tui/override_select.rs:815-841`.

**Change.** In the pre-truncation block that already recomputes
`idx.doc_next`, also write the reverse pointer:

```rust
let seam = self.doc_next_after_subtree(self.tree[idx].doc_next, &old_descendants);
self.tree[idx].doc_next = seam;
if let Some(a) = seam {
    self.tree[a].doc_prev = Some(idx);   // missing today
}
```

It must go *before* `self.tree.truncate(watermark)`, for the same reason
the `doc_next` recomputation does.

**Proving test.** Force `splice_override` to return `Err` on a preview
(an unparseable target), then navigate backward across the seam and
assert no panic and a well-formed `doc_prev`/`doc_next` chain. Assert
the chain is consistent on **both** the `Ok` and `Err` paths — the point
of the fix is that consistency stops being conditional on success.

**Blocked by:** nothing.

**Risk:** low. Four lines, one function.

---

### W2. Make the line-patch ordering check always-on, and preferably unnecessary — **DONE 2026-07-26**

Took the stronger form: `materialize_line_patches` sorts `top_level` and
each `children_of` entry by range start before merging, so ordering stops
being a caller obligation, and both `debug_assert!`s became `assert!`s on
*overlap* only, naming the offending pair. Proven by three tests in
`tests/override_apply.rs` that queue back-to-front (top-level and
nested) and one `should_panic` for overlap — all under `--release`.
`materialize_line_patches` became `pub(super)` so the tests can reach it.

**Fixes:** [C2](rendering-flaws.md).

**Files:** `protolens/src/tui/override_apply.rs:1216`,
`protolens/src/tui/override_apply.rs:1261`.

**Change.** Prefer the stronger form: have `materialize_line_patches`
sort a scratch index by `global_start` and `assert!` only non-overlap,
with a message naming the offending pair. This removes the requirement
that *callers* queue in order — the requirement that turned out to be
violable at a distance — and is O(k log k) in the batch's patch count,
not in document length.

If the sort is rejected, the minimum acceptable change is promoting both
`debug_assert!`s to `assert!` with a message naming
`prev.global_start`, `prev`'s length, and `patch.global_start`.

**Proving test.** A unit test that queues patches out of order and
asserts the resulting merge is still correct (sort form) or panics with
the directed message (assert form). Either way the test must run under
`--release`, which is the whole point.

**Blocked by:** nothing.

**Risk:** low. Contained in one function.

---

### W3. Make `run`'s terminal restore structurally unconditional — **DONE 2026-07-26**

Done as specified (closure whose `Result` is captured; panic hook moved
above the first fallible call), plus one addition the item did not
anticipate: the *setup* window — `push_keyboard_enhancement`,
`EnterAlternateScreen`/`EnableMouseCapture`, `Terminal::new` — is also
fallible with raw mode already on, and cannot be covered by the main
cleanup block because `terminal` does not exist yet. It got its own
captured `Result` with an explicit `restore_terminal()` on the error arm.
No `?` now remains between the setup and the cleanup.

**Fixes:** [C4](rendering-flaws.md).

**Files:** `protolens/src/tui/mod.rs:1615-1712`.

**Change.** Wrap the fallible middle of `run` — everything between
entering raw mode / alternate screen / mouse capture and the
`restore_terminal()` block — in a closure whose `Result` is captured,
exactly as `run_loop`'s result already is. Today `terminal.size()?`
(`:1631`) and `warm_up_heat_cues(...)?` (`:1694`) return above the
cleanup.

Do this **before** W6: spec 0168 adds two more fallible calls to this
region, and the point of the closure is that future `?`s are covered by
construction rather than by review.

Also move the panic-hook installation (`:1637-1641`) above the first
fallible call.

**Proving test.** Hard to test end-to-end without a pty harness. At
minimum, assert by inspection in review that no `?` remains between the
terminal setup and the cleanup block, and note in the code why the
closure exists.

**Blocked by:** nothing. **Blocks:** W6.

**Risk:** low, but it restructures a function that owns terminal state —
verify manually that a normal quit, a `q`-at-splash quit, and a panic
all still restore the terminal.

---

### W4. ~~Root-type thread: stack size and lifetime~~ — **DONE 2026-07-26, via spec 0180**

**Dropped 2026-07-25 by decision D-a**, then **un-dropped and done
2026-07-26.** This was the interim patch for [C3](rendering-flaws.md)
(a) and (b), to be done *only* if spec 0168 were deferred; the drop
reasoned that 0168 was being implemented and W6 would delete the spawn
site outright.

**The drop was wrong, for a reason worth recording.** 0168 was not in
fact implemented, so "a later spec will delete this code" left a
use-after-unmap and a 3.6×-margin stack live in the shipping binary for
a spec that had not landed. A pending deletion is not a fix, and the
cost of being wrong about the schedule was a `SIGSEGV` at quit.

Both halves are now closed by
[spec 0180](../specs/0180-own-the-scoring-graph-by-arc.md): (a) by the
`Arc<LoadedGraph>` of W8 part 1, which is strictly better than this
item's interim patch because it makes the guarantee structural rather
than mitigating it; (b) by S4, which moved the constant to
`tui/mod.rs` as `SCORING_THREAD_STACK_SIZE` and gave this spawn a
`thread::Builder` — exactly this item's proposal. If W6/0168 later
deletes the spawn site, it deletes an already-sound one.

---

### W26. Cap decode recursion, and turn the node budget on in production — **CLOSED 2026-07-26 (half 1 done, half 2 superseded)**

Verified against the code on 2026-07-26, not merely assumed:

- **Half 1 (depth cap) is done**, via spec 0171. `render_message` tracks
  its own recursion depth and hands the payload back as bytes at the cap
  (`render_text/mod.rs`, `helpers/len_field.rs`), and the constant is
  shared workspace-wide as `MAX_WIRE_DEPTH` in
  `prototext-core/src/helpers/bounds.rs`, with the stack margin
  *measured* rather than asserted.
- **Half 2 (turn the node budget on) is superseded, not done.** Spec 0174
  G1 *deleted* `DecodeRenderOpts::node_budget` outright and replaced it
  with an input-side byte budget on the live preview
  (`OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT`). Bounding the input bounds the
  output, so there is no *n* left to choose and no budget left to enable.
  The `Watch for` warning below — and W28, which existed only to make
  that budget safe to turn on — go with it.

**Original item, for the record:**

**Fixes:** [decode C3](../prototext/decode-flaws.md).

**Files:** `prototext-core/src/serialize/render_text/mod.rs` (the depth
cap), `protolens/src/decode.rs:769-787` (the options).

**Change.** Two halves, both required — either alone leaves the crash
reachable.

1. Thread a depth counter through `render_message` the way `LEVEL`
   already is, and emit a `MalformedKind` line at the limit instead of
   recursing. Grep confirms no depth limit exists today: `DEPTH|depth`
   in `render_text/` finds only `LEVEL`, which drives indentation and is
   never compared against anything. Emitting a malformed line rather
   than truncating silently is what keeps the renderability promise —
   "this nests deeper than we follow" is information.
2. `protolens`'s production `DecodeRenderOpts` uses
   `..Default::default()`, which supplies `node_budget: None`. Spec 0163
   built the budget and nothing ever turned it on. Set an explicit
   `Some(n)`.

Choose *n* only **after W28 lands** — see the warning below.

**Proving test.** A synthetic blob of *n* nested single-field LEN
envelopes at *n* = 10 000, decoded with production options, must produce
a malformed line and return, not overflow the stack. Add the symmetric
case for the node budget. Both belong in `prototext-core`'s test suite,
not protolens's, so the library is covered independently of its caller.

**Watch for.** Do not set the budget before W28. A tripped budget today
has a *rendering-fidelity* side effect (decode C5): the cascade probe
shares `NODE_COUNT`, so exhausting the budget reclassifies well-formed
nested messages as opaque bytes, silently. Turning the budget on first
would trade a loud crash for quiet wrong output — strictly worse.

**Blocked by:** W28 (for half 2 only; half 1 is unblocked).

**Risk:** low for half 1 (additive, and the failure path already exists).
Medium for half 2, entirely because of the *n*: too low and ordinary
documents get truncated.

---

### W27. Replace the wire-format bounds arithmetic in both crates — **DONE**

Verified 2026-07-26. The shared module landed as
`prototext-core/src/helpers/bounds.rs` (`payload_end`, `bytes_missing`,
`MAX_WIRE_DEPTH`), phrased as the subtraction and covering even the
fixed-width `8`/`4` cases that cannot wrap — "the same idiom written by
the same hand". `prototext-graph`'s `score/walk.rs` imports it and every
one of its length checks goes through it, so the "two crates wrote this
independently" defect is closed through one implementation as the item
required.

**Fixes:** [decode C1, C2](../prototext/decode-flaws.md),
[scoring C1, C3, C4](../scoring-flaws.md).

**Files:** a new shared helper module; then
`prototext-core/src/serialize/render_text/mod.rs:538-555`,
`prototext-core/src/serialize/render_text/helpers/any_field.rs:116-161`,
`prototext-graph/src/score/walk.rs:421-429`, `:453`, `:669`, `:734`,
`:817-828`, `:1031`.

**Change.** Six sites across two crates share one defect:

```rust
let length = varint as usize;
if pos + length > buflen { /* reject */ }
let data = &buf[pos..pos + length];
```

`length` is attacker-controlled up to `u64::MAX`. This project builds
release exclusively, where the addition **wraps** — so the guard passes
on exactly the input it was written to reject, and the slice then panics
with `start > end`. Phrase it as a subtraction on the known-good side
(`pos <= buflen` is an invariant of every one of these loops):

```rust
if length > buflen - pos { /* reject */ }
```

Two crates wrote this independently, and both also wrote an uncapped
blind group-skipping recursion (`skip_group`,
`parse_group_blind`). Fix the instances *through* a shared module rather
than in place, so the seventh site inherits the fix:

```rust
/// Wire-format bounds arithmetic. These are one-liners, but they were
/// each got wrong independently in two crates — the addition form
/// `pos + len > buflen` wraps in release builds and *passes*.
pub fn len_fits(len: u64, pos: usize, buflen: usize) -> bool;
/// Depth-capped blind group skip. Recursing per START_GROUP tag costs
/// one frame per byte on hostile input; stack overflow is a SIGSEGV.
pub fn skip_group(buf: &[u8], pos: usize, expected_field: u64, depth: usize) -> Option<usize>;
```

Fold in scoring C4 while here: `parse_wiretag` correctly flags
`out_of_range` field numbers (0 or `>= 2^29`) but the caller only
*scores* the flag and then does `field_number as u32`, truncating
`2^32 + 1` into field `1` — where it can find a real transition and score
a spurious match. An out-of-range field number is not a field; reject it
before the lookup.

**Proving test.** A `cargo-fuzz` target per crate — arbitrary bytes into
`render_message` with a `TextSink` and a fixed small descriptor pool, and
arbitrary bytes into `score_all` against a fixed small graph. These
findings were all reachable in minutes of fuzzing, and the fuzz targets
are the real deliverable: they stop the class, not just the six
instances. Add explicit regressions for the `u64::MAX`-length blob and
the million-START_GROUP blob so the class is pinned even without a fuzz
run.

**Blocked by:** nothing. **Note:** W26 half 1 and this item both add a
depth cap; agree on one constant and one home for it before starting
either.

**Risk:** low. Every changed site's rejection path already exists — this
makes it reachable.

---

### W28. ~~Give the cascade probe its own node-budget scope~~ — **MOOT 2026-07-26**

The leak this item plugs is the cascade probe charging the outer render's
`NODE_COUNT`. Spec 0174 G1 deleted the node budget, and with it
`NODE_COUNT` — a workspace-wide grep for that identifier now returns
nothing. There is no shared counter left for the probe to disturb, so
there is nothing to save and restore and no doc comment left to correct.

**Its dependent is gone too:** W28 blocked W26 half 2, which the same
spec superseded. Neither is actionable.

**Still worth keeping from this item:** its proving test is a good one on
its own terms — *decode the same fixture at two different resource
pressures and assert byte-identical output* pins "a resource limit must
not change what structure is recovered", which the byte budget can
violate just as a node budget could. If that invariant is ever pinned,
pin it against `OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT`.

**Original item, for the record:**

**Fixes:** [decode C5](../prototext/decode-flaws.md).

**Files:**
`prototext-core/src/serialize/render_text/helpers/len_field.rs:64-83`,
`prototext-core/src/serialize/render_text/sink.rs:865-868` (the doc
comment).

**Change.** `ProbeSink`'s doc comment states it "never mutates any shared
render-mode thread-local state" and "must not disturb that render's own
state". `render_message` increments `NODE_COUNT`, which *is* shared, so
the probe charges the outer render's budget for every speculative decode
— including payloads it then concludes are not messages at all.

Save, zero, and restore `NODE_COUNT` around the probe call, or thread an
explicit per-probe budget. The probe must remain bounded either way — an
unbounded probe is its own denial of service.

Fix the doc comment in the same commit. It currently enumerates
`tracks_level` as the mechanism; it should name `NODE_COUNT` explicitly
as a thing that is saved and restored, so the next thread-local added
does not silently join the leak.

**Proving test.** Decode the same nested-message fixture at two different
budget pressures and assert the two renderings are **byte-identical**.
That is the invariant — a resource limit must not change what structure
is recovered — and nothing currently pins it.

**Blocked by:** nothing. **Blocks:** W26 half 2.

**Risk:** low. Localized, and the test is a strong one.

---

### ~~W29.~~ Fix the scoring-graph loader's unvalidated `root_offset` (spec 0172)

> **Done (2026-07-25).** Spec 0172 S4/S5. One deviation: the bounds check
> is `root_offset > bytes.len()`, not "room for the archived root". The
> stronger form would have to hardcode a size the archived layout owns,
> and on the mmap path rkyv's checked `access` already rejects a payload
> too short for the root — so the check below it earns nothing but a
> second place to be wrong. The `from_raw_parts` is gone entirely, which
> is what made the weak check dangerous in the first place.

**Fixes:** [scoring C8](../scoring-flaws.md), and partially C10.

**Files:** `prototext-graph/src/score/load.rs:34-89`,
`prototext-graph/src/score/walk.rs:187-191`.

**Change.** `check_header` validates magic and version but returns the
header's `root_offset` unchecked, and the mmap path then does
`mmap.as_ptr().add(root_offset)` with `mmap.len() - root_offset` inside a
`from_raw_parts`. If `root_offset > mmap.len()` the `.add()` is
**undefined behavior on its own**, before any read, and the length
underflows to near-`usize::MAX`. A `.rkyv` file truncated at 24 bytes
passes every check that exists.

Validate inside `check_header` so both callers inherit it, require room
for the archived root rather than merely `<= len`, and give the `unsafe`
block a safety comment naming the precondition `check_header` discharges.
The absence of that comment is why the gap was invisible.

While in `load.rs`, move `walk.rs:187-191`'s
`assert!(graph.roots.len() <= u16::MAX)` here too and return the existing
`Result`: "this graph has more roots than this build can index" is a
message the TUI can show, not a reason to abort.

**Do not widen the index.** Whether 65 535 is the right ceiling is
**deferred** (D-h) — it is a real question, and it is not this item's.
Converting the panic into an error is correct regardless of where the
ceiling ends up, and is all this item does. Leave a doc comment on the
new check pointing at D-h so the deferral is visible at the constraint
rather than only in this document.

> **D-h has since been answered** (2026-07-26, spec
> [0179](../specs/0179-active-entry-field-widths.md)): the index is `u32`
> and the ceiling is 4 294 967 295. The check this item created is *kept*
> — `roots.len()` is a `usize`, so a 64-bit target can still express more
> roots than the index addresses — and its doc comment now records the
> answer instead of the deferral. The split was the right one: this item's
> `assert!`→`Err` conversion needed nothing from the width decision.

**Proving test.** A truncated and a `root_offset`-corrupted fixture must
both produce an `Err`, not a crash. Run the loader tests under Miri if
the mmap can be stubbed; otherwise the corrupted-header test is the
practical proof.

**Blocked by:** nothing. **Related:** the `&'static` graph lifetime
(scoring C9) is the same defect class as W4/W8 — fix it there, not here.

**Risk:** low.

---

### ~~W30.~~ Stop veto from absorbing on legal-but-unexpected encodings (spec 0172)

> **Done for C5 and C6 (2026-07-25).** Spec 0172 S2/S3. One deviation:
> the enum veto is demoted **by default** rather than *unconditionally* —
> `ScoringOpts::strict_ranges` survives as an opt-in knob (spec 0172 N3),
> because the knob is what lets the D-g format bump re-enable vetoing for
> genuinely closed enums without re-introducing the mechanism, and it is
> what the strict-mode tests now exercise.
>
> **Correction (2026-07-26): "nothing in this workspace sets it" was false,
> and C6 was therefore not done.** This block used to close with that claim.
> All three of the `prototext` CLI's scoring entry points compute
> `strict_ranges: !relax_ranges` (`prototext/src/run.rs:423`, `:500`, `:532`)
> from a bare `clap` boolean, so **the CLI's shipped default is strict** and
> C6 stayed fully live on the primary user-facing path. Only `protolens` reads
> `ScoringOpts::default()`. The lesson: flipping a `Default` impl does not
> change shipped behavior when a CLI builds the struct field by field, so an
> "interim" of that shape needs a test through the binary — which is why spec
> 0176's end-to-end test drives the `prototext` binary rather than `score_all`.
> **C6 is closed by spec 0176 instead**, at the source: an open enum emits no
> range, so `strict_ranges` has nothing to be strict about, and a closed enum
> keeps its full range (the precision loss below is recovered, not accepted).
>
> **C7 done (2026-07-26).** Spec 0175, its own spec as this item asked.
> One claim above was wrong: it does **not** need the graph to record that
> a leaf is a repeated scalar. `label` has been on every
> `TransitionEntry` since spec 0045 and spec 0173 already routed it into
> the verdict loop as `tr.label`. What was actually missing was the
> *element type*, which `reproto` was discarding — all seven packable
> types collapsed to `LEN_PACKED` — in exchange for `is_packed`, a bit that
> carries no information a scorer may act on precisely because both
> encodings are always legal. So no format change and no version bump:
> `Verdict::FoundPacked` reads and validates the run instead.
>
> **The deviation is retracted (2026-07-26).** Spec 0178 demotes the
> out-of-range check unconditionally, as this item originally asked, and deletes
> `ScoringOpts::strict_ranges` and `--relax-ranges` outright. The knob's stated
> justification was that it would let the D-g format bump re-enable vetoing for
> closed enums — but D-g was dropped (spec 0176 needed no format change), so the
> knob was preserving an option nobody was going to take, while the CLI's
> reading of it was the one path where C6 stayed live.

**Fixes:** [scoring C5, C6, C7, C12](../scoring-flaws.md).

**Files:** `prototext-graph/src/score/walk.rs:933-961`.

**Change.** Veto is absorbing — one veto eliminates a candidate
permanently — and it currently fires on input that is valid protobuf.
Two causes; **this item fixes both, without a graph format change.**

1. **C5, negative enums.** Negative values sign-extend to a 10-byte
   varint, so `val >= 2^32`, which `:935` vetoes unconditionally
   *without consulting `strict_ranges`*. The sign-extension at `:949`
   (`val as i32 as i64`) that was clearly written to handle this is
   unreachable for genuinely negative values. The treatment is inverted:
   the non-canonical 5-byte form is tolerated, the canonical form is
   fatal. Any schema with `UNKNOWN = -1` is unmatchable. Mirror the
   `INT32` arm at `:918`, which gets it right.
2. **C6, proto3 open enums.** Grepping `prototext-graph/src` for
   `syntax|proto3|open_enum|closed` returns **zero matches** — the graph
   has no idea which enums are open. `strict_ranges` defaults to `true`,
   so an unknown-but-legal proto3 enum value from a newer sender
   eliminates the correct FQDN and hands the win to a structural
   look-alike.

   **Ship the interim, not the format change.** Demote the out-of-range
   enum veto to `non_canonical` unconditionally. Losing a proto2 enum's
   discriminating power costs precision; keeping the veto costs
   correctness, and precision is the cheaper thing to lose. Recording a
   real open/closed bit per enum needs a graph format version bump, which
   is **deferred** (D-g).

   Leave a doc comment at the demoted check naming D-g, so whoever does
   the next format bump finds the one line that wants revisiting.

**Proving test.** Score a blob carrying a negative enum value against its
own true schema and assert the true FQDN wins. Same for a proto3 enum
value outside the declared set. Both should be trivially true and are
not. *(The proto3 half landed with spec 0176 as
`reproto/src/reproto/tests/test_open_enum_scoring.py` — through the `prototext`
binary, since that is the only venue where the assertion is not vacuous.)*

**Blocked by:** nothing — the D-g deferral is what makes this
self-contained. **Verify [scoring C7](../scoring-flaws.md) first** — if
packed/unpacked repeated fields also veto, it is the same fix and the
same commit.

**Risk:** medium — this changes ranking outcomes, so `reproto`'s
scoring-graph tests are the acceptance criterion, and some existing
expected-winner fixtures may legitimately change.

---

## Phase 2 — Invariants that later work depends on

Two hardening items that several later phases assume. Doing them out of
order means doing them twice.

### W5. Key the heat cache on the full range

**Fixes:** [A6](rendering-flaws.md), and unblocks A4's fix.

**Files:** `protolens/src/tui/heat_cue.rs`,
`protolens/src/tui/heat_worker.rs`, `protolens/src/tui/tiered.rs`
(key type only).

**Change.** `HeatCaches::by_range` is keyed on a bare `usize` (the
range's `start`), justified in spec 0151 lines 89-101 by "node byte
ranges are disjoint". They are **nested**, so that justification is
false. The key may still be unique today for a different reason
(length-prefix framing puts a child's payload strictly after its
parent's start), but that argument is unwritten and is not robust by
inspection to the synthetic wrapper or to packed-run absorption. (It was
*also* not robust to suspended spec 0169's elision nodes, whose ranges
would have started exactly where the next real field does. That is no
longer a live case — 0170 emits no spans — but it is worth recording as
independent evidence that the bare-`usize` key is fragile to any future
node whose range is not a decoded field's.)

Change the key to `Range<usize>`. If that is rejected on size grounds,
the minimum acceptable outcome is writing the real uniqueness argument
into spec 0151 and into the field's doc comment, plus a
`debug_assert`-free runtime check that two distinct ranges never collide.

**Proving test.** Construct a parent and its first child (which share no
`start` under framing, but do under the synthetic wrapper) and assert
their heat entries are distinct. Add a regression asserting the existing
tiered-cache tests still pass with the widened key.

**Blocked by:** nothing. **Blocks:** W7. (No longer blocks W22 — decision
D-b removed the elision nodes that created the collision.)

**Risk:** low-medium. Mechanical, but touches the cache's public shape
and spec 0164's tier tests are the acceptance criterion.

---

### W6. Resolve the root type before decoding (spec 0168)

**Fixes:** [C3](rendering-flaws.md) (both defects, by deletion),
[S11](rendering-scaling-roadmap.md), and one of P4's three blob copies.

**Spec:** [../specs/0168-protolens-resolve-root-type-before-decode.md](../specs/0168-protolens-resolve-root-type-before-decode.md)
— read it in full; it is the authority, this is a pointer.

**Gate.** The spec's open question must be answered first: measure
`resolve_root_winner_fqdn` on both fixtures (W0 adds the timing) and
proceed only if the sweep is materially cheaper than the ~10.6 s root
re-splice it eliminates. The margin is expected to be very wide. If it
is not, take the spec's stated fallback (keep the sweep async, make
`RootTypeResolved` trigger a re-decode rather than a splice) instead.

**Proving test.** As the spec's test plan: decode-called-once assertion,
`--type` skips the sweep, no-graph path unchanged, cache-seed assertions
with no re-invocation of `score_all`, and the existing
`resolve_root_winner_fqdn` tests unchanged.

**Blocked by:** W0 (the gate), W3 (the closure). **D-a is answered:
implement.** **Supersedes:** W4, which is dropped.

**Do W31 first if convenient.** W31 removes an O(A²) inner loop from
`score_all`, and this item's gate is a *measurement* of `score_all`. Gate
on the post-W31 number — gating on the pre-W31 number risks rejecting
0168 for a cost that is about to disappear.

**Risk:** medium. It deletes a subsystem (`defer_root_type`,
`root_type_pending`, `AppEvent::RootTypeResolved`,
`apply_resolved_root_type`, the spawn) and reorders startup. High
confidence, wide blast radius — do it as one reviewable commit.

---

## Phase 3 — Startup cost

W31 and W32 are library items, but they land here because startup is
where their cost is paid: `resolve_root_winner_fqdn` is one `score_all`
call over the whole blob (W31), and `display_name` runs once per rendered
line of the initial decode (W32).

### W31. Remove the per-tag O(A²) scan in `score_all`

**Done:** spec 0173, 2026-07-26. One correction to the plan below: (1)'s
"index positionally (`zip`)" is *wrong*, and the side table's own comment
said why — the mismatch loop clears entries and `active.retain()`
compacts the vector between the two loops, so `verdicts[i]` no longer
belongs to `active[i]`. The verdict moved onto `ActiveEntry` instead,
where `retain` carries it along for free. Measured at A = 4096:
1.114 s → 68.9 ms, with the cost-vs-A curve dropping from 14.2× per 4×
A to 5.3×. (2) became a closure rather than a hoist, so it costs nothing
at all when unread; (3) landed as written.

**Fixes:** [scoring P1, P4, P5](../scoring-flaws.md).

**Files:** `prototext-graph/src/score/walk.rs:834-882`, `:1041`, `:60`,
`:197`, `:860-862`.

**Change.** Three wins in one function, in decreasing order of size.

1. **The verdict lookup (P1).** Per wire tag, the walk builds one verdict
   per active state, then re-finds it per active entry group by linear
   scan — and `verdict_for` scans again in the LEN arm. That is O(A²) per
   tag with A active groups, and A is largest early in the walk, before
   vetoes have pruned. This is the leading explanation for the measured
   533 ms sweep. The irony is that `verdicts` is *built* by iterating
   `active` in order, so `verdicts[i]` already belongs to `active[i]` —
   the scan reconstructs information the previous loop just discarded.
   Index positionally (`zip`), and pass the index rather than the
   `state_id` to the LEN-arm consumer.
2. **The eager veto reason (P5).** `walk.rs:860-862` builds a `format!`
   *inside* the `for &e in &ae.entries` loop, so a group of *k* members
   builds *k* identical strings — and vetoing is the common outcome, not
   the error path. Hoist it out of the inner loop; then, if the reason is
   only ever read for diagnostics, make it lazy.
3. **`EntryScore.fqdn` (P4).** `walk.rs:197` does
   `r.fqdn.as_str().to_owned()` per root per call — 100 000 allocations
   before a byte of the blob is examined, defeating the point of the rkyv
   zero-copy mmap. Borrow: `pub fqdn: &'g str`. This is a signature
   change touching both protolens call sites and `reproto`, so do it as
   its own commit. If the lifetime is awkward where the winner must
   outlive the sweep, clone *the winner only*, at the one site that needs
   ownership.

**Proving test.** Benchmark `score_all` on `googleapis.desc` before and
after (1) — this is the one change in either library report with a large
enough expected effect to justify a dedicated benchmark. Beware the
noise caveat in `docs/performance.md`: this sandbox varies 30–45% run to
run, so repeat, and corroborate with an operation count if the wall clock
is ambiguous. (2) and (3) are structural wins provable by inspection.

**Blocked by:** nothing. Do it before W10 — W10's seeded candidate list
changes *how often* `score_all` is called, and measuring that on top of
an O(A²) inner loop confuses the two effects.

**Risk:** low for (1) and (2) — the outputs must be bit-identical, so any
existing scoring test is the regression. Medium for (3), for blast
radius only.

---

### W32. Stop allocating a `String` per rendered line in `display_name`

**Done:** spec 0173, 2026-07-26, as `write_display_name(&self, out: &mut
Vec<u8>)`. A counting allocator puts the effect at exactly one
allocation per schema-named line removed and none left over (5 485 →
2 974 per render of the 18 KB `fixtures/descriptor.pb`, whose protoc
text has 2 511 named lines), for −8% wall clock on the annotated path
and no movement on the schema-less control.

**Fixes:** [decode P4](../prototext/decode-flaws.md).

**Files:** `prototext-core/src/serialize/render_text/mod.rs:96-101`,
`prototext-core/src/serialize/render_text/helpers/output.rs:75-113`.

**Change.** `FieldOrExt::display_name()` returns `String`; both arms
allocate, and the `Field` arm allocates an owned copy of a name the
descriptor pool already owns and outlives the call. Its two call sites
are the functions whose own doc comments read "Write field-line prefix
without String allocation" (`output.rs:75`) and "Write open-brace prefix
without String allocation" (`:95`), and both do
`out.extend_from_slice(fi.display_name().as_bytes())`. At ~4.3 M lines
for a 24.5 MB descriptor set that is 4.3 M allocate-copy-free cycles
whose only purpose is satisfying a signature.

Since both call sites immediately push into an existing `Vec<u8>`, the
right shape is to write directly rather than to return anything:

```rust
fn write_display_name(&self, out: &mut Vec<u8>) { ... }
```

which removes the allocation from the `Ext` arm too. `Cow<'_, str>` is
the smaller change if a return value is needed elsewhere.

Correct the two doc comments in the same commit — they are the reason
this survived review, and leaving them stale after fixing the code just
re-arms the trap.

**Proving test.** Rendering output must be byte-identical; the existing
`render_text` tests are the regression. Add the timing to W0's baseline
rather than benchmarking it in isolation — the effect is real but is one
term among several in decode cost.

**Blocked by:** nothing.

**Risk:** low.

---

### W7. Fix the `complete` slot's unsatisfiable coverage test

**Fixes:** [A4](rendering-flaws.md).

**Files:** `protolens/src/tui/heat_worker.rs:199-203`, `:348`, `:402`,
`:240`, `protolens/src/tui/override_select.rs:485`.

**Change.** Today the override pane's only possible hit path is the
`complete` slot, because both coverage tests require
`top_n.len() >= end` and the pane requests `usize::MAX`. Prefetch work
then overwrites `complete`, so the sequence is self-sustaining: miss →
full sweep → prefetch clobbers → miss again.

Add `total_candidates: usize` to `RangeHeatEntry` and make
`covers_window` test against it rather than against `top_n.len()`, so
the test becomes satisfiable by a capped entry. Combine with W9's clamp:
after both, `by_range` holds a bounded window and still answers the
pane's coverage question truthfully.

**Proving test.** Open the pane on a range, let a prefetch wave run,
re-open the pane on the same range, and assert a cache hit (no second
`score_all` invocation — count calls behind a test hook).

**Blocked by:** W5.

**Risk:** low. Additive field plus one predicate.

---

### W8. Share the blob and the graph by `Arc` — **part 1 done 2026-07-26**

> **Part 1 (the graph) is done**, by
> [spec 0180](../specs/0180-own-the-scoring-graph-by-arc.md): the field
> is private, `graph()` exists, `DescriptorContext.graph` is
> `Option<Arc<LoadedGraph>>`, and A5 and both halves of C3 are closed.
> One deviation, recorded as that spec's N2: **the loader still returns
> a plain `LoadedGraph`**, because the soundness property comes from
> privacy rather than from `Arc`, and `prototext`'s single-threaded CLI
> would pay an allocation and an indirection for nothing.
>
> **Part 2 (the blob) remains open**, and is the whole of what is left
> here: it is a memory optimization with no soundness content, it
> touches the override-splicing paths that mutate the blob, and bundling
> it with a safety fix would have made the safety fix hard to review.
> The 3×→1× blob win below is still unbanked, so P4 stays open.

**Fixes:** [P4](rendering-flaws.md), ~~[A5](rendering-flaws.md)~~ (done),
[S4(1)](rendering-scaling-roadmap.md) and its S10 rider; ~~retires C3's
lifetime half permanently~~ (done).

**Files:** `prototext-graph/src/score/load.rs:21-25`, `:82-89`;
`protolens/src/tui/mod.rs` (the `DescriptorContext` field and both
consumers).

**Change.** Two parts.

1. `LoadedGraph.graph` is `pub` and `Copy`, so the safety comment's
   "enforced by keeping both in `LoadedGraph`" enforces nothing — any
   caller can copy the `&'static` out from under the `Mmap`. Make the
   field private, expose `pub fn graph(&self) -> &ArchivedCompiledGraph`
   (the existing `Deref` impl already provides the intended path), and
   have the loader return `Arc<LoadedGraph>`.
2. `DescriptorContext.graph` becomes `Option<Arc<LoadedGraph>>`; the
   blob becomes `Arc<Vec<u8>>`. The heat worker's `&'static` disappears,
   which retires `App`'s field-order dependency as a load-bearing
   invariant — say so in the comment at `mod.rs:727-742` rather than
   deleting it.

**Proving test.** Existing suite; plus assert (by construction, i.e. it
must not compile otherwise) that no `&'static ArchivedCompiledGraph`
remains reachable from outside `prototext-graph`.

**Blocked by:** nothing (independent of W6, though tidier after it).

**Risk:** low-medium. Mechanical but crate-crossing. Immediate 3×→1×
blob memory.

---

### W9. Bound the heat cache's entry size, and stop cloning it per frame

**Fixes:** [P5](rendering-flaws.md), [P6](rendering-flaws.md),
[S10(1)](rendering-scaling-roadmap.md) and S10(2).

**Files:** `protolens/src/tui/heat_worker.rs:384-394`,
`protolens/src/tui/tiered.rs:139-145`,
`protolens/src/tui/heat_cue.rs:330`, `protolens/src/tui/render.rs:330-333`.

**Change.** Two independent parts.

1. Clamp the worker's `top_n_len` to the same
   `max(override_list_height, HEAT_CUE_PREVIEW)` bound the synchronous
   path already uses, so `req.end = usize::MAX` from
   `upgrade_active_override_to_complete` can no longer size a cache
   entry. The full list still lands — in the `complete` slot that exists
   for it. Spec 0151 measured ~1,012,849 bytes in one entry against a
   documented "well under 1MB" total.
2. Split `TieredBounded::peek` into `touch(&mut self, key, tier)` and
   `get(&self, key) -> Option<&V>`, so `HeatCaches::window` clones only
   the `[start..end)` slice it returns rather than the whole
   `RangeHeatEntry`. Today that is two deep clones per unsettled row per
   frame.

**Proving test.** (1) Issue a `usize::MAX` request and assert the stored
`top_n.len()` never exceeds the cap. (2) Existing `tiered.rs` tests
unchanged; keep `peek` for any caller not migrated.

**Relation to spec 0165, per decision D-d.** Spec 0165 is in scope, and
its G3 (`HeatCaches::new` taking a byte budget and a `size_fn`) covers
the same ground as W20 below. Part 1 of this item is *not* in 0165 — the
`top_n_len` clamp is this review's finding — and it should land first
regardless, because a byte budget that has to accommodate a
1,012,849-byte entry is sized by an accident rather than by a policy.
Part 2 (the `peek` split) is likewise not in 0165 and is independent of
it. So: do W9 as written, then implement 0165 at W20.

**Blocked by:** W7 (part 1 changes what `covers_window` sees).

**Risk:** low. (1) is three lines; (2) is a mechanical accessor split.

---

### W10. Replace the startup full-document walk with a seeded candidate list

**Fixes:** [P1](rendering-flaws.md), [S1](rendering-scaling-roadmap.md)
— ~5.2 s of a ~7 s startup, the largest single startup cost at every
size.

**Files:** `protolens/src/decode.rs:301-369` (`build_tree`),
`protolens/src/tui/mod.rs:1232-1234` (`App::new`).

**Change.** `App::new` walks the entire document via
`render_overrides(cursor)` purely to find Any/MessageSet auto-expand
candidates. `is_auto_expand_candidate` (`override_apply.rs:522-542`) is
local: the node's own `field_number` plus its parent's (for MessageSet,
its grandparent's) `span.type_fqdn`, plus one `ctx.pool()` lookup — all
available to `build_tree` as it goes.

1. Have `build_tree` collect `auto_expand_seeds: Vec<usize>` and return
   it on `Decoded`.
2. Replace the walk with a loop over the seeds, wrapped in **one**
   explicit batch so `k` seeds cost one finalization rather than `k`.
3. Finalize from the **earliest** seed in document order, not the last
   one processed — `finalize_override_batch` takes a single origin `idx`
   for its downstream walk, and getting this backwards leaves every
   later node's `text_range` stale (see A3). Write the completeness
   argument into the code: at `App::new` time the only override entry
   that can exist is the seeded root type, already reconciled at
   `mod.rs:1223`, so no non-candidate node can need resettling.

**Proving test.** Assert the seed list equals the set of nodes today's
full walk actually splices, on the existing Any/MessageSet fixtures.
Directly testable without measuring anything.

**Blocked by:** nothing (independent of W6; both cut startup, and W0's
baseline should be re-run after each so the two wins do not confound).

**Risk:** low.

---

## Phase 4 — Frame cost during progressive display

The measured 0.5 ms frame time is for *settled* rows. These items target
the window right after startup or a scroll into cold territory — i.e.
exactly when responsiveness matters.

### W11. Memoize `current_type_key` per node

**Fixes:** [P2](rendering-flaws.md), part 2 — the low-risk half.

**Files:** `protolens/src/tui/heat_cue.rs:243`,
`protolens/src/tui/override_apply.rs:650`, `:724`,
`protolens/src/tui/navigation.rs:53`.

**Change.** Memoize in a `Vec<Option<String>>` parallel to `tree`, keyed
on `(structural_version, overrides_version)`. `structural_version`
already exists and already bumps on the right events; add an
`overrides_version` counter bumped by every `OverrideCollection`
mutation.

Note the parallel-array hazard this introduces is the same one A2
describes — if W13 lands first, put the memo inside the arena type
instead.

**Proving test.** Assert the memo is invalidated by (a) a splice, (b) an
override add/remove/rotate, and (c) nothing else. Count
`positional_path` invocations behind a test hook across a simulated
scroll and assert it drops to O(visible rows) once, not per frame.

**Blocked by:** nothing. Prefer before W12.

**Risk:** low, purely additive.

---

### W12. Store the sibling ordinal on the node

**Fixes:** [P2](rendering-flaws.md), part 1 — the structural half.

**Files:** `protolens/src/tui/navigation.rs:425-433`,
`protolens/src/decode.rs` (`build_tree`),
`protolens/src/tui/override_apply.rs:1705-1715`.

**Change.** `sibling_position` walks `prev_sibling` one node at a time
back to the first sibling — O(ordinal position). A `FileDescriptorSet`'s
repeated runs have tens of thousands of siblings. Add `ordinal: usize`
to `TreeNode`, set by `build_tree` and by `splice_override` for the
appended local tree; `sibling_position` becomes a field read.

The one subtlety is packed-run absorption, which removes `run_len - 1`
siblings and shifts every following ordinal — handle by walking
`next_sibling` from `idx` once, bounded by the run's tail rather than by
the document.

**Proving test.** Assert `ordinal` matches a freshly computed
`sibling_position` for every node, after: initial decode, an override
splice, a preview truncation, and a packed-run absorption. That last one
is where this will break if it breaks.

**Blocked by:** W11 (which may make this unnecessary — re-measure
first).

**Risk:** medium. It adds a field with a maintenance obligation in
`splice_override`, historically the exact place invariants get violated.
Do not do this item if W11 alone brings the progressive window to an
acceptable frame time.

---

### W13. Make the `tree`/`heat_states` pairing structural

**Fixes:** [A2](rendering-flaws.md).

**Files:** `protolens/src/tui/override_apply.rs:1682-1684`,
`protolens/src/tui/override_select.rs:799`,
`protolens/src/tui/heat_cue.rs:375-378`.

**Change.** Three hand-maintained sites keep `heat_states` index-parallel
to `tree`, and the third is a defensive bounds check that exists
*because* the other two are insufficient. Either move
`heat: HeatState` into `TreeNode` (smaller diff), or introduce
`Arena { nodes, heat }` whose only mutators are `push`/`truncate`/
`resize` (keeps `TreeNode` free of UI-derived state). Prefer the arena
if W11 and W12 both land, since they add two more parallel arrays.

**Proving test.** Delete the defensive bounds check at
`heat_cue.rs:375-378` as part of the change; if any test needs it back,
the invariant is not yet structural.

**Blocked by:** nothing; best done after W11/W12 so all parallel arrays
move at once.

**Risk:** low-medium. Mechanical, but touches `TreeNode`'s layout and so
every construction site including test fixtures.

---

## Phase 5 — Incremental rebuilds

### W14. Cache the per-node type key instead of recomputing it per frame

**Fixes:** [S5](rendering-scaling-roadmap.md) — removes the
progressive-window frame cliff; purely additive.

See the roadmap entry for the shape. Largely subsumed by W11 if that
lands first — check for overlap before starting, and if W11 covered it,
close this item as done rather than duplicating the memo.

**Blocked by:** W11 (for the overlap check).

---

### W15. Make `rebuild_visible_rows` incremental

**Fixes:** [S3](rendering-scaling-roadmap.md). Self-contained in
`navigation.rs`. (It used to be sequenced ahead of W16 for that reason;
W16 is dropped by D-f, and this item stands on its own — `visible_rows`
is rebuilt on every fold/unfold, which W24 does not change.)

**Blocked by:** nothing.

---

### W16. ~~Make `finalize_override_batch`'s line-map rebuild incremental~~ — **SKIPPED**

**Skipped 2026-07-25 by decision D-f.** This would have fixed
[S2](rendering-scaling-roadmap.md), the whole-document
`line_to_node`/`footer_line_to_node` rebuild on every override batch.

**Do not implement this item.** [S8](rendering-scaling-roadmap.md)'s
invariant 4 makes both maps per-window, built when a window is faulted in
— which, in S8's own words, "is also what makes S2 unnecessary rather
than merely faster". W24 therefore does not accelerate this code, it
deletes it. Since D-e put Phase 8 firmly in scope, W16 is work with a
known expiry date.

It is kept here, struck through, so that a reader who finds S2 in the
roadmap can see where it went. **S2 is resolved by W24.**

W17, by contrast, is **not** obsoleted: S8 still materializes text one
window at a time via the same render-and-split path, so splitting without
the intermediate `String` remains worth having — just on a smaller input.
Do W17.

---

### W17. Split lines without the intermediate `String`

**Fixes:** [S4(2)](rendering-scaling-roadmap.md), the second half of
P4's double-materialization.

**Change.** Per the roadmap, with the line-count assertion — the
assertion is the point, since this is where an off-by-one would silently
desync the line and node coordinate systems.

**Blocked by:** W8.

---

### W18. Patch header lines before highlighting, not after

**Fixes:** [D4](rendering-flaws.md); unblocks W19.

**Files:** `protolens/src/decode.rs:792-803`,
`protolens/src/tui/override_apply.rs:1527-1532`.

**Change.** Both sites rewrite line 0's text and then re-run
`colorize::colorize` on **that line alone** — and a tree-sitter parse of
one line in isolation is not the same parse as that line in context.
Reorder: compute the patched header first, apply it, *then* highlight.
At `decode.rs:798` both the patch target and the replacement are known
before the `colorize` call at `:792`, so this is a reordering, not new
logic.

Both special cases then disappear and the "highlight one line in
isolation" primitive stops existing — which is also the only obstacle
W19 would otherwise have had to preserve.

**Proving test.** Assert the patched header line's style hints are
identical to those the whole-document pass produces for the same text.

**Blocked by:** nothing. **Blocks:** W19.

**Risk:** low.

---

## Phase 6 — Gated on measurement

Do not start these without a fresh W0 run showing the cost is still
there.

### W19. Highlight lazily, per line, on first draw

**Fixes:** [S6](rendering-scaling-roadmap.md). Doubles as the rehearsal
for W24.

**Blocked by:** W18, and a measurement showing whole-document
highlighting is still a material share of startup after W6 and W10.

---

### W20. Byte-bound `TieredBounded` — **implement spec 0165**

**Fixes:** [S10(3)](rendering-scaling-roadmap.md).

**Spec:** [../specs/0165-protolens-heat-cue-pool-sizing-cli-and-exit-stats.md](../specs/0165-protolens-heat-cue-pool-sizing-cli-and-exit-stats.md)
— **in scope per decision D-d.** Read it in full; it is the authority,
this is a pointer.

This item was drafted independently and then found to restate 0165's G2
and G3 in less detail. Implement the spec, not this paragraph. For
orientation, 0165 delivers four things:

- **G2/G3** — the byte budget itself: `TieredBounded` gains
  `max_bytes`/`total_bytes`/`size_fn` (a plain `fn` pointer, not a boxed
  closure, so no trait bound is forced onto every `V`), maintained
  incrementally, with eviction triggered by *either* cap. This is W20's
  original content. The one cache in the stack that is not byte-bounded
  is the one holding unbounded values.
- **G1** — the four CLI flags, raising the pool defaults from
  512/8192 to 100,000/200,000/100 MB.
- **G4** — `--heat-cue-stats`, an exit-time summary of high-water
  entries and bytes, applied/evicted/rejected counts, and hit/miss
  ratio.

**Do not treat G4 as optional garnish.** Every remaining sizing question
in this worklist — 0165's own defaults, W26's node budget *n*, W9's
`top_n_len` clamp — is currently a guess, and G4 is the only instrument
that turns any of them into a measurement. It is also the cheapest
observability this campaign will get: counters incremented at call sites
that already exist, printed once at exit behind an off-by-default flag.

**Sequencing note.** 0165's G5 states its defaults are "deliberately
generous, informed by spec 0164's discussion, not measured", and that a
future spec should revisit them against real high-water data. That future
spec is unblocked the moment G4 ships — so run a `googleapis.desc`
session with `--heat-cue-stats` and record the numbers alongside W0's
baseline, rather than waiting for someone to think of it later.

**Blocked by:** W9 (its `top_n_len` clamp determines what a sane byte
budget even looks like). The original "and a measurement showing the
entry cap is the binding constraint" gate is **removed**: 0165's G2 is a
decision already taken, and G4 is the thing that produces such
measurements in the first place.

**Risk:** contained rewrite of one type — but it is the type spec 0164's
tier semantics live in, so its existing tests are the acceptance
criterion. 0165's own test plan is more thorough than this item's was;
use it.

---

## Phase 7 — Smells, cleanup, and open decisions

### W21. Cache-key and batch-counter hygiene

**Fixes:** [A1](rendering-flaws.md), [A3](rendering-flaws.md).

**Change.** Two small, unrelated items batched because each is a few
lines.

- **A1**: put `initial_level` and `indent_size` into the `RenderCache`
  key (`override_apply.rs:1450`, fields at `:1470-1471`). Correct today
  only for a non-local reason with no assertion binding it. If the key
  width is genuinely unwanted, at minimum assert `indent_size` matches
  the session's at insert time.
- **A3**: replace `override_batch_depth` with `in_override_batch: bool`
  plus an entry assertion that nesting is not supported — which is the
  truth. The counter only ever holds 0 or 1, and if nesting *were* added,
  `finalize_override_batch` would take the inner call's `idx` as its walk
  origin, a wrong-origin bug the counter's presence implies is handled.

**Blocked by:** nothing.

**Risk:** low.

---

### ~~W22.~~ Bound the preview by its input bytes (spec 0174) — *replaces spec 0170*

> **Done (2026-07-25).** Spec 0174 is implemented as written. One
> deviation from the plan below: no `TruncShape::PackedElems` rule was
> shipped — `decode::register_wrapper` always uses `Label::Optional`, so
> a packed record can never appear in a preview at all, and previewing
> them is out of scope until packed runs exist as type overrides.

**Fixes:** the round-trip break in spec 0163's `NODE_BUDGET_EXCEEDED`
marker (`encode_text` has no arm for it), plus the two representation
defects that first motivated this item — it is filed under
`MalformedKind`, a taxonomy in which every other member states a
property of the *data* rather than a decision by the *renderer*; and it
emits an editorial byte count into the `#@` channel, which exists for
data.

**Spec:** [../specs/0174-preview-interior-truncation-and-node-budget-removal.md](../specs/0174-preview-interior-truncation-and-node-budget-removal.md)
— read it in full; it is the authority, this is a pointer.

**Do not implement specs 0169 or 0170.** 0169 was the full version of the
elision idea and is **suspended** — it made the elided region a navigable
node, carrying a new `NodeSpan` field, a `splice_override` refactor, an
expansion path and a heat-cue key collision. 0170 was 0169's carved-out
subset (decision D-b, 2026-07-25) and is now **superseded** by 0174: both
rework a marker 0174 deletes outright. Read 0169's *modeling problem*
section for why the suspended half was scoped out; ignore both
Specifications.

**Scope.** The budget leaves `prototext-core` entirely.
`DecodeRenderOpts::node_budget`, `NODE_BUDGET`/`NODE_COUNT`,
`MalformedKind::NodeBudgetExceeded` and `render_node_budget_exceeded` are
deleted, restoring the unconditional round-trip promise. protolens bounds
its own preview by handing the renderer fewer *interior* bytes, re-framed
so the field stays well-formed and the cut lands inside — which preserves
correctly-typed, arbitrarily nested children up to the cut, the property
naive field-level truncation loses. Two cut rules — exact for
message/group/`bytes`, UTF-8 character boundary for `string` — and no cut
for anything else; see 0174 §S3, which also records why a packed-record
rule was considered and found unreachable.

**W22's old Q1 is answered, and favorably.** The concern was that a bare
`...` line is not valid textproto and `colorize()` parses protolens's own
rendered output, so an elision could poison the highlight of following
lines. Under 0174 the `...` is appended to `new_lines`/`new_line_styles`
*after* `colorize()` has run, with empty styles and no `NodeSpan` — the
highlighter never sees it. Moving the elision out of the renderer
dissolves the question rather than answering it.

**Also dissolves decode-flaws C5** (`ProbeSink` shares `NODE_COUNT` with
the outer render, silently demoting well-formed nested messages to
bytes). No subject once the counter is gone.

**Proving test.** 0174's test plan as written. The load-bearing one is
`preview_renders_complete_nested_fields_up_to_the_cut` — it fails against
naive field-level truncation.

**Blocked by:** nothing. **No longer blocked by W5.** **Related:** W26
proposed turning the node budget on in production — that half of W26 is
void; its recursion-depth cap half shipped with spec 0171.

**Risk:** low-to-medium. The deletion is mechanical. The new work is one
helper (`truncate_interior`), one call-site change, and a constant span
shift folded into `splice_override`'s existing `byte_offset` — no
`splice_override` structural surgery, since the rewritten frame keeps the
header line, `register_wrapper` and the whole spec-0135 splice intact.

---

### W33. Truncate over-long lines at display time

**Symptom.** A single field can render to an arbitrarily long line — a
1 MB `bytes` value escapes to several MB of text on one line. Nothing in
the pipeline bounds it: the renderer emits it, `colorize()` scans it, and
the viewport draws a window onto it. The `#@` modifiers and the heat cue
sit at the *end* of the line, so reading them requires panning right past
the entire payload.

**Why it is not spec 0174's problem.** 0174 deliberately does *not*
truncate singular scalars, strings or bytes: they render to one line
regardless of size, so they were never the node-count blowup the preview
budget guards against, and cutting a string mid-UTF-8 would manufacture a
malformity marker where the data had none. The over-long *line* is a
distinct, display-layer concern — and it applies to the main document,
not just to previews.

**Shape.** Truncate at draw time, after rendering and highlighting, so
nothing upstream learns about it: the stored line, its `NodeSpan` and its
styles stay whole and extract/copy keep yielding the full value. Related
to W17/W19's line-handling work, and cheap once those land.

**Blocked by:** nothing. **Blocks:** nothing.

**Risk:** low.

---

### W34. Toggle key to summarize long values with `...`

**Symptom.** Same root cause as W33, approached as an affordance rather
than a clamp. Even with display-time truncation, a user reading a
document full of large `bytes` fields wants the *structure* — names,
`#@` modifiers, heat cues — without the payloads, and wants to flip back
to the full value for one field on demand.

**Shape.** A toggle key that collapses over-long values to a short prefix
plus `...`, so annotations and heat information stay on screen without
panning. Purely a view state, like fold state: no re-decode, no re-render
of the underlying buffer.

**Open.** Whether this is a global mode, a per-node toggle, or both; and
what the threshold is. Not designed. Recorded so it is not lost.

**Blocked by:** W33 (share the same measurement of "over-long", and the
same display-time clamp path). **Blocks:** nothing.

**Risk:** low.

---

---

## Phase 8 — The 24.5 MB campaign

**Not optional.** D-e is answered: opening `googleapis.desc` (24.5 MB) is
a required capability. Phases 1-6 buy constant factors; this phase is
what makes the target reachable at all.

**Order, as settled by D-c.** Three parts addressing three different
terms: W25 fixes the per-node constant, W24 fixes the text, W23 bounds
arena growth across a session.

1. **W25 first, and it is not optional.** It is the only one of the three
   that the others are built on — both W23 and W24 want the final
   `NodeIdx` type rather than a migration onto it. Landing W24 first
   would also leave a ~3.9 GB floor at 24.5 MB and make a correct change
   look like a failed one.
2. **W24 second.** Independent of W23.
3. **W23 last, and deferrable.** D-c: wanted eventually, no other item
   waits on it. The cost of deferring is stated in W23 — the file opens,
   but a long override session on it grows without bound.

The memory arithmetic that sets the order (see
[S12](rendering-scaling-roadmap.md)), at 24.5 MB:

| | today | after this phase |
|---|---|---|
| arena, fresh decode | ~3.9 GB (13.9 M × 280 B) | ~1.0 GB (× ~72 B) |
| arena, after two override commits | ~16.9 GB | ~4.3 GB, then reclaimed |
| whole-document `Vec<String>` | ~275 MB | resident windows only |

The text is the *smallest* of the three. That is the finding that
reordered this phase.

### W25. Shrink `TreeNode` from 280 B to 76 B

**Fixes:** [S12](rendering-scaling-roadmap.md).

**Status: all five steps done; 280 B → 76 B.** Only the `build_tree`
rider remains. See the per-step notes below, and specs 0211/0212/0213's
Measured outcomes for what each delivered.

**Files:** `prototext-core/src/serialize/render_text/sink.rs:960-1034`
(`NodeSpan`), `protolens/src/decode.rs:269-289` (`TreeNode`),
`protolens/src/decode.rs:301-315` (`build_tree`).

**Change.** Per-field, per the roadmap's table. Land it in this order,
each its own commit:

1. ✔ **Intern `type_fqdn`** into a per-document FQDN table,
   `Option<String>` → `u32` — **done 2026-07-30** by
   [spec 0212](../specs/0212-the-span-is-a-third-as-wide.md). Highest
   value: 20 B *and* one heap allocation per message node. It was the only
   part that crosses into `prototext-core`, and it introduced the
   `FqdnTable` step 3 reuses. Landed *with* step 4 rather than before it,
   because both cross the crate boundary and their call-site churn
   overlaps almost entirely. Two things the plan did not anticipate: the
   table must be **owned by the caller and passed in**, not created
   per-call, or an id would name different types in a spliced span and in
   the arena around it; and the lookup needs a **second** reserved id
   distinct from the absent-type sentinel, or a name the document never
   produced compares equal to every typeless node.
2. ✔ **`type NodeIdx = u32;`** with a named sentinel constant, replacing
   the 7 `Option<usize>` links (112 B → 28 B) — **done 2026-07-29** by
   [spec 0211](../specs/0211-the-arenas-links-are-half-as-wide.md). Caps
   the arena at ~4.29 G nodes ≈ 7.6 GB of blob at the observed 0.566
   nodes/byte — outside the stated target, and one line to revisit.
3. ✔ **Intern `rendered_as`** — **done 2026-07-30** by
   [spec 0213](../specs/0213-the-provenance-is-one-word.md). The plan was
   a side `HashMap<NodeIdx, _>` (48 B → 0 for the overwhelming majority
   of nodes, which are never spliced); `design/arena-and-batch.md`'s
   trap 1 rejected it, because a side table is a *ninth* structure keyed
   by node index, which compaction must rekey on every relocation and
   slot reuse must clear on every free. Interning also makes `TreeNode`
   plain-old-data, which is what lets a free list have a blank slot to
   push. Two things the plan did not anticipate: it does **not** use
   step 1's `FqdnTable` — `FqdnId`'s inner field is private to
   `prototext-core` and the type half of a provenance needs three values
   that are not a type name, so the *pair* is interned whole into a
   `ProvenanceTable` of protolens's own, for 4 B rather than 8; and that
   table needs only **one** reserved id, not step 1's two, because it has
   no lookup-without-insert to make a miss dangerous.
4. ✔ **Narrow the scalars** — **done 2026-07-30** by spec 0212:
   `field_number` `u64`→`u32`, both `Range<usize>` → `Range<u32>`,
   `level` → `u16`, `packed_record_start` → `u32` + `NO_PACKED_RECORD`,
   `wire_type` → `u8`. Three plan details changed on contact: `is_message`
   stayed a plain `bool` rather than becoming a flag byte (nothing needs
   the spare bits, and `size_of` is the same either way); `field_number`
   needs no saturation, since the wire format bounds it at 2²⁹ − 1; and a
   buffer cap was needed for the `u32` offsets to be sound —
   `MAX_INDEXED_BUFFER` = `u32::MAX / 8` (511 MiB), refused by
   `decode_and_render_indexed` rather than at open time, because the
   renderer already reserved `buf.len() * 8` unconditionally and so had an
   unnamed ceiling that *aborted* instead of refusing. `text_range` was
   **not** deleted: spec 0210's "no production reader" finding is about
   the arena's stale copy, not the flat list the library returns, which
   has three live readers.
5. ✔ **Delete `natural_annotation`** — **done 2026-07-26** by
   [spec 0181](../specs/0181-delete-natural-annotation.md). Revised
   2026-07-25: this step used to read "intern it or move it to a side
   table". It needed neither — a repo-wide grep found **zero production
   readers** ([decode P2](../prototext/decode-flaws.md)). Straight
   deletion: 24 B off every `NodeSpan`, plus one heap allocation per
   container node — ~330 MB at 24.5 MB before counting the allocations.

   The side-effect check this step asked for came back clean:
   `natural_annotation_from` was a pure forward scan over the
   already-written output buffer, so `IndexMark::header_start` went with
   it. The stale doc comment at
   `protolens/src/tui/tests/override_apply.rs:199` was corrected in the
   same commit.

   At the time this was the only step done, because it was the one row
   with no design question attached. Steps 1, 2 and 4 have since landed
   too; only step 3 remains.

Also hoist `build_tree`'s per-node `let mut children = Vec::new();`
(`decode.rs:324`) out of the loop and `clear()` it. Leaves are already
free — `Vec::new` does not allocate — but every message node pays today.

**Proving test.** ✔ Written, as an **equality** rather than a bound:
`const _: () = assert!(size_of::<TreeNode>() == 120)` in `decode.rs` and
`assert!(size_of::<NodeSpan>() == 32)` in `sink.rs`. Equality was chosen
deliberately — these numbers are quoted in the headroom guard, in the
design brief and in two specs' measured outcomes, so a future field that
silently fitted into padding would falsify all of them without failing an
upper bound. Update both, and the figure below, when step 3 lands.

Measured rather than projected: on `googleapis.desc` (4 501 014 nodes),
steps 1, 2 and 4 together took peak RSS 4.18 → **2.51 GiB** and at-rest
1.87 → **1.20 GiB**. Spec 0212's Measured outcome has the breakdown and
the corrected cost model.

**Watch for.** `build_tree` is
`spans.into_iter().map(..).collect()`; the element sizes differ, so the
source `Vec<NodeSpan>` stays alive alongside the fully-allocated
`Vec<TreeNode>` — a ~5.5 GB transient peak at 24.5 MB against a 3.9 GB
steady state. Narrowing both types shrinks the peak proportionally, but
if it is still material, build the arena with `reserve` + `push` and drop
the source incrementally.

**Blocked by:** nothing. **Blocks:** W23, W24.

**Risk: low-to-moderate.** No pipeline invariant changes and the compiler
finds every site; it is mostly typing. `rendered_as` was the exception,
and in the event interning kept it mechanical too.

---

### W24. Make the tree eager and the text lazy

**Fixes:** [S8](rendering-scaling-roadmap.md).

This is the only proposal that **breaks the "whole document has a
`Vec<String>` of lines" assumption**, and the only one that would
obsolete rather than tune the 0160–0167 series. The roadmap's argument:
specs 0160 N2 and 0167 N2 declined viewport-scoped rendering because
folding, navigation, search and export need whole-document state — which
is correct about the *tree* and incorrect about the *text*.

Note what this item deliberately keeps eager: the `Vec<NodeSpan>` and the
arena. That is why **W25 must precede it** — alone, it removes ~275 MB of
a ~4.2 GB problem, and shipping it first would make a correct change look
like a failed one.

**Blocked by:** W19 (the rehearsal), W25 (so the window store is built on
the final `NodeIdx` and `text_range` types rather than migrated onto
them), and a re-measurement after Phase 5. **No longer blocked by W23**
(decision D-c): reclamation and lazy text are independent — W23 bounds
arena *growth over a session*, W24 removes the *text* term. Neither needs
the other to be correct.

Re-measure before starting to size the remaining gap, not to decide
whether to proceed.

---

### W23. Arena reclamation (spec 0162) — *deferrable, per D-c*

**Fixes:** [P3](rendering-flaws.md)'s memory half,
[D5](rendering-flaws.md), [S7](rendering-scaling-roadmap.md).

Spec 0162 is a **goals-only draft with no design section** — it cannot
be implemented as written. The tree grows 622 k → 1.69 M → 2.71 M nodes
over two override commits and nothing reclaims the orphans.

Two acceptable outcomes: design and implement it, or re-label its Status
prominently so its position among implemented neighbors (0160, 0161,
0163, 0167) stops reading as "handled".

**Re-weighted 2026-07-25.** This was filed under "open decisions"
because unreclaimed orphans read as a session-scoped nuisance. With the
per-node cost measured, the post-commit arena at 24.5 MB (~60 M nodes) is
the largest number anywhere in this review — so "re-label the Status" is
no longer an acceptable outcome on its own. Start from S7's conservative
variant: truncate trailing orphans when the arena's tail is entirely
orphaned, which needs no renumbering, and which generalizes the preview
path's existing `preview_tree_watermark` special case.

**Scheduling, per decision D-c.** Arena reclamation is wanted, but may be
deferred — it is the **last** item of this phase, not the second. It
blocks nothing.

Be explicit about what deferring it costs, because it is not nothing.
After W25 and W24, a 24.5 MB descriptor set opens in ~1.0 GB of arena and
stays there **as long as no override is committed**. Each committed
override appends a fresh subtree and orphans the old one in place —
`splice_override` never renumbers and never reclaims — so the arena grows
monotonically with the session: ~4.3 GB after two commits, and it does
not come back down until the file is reloaded. Without W23 the honest
statement is "protolens opens `googleapis.desc`, and a long override
session on it will eventually exhaust memory."

That is an acceptable trade if override sessions on 24.5 MB inputs are
short. It is not acceptable as a permanent state, which is why this item
stays in the phase rather than being dropped.

**Blocked by:** W25 (so reclamation is designed against the final
`NodeIdx` type rather than migrated onto it). **Blocks:** nothing.

---

## Coverage note

This worklist covers the rendering pipeline: decode, the `TreeNode`
arena, the override splice, per-frame viewport draw, syntax
highlighting, and the background heat-cue subsystem. It does **not**
cover `extract.rs`, `mouse.rs`, `command_line.rs`, `neovim.rs` or
`manage_pane.rs` except where they touch rendering. Those were not
audited and no claim is made about them either way.
