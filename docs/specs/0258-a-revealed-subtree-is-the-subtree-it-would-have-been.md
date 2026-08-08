<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0258 — a revealed subtree is the subtree it would have been

Status: implemented
Implemented in: 2026-08-08
App: protolens
Refs: docs/specs/0249-a-large-document-answers-the-user-first.md (S1/S3,
        the row budget and `auto_folded`; S8, `expand_auto_fold`, one of
        the two defective call paths),
      docs/specs/0255-the-document-finishes-itself-while-nobody-waits.md
        (the bake, the other defective call path),
      docs/specs/0120-protolens-any-messageset-as-auto-overrides.md (the
        expansion that goes missing),
      docs/specs/0183-prune-the-override-walk.md (S3's
        `mark_fresh_subtree`),
      docs/specs/0188-the-batch-updates-what-changed-not-what-exists.md
        (S4/S5's watermark, which makes the missing call permanent
        rather than merely late),
      docs/specs/0221-a-refused-override-is-reported.md (S5's status
        line, which a background pass must not write),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (the fixed
        arena that leaves the watermark nothing to rescan),
      docs/specs/0257-the-first-pane-does-not-wait-for-the-last-line.md
        (which cannot land until this one has)

## Background

A bounded render stops at a node and folds it; opening it — by the user
scrolling to it (spec 0249 S8) or by the idle bake (spec 0255) — renders
its body. Both paths run through `expand_auto_fold`, which calls
`splice_override` directly.

Nothing on that path enters a `render_overrides` batch. So the subtree a
splice reveals never gets the pass that seeds and applies overrides to
freshly rendered nodes, and spec 0120's `Any` and MessageSet
auto-expansion does not happen inside it.

Established by a throwaway test on the `acme.Container { payload: Any }`
fixture. An unbounded render expands the `Any` to `acme.Payload`. A
bounded splice of the root followed by a full drain leaves:

```
1 {  #@ Container = 1
  payload {  #@ Any = 1
    type_url: "type.googleapis.com/acme.Payload"  #@ string = 1
    value: "\n\005hello"  #@ bytes = 2
  }
}
```

The same holds for an override entry whose origin is a path inside the
revealed subtree, and for MessageSet items.

### Why it is permanent, not late

`descend` is a watermark (spec 0188 S4/S5): once the arena's prefix has
been scanned, later batches only scan the unexamined suffix, and spec
0216 makes the arena a fixed size — so after the first batch that suffix
is empty. A splice does not append slots; it rewrites the overlay on
slots that were scanned long ago and found to be nothing. That is
precisely what `mark_fresh_subtree` exists to repair, and it is called
from `render_overrides_inner` after *its* splices only.

The consequence is that no later batch will ever revisit the revealed
nodes. A subsequent unrelated `render_overrides` does not fix the
document up; the unexpanded `Any` stays unexpanded for the life of the
session.

### How visible it is today, and why that is about to change

Reachable now by overriding a large root and scrolling into what the
bake reveals — narrow enough that it has not been reported. Spec 0257
bounds the *startup* render, which makes nearly the whole document
baked content in every session, on every file. It is a blocker for that
spec, so it is being fixed on its own first.

## Goals

- **G1.** A subtree revealed by `expand_auto_fold` shows the same
  auto-expansions and honors the same override entries as the same
  subtree rendered unbounded.
- **G2.** A bounded render followed by a full expansion produces the
  document an unbounded render produces — the same text, the same
  counts, the same exported bytes.
- **G3.** The bake stays background work: the resolution pass adds no
  message to the status line and no unbounded render.

## Non-goals

- **N1.** Bounding the startup render. That is spec 0257; this spec is
  its prerequisite and stands on its own.
- **N2.** Making the pass cheaper than the splice it follows. Both are
  O(revealed subtree), which is the order the caller already pays. S4
  records the gate that keeps it from being paid when there is nothing
  to find; a cheaper structure is only worth designing if that gate
  turns out not to be enough.
- **N3.** Reporting refusals from a background pass anywhere. S3 says
  they are dropped. Surfacing them needs a place to put them that is not
  the status line — a log, a badge, a pane — and that is a question
  about the interface, not about this defect.

