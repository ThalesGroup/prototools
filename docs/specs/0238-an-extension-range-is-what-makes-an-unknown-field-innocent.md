<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0238 — an extension range is what makes an unknown field innocent

Status: draft
App: reproto | prototext-graph | protoscan
Refs: docs/protoscan/scan.md (the protoscan investigation this comes
        from: why the current scanner fails on a FileDescriptorSet, and
        the four candidate termination strategies)

## Background

`protoscan` must decide where a candidate `FileDescriptorProto` ends.
Nothing on the wire says: a top-level message has no terminator and no
length prefix, and both signals one would want to use as a boundary —
a repeated singular field, an unknown field number — are legal
protobuf. The boundary is a *semantic* fact, derived from the schema.

Two schema facts are needed, and the compiled graph carries exactly
one of them.

**Cardinality is already there.** `TransitionEntry.label` (0=optional,
1=required, 2=repeated, `build_scoring_graph/serial.rs:34`) survives
compilation, and is part of the Hopcroft bisimulation key — the
alphabet Σ is *(field_number, label)* pairs (`hopcroft.rs:94-99`), so
`graph.rs:289` can state that each `(src_block, field_number)` has at
most one label. Nothing is needed here.

**Extension ranges are not.** `CompiledGraph` is
`{nodes, transitions, roots, ranges, num_states}` (`serial.rs:51`),
where `ranges` is *value* ranges for RANGE leaves — bool 0/1,
closed-enum extents (`serial.rs:57`) — not field-number extension
ranges. `prototext-graph` contains no occurrence of `extension_range`
at all.

Without them the walk cannot distinguish the two populations of
unknown field:

- a custom option at field ≥ 1000 inside a `*Options` message, which
  the schema explicitly declares as extensible and which is therefore
  *expected*;
- a field number the schema does not declare and does not permit,
  which for a closed schema is evidence the bytes are not this type.

`descriptor.proto` declares extension ranges on exactly ten messages:
the nine `*Options` types (`ExtensionRangeOptions`, `FileOptions`,
`MessageOptions`, `FieldOptions`, `OneofOptions`, `EnumOptions`,
`EnumValueOptions`, `ServiceOptions`, `MethodOptions`), each
`extensions 1000 to max;` beside `uninterpreted_option = 999`; plus
`FeatureSet` with `1000 to 9994`, `9995 to 9999`, `10000`. **Every
other message in `descriptor.proto` is closed.** So the legitimate
unknown-field surface in an FDP is small, bounded, and machine-readable
— which is what makes a termination rule possible at all.

### When this actually bites

reproto already folds pool-visible extensions into a message's declared
field list (`phases.py:1559-1561`):

```python
known_fields = list(desc.fields_by_number.values())
if not desc.GetOptions().message_set_wire_format:
    known_fields += list(ctx.pool.FindAllExtensions(desc))
```

So for a corpus database that *contains* the extension definitions —
googleapis defines `google.api.http` and its siblings — those custom
options are already ordinary declared fields and score as matches. The
problem is confined to extensions the database does not define, which
is total for protoscan: its database is `descriptor.proto` alone, which
declares no extensions of the options messages, so every custom option
in every scanned descriptor is unknown.

This is why the feature is opt-in rather than universal: for the
scoring corpus it changes little, and for protoscan it is the whole
game.

## Goals

- **G1.** Carry extension ranges from the descriptor through reproto's
  scoring YAML into `hopcroft.rkyv`, as data, under an opt-in reproto
  flag. Default output stays byte-identical.
- **G2.** Extension ranges participate in Hopcroft state identity, so
  two messages that differ only in extensibility do not merge.
- **G3.** Give the walk a `SCAN` policy that uses extension ranges and
  cardinality to determine where a top-level message ends, and reports
  that offset alongside the score.
- **G4.** `SCORE` — every existing caller — is unchanged in behavior
  and unchanged in output.

## Non-goals

- **N1.** Per-unique-field-number counting of `matches` and `unknowns`,
  in any form. **Deferred to future work**, because no consumer needs
  it: `SCORE` keeps instance counting so existing baselines hold, and
  protoscan's accept rule is built from the *defect* counters —
  `vetoed`, `mismatches`, `out_of_range`, `non_canonical` — which are
  already size-independent. All five read zero for all 7 771 genuine
  FDPs in `googleapis.desc`, whose sizes span four orders of magnitude,
  so `matches` need not enter the decision at all.

  Recorded so the design is not re-derived: the open sub-question is
  whether dedup is scoped **(1) within one message instance** or
  **(2) across all instances of an FQDN**, and the motivating example
  settles it against (1). An FDP with 200 methods, each carrying 3
  distinct custom options, has 600 unknown instances today. Under (1)
  each `MethodOptions` instance still contributes its 3, so the total
  stays 600 — (1) only helps when the *same* field number repeats
  *inside one instance*, i.e. a repeated extension. Under (2) the total
  is 3. Scoping (2) is what delivers the stated benefit and what makes
  `score()` size-independent; scoping (1) does neither.

