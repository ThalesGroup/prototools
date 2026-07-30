<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0212 — the span is a third as wide

Status: implemented
Implemented in: 2026-07-30
App: protolens (with a breaking change to `prototext-core`)
Refs: docs/protolens/design/arena-and-batch.md (the redesign brief; its
        annex rows 2–9 are what this spec executes, and its traps 2 and 3
        are what this spec resolves),
      docs/protolens/rendering-scaling-roadmap.md S12 and
        docs/protolens/rendering-worklist.md W25 (the earlier plan; both
        already name FQDN interning as the one row that crosses the crate
        boundary),
      docs/protolens/design/README.md (the three-item `prototext_core`
        interface contract, which this spec amends),
      docs/specs/0181-delete-natural-annotation.md (the first row of the
        plan to land),
      docs/specs/0202-an-override-is-refused-rather-than-fatal.md (the
        headroom guard, whose `STRING_ALLOWANCE` this spec re-derives),
      docs/specs/0203-the-override-arena-is-compacted.md (`verify_arena`,
        the safety net),
      docs/specs/0210-a-node-counts-its-own-lines.md (the line counters,
        and the audit of who reads `text_range`),
      docs/specs/0211-the-arenas-links-are-half-as-wide.md (the
        predecessor; row 1, and the measurement method this spec reuses)

## Background

Spec 0211 took `TreeNode` from 272 B to 184 B by narrowing the seven
inter-node links from `Option<usize>` to a 4-byte `NodeIdx`. It moved
the arena at rest by 396 MB and the peak by 908 MB — a measured
**2.29×** multiplier on the slot, not the 3× the brief's peak model
predicted.

The reason the multiplier fell short is the point of departure for this
spec. The peak has three roughly equal terms: the arena's superseded
half, the throwaway `local_tree`, and the render cache's clone. Spec
0211 moved the first two, because both are arenas of `TreeNode`. It
moved the render cache by **zero**, because the render cache stores
`Vec<NodeSpan>` and 0211 did not touch `NodeSpan`.

`NodeSpan` is 96 B of the surviving 184, and it is the only part of the
slot that also exists outside the arena. Narrowing it is therefore the
row that finally prices in all three terms.

### What the 96 bytes are

```rust
pub struct NodeSpan {
    pub field_number: u64,                     //  8
    pub raw_range: Range<usize>,               // 16
    pub text_range: Range<usize>,              // 16
    pub level: usize,                          //  8
    pub type_fqdn: Option<String>,             // 24  + a heap allocation
    pub packed_record_start: Option<usize>,    // 16
    pub wire_type: u32,                        //  4
    pub is_message: bool,                      //  1
}                                              // = 93, padded to 96
```

Every one of those fields is over-wide for what it holds. A field
number is a `u32` by protobuf's own definition. A level is bounded by
`MAX_WIRE_DEPTH`. `packed_record_start` pays 8 bytes of discriminant to
distinguish "absent" from an offset that is never `u32::MAX`. And
`type_fqdn` pays 24 bytes inline *plus* a separate heap allocation per
node, for a string drawn from a set of at most a few tens of thousands
of distinct values — 58 777 for `googleapis.desc`, against 4 501 014
nodes.

### Why this crosses the crate boundary

`NodeSpan` is `prototext-core`'s public type. The brief's annex offers a
dodge: leave the library's `NodeSpan` alone and have protolens convert
each one into a private packed node on the way into the arena. This spec
declines the dodge and narrows the library's type directly, for three
reasons.

First, protolens is the only consumer. No other crate in the workspace
mentions `NodeSpan`; the sole out-of-crate reference is
`prototext/tests/node_span.rs`, an integration test parked there because
protolens is a bin-only crate and cannot host one.

Second, a conversion layer does not narrow the render cache. The cache
stores what the library returns. Keeping the library's span at 96 B
leaves 432 MB of the peak in place, which is most of what this spec is
for.

Third, `type_fqdn` cannot be interned on protolens's side of a
conversion without protolens re-hashing every string the library just
allocated — one hash and one allocation per node, on the pipeline's
slowest path, to undo work the library did not need to do.