## Specification

### S1. `expand_auto_fold` resolves what it revealed

After the splice succeeds, and only then:

1. `mark_fresh_subtree(idx, &self.positional_path(idx))`. The slots the
   splice just wrote did not exist as rendered nodes when
   `compute_descend_marks` ran, and the watermark means no later batch
   will scan them. This is exactly what `render_overrides_inner` does
   after its own splices (spec 0183 S3); `expand_auto_fold` has to do it
   itself because its splice is not inside a batch.
2. If that marked nothing, stop — S4.
3. Otherwise `render_overrides(idx)`.

`resettle_node` on `idx` itself is a no-op: the splice just wrote
`idx`'s provenance to the target it rendered under, so the pass finds
nothing to change there. The pass exists for what is *below* `idx`.

Placing this in `expand_auto_fold` rather than in `bake_step` covers
both callers with one change. The user's open gesture
(`navigation.rs:221`) has the same defect as the bake and the same fix.

### S2. The nested splices are themselves bounded

They go through `resettle_node`, which already asks
`confirm_row_budget()`. With `bounded_confirms` set, an auto-expanded
`Any` under a revealed node registers its own stops and is baked in
turn. Unbounding them reintroduces one level down exactly the freeze
spec 0249 removed.

Nothing needs writing for this — it is what the existing code does — but
it is what makes the recursion safe, so it is stated rather than left to
be rediscovered.

### S3. The pass reports no refusal

`render_overrides` summarizes a batch's refusals into `self.message` at
the end of the outermost call (spec 0221 S5). A bake runs thousands of
batches with nobody having asked for anything, and a refusal deep in a
subtree the user has not scrolled to is not an answer to a question they
asked.

The suppression is uniform across both callers rather than reserved for
the bake. A user opening a fold asked to see a body, not to apply an
override; a refusal from a pass they did not request is the same
non-sequitur on either path.

The mechanism is a flag consulted where `self.message` is assigned, not
a saved-and-restored message: restoring would race with anything else
that wrote the status line during the pass.

### S4. Nothing markable means nothing to do

Step 1's return value is the gate. Most revealed subtrees contain no
`Any`, no MessageSet and no override origin, so most bake steps do one
`collect_descendants` and stop. `mark_fresh_subtree` returns nothing
today; it gains a `bool`, in the shape `render_overrides_inner` already
uses for `resettle_node`'s `spliced`.

The gate is what keeps a drain from paying a full override walk per
step. Its effect is the number to measure against the drain (S5's
corpus run); if it is not enough, the next lever is the scan inside
`collect_descend_targets`, not the walk.

## Alternatives considered

**Call `render_overrides(idx)` unconditionally after every splice.** One
line, and it does the right thing. Rejected on cost, and the cost was
then measured: the drain goes from 5.65 s to **22.23 s** on
`googleapis.desc`, and its worst step from 29 ms to **69 ms** — past the
50 ms the bake exists to stay under. The step count goes 70 857 →
419 702, because an unconditional pass resettles descendants that then
register bounded stops of their own. S4's gate is one
`collect_descendants` the splice's own bookkeeping is the same order as.

**Put the fix in `bake_step` instead.** Leaves the user's open gesture
broken, which is the path a user actually notices, and splits one bug
into two fixes.

**Have `splice_override` do it, so no caller can forget.** It is called
from inside `render_overrides_inner` (through `resettle_node`), so it
would re-enter the pass that called it, once per node, unboundedly.
The batch machinery is the caller's concern by construction.

**Drop the watermark and rescan the arena each batch.** Fixes this by
brute force and reintroduces spec 0188's 17 ms per batch on a 382 k-node
arena — at thousands of batches per drain, minutes.

**Save and restore `self.message` around the pass.** Cheaper to write
than S3's flag, and it clobbers anything else that wrote the status line
while the pass ran.

## Test plan

1. `a_revealed_subtree_expands_any` — the Background transcript
   promoted: bounded splice of the `acme.Container { payload: Any }`
   root, drain, `acme.Payload` present. Fails on today's code, and is
   the regression test the whole spec exists for.