- **N2.** Vetoing on out-of-range unknowns under `SCORE`. The governing
  principle vetoes only the impossible, and an unknown field outside
  every extension range is the single most ordinary schema evolution
  there is — a later version added a field. `walk.rs:78` states this:
  "`unknowns` is mildest because it has a benign explanation — a newer
  sender talking to an older schema is what forward compatibility is
  for." Strictness is sound only under the additional condition that
  the schema is *closed and current for the corpus*, which holds for
  protoscan's pinned `descriptor.proto` and does not hold in general.
  Hence a policy, not a default.

- **N3.** Termination below the top level. A nested message is
  length-delimited; its extent is already stated on the wire. Only the
  outermost message has an undetermined end, so `SCAN` decides
  termination at depth 0 only.

- **N4.** Reading `hopcroft.rkyv` files written before this spec.
  Databases are build artifacts regenerated by reproto; a version
  mismatch is an error with a rebuild instruction, not a compatibility
  shim.

- **N5.** Implementing protoscan itself. This spec provides what
  protoscan needs; the scanner, its anchor, its property predicates and
  its Python surface are separate work (see `docs/protoscan/scan.md`).

- **N6.** Recovering a boundary from a veto. A veto fires *inside* a
  field already consumed, so unlike a termination it leaves both the
  entry's counters and the frame's occurrences polluted by bytes that may
  lie past the true end of the record. `SCAN` therefore reports no usable
  boundary for a vetoed entry (S14), and protoscan rejects the candidate
  outright.

  This costs a genuine descriptor in exactly one situation. The record
  really ends at `E`, and the byte at `E` happens to be one of the seven
  tags for a *repeated* LEN field of `FileDescriptorProto` —
  `dependency` `0x1A`, `message_type` `0x22`, `enum_type` `0x2A`,
  `service` `0x32`, `extension` `0x3A`, `public_dependency` `0x52`,
  `weak_dependency` `0x5A` — followed by a plausible length. Neither S12
  rule fires: the field is declared, and it is repeated. The walk
  descends into foreign bytes, vetoes there, and a real descriptor is
  thrown away.

  That requires arbitrary binary padding after the record. It cannot
  happen inside a `FileDescriptorSet`, where the next byte is the record
  header `0x0A` and S12 rule 2 fires cleanly, nor in the
  Google-internal MPM package layout. It is a stripped-executable
  concern, and protoscan does not face it first.

  The mechanism, recorded so it is not re-derived when that day comes:
  at each depth-0 field boundary, snapshot
  `(ws.scores[e], ae.occurrences, pos)` into a single rolling buffer
  overwritten in place — O(live entries) memory,
  O(live entries + occurrences) time per top-level field, and nothing at
  all under `SCORE`, which never takes the snapshot. On veto, report the
  snapshot instead of the live state, applying cardinality against the
  snapshotted occurrences. It is restore-by-copy, not
  unwind-by-subtraction, so it is exact by construction rather than by
  arithmetic that has to be got right. The snapshot is small:
  `occurrences` is a `SmallVec<[(u32, u32); 2]>` of sorted
  `(field_number, count)` pairs (`walk.rs:659`) holding at most 13 of
  them at depth 0 for an FDP. If adopted it should also surface a
  `recovered: bool`, since a candidate that needed backtracking is weaker
  evidence than one that terminated by rule — it is equally consistent
  with an accidental anchor.

## Specification

### Refactoring prerequisite

- **S1.** The two scoring-YAML emitters in
  `reproto/src/reproto/phases.py` are near-duplicates and must be
  unified before either gains a new key. `_phase_build_schema_db`'s
  local `_collect` (line 1551) and `_phase_emit_scoring_graphs`'s local
  `_collect` (line 1889) are identical except in three places:

  | | schema-db copy | emit copy |
  |---|---|---|
  | child name | `_canonical_scoring_name(ctx, child)` | `child` |
  | messages/entries key | `canonical_name` | `desc.full_name` |
  | MessageSet call | 4-arg, passes `canonical_name` | 3-arg |

  The duplication is already acknowledged in the source — the
  schema-db copy's comment at line 1556 tells the reader to consult its
  sibling for the rationale it does not repeat.

  Replace both with one module-level
  `_collect_scoring_messages(ctx, desc, messages, group_fqdns, entries, rename)`,
  where `rename: Callable[[str], str]` is identity for the emit path
  and `partial(_canonical_scoring_name, ctx)` for the schema-db path.
  With the caller always supplying the renamed name,
  `_synthesize_message_set_item`'s optional `canonical_full_name`
  parameter (`phases.py:1793`) exists only to serve the un-factored call
  site and becomes a required positional — one shim removed, not merely
  one copy.

