<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0175 — prototext-graph + reproto: accept both packed and expanded repeated scalars

Status: implemented
Implemented in: 2026-07-26
App: prototext-graph, reproto
Refs: docs/scoring-flaws.md (C7),
      docs/protolens/rendering-worklist.md (W30),
      docs/specs/0045-reproto-emit-graph.md (§ kind mapping)

## Background

Every repeated scalar field has **two** legal wire encodings, in both
proto2 and proto3:

- **expanded** — one tag per element, each carrying the element's own wire
  type (0, 1 or 5);
- **packed** — one LEN tag whose payload is the elements back to back with
  no tags.

The `[packed = ...]` option, and proto3's default, control only what a
*writer* produces. A **reader must accept both**, unconditionally: that is
what makes it safe to flip the option on a deployed schema. `score_all`
is a reader.

It rejects one of the two, and which one depends on the schema.

### The expanded-declared field, packed on the wire

A repeated `int32` with `is_packed == False` (proto2's default, or an
explicit `[packed = false]`) reaches the compiled graph as a plain `int32`
leaf, whose `NodeEntry.wire_type` is the internal discriminant `9` and
whose protobuf wire type `node_wire_type` reports as `0`
(`walk.rs:568-588`). Nothing records that the field is repeated *at the
leaf* — and the verdict loop compares wire types and nothing else
(`walk.rs:885-899`):

```rust
let expected_wt = node_wire_type(ws.graph, tr.child_state_id) as u32;
if wire_type == expected_wt {
    Verdict::Found(tr.child_state_id, tr.label)
} else {
    Verdict::Mismatch
}
```

A packed encoding arrives as wire type 2, so this returns
`Verdict::Mismatch`, and the mismatch loop (`walk.rs:904-916`) vetoes the
candidate outright. Veto is absorbing: one packed field anywhere in the
blob eliminates the correct FQDN and hands the win to some structurally
similar alternative.

### The packed-declared field, expanded on the wire

The mirror case fails for a different and worse reason. `reproto` collapses
every packed repeated scalar to a single kind, discarding the element type
(`phases.py:1850-1878`):

```python
if TYPE in (FD.TYPE_DOUBLE, FD.TYPE_FIXED64, FD.TYPE_SFIXED64):
    if field.is_packed:
        return 'LEN_PACKED', None, None
    return 'double', None, None
```

and the builder maps that kind onto the same leaf as `bytes`
(`graph.rs:95`):

```rust
ScoringKind::LenBytes | ScoringKind::LenPacked => LEAF_LEN,
```

So the graph knows only "wire type 2, not a string". An expanded encoding
arrives as wire type 0, 1 or 5 — a mismatch against 2 — and is vetoed.
And because the element type is gone, there is nothing left to validate an
expanded run *against* even if the mismatch were tolerated.

### Why this is the last of a family

C4, C5, C6 and C7 in `docs/scoring-flaws.md` are one failure repeated: a
*soft* signal — a non-canonical tag, a forward-compatible enum value, an
encoding choice — is treated as *proof* the candidate is wrong. Spec 0172
fixed the first three. The governing principle it established is stated in
that document's cross-cutting section:

> veto only for what the wire format makes impossible; score everything
> that is merely unlikely.

A packed encoding is not merely possible, it is *mandatory* to accept. C7
is the remaining violation, and the only one still unmitigated: spec 0172
could demote C6's enum veto to `non_canonical` as an interim, but there is
no interim for C7 — accepting the encoding requires knowing the element
type, which the pipeline currently throws away.

### The key observation

`is_packed` is the wrong question to ask the descriptor. Because both
encodings are always legal, the answer carries **no information a scorer
may act on**. Trading the element type for a bit that must then be ignored
is a net loss of exactly the information needed.

What the scorer does need is already in the compiled graph:

