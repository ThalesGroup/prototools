<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0173 — prototext-graph, prototext-core: `score_all`'s quadratic verdict scan and per-line allocations

Status: draft
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

- **S1/S2/S3 equivalence** — `score_all_output_is_unchanged`: over the
  existing `score::tests` corpus plus the `hopcroft_suite` fixtures,
  assert every `EntryScore` field (`matches`, `unknowns`, `mismatches`,
  `non_canonical`, `vetoed`) and the resulting rank order match the
  values recorded before the change. Committed as explicit expected
  values, so the assertion survives a future refactor of the same code.
- **S1 verdict-after-retain** — the hazard the old design existed to
  avoid, pinned directly: a fixture in which one `ActiveEntry` is vetoed
  and removed by the mismatch loop while others survive, asserting the
  survivors' subsequent per-wire-type handling still sees their own
  verdicts. This is the test that fails if a future change reintroduces
  positional indexing.
- **S2 debug output** — with `PROTOTEXT_DEBUG_FQDN` set to a candidate
  that gets vetoed, the `[veto]` line still appears on stderr with the
  same text. Run as a child process so the env var does not leak across
  the test binary's threads.
- **S4 byte-for-byte** — `decode_and_render` over the committed
  `prototext-core/fixtures/descriptor.pb` produces output identical to
  `fixtures/descriptor_protoc.txt`, covering both regular and extension
  field names. Extensions specifically: a fixture containing an
  extension field still renders `[pkg.ext_name]:` with the brackets.

Performance, reported but not asserted (this sandbox's Criterion runs
vary 30–45% on unchanged code — see `docs/bench-process.md`):

- `cargo bench -p prototext-core --bench codec` before and after S4.
- A timed `score_all` over `googleapis.desc` before and after S1, which
  is the change with a predicted effect large enough to clear the noise
  floor.