### Extension ranges through the pipeline

- **S2.** reproto gains an opt-in flag (working name
  `--emit-extension-ranges`). When set, a message in the scoring YAML
  may carry an `ext_ranges` key: a list of `[start, end]` pairs,
  inclusive at both ends. When unset the key is never emitted and the
  YAML is byte-identical to today's.

- **S3.** Ranges are canonicalized at emission, and the canonical form
  is **unique for a given set of field numbers**. That is the whole
  point: S5 makes the intern index the equality test, so two messages
  admitting exactly the same extension field numbers must reach the
  same index no matter how their `.proto` spelled it. The procedure:

  1. Convert protoc's half-open `[start, end)` to inclusive
     `[start, end - 1]`. **`extension_range.end` is exclusive** —
     `extensions 1000 to max` arrives as `(1000, 536870912)`, and
     `extensions 10000` as `(10000, 10001)`.
  2. Drop degenerate ranges (`end <= start`). protoc cannot emit one,
     but a hand-assembled descriptor can.
  3. Sort by start.
  4. Merge overlapping **and adjacent** ranges: fold the next range in
     whenever `next.start <= current.end + 1`.

  The result is the unique minimal list of maximal, disjoint,
  **non-adjacent**, ascending inclusive intervals. Uniqueness needs all
  four steps, and step 4's adjacency clause is not hypothetical:
  `FeatureSet` declares `1000 to 9994`, `9995 to 9999`, `10000` and so
  arrives as three touching ranges, which canonicalize to the single
  `[1000, 10000]` — the same set another `.proto` would get from one
  `extensions 1000 to 10000` clause.

  `max` needs no sentinel handling: protoc has already materialized it
  as `536870912` (exclusive) by the time reproto sees the descriptor,
  giving `536870911` = 2²⁹−1 inclusive. The requirement is only that
  reproto must not *re*-introduce a symbolic `max` on the way out —
  `1000 to max` and `1000 to 536870911` are the same set and must not
  intern as two.

- **S4.** `load.rs` gains `ext_ranges: Option<Vec<(u32, u32)>>` on
  `YamlMessage` and on the merged message representation. `MessageDef`'s
  equality (`load.rs:224`), which today compares number and label, must
  compare the range set too — otherwise two messages identical except in
  extensibility are deduplicated before Hopcroft ever sees them.

### Interning

- **S5.** Range sets are **interned**, not hashed: a table
  `ext_range_sets` holds each distinct canonical set once, and a node
  carries a `u16` index into it. This is exact — no collision path — and
  the index that makes Hopcroft comparison an integer compare is the
  same index the walk uses to fetch the ranges. A hash would give the
  first property and not the second.

  This reuses the shape already in the codebase: `LeafRegistry` interns
  value ranges (`graph.rs:55-57`) and `NodeEntry` carries
  `range_idx: u16` with `0xFFFF = none` (`serial.rs:20`). `NodeEntry`
  gains `ext_range_idx: u16`, same sentinel.

  The cost is **4 bytes per node, not 2**. `NodeEntry` is today
  `{u32, u8, bool, u16}` = exactly 8 bytes at alignment 4, with no
  padding hole to absorb a new field; a fifth field takes it to 12,
  growing the node table by 50%. Narrowing the new index to `u8` saves
  nothing — 9 bytes still pads to 12 — and the intern table is expected
  to hold three entries anyway, so `u16` for consistency with
  `range_idx` is the right call. The absolute figure should be recorded
  at step 2's checkpoint, where the format change is isolated from every
  semantic change and the artifact can be measured directly.

  Table size is small. Measured on `descriptor.proto` at step 3, **three**
  entries cover its 34 messages:

  | canonical set | messages |
  |---|---|
  | `[[1000, 536870911]]` | the nine `*Options` types |
  | `[[1000, 10000]]` | `FeatureSet`, after S3's adjacency merge |
  | `[[536000000, 536000000]]` | `FileDescriptorSet`, `SourceCodeInfo` |

  The last row is the one this spec did not predict: both reserve the
  single field number 536000000 for an internal declaration. Every other
  message takes the `NO_EXT_RANGES` sentinel rather than an empty set, so
  the empty set never occupies a slot. Small on any corpus too, because
  extension ranges are rare and overwhelmingly spelled `1000 to max`.

