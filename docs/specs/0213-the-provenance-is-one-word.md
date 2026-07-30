<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0213 — the provenance is one word

Status: implemented
Implemented in: 2026-07-30
App: protolens
Refs: docs/protolens/design/arena-and-batch.md (the redesign brief; its
        annex row 10 is what this spec executes, and its traps 1 and 2 are
        what this spec resolves),
      docs/protolens/design/document-tree.md ("`rendered_as`: provenance,
        not just \"is there an override\"" — the semantics this spec
        preserves exactly),
      docs/protolens/design/override-collection.md (the render pass that
        reads and writes it),
      docs/protolens/rendering-scaling-roadmap.md S12 and
        docs/protolens/rendering-worklist.md W25 step 3 (the earlier plan,
        which proposed a side table),
      docs/specs/0202-an-override-is-refused-rather-than-fatal.md (the
        headroom guard, whose `STRING_ALLOWANCE` this spec finally
        re-derives),
      docs/specs/0211-the-arenas-links-are-half-as-wide.md (row 1),
      docs/specs/0212-the-span-is-a-third-as-wide.md (rows 2–9, whose
        interning pattern and two-sentinel lesson this spec follows)

## Background

Specs 0211 and 0212 took `TreeNode` from 272 B to 120 B — the links to
`u32`, every span scalar to its natural width, and `type_fqdn` to a 4-byte
index into a shared intern table. Measured on `googleapis.desc`: the peak
of one root retype fell from 4.18 GiB to 2.51 GiB, and the arena at rest
from 1.87 GiB to 1.20 GiB.

Of the 120 bytes that remain, **48 are one field**:

```rust
pub rendered_as: Option<(Option<Option<String>>, String)>,
```

It is the largest single entry left in the slot, and — like `type_fqdn`
before it — it is also the *least* likely to be occupied. It is `None` on
every node that has never been spliced, which on the reference corpus is
4 499 336 of 4 501 014 nodes. So 48 bytes are paid 4.5 million times to
express "nothing here yet", and paid again for each of the other three
copies of the arena a commit holds at its peak.

### What the field means

It is the node's **provenance**: which override produced the text
currently on screen, and under what field name it was rendered. The
render pass's entire decision is one comparison against it
(`resettle_node`):

```rust
let current = Some((target, field_name));
if current != self.tree[idx].rendered_as { splice }
```

The nesting is load bearing and none of it is redundant:

| value | means |
| --- | --- |
| `None` | never rendered by an override pass |
| `Some((None, name))` | rendered with *no active override* — the type came from `natural_type` |
| `Some((Some(None), name))` | rendered raw, because an active entry explicitly says raw |
| `Some((Some(Some(t)), name))` | rendered as type `t` |

Distinguishing the middle two is what detects a demotion, and the field
name is the second half because a rename (spec 0119 G4) changes the
rendered text without changing the type.

### Why the value, not the halves

The brief's annex (row 10) proposes interning the two halves separately —
a `FqdnId` for the type and something for the field name — for 48 B → 8 B.
Interning the **pair as one value** is strictly better, and gets 48 B → 4:

- **It has room for the third state.** `FqdnId`'s inner `u32` is private
  to `prototext-core`, so protolens can name exactly two ids it did not
  get from the table: `NO_FQDN` and `UNINTERNED`. The type half needs
  *three* values that are not a type name — no override, explicit raw,
  and never rendered — and there is no honest third sentinel to be had
  without either punning on `UNINTERNED` or widening the library's API
  for a protolens concern.
- **The distinct set is tiny.** The pairs actually used are bounded by
  (override targets in play) × (field names under them), not by nodes. A
  document-wide retype gives all 7 771 targets the same type and a handful
  of distinct field names.
- **It keeps the semantics literally unchanged.** The tuple keeps its
  exact current shape *inside* the table, so no consumer has to reason
  differently about a provenance than it does today — only about how to
  reach it.

The cost is one hash of the pair per visited node in place of one tuple
comparison. The pair's `String` is already freshly allocated per visit
today (`field_name_for_by_path` returns an owned `String`), so nothing new
is allocated on the hot path; what disappears is the *storing* of it.

### Why not a side table

S12 and W25 step 3 both propose moving the field to a
`HashMap<NodeIdx, _>`, which saves 48 B rather than 44. It is the worse
trade, and the brief's trap 1 says why: it adds a *ninth* structure keyed
by node index, which spec 0203's compaction has to rekey on every
relocation and spec 0206's slot reuse has to clear on every free. This
spec supersedes that plan; the interning route adds no index holder.