The cost is that `prototext-core` is packaged for crates.io
(`version = "0.3.0"`, with a `repository` key and no `publish = false`),
so this is a semver-breaking change for any external user, even though
no in-repo crate depends on it. That is accepted: the crate is young
enough that the narrower API is worth taking now rather than carrying
the wide one indefinitely.

### The 4 GiB question

Narrowing `raw_range` to `Range<u32>` is only sound if no offset into
the decoded buffer can exceed `u32::MAX`. Otherwise the offsets wrap
silently and `extract.rs` and `heat_cue.rs` reslice unrelated bytes
while reporting success — the worst failure mode available, a wrong
answer with no error.

The decisive fact is that a much lower ceiling already exists.
`render_text/mod.rs:397` does:

```rust
let capacity = buf.len() * 8;
```

— one unconditional allocation of eight times the input. A 4 GiB input
already asks for a 32 GiB reservation and aborts. So this spec is not
imposing a new constraint; it is **naming an existing one** and turning
an abort into a refusal.

## Goals

G1. Take `NodeSpan` from 96 B to 32 B, and `TreeNode` from 184 B to
    120 B, by narrowing every scalar field and replacing `type_fqdn`'s
    `Option<String>` with a 4-byte id into a shared table.

G2. Remove one heap allocation per message node — the `type_fqdn`
    `String` — replacing 58 777 × N allocations with 58 777.

G3. Move all three terms of the peak, including the render cache's
    clone, which spec 0211 could not touch.

G4. Give `decode_and_render_indexed` an explicit buffer-size cap with a
    refusal, replacing today's unnamed abort at 1/8th of the same
    threshold.

G5. Keep the change mechanical at the call sites: no consumer should
    have to reason differently about a span than it does today.

## Non-goals

N1. `rendered_as`. It is 48 B of the surviving `TreeNode` and the
    annex's row 10 — a separate spec, which will have to re-derive
    `STRING_ALLOWANCE` a second time.

N2. The hot/cold column split (the annex's row 11). It changes how
    nodes are stored, not how wide they are, and it is the only row of
    the plan that is not pure arithmetic.

N3. **Keying the heat cache by `FqdnId`.** This looks adjacent and is
    not. Two independent obstacles:

    - `current_type_key` has three sources and two of them are not
      FQDNs. `resolve_active_override` returns the override
      collection's own `String`, and `natural_type` returns either a
      pool FQDN *or* one of about fifteen primitive keyword literals
      (`"int32"`, `"bool"`, `"sfixed64"`, …). An id-keyed heat cache
      therefore needs a general-purpose string interner, not a table of
      type names.
    - `heat_worker.rs:302`'s
      `current_score: TieredBounded<(usize, String), Option<i64>>` sits
      behind a `Mutex` shared with the background worker, and the
      scoring boundary on the far side (`override_pane::inferred_score`,
      `heat_cue::score_of`) takes `&str` because `prototext_graph`'s
      candidate lists are strings. Changing the key changes the
      worker's interface too.

    The genuinely cheap win in that area is a different one: four
    `key.to_string()` calls made purely to build a lookup key
    (`heat_cue.rs:327`, `heat_worker.rs:400`, `:458`, `:478`), removable
    by giving `TieredBounded` a borrowed probe key. That is pure
    protolens, needs no interning, and belongs in its own small spec.

N4. Deleting `text_range`. See S7 — it has live readers.

N5. Making `FqdnTable` thread-safe or shareable across documents. One
    table, one owner, one thread.

N6. Spec 0210's S7–S9 (moving rendered text into the nodes). Still
    deferred until the slot work concludes.

N7. Narrowing protolens's other index holders (cursor, selection,
    `heat_states` keys, and so on). Spec 0211 left those as `usize`
    deliberately; nothing here changes that.

## Implementation steps

1. **The cap and the refusal.** Add the buffer-size cap to
   `prototext-core` and make `decode_and_render_indexed` return a
   `Result`. Two production callers to update, plus protolens's
   open-time refusal.
