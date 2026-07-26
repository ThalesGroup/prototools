<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0173 — prototext-graph, prototext-core: `score_all`'s quadratic verdict scan and per-line allocations

Status: implemented
Implemented in: 2026-07-26
App: prototext-graph, prototext-core
Refs: docs/scoring-flaws.md (P1, P4, P5),
      docs/prototext/decode-flaws.md (P4),
      docs/protolens/rendering-worklist.md (W31, W32)

## Background

Four hot-path costs, all of them paid per wire record or per output line
and none of them buying anything.

### P1 — the verdict table is scanned linearly, per active entry, per tag

`score_message_multi` computes one verdict per `ActiveEntry` per wire tag
and parks them in a side table keyed by `state_id`
(`walk.rs:785-787`, `:834-848`):

```rust
let mut verdicts: Vec<(u32, Verdict)> = Vec::new();
...
verdicts.clear();
for ae in active.iter() {
    let v = ...;
    verdicts.push((ae.state_id, v));
}
```

and then reads them back by searching (`walk.rs:851-855`, and again
through the `verdict_for` closure at `:876-882`, which every wire-type
arm calls once per surviving entry):

```rust
let v = verdicts.iter().find(|(sid, _)| *sid == ae.state_id).map(|(_, v)| v);
```

The comment above the table explains the design — *"Keyed by state_id so
it remains valid after `active.retain()`"* — and that concern is real:
the mismatch loop clears entries and `retain` compacts the vector, so
positional indices do not survive. But the cure is a linear scan of a
vector whose length is the number of active entries `A`, executed once
per active entry, i.e. **O(A²) per wire tag**. On a 24.5 MB descriptor
set with ~13 000 root candidates all still alive near the top of the
walk, this is the 533 ms measured in `docs/scoring-flaws.md`.

The verdict is per-`ActiveEntry` data. Keeping it anywhere other than on
the `ActiveEntry` is what creates the lookup in the first place.

### P5 — the veto reason is formatted whether or not anyone reads it

`set_vetoed` takes `reason: &str` and uses it only inside

```rust
if let Some(ref dbg) = self.debug_fqdn {
    if self.scores[ei].fqdn == *dbg { eprintln!("[veto] {} — {}", ..); }
}
```

`debug_fqdn` is `std::env::var("PROTOTEXT_DEBUG_FQDN").ok()` — `None` in
every production run. Yet callers build the string eagerly:

```rust
ws.set_vetoed(e, &format!("wire-type mismatch on field {field_number} (wire_type={wire_type})"));
```

inside a loop over `ae.entries`, so the same string is formatted and
dropped once per candidate per mismatching tag. Six call sites use
`format!`; the rest pass literals and cost nothing.

### P4 (scoring) — one `String` per root per call

```rust
pub struct EntryScore { pub fqdn: String, .. }
...
.map(|r| EntryScore { fqdn: r.fqdn.as_str().to_owned(), .. })
```

`score_all` allocates and copies one FQDN per root **before scoring
starts**, so a 13 000-root graph pays 13 000 allocations even when the
blob is 40 bytes and every candidate is vetoed on the first tag. The
source is `r.fqdn`, an `ArchivedString` inside a graph that outlives the
call — protolens holds it as `&'static ArchivedCompiledGraph` — so the
copy is pure overhead.

This is not a once-per-process cost. `score_all` is the heat-cue
worker's per-range sweep (`protolens/src/override_pane.rs:56-73`,
`inferred_candidates`), run again for every window the user scrolls
into. `inferred_candidates` then compounds it with a redundant
`r.fqdn.clone()` inside an `into_iter()` that already owns the `String`.

### P4 (decode) — `display_name()` allocates per output line

```rust
pub(super) fn display_name(&self) -> String {
    match self {
        FieldOrExt::Field(f) => f.name().to_owned(),
        FieldOrExt::Ext(e) => format!("[{}]", e.full_name()),
    }
}
```

Its only two callers are `wfl_prefix_n` and `wob_prefix_n`
(`output.rs:89`, `:109`), which immediately do
`out.extend_from_slice(fi.display_name().as_bytes())` and drop the
`String`. Both functions carry a doc comment reading *"Write field-line
prefix without String allocation"* / *"Write open-brace prefix without
String allocation"*, which is false. Every schema-named field line in
the document pays an allocate-copy-free cycle: 193 072 of them on the
1.1 MB fixture.