- **that the field is repeated** — `TransitionEntry.label`, `2` for
  repeated (`graph.rs:238-242`), read in the verdict loop as `tr.label`
  and already carried on `Verdict::Found`;
- **the element's wire type** — the leaf's, the moment `reproto` stops
  collapsing it.

So this costs no compiled-graph format change and no version bump.

## Goals

1. A repeated packable scalar field matches whichever of the two legal
   encodings the blob uses, in either direction, without a veto.
2. A packed payload is *validated*, not merely tolerated: an element run
   that cannot exist (a fixed-width run of the wrong length, a varint
   running past the payload) vetoes, which is where the discriminating
   power lost to the collapse comes back.
3. A packed element is scored by the same rules as the equivalent
   expanded one — the same 32-bit-gap veto, the same `non_canonical`
   penalties, the same range test.
4. The rule stays narrow: it must not degrade into "wire type 2 is always
   acceptable".

## Non-goals

- **No compiled-graph format change, no version bump.** Version stays 2.
  `label` and the element wire type are both already present; nothing about
  packedness needs storing.
- **No `packed` flag in the scoring-graph YAML.** It would be a field the
  consumer is required to ignore.
- **Open vs. closed enums (decision D-g)** stay as they are. The range
  check invoked per packed element behaves exactly as the expanded path's
  does today, under whatever `strict_ranges` gives it.
- **Packed strings, bytes, messages and groups.** Protobuf forbids
  packing them, so there is no encoding to accept.
- **`prototext`'s decoder and renderer.** They already read packed
  payloads (spec 0016, `NodeSpan::packed_record_start`). This spec is
  scoring only.
- **Float/double element scrutiny** (NaN, denormals). Out of scope; a
  fixed-width element is validated by length alone.

## Specification

### S1 — `reproto`: stop collapsing the element type

In `_scoring_kind` (`reproto/src/reproto/phases.py:1838-1879`), delete all
seven `if field.is_packed: return 'LEN_PACKED', None, None` branches. Each
type then returns its element kind unconditionally. `_field_label`
(`:1827-1835`) already emits `label: repeated` independently, so a repeated
packed `int32` becomes:

```yaml
- number: 7
  type: int32
  label: repeated
```

`ScoringKind::LenPacked` is then unreachable. Delete it — the variant
(`load.rs:78`), its YAML spelling (`load.rs:204`), and its arm in
`leaf_for_field` (`graph.rs:95`, which becomes plain
`ScoringKind::LenBytes => LEAF_LEN`).

No compatibility alias for the `LEN_PACKED` spelling. Scoring-graph YAML
is a generated artifact (`_phase_emit_scoring_graphs`), and no committed
fixture uses the kind — it appears only in `load.rs`, `graph.rs`,
`phases.py`, and specs 0042/0045.

Update `docs/specs/0045-reproto-emit-graph.md`'s kind-mapping table
(`:177-186`, `:302-315`, `:363-367`) to record that the mapping no longer
consults `is_packed`, and why.

**Hopcroft consequence.** Two packed fields of different element types
used to share `LEAF_LEN` and could therefore be merged; they now reach
distinct leaves and will not be. That is a correctness improvement — they
are not wire-equivalent — but it changes minimized partitions. See the test
plan.

### S2 — the verdict loop: one extra accepted wire type

A transition is **packable** iff both hold:

- `tr.label == 2` (repeated);
- its child's protobuf wire type is 0, 1 or 5.

The second clause is the whole of the narrowness requirement in Goal 4.
String and bytes leaves report wire type 2, messages 2, groups 3, so all of
them fall outside `{0, 1, 5}` and are excluded without a special case —
including an empty message state, which has no transitions but still
reports 2.

`Verdict` gains a variant carrying the element wire type, since the LEN arm
needs it to read the run:

```rust
Found(u32, u8),           // (child_state_id, label)  — unchanged
FoundPacked(u32, u8),     // (child_state_id, element wire type)
```