- **S6.** The intern table is ordered deterministically — sorted by
  canonical range set, not by first encounter — so that `hopcroft.rkyv`
  stays reproducible.

### Hopcroft

- **S7.** Extension ranges seed the **initial partition**: states whose
  `ext_range_idx` differ start in different blocks and can never merge.
  They do *not* join the alphabet Σ. `label` could join Σ because it is
  an attribute of an edge; an extension range is an attribute of the
  state, which no edge carries. The refinement loop and the worklist are
  untouched.

### File format

- **S8.** `CompiledGraph` gains `ext_range_sets`, stored flat:
  `Vec<(u32, u32)>` of all ranges concatenated, plus `Vec<(u32, u32)>`
  of `(offset, len)` per set. Flat rather than `Vec<Vec<_>>` because the
  archived form is read through `access_unchecked`, which is friendlier
  over a flat slice.

- **S9.** `CompiledGraph` gains `has_extension_ranges: bool` — true iff
  the graph was built by a reproto run with S2's flag set. It is a
  **precondition, not a behavior switch**: requesting `SCAN` against a
  graph with `has_extension_ranges == false` is an error naming the
  missing reproto flag.

  It is not a policy modifier because `SCAN` is strict — an empty range
  set means no extension is permitted (S12) — and a graph built without
  the flag has an empty set on *every* message. Silently honoring that
  would terminate on the first custom option of every descriptor and
  produce plausible, wrong answers. Failing loudly is the only safe
  reading of missing data.

- **S10.** `VERSION` (`serial.rs:65`) goes 2 → 3. A v2 file loads with
  an error naming the version and instructing a rebuild (N4).

### The SCAN policy

- **S11.** `ScoringOpts` gains `policy: Policy` — a plain field with
  variants `Score` and `Scan`, **not** an `Option<Policy>`, with
  `Default` yielding `Score`.

  `Option<Policy>` was asked for on backwards-compatibility grounds and
  does not deliver it. Rust has no default-valued struct fields, so a
  *complete* struct literal must name every field regardless of whether
  its type is `Policy` or `Option<Policy>`; both break the same call
  sites identically. `Option` would only add a second spelling of the
  same state (`None` and `Some(Score)`) for every match site to handle.

  The compatibility lever is `Default` plus struct-update syntax, and it
  has to be applied at the call sites: `prototext/src/run.rs:436`,
  `:511` and `:541` build `ScoringOpts` field by field and must become
  `ScoringOpts { expand_any: …, ..Default::default() }`. After that one
  edit, future field additions cost nothing.

  `ScoringOpts` is **not** marked `#[non_exhaustive]`. That attribute
  would make the struct-update discipline compiler-enforced by
  forbidding foreign crates from writing a `ScoringOpts { … }` literal
  at all — but it forbids `..Default::default()` too, leaving external
  callers only `let mut o = ScoringOpts::default(); o.policy = …;`. The
  guarantee it buys is API stability for consumers outside this
  workspace, and there are none: `protolens`, `prototext` and
  `protoscan` are all in-tree, where a missing field is a compile error
  fixed in the same commit. Not worth the uglier construction form for a
  two-field options struct.

  Those exact three sites have already misled this workspace once, and
  the lesson is recorded in `docs/protolens/rendering-worklist.md:638`:
  a `Default` impl does not describe shipped behavior when a CLI builds
  the struct field by field, so **the non-regression test for this step
  must drive the `prototext` binary, not `score_all`**.

- **S12.** Under `Scan`, at depth 0 only, a root's walk terminates
  *before* consuming a field when either:

  1. the field number is not declared for the current state and is not
     contained in that state's extension range set; or
  2. the field number is declared **singular — `optional` or
     `required`** — and is already present in this frame's
     `occurrences`.

  Both rules are evaluated per `ActiveEntry`, i.e. per depth-0 *state*,
  not per root: entries sharing a state share their declared fields,
  their range set and their occurrence counts, so they necessarily
  terminate together. Roots at different states terminate independently
  (S13).

  Rule 1 is strict by construction: a state with an empty range set
  declares itself closed, and an undeclared field number in a closed
  state is a boundary. Rule 2 generalizes protodump's "consume field 1
  once" to every singular field, and is what makes a
  `FileDescriptorSet` legible — the outer record header is a second
  field 1. `required` terminates for the same reason `optional` does;
  the rule is about cardinality, and a repeated `required` is a
  repeated singular. Under `Score` a second `required` is a
  `mismatches` candidate, and that is unchanged.