2. `a_revealed_subtree_applies_a_path_override` — an override entry
   whose origin is a path *inside* the revealed subtree takes effect
   after the reveal. `Any` expansion and an entry are different sources
   in `collect_descend_targets`; one passing does not imply the other.
3. `a_bounded_document_is_the_unbounded_document_with_any` — G2 over a
   fixture containing an `Any`, at budgets from `MIN_EXPAND_ROWS` up.
   The existing `a_baked_document_is_the_unbounded_document` (spec 0255)
   has no `Any` in its fixture, which is why it passes today.
4. `a_revealed_subtree_reports_no_refusal` — an override that cannot
   apply, inside a revealed subtree, leaves `app.message` untouched. The
   S3 rule; without it this is only found by a user watching a message
   they never asked for.
5. `an_expand_under_a_revealed_any_is_bounded` — after a reveal whose
   nested `Any` expansion is itself bounded, the new stops are in
   `auto_folded`. The S2 rule, and the guard against the recursion
   rendering a document unbounded from inside a bake step.
6. Corpus, not a test: `--raw` root override on `googleapis.desc`,
   drain, export, `cmp` against an unbounded export, and the drain time
   before and after. Spec 0256's recipe unchanged.

### What the plan got wrong

Item 5 is **not what was written**, and it is not merely renamed. On any
fixture small enough to reason about, the resolution pass converges
within a single bake step, and S2's bound produces the same final
document whether the nested splices are bounded or not — so there is no
assertion that distinguishes them. S2 is unobservable at fixture scale.

What shipped in its place is `an_opened_stop_expands_any_too`, covering
the *other* caller: the user's own open gesture at `navigation.rs:221`,
which S1 exists to fix as much as the bake does and which no other test
touched. Both were mutation-checked — with the pass disabled, four of
the four new tests fail, and with only S3's arm removed,
`a_revealed_subtree_reports_no_refusal` fails alone.

Item 2 also had to be rewritten. Its natural assertion — the override
replaced the `label` field, so the pass reached it — is vacuous: `label`
is equally absent when the `Any` never expanded at all, and the test
passed with the fix removed. It now compares against the *unbounded
document with the same entry already applied*, so a missing expansion
and a missing override each fail it.

## Measured outcome

`googleapis.desc` (25.6 MB), `/` overridden to
`google.protobuf.FileDescriptorSet`, 50-row pane, release build pinned
with `taskset -c 4-7`. An A/B on a single binary — one switch selecting
whether S1's block runs — three alternating pairs:

| | before | after |
|---|---|---|
| confirm | 0.126 / 0.128 / 0.125 s | 0.124 / 0.130 / 0.122 s |
| drain | 5.504 / 5.488 / 5.484 s | **5.651 / 5.655 / 5.610 s** |
| worst step | 26.4 / 24.8 / 24.0 ms | 28.7 / 28.1 / 29.2 ms |
| steps | 70 857 | 70 857 |
| document | 249 734 534 B | 249 734 534 B, `cmp`-identical |

**S1 costs +0.15 s on a 5.5 s drain — 2.7%, or 2.1 µs per bake step**,
and the spread within each column is 20 ms, so the difference is real
rather than noise. **G2 holds at corpus scale**: the drained document is
byte-identical to the unbounded one, and the confirm is untouched.

**S4's gate stops all 70 857 steps** — 70 857 asked, 0 let through. So
the whole +0.15 s is the gate's own `collect_descendants` plus
`collect_descend_targets`, and none of it is the walk. That also settles
N2: no cheaper structure is worth designing, because the gate is already
the entire cost. Ungated, the same drain is 22.23 s (see Alternatives).

### The corpus does not exercise the defect

Zero gate hits means the drained document was byte-identical *before*
the fix too. `googleapis.desc` under a `FileDescriptorSet` root contains
`Any` and MessageSet *declarations* and no instances of either, and the
run carried no user override inside a revealed subtree. So the corpus is
a cost measurement and not a correctness one; the correctness argument
is the four mutation-checked fixtures, and the corpus's contribution is
that the cost of insuring against it is 2.7%.