`Verdict` is `Copy` and lives inline on `ActiveEntry` (spec 0173), so this
is a one-word addition.

The verdict becomes:

| tag wire type | packable | verdict |
|---|---|---|
| `== expected_wt` | either | `Found` (unchanged) |
| `WT_LEN`, and `expected_wt != 2` | yes | `FoundPacked` |
| anything else | either | `Mismatch` (unchanged) |

The mismatch loop matches only `Verdict::Mismatch` and is untouched.

`FoundPacked` is produced only when the tag's wire type is LEN, so in the
VARINT, I64, I32 and group arms it cannot occur. Those arms must still
name it, and they treat it exactly as they treat `Mismatch`: a no-op. Not
`unreachable!()` — an unreachable-but-inert arm costs nothing, whereas a
panic reachable from wire-format-derived state is the shape of C1 through
C4.

### S3 — the LEN arm: read the run

In `WT_LEN` (`walk.rs:1094-1129`), `FoundPacked(child, elem_wt)` reads
`payload` as a run of elements.

**Validation** — a run that cannot exist is a veto, because the wire format
makes it impossible, not unlikely:

| `elem_wt` | requirement | on failure |
|---|---|---|
| 1 (I64) | `payload.len() % 8 == 0` | veto |
| 5 (I32) | `payload.len() % 4 == 0` | veto |
| 0 (varint) | varints decode to exactly the payload end | veto |

An **empty** payload is a legal packed field with zero elements: it matches
and is not vetoed, but it takes `non_canonical += 1`. No conformant writer
emits one — protoc omits the field entirely rather than encoding a
zero-length run — so it is legal-but-suspicious, and the scoring heuristic
deliberately penalizes suspicious serialization as much as erroneous
serialization. That is a voluntary posture, and it is the reason
`non_canonical` exists (confirmed 2026-07-26). The governing principle in
`docs/scoring-flaws.md` constrains **veto**, not penalty.

**What is *not* penalized: the encoding the schema's own `packed` option
does not name.** The same posture might seem to argue for keeping
`is_packed` purely as a penalty signal, at the cost of a `packed` flag in
the YAML and the format. It does not, because that encoding is not
suspicious: a proto3 repeated scalar is `is_packed == True` by default, yet
expanded output from it is routine — older writers, hand-rolled encoders,
and any implementation that simply does not pack. Penalizing it would
charge the *correct* schema for ordinary valid traffic, which is C6's
failure mode in penalty form. An empty packed run and a 5-byte negative are
suspicious because no conformant writer produces them; a legal choice
between two encodings that writers routinely make either way is not.

**Scoring** — `matches += 1` and one `record_occurrence`, exactly as for
any other single wire occurrence. *Not* one per element. A `match` counts
wire occurrences the schema explained, and a packed run is one occurrence;
awarding N would make the two legal encodings of identical data score
differently, and since every candidate sees the same bytes, the inflation
discriminates between nothing — it is an undeserved bias toward whichever
encoding the writer happened to pick. (Confirmed 2026-07-26.) The
discriminating power comes from the validation above instead.

**The validation is per payload, not per active entry.** It must be
hoisted out of the `for ae in active.iter_mut()` loop. It depends on
nothing but `elem_wt`, which has three possible values, and two of them
(1 and 5) are a length modulo rather than a scan — so the varint run is
scanned **at most once per LEN payload**, memoized across the entries that
share an `elem_wt`. Written as the natural inner check instead, this is
O(A × payload) per tag: the exact shape spec 0173 removed from the verdict
loop, reintroduced one arm lower down.

Cardinality is never at risk: only repeated fields are packable, and
`apply_cardinality_multi`'s label-2 arm (`walk.rs:645-690`) imposes no
upper bound.

The length-prefix overhang penalty at `walk.rs:1077-1083` already applies
to every active entry regardless of verdict and is unchanged.