- **S13.** Termination is **recorded, not obeyed** — by the *walk*. The
  walk does not stop, because roots terminate at different offsets (each
  has its own singular set and its own extension ranges) and a single
  walk that halted at the first termination would truncate every other
  root's score. One pass, N independent termination offsets.

  The terminating entry does stop, and stopping it needs no new
  machinery. At the termination point, before the offending field is
  consumed: apply `apply_cardinality_multi` (`walk.rs:801`) against the
  frame's occurrences as they stand, record the offset, and clear the
  entry out of its `ActiveEntry` — the same `entries.clear()` /
  `active.retain` path a veto already uses (`walk.rs:1308`, `:1320`).

  Nothing has to be rolled back, and that is the point: **termination
  fires before a field is consumed**, so at that instant both
  `ws.scores[e]` and `ae.occurrences` describe exactly the record that
  ended. A veto is the opposite — it fires inside a field already
  consumed, leaving both polluted — which is the whole reason recovering
  a boundary from one is deferred (N6).

  Two details worth pinning, because both are silent if got wrong:

  - The recorded offset is the **first byte of the tag**, saved before
    the tag is decoded. By the time verdicts are known, `pos` has already
    advanced past the tag and any length prefix (`walk.rs:1195`), so
    reading `pos` at the termination point yields a boundary several
    bytes late.
  - Cardinality is applied **at the termination point, not at EOF**. A
    `required` field that would have appeared after the boundary is
    genuinely absent from the record that ended and must count as a
    `mismatches`; deferring the pass to EOF would instead evaluate it
    against occurrences polluted by the following record.

- **S14.** `EntryScore` gains `termination: usize` — the byte offset at
  which this root stopped consuming. Under `Scan` that is the
  termination offset, or `pb.len()` if the root consumed the whole
  buffer. Under `Score` it is `pb.len()` unconditionally, so every
  result is a `(score, termination)` pair and no caller branches on the
  policy.

  It is `usize`, not `Option<usize>`, for three reasons. There is no
  state to distinguish: termination fires *before* consuming a field, so
  a root cannot terminate at `pb.len()` — "terminated at the end" and
  "ran to the end" are the same fact. Under `Score` it is a constant,
  costing no hot-path work. And `Option<usize>` is 16 bytes where
  `usize` is 8, `usize` having no spare niche — at 49 255 roots in the
  corpus graph that is 400 KB of `score_all` result per call, not 800.

  A **vetoed** entry's `termination` is `pb.len()` under both policies,
  and is meaningless: the entry stopped part-way through a field, at an
  offset that is not a record boundary and that may lie past the true end
  of the record. protoscan rejects vetoed candidates outright, so it
  never reads the value. Recovering a usable boundary from a veto is N6.

  This is source-compatible for consumers because `EntryScore` is
  **never constructed outside `prototext-graph`**: the only literals are
  `walk.rs:323`, `walk.rs:1584` and `score/tests.rs:210`. External code
  reads fields, and reading is unaffected by an added field. Returning a
  distinct type from `Scan` would instead split every call site.

- **S15.** Under `Scan`, an unknown field *inside* a declared extension
  range is not counted in `unknowns` and carries no penalty. It cannot
  be validated — its type is unknown — so it is neither evidence for nor
  against, and its payload is skipped by wire type.

  Under `Score` this is unchanged even for a graph that carries range
  data: an unknown is an unknown, and G4 is unconditional. The range set
  is read only under `Scan`.

- **S16.** The UTF-8 veto (`walk.rs:1302`) must not fire on a field of
  unknown type. A custom option at field ≥ 1000 may be `bytes` or a
  submessage, and vetoing it as a bad string would reject legitimate
  descriptors.

  **This already holds and needs no change.** The veto sits in the
  `Verdict::Found` arm behind `node.is_string` (`walk.rs:1291-1302`),
  while an unknown LEN field takes the `Verdict::Unknown` arm, which only
  increments `unknowns` (`walk.rs:1210-1214`) — S15 removes even that
  increment inside a declared range. The item stays in the spec as a
  **regression guard** (test 13), because S15 is the first change to make
  unknown-field handling policy-dependent, and that is where the
  invariant would break.

## Implementation sequence

Each step is separately committable and separately revertible. Steps 1,
2 and 5 are behavior-preserving by construction, which is what makes
their checkpoints meaningful: any diff at those points is a bug, not a
judgement call.

**Step 0 — optional, and not part of this spec.** Land the one-condition
boundary fix in `fdp-scan-pyo3/src/lib.rs` described in
`docs/protoscan/scan.md` §1.3. It takes `googleapis.desc` from 1
candidate to 7 771.
*Checkpoint:* the seven existing inline unit tests still pass; 7 771
distinct names extracted.