## Goals

- **G1**: Verdict lookup is O(1), removing the O(A²)-per-tag scan.
- **G2**: Veto reasons are formatted only when a reader exists.
- **G3**: `score_all` allocates no FQDN copy per root.
- **G4**: `wfl_prefix_n`/`wob_prefix_n` allocate nothing, making their
  existing doc comments true.

## Non-goals

- **N1**: Any change to scoring *semantics*. Every one of these is
  observationally equivalent by construction; the test plan's central
  assertion is that scores are unchanged.
- **N2**: Widening `ActiveEntry::entries` past `u16` (decision D-h), or
  any other change to how candidates are addressed.
- **N3**: `EntryScore.fqdn` becoming anything other than a borrow — no
  interning, no `Cow`, no index-into-graph.
- **N4**: Hoisting `child_pairs`' per-LEN-record `Vec::new()`
  (`walk.rs:1038`, flaws P3). It is a genuine allocation per LEN record
  but hoisting it into `WalkState` means threading a scratch buffer
  through a recursive function that already borrows `ws` mutably; it
  wants its own change with its own measurement.
- **N5**: Removing `NodeSpan::natural_annotation` (decode flaws P2). It
  is dead weight and should go, but it is a `NodeSpan` field that
  protolens's arena carries, so it belongs with the arena-sizing work
  (worklist W25), not here.

## Specification

### S1. The verdict lives on the `ActiveEntry` (P1)

`enum Verdict` moves from inside `score_message_multi` to module scope
(it is `Copy` and three variants wide), and `ActiveEntry` gains a field:

```rust
struct ActiveEntry {
    state_id: u32,
    entries: SmallVec<[u16; 4]>,
    occurrences: Vec<(u32, u64)>,
    /// This entry's verdict for the wire tag currently being processed.
    /// Overwritten at the top of every tag iteration and never read
    /// across iterations.
    ///
    /// Held here rather than in a side table keyed by `state_id`
    /// because `active.retain()` compacts the vector mid-iteration:
    /// positional indices into a parallel array do not survive, and the
    /// state_id-keyed lookup that used to bridge that gap was a linear
    /// scan run once per entry per tag — O(A²) per tag, and 533 ms of a
    /// 24.5 MB sweep. A field moves with its owner through `retain` for
    /// free.
    verdict: Verdict,
}
```

`group_by_state` and `score_one` initialize it to `Verdict::Unknown`.
The `verdicts: Vec<(u32, Verdict)>` buffer and the `verdict_for` closure
are deleted; every `verdict_for(ae.state_id)` becomes `ae.verdict`, and
the two places that need the verdict while iterating `&active`
immutably read `ae.verdict` directly.

The one subtlety this preserves: `propagate_vetoes` (`walk.rs:579-584`)
and every `active.retain(..)` only ever *remove* entries, never reorder
or rebuild them, so an entry's verdict is still its own after any of
them. The old design's stated hazard is therefore genuinely eliminated
rather than worked around.

### S2. Veto reasons are lazy (P2/P5)

`set_vetoed` takes a closure instead of a formatted string:

```rust
/// `reason` is a closure because it is read only when
/// `PROTOTEXT_DEBUG_FQDN` names this exact candidate — i.e. never, in
/// production. It used to be a `&str`, which meant six `format!` call
/// sites building and dropping a string once per candidate per
/// mismatching tag.
fn set_vetoed(&mut self, e: u16, reason: impl FnOnce() -> String)
```

with the early-return-if-already-vetoed check before the closure is
called. The six `format!` sites become `|| format!(..)`; the literal
sites become `|| "…".to_string()`.

`veto_all` (which takes one reason for a whole vector) takes the same
closure shape and calls it at most once, inside the same `debug_fqdn`
guard.

### S3. `EntryScore` borrows its FQDN (P4, scoring)

```rust
pub struct EntryScore<'g> {
    /// Borrowed from the graph's `ArchivedString`, which outlives every
    /// call: copying it allocated once per root *before scoring began*,
    /// so a 13 000-root graph paid 13 000 allocations to score a 40-byte
    /// blob that vetoes on the first tag.
    pub fqdn: &'g str,
    ..
}
```