2. **The table.** Add `FqdnId`, `NO_FQDN`, and `FqdnTable` to
   `prototext-core`, and thread a caller-supplied `&mut FqdnTable`
   through the indexing sink.
3. **The narrowing.** Change `NodeSpan`'s eight fields, and the sink's
   construction of them. Add the size assertion.
4. **The library's own tests.** `prototext/tests/node_span.rs` and the
   in-crate render tests: mechanical `as` casts, plus the interned-needle
   idiom for `type_fqdn` comparisons.
5. **protolens.** `Decoded` and `App` gain the table; `TreeNode`'s
   arithmetic changes; the ~200 field sites take casts. Add the second
   size assertion.
6. **The guard.** Re-derive `STRING_ALLOWANCE` and rewrite its comment.
7. **Measure**, using spec 0211's pty driver, and fill in the Measured
   outcome section.
8. **Docs.** Mark the annex's rows 2–9 done, correct the running total,
   and record in traps 2 and 3 that the crate-boundary route was taken.

## Specification

### S1 — the buffer cap and the refusal

`prototext-core` gains a public constant naming the largest buffer
`decode_and_render_indexed` will accept. It is set so that the existing
8× capacity reservation cannot exceed `u32::MAX`, and so that every
offset a `NodeSpan` can hold fits a `u32`.

`decode_and_render_indexed` returns a `Result`. Over the cap it refuses,
naming the actual and permitted sizes. Under it, behavior is unchanged.

protolens additionally refuses at open time, before decoding, so that
the user sees a size complaint rather than a decode complaint.

Rationale: this is not a new limit. `render_text/mod.rs:397`'s
`buf.len() * 8` already aborts the process on a buffer an eighth of this
size. The change converts an unnamed abort into a named refusal and
makes the `u32` offsets in S2 sound rather than merely probable.

### S2 — the narrowed fields

```rust
pub struct NodeSpan {
    pub field_number: u32,                //  4  (was u64,           -4)
    pub raw_range: Range<u32>,            //  8  (was Range<usize>,  -8)
    pub text_range: Range<u32>,           //  8  (was Range<usize>,  -8)
    pub type_fqdn: FqdnId,                //  4  (was Option<String>, -20
                                          //      and one heap alloc)
    pub packed_record_start: u32,         //  4  (was Option<usize>, -12)
    pub level: u16,                       //  2  (was usize,         -6)
    pub wire_type: u8,                    //  1  (was u32,           -3)
    pub is_message: bool,                 //  1  (unchanged)
}                                         // = 32, align 4, no padding
```

32 B exactly, with the sum landing on the alignment boundary rather than
relying on tail padding. Declaration order in the source need not match
this; Rust reorders. Four of the eight deserve a note.

`level: u16` — `MAX_WIRE_DEPTH` is 1000, so `u8` is too small and `u16`
has 65× headroom. Consumers that index or compare with a `usize` cast at
the use site.

`wire_type: u8` — a wire type is the low three bits of a tag, so the
value is 0–7 and `WT_END_GROUP` is 4. The `WT_*` constants in
`helpers/wire.rs` **stay `u32`**: they are used 457 times across the
workspace, mostly in tag arithmetic where `u32` is the natural type, and
retyping them to save this field's 3 bytes a second time would be a far
larger change than the field itself. Instead the twelve sites that
compare a *span's* `wire_type` to a constant cast at the comparison.
Those twelve are enumerated in `override_select.rs:36`, `:689`,
`override_apply.rs:608`, `:657`, `:796`, `:844`, `:1636`, `:2728`,
`:2759`, `command_line.rs:236`, `:528`, and `tests/profiling.rs:120`.

`is_message: bool` stays a `bool`. Folding it and `wire_type` into a
single flag byte is what the annex proposes and is declined: it saves
0 B, because the two already occupy adjacent bytes inside the 32, and it
would churn about sixty call sites for nothing.