*Amended 2026-08-04 — this step is no longer a precondition.* It was
listed as one because step 7 diffed against it; step 7 now uses step 6's
length-prefix ground truth, which is independent of both stop rules
rather than a second opinion from the same family of guess.

What step 0 fixes is real — the shipped `protoscan` 0.2.1 returns one
name where it should return 7 771, and returns it *silently*, because
field 1 is singular and protobuf's last-wins rule overwrites `name`
7 770 times without error. But the fix lands in `walk_protobuf_fields`
and `looks_like_fdp_start`, both of which **step 7 deletes**. So it is
scaffolding: worth landing to cut a corrected release before step 7, and
worth skipping entirely if step 7 is the next thing done.

**Step 1 — factorize the emitters (S1).**
*Checkpoint:* reproto suite green; scoring-graph YAML byte-identical on
existing fixtures; rebuild the googleapis db and confirm
`hopcroft.rkyv` is byte-identical to the pre-change artifact.

**Step 2 — format v3, inert (S8, S10).** Add `ext_range_sets` (empty),
`has_extension_ranges = false`, `NodeEntry.ext_range_idx = 0xFFFF`, and
bump `VERSION`. No producer, no consumer.
*Checkpoint:* Rust suite green; v2 rejection test; rebuild the db and
confirm `score_all` over the corpus yields `EntryScore`s identical to
step 1. This deliberately isolates the format change from any
semantics — so **record the `hopcroft.rkyv` size delta here**, where it
is attributable to `NodeEntry`'s padding (S5) and nothing else.

*Measured 2026-08-04.* googleapis `hopcroft.rkyv` 4 791 276 →
4 858 080 bytes, **+66 804 (+1.39%)**, which is 4 bytes × 16 701 nodes
to within the three new (empty) vector headers — exactly the padding S5
predicts and nothing else. `prototext list-schemas` over the corpus's
375 instances is byte-identical across 2 903 922 lines of output
against the pre-change binary reading the pre-change v2 database.

**Step 3 — reproto emits ranges under the flag (S2-S4).**
*Checkpoint:* with the flag off, YAML and `hopcroft.rkyv` byte-identical
to step 2; with the flag on, a `descriptor.proto`-only db carries the
distinct range sets tabulated in S5.

*Measured 2026-08-04.* Flag off: googleapis `hopcroft.rkyv` and the
whole rendered `.proto` tree byte-identical to step 2. Flag on:
`descriptor.proto`'s 34 messages carry **three** distinct sets, S5's
table — including `FeatureSet`'s `[[1000, 10000]]`, which is S3's
adjacency merge doing its job on real input, since that message's three
declared clauses touch.

**Step 4 — interning and the Hopcroft initial partition (S5-S7).**
*Checkpoint:* with the flag off, state count and artifact unchanged from
step 2. With the flag on, **record the state-count delta** — this is the
one quantity this spec predicts (small, because extension ranges are
rare) without having measured it.

*Measured 2026-08-04.* Flag off: googleapis `hopcroft.rkyv`
byte-identical to step 3. Flag on: **the state-count delta is zero** —
16 696 states before and after, with the same 85 806 transitions. The
prediction that it would be small was right for a reason weaker than the
argument assumed: on this corpus the twelve extensible messages are all
`descriptor.proto` types whose transition signatures were already
unique, so S7's new partition key splits nothing that Σ was not already
splitting. It earns its keep as a *guarantee* rather than as an
observed refinement — nothing in the graph would otherwise stop a future
schema's extensible message from merging with a closed twin. The file
grows by 48 bytes: three ranges and three `ExtRangeSet` headers.

The three interned sets are exactly S5's table, held by twelve nodes:
the nine `*Options` types plus `ExtensionRangeOptions` take
`[[1000, 536870911]]`, `FeatureSet` takes `[[1000, 10000]]`, and
`FileDescriptorSet`/`SourceCodeInfo` share `[[536000000, 536000000]]`.
The table's order confirms S6 — sorted by the set, so the two
`1000`-based sets are adjacent regardless of which message the loader
happened to visit first.

**Step 5 — policy plumbing, `Scan` ≡ `Score` (S11).** Add the enum,
thread it through, convert the three `run.rs` literals to
`..Default::default()`. `Scan` executes the `Score` path unchanged.
*Checkpoint:* corpus scores identical under both variants; plus a test
that drives the `prototext` binary, per S11.