### S4 — per-element varint checks, shared with the expanded path

The VARINT arm's value-checking block (`walk.rs:948-1022`) is where C5's
and C6's rules live: the 32-bit-gap veto for `int32`, the `non_canonical`
penalty for a negative written in the 5-byte form, and the `Range` leaf's
`[min, max]` test gated on `strict_ranges`.

A packed varint run must get the same treatment. Otherwise packed
encodings silently bypass checks the expanded encoding of the same values
receives — a fresh asymmetry between the two forms, of exactly the family
this spec exists to close.

Extract the block into a function over `(ws, ae, node, val, overhang)` and
call it from both arms: once in the VARINT arm, once per element in the
packed run. On the expanded path this must be **pure motion** — no
behavioral change whatsoever.

Unlike S3's validation this genuinely is per entry — the leaf, and so the
range, differs between candidates. It is therefore skipped entirely for
leaves with nothing to check: a `Uint64`, `I64` or `I32` leaf with
`range_idx == 0xFFFF` has no gap veto, no 5-byte-negative penalty and no
range, so its elements need no inspection at all. Only `int32`/`uint32`
(the 32-bit gap) and bool/enum (the range) walk the run.

## Performance

Three of the four steps are free or nearly so, and the one real cost is not
in this code.

- **S2** adds `tr.label == 2 && matches!(expected_wt, 0 | 1 | 5)` to the
  verdict loop — two comparisons on values already loaded.
- **S3** is O(payload) once per LEN payload if hoisted as required above,
  and O(1) for fixed-width elements. There is **no recursion**: packed
  elements are scalars, so no `score_message_multi` call, no depth
  increment, no `group_by_state`. The LEN arm's recursion is reached only
  through `child_pairs`, which only `is_message` children populate.
- **S4** is O(A × N) worst case, but that is the same order as the
  *expanded* encoding of the same values, and strictly less work per
  element: no tag to parse, and no `find_transition` binary search
  (O(log T) per entry per tag). Packed stays cheaper than expanded after
  this spec, as it was before it.

The cost that does exist is indirect and cannot be avoided: today a packed
field **vetoes**, which eliminates candidates and lets the walk finish
early. Accepting it keeps them alive, so A — the number of distinct states
in the active set, and the thing `score_all`'s cost actually scales with
(spec 0173) — stays wider for longer. The current speed on a blob with
packed fields is the speed of being wrong about it.

`prototext-graph/benches/score` parameterizes on A and can measure this
directly. Take a before/after reading on a blob containing packed fields;
a slowdown there is expected and is not a regression, whereas a slowdown
on the existing packed-free workloads means S3 was not hoisted.

### Measured, 2026-07-26

`benches/score` had no packable field, so `bench_packed_vs_expanded` was added
(field 3, `repeated uint64`, 256 elements, root count 64→4096). Before/after is
across the implementing commit, same bench file in both trees.

| roots | packed, before | packed, after | expanded, before | expanded, after |
|---|---|---|---|---|
| 64 | 3.14 µs | 9.34 µs | 539 µs | 541 µs |
| 256 | 12.8 µs | 40.1 µs | 2.56 ms | 2.62 ms |
| 1024 | 87.4 µs | 198 µs | 19.4 ms | 18.2 ms |
| 4096 | 437 µs | 949 µs | 102 ms | 100 ms |

**The A-widening cost is 2.2–3.0×, and it is the predicted one.** "Before" on
the packed blob is a veto at the first tag, i.e. an immediate exit, so this is
the price of not being wrong rather than a regression. In absolute terms it is
small: a packed field is *still* ~100× cheaper than the expanded encoding of
the same 256 values (949 µs vs 100 ms at 4096 roots), so S4's claim that packed
stays the cheaper encoding holds by two orders of magnitude — expanded pays a
tag parse and a `find_transition` per element per candidate, packed pays one
memoized run scan.