`score_all` and `score_one` become generic over `'g`, tied to the
`&'g ArchivedCompiledGraph` parameter. `WalkState<'a>` already borrows
the graph for `'a`; `scores: &'a mut Vec<EntryScore<'g>>` adds the second
lifetime.

Callers:
- `protolens/src/decode.rs:193` — `resolve_root_winner_fqdn` returns
  `Option<String>`; the single surviving winner is `.to_owned()` at the
  end, where it already is.
- `protolens/src/override_pane.rs:61` — `inferred_candidates` becomes
  `.map(|r| (r.fqdn.to_owned(), r.score()))`, one allocation per
  *surviving* candidate instead of one per root plus a redundant
  `.clone()` on an already-owned `String`.
- `prototext-graph`'s own tests and `bin/hopcroft_dump.rs` — mechanical.

### S4. `write_display_name` (P4, decode)

`FieldOrExt::display_name(&self) -> String` is replaced outright — it has
no other callers — by

```rust
/// Append the name to use in field-line output directly to `out`.
///
/// Regular field: `name`. Extension field: `[full.qualified.name]`.
///
/// Writes rather than returns, because both callers
/// (`wfl_prefix_n`/`wob_prefix_n`) only ever append it to a buffer, and
/// the `String` this used to return was allocated, copied and dropped
/// once per schema-named line — 193 072 times on the 1.1 MB fixture,
/// under two doc comments that claimed the opposite.
pub(super) fn write_display_name(&self, out: &mut Vec<u8>) {
    match self {
        FieldOrExt::Field(f) => out.extend_from_slice(f.name().as_bytes()),
        FieldOrExt::Ext(e) => {
            out.push(b'[');
            out.extend_from_slice(e.full_name().as_bytes());
            out.push(b']');
        }
    }
}
```

`wfl_prefix_n` and `wob_prefix_n` call it in place of their
`extend_from_slice(fi.display_name().as_bytes())`. Their doc comments,
which already promise this, are left as they stand.

## Test plan

Correctness first: none of this may change a single byte of output or a
single counter.

- **S1/S2/S3 equivalence** — the existing `score::tests` corpus (54
  cases) and the `hopcroft_suite` fixtures already assert every
  `EntryScore` field (`matches`, `unknowns`, `mismatches`,
  `non_canonical`, `vetoed`) against committed expected values, over
  exactly the schemas and wire shapes this change touches. A second test
  re-asserting the same facts over the same fixtures would restate them,
  not check anything more, so the corpus is used as-is: all 54 must stay
  green unchanged.
- **S1 verdict-after-retain** — the hazard the old design existed to
  avoid, pinned directly:
  `score::tests::survivors_keep_their_own_verdict_after_a_mismatch_retain`.
  Three roots, one tag: one root mismatches and is removed by the
  mismatch loop's `retain`, and the two that outlive it must still be
  handled with their own verdicts (one `Found`, one `Unknown` — the two
  arms a positional swap would visibly exchange). The removal only shifts
  anything if the mismatching root sorts first, and that order is not the
  test's to choose: `graph::build` assigns node IDs by `HashMap`
  iteration order, so `state_id`s differ from process to process. (An
  earlier draft asserted an ordering instead; it passed locally and
  failed under `nix-build`.) The three roots are therefore arranged in a
  cycle — for a LEN tag on field `f`, root `R{f}` mismatches, `R{f-1}` is
  `Found` and `R{f+1}` is `Unknown` — and the test reads back which root
  sorts first and aims the blob at it. Every permutation then exercises
  the hazard and none goes vacuous.
- **S2 lazy reason** — `walk::set_vetoed_tests`. `WalkState::new` reads
  `PROTOTEXT_DEBUG_FQDN` from the process environment, which the test
  binary's threads share, so rather than a child process the two tests
  construct `WalkState` directly (the module is a child of `walk`, so
  its private fields are in scope) and assert on whether the closure
  runs: not built with no `debug_fqdn`, built exactly once for the named
  candidate, and not built again on a repeat veto — i.e. the early
  return still precedes it.
- **S4 byte-for-byte** —
  `render_text::tests::descriptor_fixture_renders_byte_for_byte`:
  `decode_and_render` over the committed
  `prototext-core/fixtures/descriptor.pb` produces output identical to
  `fixtures/descriptor_protoc.txt` (2511 schema-named lines). That
  payload is a `FileDescriptorSet` and so reaches only the
  regular-field arm; the extension arm's brackets are already pinned by
  `prototext/tests/roundtrip.rs`'s `[acme.blade_count]` assertion, which
  the test references rather than duplicating.