`packed_record_start` loses its `Option` in favor of a `NO_PACKED_RECORD`
sentinel, the same trade spec 0211 made for the links: the value it
guards is a buffer offset, and the cap in S1 makes `u32::MAX`
unreachable as a real offset.

Every documented meaning is preserved verbatim: `field_number` is still
`0` for virtual wrapper nodes; `raw_range` is still absolute with
respect to the top-level buffer; `wire_type` still reports the
*claimed* type for a malformed node and is still the one place
`WT_END_GROUP` appears; `is_message` is still the structural
discriminator consumers should prefer over testing `type_fqdn`.

That last point changes shape slightly and must be re-stated in the doc
comment: today the advice is "do not use `type_fqdn.is_some()`", and
after this spec the equivalent trap is "do not use
`type_fqdn != NO_FQDN`".

### S3 — the table

```rust
/// An index into a `FqdnTable`. `NO_FQDN` means the node carries no
/// type name — a scalar, or a message whose type could not be resolved.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FqdnId(u32);

pub const NO_FQDN: FqdnId = FqdnId(u32::MAX);

/// The set of type names a render referred to, each stored once.
pub struct FqdnTable { /* … */ }

impl FqdnTable {
    pub fn new() -> Self;

    /// The name behind an id, or `None` for `NO_FQDN`.
    pub fn get(&self, id: FqdnId) -> Option<&str>;

    /// The id of a name already in the table, or `NO_FQDN`. Does not
    /// insert. This is the lookup a comparison should use.
    pub fn id_of(&self, name: &str) -> FqdnId;

    /// The id of a name, inserting it if absent.
    pub fn intern(&mut self, name: &str) -> FqdnId;

    pub fn len(&self) -> usize;
}
```

`intern` is public even though protolens never writes `type_fqdn`
itself. Callers that need to name a type that no render produced — and a
future id-keyed heat cache would be exactly that — must be able to.

### S4 — the table's lifecycle and ownership