*Measured 2026-08-04.* `score_all` run over the 375 googleapis corpus
blobs against the step-4 graph, once per variant: **18 470 625 scored
rows, zero differing** on any of `fqdn`, `matches`, `unknowns`,
`out_of_range`, `non_canonical`, `mismatches` or `vetoed`. That result is
a tautology at this step and worth naming as one — `policy` has no
readers yet, so the two variants cannot diverge; what the sweep actually
pins is that widening `ScoringOpts` and rewriting the three `run.rs`
literals as struct updates changed no scoring behavior. The half of the
checkpoint with teeth is the binary-driven test
(`e2e::score_still_honors_no_expand_any`): it spawns `prototext score`
twice on the same payload and asserts 3 versus 2 under
`--no-expand-any`, so a field silently dropped from one of those literals
fails the suite. Getting a difference to show at all required nesting the
`Any` inside a `google.protobuf.Option` — Any-expansion fires from a
*field* pointing at `ANY_BLOCK_ID`, never from scoring `Any` as the root
type.

**Step 6 — SCAN semantics (S9, S12-S16).**
*Checkpoint:* unit tests 7-14 below; then test 16 — all 7 771 FDPs in
`googleapis.desc` terminate exactly at their record boundary.

*Measured 2026-08-04.* Test 16 is **exact: 7 771 of 7 771**, none vetoed,
none off by a byte. Each FDP payload was scored under `Scan` from its own
start to the *end of the whole 25.6 MB buffer* — the walk is given no
length prefix and no hint of where to stop — and every one of them
reported its own record's length. The rule that fires is S12 rule 2: an
FDP declares `name` singular, so the next `file` entry's field-1 tag is
the boundary. The 7 771 records also tile the buffer exactly, which is
the same fact said twice and worth saying: no gaps, no overlaps.

The `Score` path is byte-for-byte what it was at step 5. A digest over
`(fqdn, matches, unknowns, out_of_range, non_canonical, mismatches,
vetoed)` for all 18 470 625 scored rows of the 375-blob corpus is
`15ba501096a664a1` both before and after this step, compared against a
build of the step-5 commit in a scratch worktree. `termination` was
`pb.len()` on every one of those rows, so S14's "no caller branches on
the policy" is a measured claim rather than a design intention.

One deviation from the test plan: **item 14 is a unit test, not a corpus
baseline.** It asserts the `Score` counters and `termination == pb.len()`
against a graph that *does* carry range data — which is the part a
regression would break — while the corpus half of the same guarantee is
the digest above. Recording a 18 M-row baseline in the repo to assert
what a 16-hex-digit comparison already asserts would be storage for its
own sake.

**Step 7 — protoscan switches over.** Separate work (N5). Also carries
`--emit-extension-ranges` into `default.nix`'s `wktRkyv` derivation and
regenerates `prototext/wkt/prebuilt/*.rkyv`, without which `Policy::Scan`
against the embedded WKT graph trips the S9 assert.
*Checkpoint:* protoscan's output on `googleapis.desc` is 7 771 names
matching **step 6's boundaries**, in order.

*Amended 2026-08-04.* This checkpoint originally diffed against step 0's
oracle. That is the weaker of the two available, for a reason worth
stating: step 0's stop rule is *this spec's* S12 rule 2 restricted to
field 1 and written by hand, so diffing against it confirms only that the
schema-derived rule reduces to the hand-written one on this input. S12 is
strictly stronger — every singular field (1, 2, 8, 9, 12, 14), plus
undeclared numbers, plus `out_of_range` — but on `googleapis.desc` every
record opens with field 1, so the extra strength never fires and the two
agree by construction. It is a regression check dressed as a validation.

Step 6's measurement is genuine ground truth instead: `googleapis.desc`
is a real `FileDescriptorSet`, so the true boundaries are read off the
*length prefixes*, with no heuristic on either side of the comparison.
That oracle is independent of both rules and is already in hand.

## Alternatives considered

**`Option<Policy>` for backwards compatibility.** Rejected because it
provides none: complete struct literals must name every field whatever
its type, so the breakage is identical, and `Option` adds a redundant
second spelling of `Score` (S11).

**A separate `ScanOpts` struct instead of a field on `ScoringOpts`.**
Rejected: scanning is a way of scoring, not a different operation. A
second struct would split `score_all`'s signature in two and force every
shared knob — `expand_any` first — to be declared twice.

**Hash the canonical range set instead of interning it.** Gives fast
Hopcroft comparison but still requires storing the canonical form
separately for the veto check, and adds a collision path. Interning
gives both properties with one table and no collisions (S5).

**Treat a missing range table as "nothing terminates".** The first
reading of the backwards-compatibility rule: a graph with no
extension-range data would run `SCAN` with rule 1 disabled. Rejected —
it makes `SCAN` silently inert on exactly the graphs a user is most
likely to point it at, and inert means "consume to EOF", which is the
failure mode this whole line of work exists to fix. Erroring (S9) says
the same thing out loud.