## Measurements

A `score_all` bench did not exist; `prototext-graph/benches/score.rs`
adds one (`cargo bench -p prototext-graph --bench score`). It
parameterizes on **A**, the number of distinct states alive in the
active set — which is what the walk's cost actually scales with, since
Hopcroft collapses structurally identical roots onto one state. Every
synthetic root shares fields 1 and 2 (so the blob matches all of them
and nothing is vetoed, holding the active set at full width) and adds
one field number unique to itself (so Hopcroft cannot merge them).

Run on a 4-core VM; run-to-run variance on unchanged code was 1–4%,
against the 30–45% recorded in `docs/bench-process.md` for the previous
single-core sandbox. Two runs each; both are given.

### Wall clock — `score_all`

| bench | before | after | change |
|---|---|---|---|
| `by_root_count/64` | 744.5 / 730.5 µs | 461.8 / 451.2 µs | −39% |
| `by_root_count/256` | 6.385 / 6.633 ms | 1.971 / 2.048 ms | −69% |
| `by_root_count/1024` | 78.95 / 78.87 ms | 14.65 / 13.06 ms | −82% |
| `by_root_count/4096` | 1.114 / 1.117 s | 68.87 / 68.81 ms | −94% |
| `setup/1024` (1 record) | 1.417 / 1.401 ms | 321.8 / 323.1 µs | −77% |
| `setup/4096` (1 record) | 18.43 / 18.18 ms | 1.457 / 1.576 ms | −92% |
| `score_one` (A = 1) | 24.40 / 24.16 µs | 22.70 / 22.69 µs | −6% |

The shape is the point. Each row quadruples A, so a linear walk costs
4× more and a quadratic one 16×:

| A step | before | after |
|---|---|---|
| 64 → 256 | 8.75× | 4.54× |
| 256 → 1024 | 11.9× | 6.38× |
| 1024 → 4096 | 14.2× | 5.27× |

Before, the ratio climbs toward 16 as A grows — the O(A²) scan
dominating everything else. After, it sits near 4–6, i.e. linear plus
cache effects. `score_one` holds A at 1 and so isolates what is left
when the scan cannot bite: −6%, which is S2 and S3 alone.

### Wall clock — `decode_and_render`

`cargo bench -p prototext-core --bench codec`:

| bench | before | after | change |
|---|---|---|---|
| A1 (no schema — control) | 266.5 / 269.3 / 266.4 µs | 264.3 / 263.3 µs | −1% (noise) |
| A2 (schema + annotations) | 428.8 / 424.7 / 416.8 µs | 379.2 / 396.6 µs | −8% |

A1 renders no schema names, so it never reaches `display_name` and
should not move — it does not, which is what makes A2's −8%
attributable to S4 rather than to the machine.

### Structural — allocation counts

Measured with a counting `GlobalAlloc` wrapper, one call each, against
this branch and against `HEAD` in a throwaway worktree. The probe was
temporary and is not committed.

| call | before | after | delta |
|---|---|---|---|
| A1 `decode_and_render` (no schema) | 1 654 | 1 654 | 0 |
| A2 `decode_and_render` (schema + annot) | 5 485 | 2 974 | **−2 511** |
| `score_all` / 256 roots, 1 record | 552 | 288 | −264 |
| `score_all` / 1024 roots, 1 record | 2 098 | 1 064 | −1 034 |
| `score_all` / 4096 roots, 1 record | 8 252 | 4 144 | −4 108 |
| `score_all` / 256 roots, 64 records | 2 001 | 1 674 | −327 |
| `score_all` / 1024 roots, 64 records | 3 925 | 2 828 | −1 097 |
| `score_all` / 4096 roots, 64 records | 10 457 | 6 286 | −4 171 |

−2 511 on A2 is *exactly* the number of schema-named lines in
`descriptor_protoc.txt`: one `String` per line, as S4 predicted, and
none left over. A1's zero delta confirms nothing else moved.

On `score_all` the delta is a little above the root count (−1 034 for
1 024 roots): the roots themselves are S3, and the remainder is the
`verdicts: Vec` growth S1 deleted — one vector per message frame,
doubling its way up to A, which is why the 64-record rows shed more
than the 1-record rows at the same root count.