## Goals

- G1. `size_of::<TreeNode>() == 76`, pinned by a compile-time equality
  assertion as in 0211 and 0212.
- G2. Provenance semantics unchanged — every distinction in the table
  above still detected, so no splice happens that did not happen before
  and none is skipped that did.
- G3. No per-node heap allocation left anywhere in the arena. After this
  spec a `TreeNode` is 76 bytes of plain scalars and owns nothing.
- G4. Keep the change mechanical at the call sites: a site that asked
  `rendered_as.is_some()` asks one comparison instead, and a site that
  wrote `rendered_as: None` writes a named constant.
- G5. Re-derive `STRING_ALLOWANCE` (spec 0202's headroom guard) against
  what is actually still off-slot, rather than leaving a constant whose
  stated referents no longer exist.

## Non-goals

- N1. Row 11, the hot/cold column split. It is a refactor, not a retype,
  and it should be justified by a navigation profile taken *after* this
  spec.
- N2. Slot reuse (spec 0206) and the `local_tree` in-place build. Both
  remain the next levers on the peak; this spec only makes each of their
  slots cheaper.
- N3. Shrinking the provenance table. It grows monotonically with distinct
  provenances, which is bounded by the overrides in play and is orders of
  magnitude below the node count. Reclaiming it is not worth a mechanism.
- N4. Any change to `prototext-core`. Unlike 0212 this row is entirely
  protolens's own, and the interning it needs is protolens's own too.

## Implementation steps

1. The table and its id (S1), with unit tests.
2. The slot (S2) and the size assertion (S4).
3. The call sites (S3).
4. The headroom guard (S5).
5. Documentation (S6).
6. Measurement, by the same pty harness 0211 and 0212 used.

## Specification

### S1 — the table

A new module, `protolens/src/provenance.rs`. "Provenance" is already the
word `document-tree.md` uses for this field, so the module name needs no
introduction.

```rust
/// What one node's rendering came from — the shape `rendered_as` used to
/// hold inline, kept verbatim.
pub type Provenance = (Option<Option<String>>, String);

/// An index into a `ProvenanceTable`.
pub struct ProvenanceId(u32);

/// The never-rendered sentinel — what `None` used to be.
pub const NOT_RENDERED: ProvenanceId = ProvenanceId(u32::MAX);

pub struct ProvenanceTable { /* Vec<Provenance> + HashMap<Provenance, ProvenanceId> */ }

impl ProvenanceTable {
    pub fn new() -> Self;
    /// The provenance behind an id, or `None` for `NOT_RENDERED`.
    pub fn get(&self, id: ProvenanceId) -> Option<&Provenance>;
    /// The id of a provenance, inserting it if absent. Borrows rather
    /// than takes, so the hit path clones nothing.
    pub fn intern(&mut self, p: &Provenance) -> ProvenanceId;
}
```

`ProvenanceId` is `Copy`, `Eq` and `Debug`.

**There is deliberately no `id_of`.** 0212 needed a lookup-without-insert
and therefore needed a second reserved id (`UNINTERNED`), distinct from
its absent sentinel, precisely so that a needle the table had never seen
would not compare equal to a span with no type. That hazard does not
arise here because it has no *cause* here: the only caller interns, so the
only id it can be holding is a real one. If a later change adds a
lookup-without-insert, it must add a second sentinel with it — a miss
answering `NOT_RENDERED` would make every never-rendered node compare
equal to a brand-new provenance and skip the splice it needs, which at
startup is every node in the document.

`intern` asserts that the table cannot reach the reserved id, as
`FqdnTable::intern` does.

### S2 — the slot

```rust
pub rendered_as: ProvenanceId,
```

The field keeps its name. It is written `NOT_RENDERED` where it was
written `None`.

### S3 — the call sites

Four shapes, all mechanical:

1. **Construction.** `rendered_as: None` → `rendered_as: NOT_RENDERED`, in
   `build_tree`, the three `extract.rs` synthetic nodes,
   `override_apply.rs`'s spliced local root, and the test builders.
2. **Occupancy.** `rendered_as.is_some()` → `rendered_as != NOT_RENDERED`.
3. **The comparison** (`resettle_node`). Intern `current`, then compare
   ids:

   ```rust
   let current = self.provenance.intern(&(target, field_name));
   if current != self.tree[idx].rendered_as { /* splice, then store `current` */ }
   ```

   Interning before the comparison rather than after means a provenance
   whose splice then fails is left in the table. That is bounded by the
   number of failed splices and costs one entry; the alternative is two
   lookups on every visit.
4. **Display.** The profiling tests print the field in assertion messages.
   They resolve it through the table (`app.provenance.get(..)`), which is
   what keeps those messages readable — a bare `ProvenanceId(37)` names
   nothing.

`App::new`'s seeding of the wrapper root (spec 0118 §2.1) interns before
it assigns, since the table and the node are two disjoint borrows of
`App`.

### S4 — where the table lives, and the assertion

`App.provenance: ProvenanceTable`, one per session, reached as
`&mut self.provenance` rather than through a helper method — the same
disjoint-borrow argument `App.fqdns` records.

It does *not* live on `Decoded`, and a splice's `local_tree` does not need
one: every freshly built node is `NOT_RENDERED`, so `build_tree` never
interns. This is the whole reason the table can be App-private while
0212's had to be threaded through the library.

```rust
const _: () = assert!(std::mem::size_of::<TreeNode>() == 76);
```

76 = span 32 + 7 links 28 + `sibling_ordinal` 4 + `lines_total` 4 +
`lines_visible` 4 + `rendered_as` 4. Every field is 4-aligned, so there is
no padding. An equality, not a bound, for the reason 0211 gives: the
number is quoted in the guard and in three documents.

### S5 — `STRING_ALLOWANCE`, re-derived

Spec 0202's guard estimates one more batch as
`tree.len() * (size_of::<TreeNode>() + STRING_ALLOWANCE)`. The allowance
has always covered two different things: the `String` heap hanging off
each node, and the arrays parallel to the arena. 0212 removed the
`type_fqdn` half of the first and left the constant at 64 deliberately;
this spec removes the rest of it, so nothing off-slot is a `String` any
more.

Trap 2 says to "take it to 0 deliberately". **That is wrong, and this
spec corrects it**: the parallel arrays are still there, and they are not
small. `heat_states` is a `Vec<HeatState>` sized to the arena, and
`descend` and `dead` are a `Vec<bool>` each. The trap's "~42 B/node"
figure was never measured; the honest number is whatever those three
types add up to.

So the constant goes away in favor of the sum itself:

```rust
let per_node = (size_of::<TreeNode>()
    + size_of::<heat_cue::HeatState>()   // parallel to the arena
    + 2 * size_of::<bool>())             // `descend` and `dead`
    as u64;
```

This is exact, it cannot drift when one of those types changes, and it
removes the last unexplained magic number from the guard. The direction it
moves the refusal threshold is recorded in the measured outcome; a
deliberate *re-tuning* of the guard still needs a measurement of the
guard, and is still not this spec.

### S6 — documentation

- `arena-and-batch.md`: mark row 10 done, correct its saving (44 B, not
  40) and the slot total (76 B); rewrite trap 1 to record that the side
  table was rejected and why the pair rather than the halves was interned;
  rewrite trap 2 to record that "take it to 0" was wrong and what
  replaced the constant; strike step 4 of the suggested order.
- `document-tree.md`: the provenance section keeps its argument and gains
  a sentence on where the value now lives.
- `rendering-scaling-roadmap.md` S12 and `rendering-worklist.md` W25 step
  3: mark done, and mark the side-table plan superseded.

## Test plan

1. **Table unit tests.** Interning the same provenance twice yields one
   entry; distinct provenances get distinct ids; `get` round-trips;
   `get(NOT_RENDERED)` is `None`.
2. **The three type states are three ids.** `(None, "f")`,
   `(Some(None), "f")` and `(Some(Some("a.B")), "f")` intern to three
   different ids. This is the distinction the field exists for, and the
   one an over-eager encoding would collapse.
3. **A rename is a different provenance.** `(Some(Some("a.B")), "f")` and
   `(Some(Some("a.B")), "g")` differ.
4. **No fresh provenance is `NOT_RENDERED`.** The property S1 relies on
   for having only one sentinel, asserted directly.
5. **The existing override suite is the real test.** Every splice
   decision in the crate runs through `resettle_node`'s comparison, so the
   prune, override-apply, override-select and manage-pane suites all
   witness G2. In particular `prune.rs`'s "an unpruned walk finds every
   node already matching its `rendered_as`" test asserts the no-op-batch
   property this spec could most easily break.
6. **The size assertion** is the test for G1.

## Measurement

The pty harness 0211 and 0212 used: open `googleapis.desc` with a
descriptor set, `PROTOLENS_NO_MEMORY_GUARD=1`, `:type-as-raw` on line 0
(the root header — one `Down` first retypes a 35-line record instead and
does not move the peak at all). Report `VmRSS` at rest, `VmHWM` at the
retype, and `VmRSS` after the commit, against 0212's figures.

Expected: 44 B × 4 501 014 = 198 MB per copy of the arena. At rest that
is one copy; at the peak, 0212 measured the constant being paid 3.06×, so
about 600 MB. The off-slot `String` heap this also removes is *not* part
of those figures, but it is negligible — only the ~1 678 spliced nodes
ever held one, against the 4.5 M that held the 48 empty bytes.

## Open questions

None.

## Measured outcome

`size_of::<TreeNode>() == 76`, asserted at compile time. 777 workspace
tests pass; `cargo fmt --all --check` and
`cargo clippy --release --workspace --all-targets` are clean.

The pty harness above, on `googleapis.desc` (4 501 014 nodes, so one copy
of the 44 B saving is 198 044 kB):

| | 272 B (pre-0211) | 120 B (0212) | 76 B (0213) | delta vs 0212 | in units of 44 B/node |
|---|---|---|---|---|---|
| `VmRSS` at rest | 1 959 900 kB | 1 256 708 kB | **1 063 420 kB** | −193 288 kB | **0.98** |
| `VmHWM`, root retype | 4 379 916 kB | 2 631 224 kB | **2 188 064 kB** | −443 160 kB | **2.24** |
| `VmRSS` after commit | 2 152 308 kB | 1 503 528 kB | 1 378 684 kB | −124 844 kB | 0.63 |

Peak 2.51 GiB → **2.09 GiB** (−16.8%); at rest 1.20 GiB → **1.01 GiB**
(−15.4%). Cumulatively across specs 0211, 0212 and 0213 the peak is down
**50.0%** and the arena at rest **45.7%**. Post-commit RSS is not
comparable between runs — compaction runs in idle time, so each sample
catches its binary mid-reclaim.

### The multiplier confirms the corrected peak model

0212 measured the slot being priced **3.06×** at the peak, above the
brief's stated bound of 3, and corrected the model: the constant is paid
in *four* places, not three, because the render cache holds
`Vec<NodeSpan>` rather than `Vec<TreeNode>`.

This spec is the control for that correction. `rendered_as` lives on
`TreeNode` and not on `NodeSpan`, so it appears in exactly the three
arena-shaped terms and not in the render cache's clone — and the measured
multiplier duly falls back to **2.24**, within noise of the 2.29 spec 0211
measured for its own `TreeNode`-only change, and well clear of 0212's
3.06. A span-shaped change prices ~3.06×; a slot-shaped one prices ~2.29×.
That is now measured twice each way, and it is the number any future row
should be predicted with.

### The guard

`STRING_ALLOWANCE` is gone. `arena_bytes_per_node()` computes
`size_of::<TreeNode>() + size_of::<HeatState>() + 2` — 76 + 40 + 2 =
**118 B**, against the old 120 + 64 = 184. So the refusal threshold
loosens by a third, which is the honest direction: the old figure was
over-charging by 66 B/node for `String`s that no longer exist.

Making it a function rather than an expression was not planned, and was
forced by the implementation: three of the guard's own tests had the
`+ 64` formula copied into them, and duplicating the formula is what made
them fail this spec's slot change for a reason that had nothing to do with
the guard. They now call the same function the guard does.

### Deviations from the plan

- **The pair is interned, not the two halves** (44 B saved rather than the
  annex's projected 40, and a 76 B slot rather than 80). The forcing
  reason is recorded above: `FqdnId`'s inner `u32` is private to
  `prototext-core`, so there is no third sentinel available for the type
  half's three not-a-type-name states.
- **No second sentinel, because there is no lookup-without-insert.** 0212
  needed `UNINTERNED` alongside `NO_FQDN`; this table needs only
  `NOT_RENDERED`. The hazard is recorded on `intern` for whoever adds such
  a lookup later.
- **`ProvenanceTable::get` and `len` are `#[cfg(test)]`.** Nothing in the
  shipping binary resolves an id back — production only compares one
  provenance against another, which is the entire point — but the tests
  print it, where a bare `ProvenanceId(37)` names nothing.
- **Trap 2's instruction was wrong and is corrected rather than followed.**
  See the guard section above.