**Make out-of-range unknowns veto under `SCORE`.** Rejected as N2: it
breaks forward compatibility, which is the ordinary case for a corpus
database a version behind the blob it is scoring.

**Put extension ranges in the alphabet Σ alongside `label`.** Does not
typecheck against the problem: Σ is an alphabet of edge symbols and an
extension range belongs to a state. The initial partition is both
correct and cheaper (S7).

**Let `SCAN` stop the walk at the first termination.** Would make
multi-root scans silently wrong (S13). Harmless for protoscan, which has
one or two roots, and wrong for anything else — so not worth the trap.

**Recover from a veto by replaying the walk over the truncated prefix.**
An alternative to N6's snapshot that needs no walker bookkeeping at all:
record only the last depth-0 boundary offset (a single shared `usize`)
and let the caller re-run the walk over `[start, boundary)`. The replay
is byte-identical to the vetoed run up to the cut, so it cannot itself
veto. Rejected in favor of the snapshot, for when N6 is taken up: N
roots vetoing at N different offsets need N replays, and it pushes
orchestration into every caller instead of keeping the answer inside the
walk.

**Derive termination from the ten `*Options` type names, hardcoded.**
Avoids the whole pipeline change. Rejected: it would have silently
missed `FeatureSet` and the editions-era additions, and it puts a schema
fact in code rather than in the schema.

## Test plan

1. `reproto: test_collect_scoring_messages_shared` — the factored
   collector produces byte-identical YAML for both callers against
   existing fixtures (S1).
2. `reproto: test_emit_extension_ranges` — a fixture with
   `extensions 1000 to max` emits `ext_ranges: [[1000, 536870911]]` with
   the flag, and no `ext_ranges` key without it (S2, S3).
3. `load: test_ext_ranges_parsed`,
   `test_message_eq_distinguishes_ext_ranges` — S4.
4. `graph: test_ext_ranges_interned_and_deduped` — two messages with the
   same canonical set share an index; `1000 to max` and
   `1000 to 536870911` intern identically (S3, S5).
4b. `reproto: test_ext_ranges_canonical_form` — the uniqueness property
   of S3, driven from spellings that differ only in form: adjacent
   clauses (`1000 to 1999; 2000 to 2999`), out-of-order clauses,
   overlapping clauses and a single-number clause (`10000`) must all
   reduce to the one canonical list their field-number set determines.
5. `hopcroft: test_extensible_and_closed_do_not_merge` — two messages
   with identical transitions, one extensible, land in different states
   (S7).
6. `serial: test_version_3_roundtrip`,
   `test_v2_rejected_with_rebuild_message` — S8, S10.
7. `walk: test_scan_requires_extension_ranges` — `Scan` against a graph
   with `has_extension_ranges == false` errors (S9).
8. `walk: test_scan_terminates_on_repeated_singular` — a
   `FileDescriptorSet` prefix; the FDP root terminates at the second
   field 1. Parameterized over `optional` and `required` (S12 rule 2).
9. `walk: test_scan_terminates_on_closed_state_unknown` — an undeclared
   field number in a state with an empty range set terminates (S12
   rule 1).
10. `walk: test_scan_termination_offset_is_the_tag` — the reported offset
    is the first byte of the terminating tag, not a position after it
    (S13). A one-byte error here is invisible to every other test in this
    list and fatal to protoscan.
11. `walk: test_scan_cardinality_applied_at_termination` — a schema whose
    `required` field appears only *after* the boundary: the terminating
    root takes the `mismatches` for the missing required, proving
    cardinality runs at the termination point rather than at EOF (S13).
12. `walk: test_scan_roots_terminate_independently` — two roots whose
    S12 rules fire at different offsets are both scored correctly in a
    single pass (S13).
13. `walk: test_scan_does_not_terminate_on_custom_option` — an FDP with
    an extension at field ≥ 1000 inside `MethodOptions` runs to the end
    and scores no unknowns (S12 rule 1, S15); its option carries
    non-UTF-8 bytes and does not veto (S16).
14. `walk: test_score_policy_output_unchanged` — `EntryScore` under
    `Score` matches a recorded baseline on the corpus, with `termination`
    always `pb.len()` (G4).
15. `prototext` binary end-to-end — the CLI's shipped scoring behavior
    is unchanged, driven through the binary rather than `score_all`
    (S11).
16. Corpus: all 7 771 FDPs in `googleapis.desc`, scored under `Scan`
    against a `descriptor.proto`-only graph, terminate exactly at their
    record boundary.

## Open questions

None outstanding.