The table is **supplied by the caller**, not returned freshly per call.
This is the single most consequential decision in the spec and the
alternative is worse in three ways: a per-call table makes
`type_fqdn == FqdnId(3)` mean different types in a spliced span than in
the arena, which would require a translation pass on every splice (on
protolens's slowest path), a per-entry table inside the render cache
(whose clone is one of the three peak terms this spec is trying to
shrink), and would make `u32` comparison across the two silently wrong.

Shared, the table costs about 4 MB in the worst case measured
(58 777 googleapis types at roughly 66 B each) — against the hundreds of
megabytes it removes.

Lifecycle:

- The table is created inside protolens's `decode()` free function,
  passed to `decode_and_render_indexed`, and moved into `Decoded`.
- `App::new` moves it out of `Decoded` into an `App::fqdns` field,
  matching how `App` already flattens `blob`, `lines` and `tree`.
- In production `decode()` runs once per process (`main.rs:358`), so
  there is one table for the life of the run and every `FqdnId` in every
  arena, splice, and cache entry is comparable with every other.

Access rule: reach the table as **`&mut self.fqdns` directly, never
through a `&mut self` helper method**. Direct field access lets the
borrow checker split disjoint fields; a method call borrows all of
`App` and conflicts with the `&self.tree` / `&mut self.render_cache`
borrows live at the same points.

If a re-decode is ever added, it must either keep the existing table or
clear the `render_cache` in the same breath, because cache entries hold
ids from the table that produced them.

### S5 — the return type

`decode_and_render_indexed` returns a named struct rather than a tuple:

```rust
pub struct IndexedRender {
    pub text: Vec<u8>,
    pub spans: Vec<NodeSpan>,
}

pub fn decode_and_render_indexed(
    buf: &[u8],
    root_desc: Option<&MessageDescriptor>,
    fqdns: &mut FqdnTable,
    opts: DecodeRenderOpts,
) -> Result<IndexedRender, RenderError>;
```

The table is borrowed in rather than returned, so it does not appear in
`IndexedRender`. A struct rather than a tuple means the next field added
to the output is not itself a breaking change — which, given this is the
second consecutive spec to widen this signature, is worth the small
verbosity now.

### S6 — the comparison idiom

The overwhelmingly common thing done with `type_fqdn` today is compare
it to a literal, usually inside an iterator chain:

```rust
app.tree.iter().position(|n| n.span.type_fqdn.as_deref() == Some("test.Inner"))
```

Resolving each span's id back to a string inside the closure would need
`&app.fqdns` while `app.tree` is borrowed and would force these chains
into index loops. Instead, **intern the needle once and compare ids**:

```rust
let want = app.fqdns.id_of("test.Inner");
app.tree.iter().position(|n| n.span.type_fqdn == want)
```

Two lines, the iterator shape preserved, and one string hash instead of
N.

For that substitution to be exact, `id_of`'s miss needs a **reserved id
of its own**, `UNINTERNED`, distinct from `NO_FQDN`. The string form
asked `span.type_fqdn.as_deref() == Some(name)`, which is `false` for a
span with no type. Were a name the table has never seen to answer
`NO_FQDN`, it would instead compare *equal* to every typeless span — so
on a document containing no `google.protobuf.Any`, `is_any_node` would
answer yes for every scalar in the tree. `UNINTERNED` is a value no span
can hold, which gets the right answer at every call site with no special
casing there.

This is the idiom for production code and tests alike. Reaching for
`get()` inside a closure over the tree is the sign of having taken the
wrong route.

### S7 — `text_range` stays

`text_range` is narrowed to `Range<u32>` and **not** deleted.

Spec 0210's finding that `text_range` has no production reader applies
to the **arena's stored copy** only, and only after a splice has made it
stale. On the flat list the library returns, there are three live
readers:

- `decode.rs:667` derives each node's line count from it at build time.
  This is the value spec 0210 made authoritative; deleting the field
  would delete its source.
- `override_apply.rs:302–312` (`insert_truncation_marker`) uses it as
  the line-ownership key when shifting spans past an inserted marker
  line.
- `override_select.rs:822–825` reads it as a **return channel**:
  `override_apply.rs:2679` deliberately overwrites the field with
  `self.node_lines(idx)` before handing the span back.

The arena's copy remains what spec 0210 declared it: written at build
time, not to be read afterwards. That doc comment stays.

### S8 — the size assertions

Both sizes are pinned by compile-time assertions, as **equalities**, not
upper bounds:

- `size_of::<NodeSpan>() == 32` in `prototext-core`.
- `size_of::<TreeNode>() == 120` in protolens.

An equality is deliberate. These numbers are quoted in the headroom
guard, in the design brief, and in this spec's Measured outcome; a
future field that silently fits in padding would falsify all three
without failing an upper bound.

The 120 is: `span`(32) + seven `NodeIdx`(28) + `sibling_ordinal`(4) +
`lines_total`(4) + `lines_visible`(4) + `rendered_as`(48) = 120, no
padding.

### S9 — `STRING_ALLOWANCE`, re-derived

`override_apply.rs`'s headroom guard budgets

```rust
let per_node = (size_of::<TreeNode>() + STRING_ALLOWANCE) as u64;
```

with `STRING_ALLOWANCE = 64`, covering two things its comment names:
the two `String` heap allocations at a measured ~41 B/node, and the
`heat_states` / `descend` / `dead` parallel arrays at a further
~42 B/node — ~83 B, which 64 already under-covers.

This spec removes the `type_fqdn` half of the 41 and leaves the
`rendered_as` half and all 42 of the parallel arrays. **The constant
stays 64.** That is deliberate, not inertia: 64 remains conservative in
the right direction, and re-tuning it belongs in a spec that has
measured the guard, not one that has measured the slot.

What must change is the comment, which currently promises the constant
covers "`type_fqdn` and `rendered_as`". It is rewritten to name
`rendered_as` and the parallel arrays as the remaining referents, to
record that `type_fqdn` no longer allocates, and to keep the standing
warning that when `rendered_as` is interned too the first half of the
allowance stops naming anything at all and the constant must be taken to
0 deliberately rather than left to drift.

### S10 — documentation

- `docs/protolens/design/arena-and-batch.md`: mark annex rows 2–9 done,
  correct the running slot total to 120 B, and record under trap 2 that
  the guard was re-derived and left at 64 and why. Under trap 3, record
  that the crate-boundary route was taken rather than the
  protolens-local packed node, with the three reasons from Background.
- `docs/protolens/design/README.md`: the `prototext_core` interface
  contract grows a fourth item, the `FqdnTable`, and its `NodeSpan` and
  `decode_and_render_indexed` items are corrected.
- `docs/protolens/rendering-scaling-roadmap.md` S12 and
  `docs/protolens/rendering-worklist.md` W25: mark the FQDN-interning
  row done and point at this spec.

## Test plan

1. The two size assertions of S8 — the primary regression fence.
2. The full existing suites, unchanged in intent: 524 protolens tests
   and `prototext/tests/node_span.rs`. The latter's
   `owners_per_line` helper is the load-bearing one, since it pins spec
   0210's "every rendered line is owned by exactly one span" across all
   seven `MalformedKind` variants and the depth-cap case; the narrowing
   must not perturb it.
3. `verify_arena` and `assert_line_counts_are_exact`, which already run
   after every override commit in the test harness.
4. The three headroom-guard tests (`tests/override_apply.rs:2643`,
   `:2690`, `:2759`) compute `per_node` themselves and so will fail
   loudly if S9's arithmetic and theirs diverge.
5. New: a buffer over S1's cap is refused with a message naming the
   size, and does not abort.
6. New: `id_of` on a name no render produced returns `UNINTERNED`, which
   is *not* `NO_FQDN` and so does not compare equal to a typeless span —
   the invariant S6's idiom rests on.
7. New: two spans produced by *different* calls that share one table
   compare equal for the same type name — the invariant S4's shared
   table rests on, and the one a per-call table would break.
8. Measurement per the Measurement section below.

## Measurement

Reuse spec 0211's method exactly, so the two rows are comparable: the
pty driver at `/tmp/measure_0211_tui.py`, `googleapis.desc`,
`PROTOLENS_NO_MEMORY_GUARD=1`, both the `record` and `root` cursor
positions, `VmRSS` at rest and `VmHWM` at the peak.

Expectations to test, not to assume:

- **At rest**, 64 B/node × 4 501 014 nodes = 281 MB of struct, plus the
  `type_fqdn` heap allocations, which the guard's comment measures at
  part of ~41 B/node against RSS. At rest `rendered_as` is empty, so
  most of that 41 is `type_fqdn` and is also removed. The honest
  prediction is therefore a range, 280–450 MB, and the point of
  measuring is to find out where in it.
- **At the peak**, expect a multiplier **above** 0211's 2.29, because
  narrowing `NodeSpan` is what finally moves the render cache's clone.
  If it comes out at 2.29 again, the render cache is not being priced as
  the model claims and the model is what needs fixing.

Only memory is reported. Timings under a pty driver are not user-facing
latency and must not be quoted as such.

## Open questions

All three were settled during implementation.

1. ~~Where exactly to set S1's cap.~~ **`u32::MAX / 8`** (511 MiB), as
   `MAX_INDEXED_BUFFER` in `prototext-core/src/helpers/bounds.rs`, beside
   `MAX_WIRE_DEPTH`. Reading `render_text/mod.rs` settled it: it already
   did `let capacity = buf.len() * 8;` unconditionally, so the 8×
   reservation was an *existing* unnamed ceiling that aborted rather than
   refused. Tying the cap to it names what was already there, and a
   future removal of the reservation is the thing that should revisit the
   number.
2. ~~Whether `RenderError` should be a new type.~~ **Reused
   `CodecError`**, with a new `InputTooLarge { len, max }` variant. The
   enum is already `#[non_exhaustive]`, so the variant is not itself a
   semver break, and `decode_and_render_indexed` had no error type of its
   own to keep separate from.
3. ~~Whether the interned-needle idiom wants a small test helper.~~
   **Yes, three**, in `protolens/src/tui/tests/support.rs`:
   `node_with_type`, `has_node_with_type` and `type_name_of`. The
   mechanical pass produced the evidence the question asked for — the
   two-line form appeared ~40 times, and inside an iterator chain over
   `app.tree` it needs `app.fqdns` borrowed alongside, which would have
   turned each chain into an index loop. `type_name_of` exists separately
   because a handful of assertions want to *print* the name rather than
   match it, and `FqdnId(37)` says nothing in a failure message.

## Measured outcome

`googleapis.desc` (4 501 014 nodes, 5 281 124 lines, 25.6 MB) driven
under the pty harness spec 0211 used (`/tmp/measure_0211_tui.py`), with
`PROTOLENS_NO_MEMORY_GUARD=1` so the guard's own change of threshold
cannot decide whether the batch runs. The 0211 column is that spec's own
recorded run, unchanged, so the three columns are directly comparable.
The unit in the last column is one copy of this spec's saving:
4 501 014 × 64 B = 281 313 kB.

| | 272 B (pre-0211) | 184 B (0211) | 120 B (this spec) | delta vs 0211 | in units of 64 B/node |
|---|---|---|---|---|---|
| `VmRSS` at rest | 1 959 900 kB | 1 573 032 kB | **1 256 708 kB** | −316 324 kB | **1.12** |
| `VmHWM`, root retype | 4 379 916 kB | 3 493 144 kB | **2 631 224 kB** | −861 920 kB | **3.06** |
| `VmRSS` after the commit | 2 152 308 kB | 1 902 200 kB | 1 503 528 kB | −398 672 kB | 1.42 |

The peak falls **3.33 → 2.51 GiB, −24.7%**, and cumulatively over the two
specs **4.18 → 2.51 GiB, −37.0%**. At rest, 1.50 → 1.20 GiB (−20.1%);
cumulatively 1.87 → 1.20 GiB (−35.9%).

As in spec 0211, the `record` cursor position moves the peak not at all
(`VmHWM` stays at its at-rest 1 426 140 kB), because a 35-line subtree's
replacement never materializes a full-document `local_tree`. Only the
root retype exercises the peak, and that is the row above.

**At rest, 1.12 rather than 1.00.** Spec 0211's at-rest saving was
exactly one slot copy, because a link change touches nothing but the
slot. This spec's is 12% more, and the excess is row 9's heap: 35 011 kB
= 34.2 MiB of `type_fqdn` `String` allocations that no longer exist,
≈8 B per node averaged over the whole arena. That is well below the
~41 B/node the headroom guard's comment attributes to the two `String`s
together — which is consistent, since at rest `rendered_as` is empty on
all but the auto-expansion targets and only message nodes carry a
`type_fqdn` at all. It is also a further argument for S9's decision:
`STRING_ALLOWANCE = 64` was already under-covering the parallel arrays,
and the half it loses here was worth less than the comment claimed.

**At the peak, 3.06 — above the 3 the annex called an upper bound.** The
spec predicted a multiplier above 0211's 2.29 and got one, but the reason
it clears 3 is worth stating exactly, because it corrects the model
rather than confirming it. The annex prices the slot three times at the
peak (the surviving arena, the arena's new half, `local_tree`), and 0211
measured those three at 2.29 on this workload — well under 3, because the
raw-message replacement is ≈2.9 M nodes against the survivor's 4.5 M.
Those same three terms are still worth 2.29 here. The remaining **≈0.77,
≈212 MiB, is span-shaped, not slot-shaped**: `NodeSpan` narrowed by
exactly the same 64 B, and the render cache's clone plus the flat span
lists that feed each splice are copies of *spans*, which a link-only
change could not move and which 0211 explicitly predicted would start
moving here. So the "bounded above by 3" claim is sound for the three
arena terms and simply does not enumerate all the places the constant is
paid. Whichever spec takes row 10 should expect the same shape: the
arena terms at ≈2.3 and a fourth term for anything holding spans.

Only memory is reported. Timings under a pty driver are not user-facing
latency and are not quoted as such.