**Scaling.** Packed grows 4.3×/4.95×/4.8× per 4× roots (≈ A^1.13); expanded
4.8×/6.97×/5.5× (≈ A^1.26). Neither is O(A²), so spec 0173 holds, but the
expanded curve's bend at 256→1024 is unexplained and worth a look if varint-
heavy blobs ever matter — plausibly `EntryScore` working-set effects, not the
verdict loop.

**One real regression found and fixed: `check_varint_value` needs
`#[inline]`.** Extracting the shared per-element checks out of the `WT_VARINT`
`Found` arm (S4) put a five-argument, non-inlined call in the hottest per-value
path in the walk, costing **2.1× on a pure-varint blob** (expanded at 64 roots:
539 µs before → 1135 µs, restored to 541 µs by `#[inline]`; the same 2× at every
root count). This is the finding the measurement was worth taking for, and note
that the pre-existing mixed workload `score_all_by_root_count` was **identical**
across the change (464.5 → 464.9 µs at 64 roots, 63.48 → 63.36 ms at 4096) — it
spends most of its time on strings and submessages, so it hid the regression
entirely. A workload that isolates the arm being edited is what exposed it.

## Test plan

### `reproto`

- `tests/test_emit_scoring_graphs.py:238-252`: the two
  `assert fields[n]["type"] == "LEN_PACKED"` assertions become element-kind
  assertions (`"int32"`) plus `label == "repeated"`, over the existing
  `packed_proto2.proto` / `packed_proto3.proto` fixtures. The test's
  docstring — "default_int (no [packed] option) -> LEN_PACKED" — states the
  behavior being deliberately removed and must be rewritten to say why the
  option is no longer consulted.
- The full 240-case pytest suite stays green.

### `prototext-graph`

Both directions, over a hand-built graph in the style of
`score::tests::survivors_keep_their_own_verdict_after_a_mismatch_retain`
(no fixture needed):

| declared | on the wire | expected |
|---|---|---|
| `repeated uint64` | packed | match, no veto |
| `repeated uint64` | expanded | match, no veto — regression guard on the unchanged path |
| `repeated fixed64` | packed, 12-byte payload | **veto** (12 % 8 ≠ 0) |
| `repeated fixed32` | packed, 12-byte payload | match (12 % 4 == 0) |
| `repeated uint64` | packed, last varint's continuation bit set | **veto** |
| `repeated uint64` | packed, empty payload | match, no veto, `non_canonical += 1` |
| **`optional`** `uint64` | LEN tag | **veto**, unchanged — the guard on Goal 4 |
| `repeated string` | varint tag | **veto**, unchanged — packability must not run backwards |
| `repeated` enum, value outside range | packed | `non_canonical`, identical to what the expanded path gives under the default `strict_ranges: false` |

The last row is the one that pins S4: it must be asserted against the
*expanded* encoding of the same values, so the two are compared to each
other rather than to a hardcoded number.

Note that any test asserting an ordering of `state_id`s is flaky —
`graph::build` assigns node IDs by `HashMap` iteration order, so they differ
per process (see spec 0173's test plan).

### Regressions

- The 57-case `score::tests` corpus stays green unchanged. This is what
  certifies S4's extraction as pure motion.
- The 8 `hopcroft_suite` fixtures stay green. If any minimized partition
  changes, that is S1 splitting states that were wrongly merged: regenerate
  the fixture **and explain the diff in the commit message**, never accept
  it silently. A partition change is either a bug found or a bug
  introduced, and the two look identical in a regenerated golden file.
- `nix-build -A ci` green — it runs the reproto pytest suite, which is
  where S1's blast radius is.

## Follow-up

On implementation, mark C7 in `docs/scoring-flaws.md` and W30 in
`docs/protolens/rendering-worklist.md` as fixed by this spec, with the
date. That closes the C4/C5/C6/C7 family, and with it the veto-correctness
half of the `score_all` review.
