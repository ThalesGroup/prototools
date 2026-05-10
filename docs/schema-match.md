<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Schema Matching and Graph Deduplication Design

## Overview

Given a protobuf wire message of unknown schema, and a database of known
FileDescriptorProtos (FDPs), identify the FDP whose root message type best
explains the message — enabling schema-aware, human-readable display without
requiring the user to specify a schema upfront.

To make this feasible at scale (100,000+ FDPs), the FDPs are pre-compiled
into an efficient shared graph representation.  Structurally identical
subgraphs (across and within FDPs) are deduplicated to reduce memory
footprint.

The two problems are separable:

- **Part 1 — Matching**: given a wire message and the compiled graph, rank
  all FDP root nodes and return the best match (or a ranked candidate list).
- **Part 2 — Deduplication**: pre-process the FDP graph to minimize RAM by
  merging equivalent subgraphs, without changing the set of schemas or the
  outcome of matching.

### Prototype

The function `parse_message()` in `src/prototext/decode.py` is the prototype
for the single-schema walk.  It parses a binary protobuf against one
`Descriptor`, recursing into sub-messages only when the schema declares the
field as `MESSAGE` type, and falling back to wire-level representation when
no schema match is found.  The multi-schema matcher generalizes this to
propagate all candidate schemas simultaneously.

---

## Concepts and Terminology

**Wire-level**
: Refers to the raw binary encoding of a protobuf message, independent of
  any schema.  At the wire level, a field is identified only by its
  `(field_number, wire_type)` tag; no proto-level type names (e.g. `string`,
  `int32`) are known.  Wire types are: 0 = varint, 1 = 64-bit, 2 =
  length-delimited, 3 = group start, 4 = group end, 5 = 32-bit.

**Walk**
: A recursive traversal of a protobuf wire message, processing fields one
  by one and descending into sub-messages as directed by the candidate
  schemas.

**Candidate**
: A schema (FDP root message type) that has not yet been vetoed at the
  current point in the walk.  The set of candidates shrinks as the walk
  progresses and vetoes eliminate schemas with proto-level errors.

**State**
: A `(schema_id, message_type)` pair representing the position of one
  candidate within its schema graph at a given nesting level.  A schema with
  N message types contributes N states to the global state pool, but only
  one state per schema is active at any nesting level.

**Active set**
: The set of states currently participating in the walk at a given nesting
  level.  At the root, one state per candidate schema is active.  When the
  walk recurses into a sub-message, only the states that declared the field
  as a message type enter the recursion — the rest are temporarily suspended
  (their state is preserved on the call stack) and rejoin when the recursion
  returns.  The active set is not a global structure; it is the top frame of
  the call stack.

**Propagate**
: To carry the active set forward through the walk, updating it at each
  field.  In the algorithm description, "propagating" the active set means
  passing it as a parameter into each recursive call.

**Depth**
: The nesting level of the current position in the walk.  Depth 0 is the
  top-level message; depth 1 is inside the first level of sub-messages; and
  so on.  Depth determines which state each schema is in.

**Sparse (active set)**
: The active set is sparse when the number of active states is much smaller
  than the total number of states in the compiled graph.  At depth 0 the
  active set has ~100,000 entries out of potentially millions of states;
  it becomes sparser with each level of recursion.

**Transition table**
: A per-state mapping from wire tag `(field_number, wire_type)` to a
  `FieldInfo` record describing whether the field is a scalar or a
  sub-message, and if a sub-message, which child state to transition to.
  Compiled once from the FDP database; read-only at match time.

**StateID**
: A dense integer assigned to each state during compilation.  The active
  set is represented as a list of (StateID, back-reference list) pairs.

**Static back-references**
: The set of `(schema_id, message_type)` pairs compiled into a StateID
  during deduplication: the set of `(schema, type)` nodes that were merged
  into the same equivalence class.  This is a compile-time annotation on the
  StateID.  Its size is 1 for the common case (no merging); it exceeds 1 only
  when two or more schemas have structurally identical message types that
  Hopcroft has merged.

**Active back-references**
: For each entry in the active set at a given call-stack frame, the list of
  initial (root) schema IDs that led to this StateID being active at the
  current nesting level.  This is a runtime property of the walk, not a
  compile-time property of the StateID.  When the walk starts, each active
  set entry carries exactly one initial schema.  As the walk descends and
  schemas are vetoed, entries may shrink or be removed.  Active
  back-references are what determine which schema counters are updated at each
  field — **not** the static back-references, which may contain schemas that
  have already been vetoed or that did not contribute to this entry at all at
  the current depth.

**Veto**
: A definitive invalidation of a candidate schema, stronger than a mismatch.
  A veto is triggered by a proto-level error encountered while interpreting the
  wire message against that schema: a wire-type/proto-type conflict (e.g. the
  schema declares an `int32` but the wire type is length-delimited), an
  ENDGROUP tag (wire type 4) that does not match the expected group tag, or an
  invalid UTF-8 sequence when the schema declares a `string` field.  A vetoed
  schema is removed from its active-set entry immediately, and the removal is
  **propagated up through all ancestor call-stack frames** — the schema is
  eliminated from the active set at every nesting level.  It does not
  accumulate any further match or mismatch counts after the veto.

---

## Part 1 — Matching a Wire Message to a Schema

### The Matching Problem

A protobuf wire message `P` is a sequence of `(tag, value)` pairs at each
nesting level, where each tag encodes a `(field_number, wire_type)`.  A
schema `S` (an FDP root message type) defines expected tags at each nesting
level.

When the walk encounters a field with tag `T` and a given candidate schema
`S` is active, one of three outcomes applies, each carrying increasingly
strong negative evidence:

- **Match**: `T` is declared in `S`'s current message type and the wire
  type is compatible with the proto-type → `matches[S] += 1`.  Weak
  positive evidence: many schemas may declare common field numbers.
- **Unknown**: `T` is not declared in `S`'s current message type at all →
  `unknowns[S] += 1`.  Moderate negative evidence: the schema does not
  recognise this field.  Legitimately occurs due to unknown extensions or
  version skew (schema predates the field), but accumulation of unknowns
  suggests a wrong schema.
- **Mismatch / invalid**: `T` is declared in `S`'s current message type
  but the wire type conflicts with the proto-type, or a proto-level validity
  error is detected (invalid UTF-8 for a `string` field, ENDGROUP tag
  mismatch) → **veto** `S`.  The schema is definitively wrong.  It is
  immediately removed from all active-set entries in all ancestor call-stack
  frames and accumulates no further counts.

All three counters are tracked separately per schema.  The rank metric and
ranking strategy are derived from them at reporting time (see Open
Questions).

This applies to **every** wire type:

| Wire type | Outcome for a matching schema | Veto condition |
|---|---|---|
| 0, 1, 5 | Match or unknown. No recursion. | Schema declares the field with an incompatible proto-type (e.g. schema says `bytes` but wire type is 0). |
| 2 (length-delimited) | Match or unknown. Recurse if at least one active state declares it as a message type; only those states enter the recursion. | Schema declares the field as `string` and the content is not valid UTF-8. |
| 3 (group start) | Match or unknown. Recurse if at least one active state declares it as a group; only those states enter the recursion. | Schema declares the field with an incompatible proto-type. Groups require careful handling (see note below). |
| 4 (group end) | Terminates a group recursion. | ENDGROUP tag does not match the tag of the enclosing group start for a given schema → veto that schema. |

**Note on groups (wire type 3/4)**: group end tags may be missing or
mismatched in malformed data, so the group boundary cannot be determined at
the onset of recursion.  This requires careful handling in the algorithm
(see `parse_message()` in `src/prototext/decode.py` for the reference
implementation) but is not a fundamental obstacle.

**A schema that does not recurse** into a length-delimited or group field
is temporarily suspended for the duration of that recursion: it does not
receive any updates for fields inside the sub-message.  It pays exactly one
outcome (match, unknown, or veto) at the field that triggered recursion,
then rejoins the active set when the recursion returns (its state is
preserved on the call stack).  This means that deep matches are a stronger
signal than shallow ones: a schema that recurses into a sub-message and
matches fields there accumulates more score than one that stops at the
boundary.

### Taxonomy of Validity and Canonicality

The walk is schema-guided: whether a length-delimited blob is entered as a
nested message or treated as opaque bytes depends entirely on whether the
candidate schema declares the field as `message` type.  All observations
inside a sub-message are therefore conditional on the schema having directed
recursion into it.  Invalidity found inside a sub-message vetoes only the
schemas **active at that nesting level** (those that recursed in); schemas
that treated the blob as `bytes` or `string` are suspended on the stack and
unaffected.

The taxonomy has two axes: **validity** (invalid vs. non-canonical) and
**semantic depth** (wire-level vs. proto-type-level).  Wire-level issues are
detectable from the binary encoding alone — no proto-type knowledge is
needed.  Proto-type-level issues require knowing the proto-type the schema
assigns to the field.

#### Wire invalids

Binary format is broken at the current nesting level.  Every candidate schema
active at this depth is vetoed.  The wire-level invalidity is independent of
proto-type assignment but is still observed only by schemas that recursed here.

| Code ref | Trigger | Causes |
|---|---|---|
| `INVALID_TAG_TYPE` | Tag varint has wire type 6 or 7 | Wire types 6 and 7 are permanently reserved.  The remainder of the buffer after the bad tag is uninterpretable; parsing of this sub-message terminates. |
| `INVALID_VARINT` | Scalar varint does not terminate | The varint for a field value never finds a byte with MSB=0 before end of (sub-)buffer; also triggered when the accumulated value reaches ≥ 2⁶⁴ (see note below).  Causes: truncated buffer, corrupted bytes. |
| `INVALID_BYTES_LENGTH` | Length-prefix varint does not terminate | Same condition as `INVALID_VARINT` but for the length prefix of a length-delimited field.  Causes: truncated buffer, corrupted prefix. |
| `INVALID_FIXED64` | Fewer than 8 bytes remain | A wire-type-1 (64-bit) field is at the end of the buffer with fewer than 8 bytes remaining.  Causes: truncated buffer, wrong wire type for the actual content. |
| `INVALID_FIXED32` | Fewer than 4 bytes remain | A wire-type-5 (32-bit) field is at the end of the buffer with fewer than 4 bytes remaining.  Causes: same as `INVALID_FIXED64`. |
| `TRUNCATED_BYTES` | Content shorter than declared length | The length prefix of a length-delimited field is valid, but `pos + length > buflen`.  Causes: truncated buffer, length field corrupted to a larger value. |
| `open_ended_group` | GROUP START with no GROUP END | A wire-type-3 (group start) field opens a group recursion, but the buffer ends before a wire-type-4 (group end) is found.  Causes: truncated buffer. |
| `INVALID_GROUP_END` | GROUP END outside any group | A wire-type-4 (group end) tag is found at a nesting level where no GROUP is open (`my_group is None`).  Parsing of the sub-message terminates.  Causes: misaligned recursion (schema directed entry into a blob that isn't a message), corrupted data. |
| `mismatched_group_end` | GROUP END field number ≠ GROUP START field number | Group recursion always happens when wire type 3 is encountered, regardless of schema.  If the returned GROUP END tag carries a different field number from the GROUP START, the group structure is internally inconsistent.  Unlike the other wire invalids, this does not terminate parsing — the nested content is still recorded — but it signals structural corruption. |

**Note on too-big varints**: the `Varint` parser sets `varint_gar` when
the accumulated value reaches ≥ 2⁶⁴ (line 159 of `lib/varint.py`: `if
shift > 1024 or v >= (1<<64): pos = buflen`), treating it identically to a
truncated varint.  A too-big varint therefore falls under `INVALID_VARINT` or
`INVALID_BYTES_LENGTH`, not under a proto-type check.  No additional
handling is needed.

#### Wire non-canonicals

The (sub-)buffer is valid and fully parseable; the encoding is legal but uses
more bytes than needed.  These are observable without proto-type knowledge.
They do not discriminate between candidate schemas (all schemas active at
this depth observe the same non-canonical encoding) but may reveal something
about the encoder.

| Code ref | Trigger | Causes |
|---|---|---|
| `tag_overhang_count`, `end_tag_overhang_count` | Tag varint over-encoded | The tag varint (encoding `field_number << 3 \| wire_type`) uses more bytes than the minimum, with trailing zero-continuation bytes.  Also applies to group end tags. |
| `length_overhang_count` | Length prefix over-encoded | The length prefix varint of a length-delimited field uses more bytes than needed (e.g. length 100 encoded as two bytes instead of one). |
| `value_overhang_count` | Scalar varint over-encoded | A varint value field uses more bytes than needed (e.g. value 1 encoded as `\x81\x80\x00` instead of `\x01`). |
| `records_overhung_count` | Packed element varints over-encoded | Individual varint elements within a packed repeated field each use more bytes than needed.  Only appears for varint-encoded packed types (INT32, INT64, UINT32, UINT64, SINT32, SINT64, BOOL, ENUM). |
| `tag_is_out_of_range`, `end_tag_is_out_of_range` | Field number outside valid range | Field number = 0 (field numbers are 1-indexed); field number in [19000, 19999] (reserved for protobuf implementation use); field number > 2²⁹ − 1 = 536870911 (maximum allowed by spec). |

#### Proto-type invalids

The field number is declared by the schema (so the field was matched at the
field-number level), but the wire content is incompatible with the proto-type.
Only the schemas that declared the field are vetoed; schemas that do not
declare the field treat it as unknown and are unaffected.

**This is the most informative category for schema discrimination**: the
schema was already "credited" for knowing the field, yet the wire content
contradicts its declared proto-type.

**Wire-type / proto-type mismatch** — the wire type present in the message
is incompatible with the wire type that the schema-declared proto-type
requires:

| Schema proto-type | Expected wire type | All other wire types → mismatch |
|---|---|---|
| INT32, INT64, UINT32, UINT64, SINT32, SINT64, BOOL, ENUM | 0 (varint) | wire types 1, 2, 3, 5 |
| FIXED64, SFIXED64, DOUBLE | 1 (64-bit) | wire types 0, 2, 3, 5 |
| STRING, BYTES, MESSAGE, packed-repeated | 2 (length-delimited) | wire types 0, 1, 3, 5 |
| GROUP | 3 (group start) | wire types 0, 1, 2, 5 |
| FIXED32, SFIXED32, FLOAT | 5 (32-bit) | wire types 0, 1, 2, 3 |

When the wire type IS the expected varint (type 0) but the value is out of
range for the declared proto-type, the mismatch is also a proto-type invalid
(`proto2_has_type_mismatch` flag set):

| Proto-type | Out-of-range condition |
|---|---|
| BOOL | Value > 1 (bool must be exactly 0 or 1) |
| UINT32 | Value ≥ 2³² |
| UINT64 | Value ≥ 2⁶⁴ (not reachable in practice: caught as `varint_gar` first) |
| INT32, ENUM | Value not in [0, 2³¹−1] (non-negative canonical), [2³¹, 2³²−1] (5-byte truncated negative), or [2⁶⁴−2³¹, 2⁶⁴−1] (10-byte sign-extended negative) |
| SINT32 | Value ≥ 2³² (after zigzag decode, result must fit int32) |
| INT64 | Value ≥ 2⁶⁴ (not reachable in practice: caught as `varint_gar` first) |
| SINT64 | Value ≥ 2⁶⁴ (same) |
| DOUBLE, FIXED64, SFIXED64, FLOAT, FIXED32, SFIXED32, STRING, BYTES, MESSAGE, GROUP | Any varint value (these proto-types never use wire type 0; any varint is a wire-type mismatch) |

**`INVALID_STRING`** — wire type 2 and schema declares `string`, but the
bytes are not valid UTF-8.  The protobuf spec requires `string` fields to
carry valid UTF-8.  This is a common and highly informative case in practice:
many length-delimited blobs contain binary data that is not valid UTF-8, so
any schema that declares a given field as `string` will be vetoed whenever
that field carries non-text bytes.

**`INVALID_PACKED_RECORDS`** — wire type 2 and schema declares a packed
repeated field (`[packed=true]`), but the content does not decode correctly
as the declared element type.  Causes: byte count not a multiple of the
element size (for fixed-width types: DOUBLE, FIXED64, SFIXED64 need multiples
of 8; FLOAT, FIXED32, SFIXED32 need multiples of 4); or a varint element
within the packed blob terminates with a value out of range for the element
type (same OOR conditions as the scalar varint table above).

#### Proto-type non-canonicals

Valid and recoverable, but non-standard.  Detectable only with schema
knowledge.  Not a veto.

| Code ref | Trigger | Explanation |
|---|---|---|
| `proto2_neg_int32_truncated` | Negative INT32 or ENUM in 5-byte form | A negative `int32` or `enum` value encoded using only the lower 32 bits (5-byte truncated form) rather than the canonical 10-byte sign-extended form.  Some proto2 encoders produced this.  Detectable only when the schema declares the field as `INT32` or `ENUM`.  The value is correctly recovered; the non-canonical encoding is a weak positive signal for schemas that declare the field as one of those types. |

---

After the walk, each non-vetoed schema `S` has two counters (vetoed schemas
are excluded from ranking entirely):

```
matches[S]  // fields in P declared by S with a compatible wire type
unknowns[S] // fields in P not declared by S at the relevant nesting level
```

These are the primary outputs of the walk.  Any ranking metric is derived
from them at reporting time.  The simplest is:

```
Rank(P, S) = matches[S] - unknowns[S]
```

The schema with the highest Rank is the best match.  Whether this simple
difference captures the asymmetry between matches and unknowns adequately,
or whether a more nuanced metric is appropriate, is discussed in Open
Questions.

### User-Facing Behavior

- **Best match**: return the schema with the highest Rank — a single result,
  analogous to "I'm feeling lucky."
- **Candidate list**: return the top-k schemas by Rank for ambiguous cases
  or user exploration.  Multiple ranking metrics can be computed from the
  same walk (see Open Questions) and presented together to help the user
  distinguish candidates.
- **Ties and wrong matches**: two schemas with identical wire-level structure
  will score identically.  The tool must surface this uncertainty rather than
  silently committing to one result.

### The Walk Algorithm

The walk is **schema-guided and interleaved**: there is no separate
schema-independent parse phase.  The wire message and the candidate schemas
are traversed together in a single recursive pass, exactly as in
`parse_message()`.

```
active   : list of (StateID, [schema_id])  // StateID + active back-references
matches  : array[schema_id] of i64         // match counter, one per schema
unknowns : array[schema_id] of i64         // unknown counter, one per schema
vetoed   : set of schema_id               // schemas definitively invalidated
```

`matches`, `unknowns`, and `vetoed` are global across the entire walk.
`active` is local to each recursive call — it is the top frame of the call
stack.  Each entry `(s, refs)` carries the StateID and the list of
still-active initial schema IDs that are currently routing through `s` at
this nesting level.

The initial active set has one entry per candidate schema:
`(root_state_id[schema], [schema])`, with each back-reference list of size
one.

At each field with tag `T`:

1. **Score step**: for each `(s, refs)` in `active`, look up `T` in
   `transition_table[s]`:
   - Found and wire type compatible → for each `r` in `refs`:
     `matches[r] += 1`
   - Found but wire type incompatible, or validity error (invalid UTF-8,
     etc.) → for each `r` in `refs`: add `r` to `vetoed`; remove `r` from
     `refs`.  If `refs` becomes empty, remove the entry from `active`.
   - Not found → for each `r` in `refs`: `unknowns[r] += 1`

2. **Recurse step** (wire type 2 or 3 only): build the child active set by
   collecting, for each `(s, refs)` in `active`, the non-vetoed schemas in
   `refs` that declare this field as a message or group type, mapping each to
   the child StateID via `transition_table[s][T].child_state_id`.  Entries
   for the same child StateID are merged (their back-reference lists
   concatenated).  If the child active set is non-empty, recurse; on return,
   restore `active` from the stack and remove any newly vetoed schemas from
   each entry's back-reference list (propagation).

3. After the full walk, exclude vetoed schemas and derive ranking metrics
   from `matches` and `unknowns` for the remaining candidates.

Note that all three outcomes are evaluated for **every** active candidate at
every field, regardless of whether the field causes recursion.  Candidates
that do not enter a recursion are not updated inside it — they wait on the
stack.  Any veto discovered inside a recursion is propagated upward: the
schema is removed from back-reference lists in all ancestor frames before
those frames resume processing.

### Scoring with Back-References

The walk uses **active back-references** (not static back-references) to
determine which schema counters to update at each field.  The distinction
matters:

- **Static back-references** are a compile-time annotation on each StateID:
  the set of `(schema_id, message_type)` nodes merged into it by Hopcroft.
  Their size is 1 for most states (no merging occurred).
- **Active back-references** are the per-entry, runtime lists in the active
  set: the set of initial schema IDs that are routing through a given StateID
  at the current nesting level.  These lists are constructed during the walk
  and shrink as schemas are vetoed.

The two lists are **not** the same.  A StateID Q with static back-refs
`{(schema_A, PersonMessage), (schema_B, EmployeeMessage)}` will, at runtime,
only update the schemas that are currently routing through it — which may be
`{schema_A}`, `{schema_B}`, or `{schema_A, schema_B}`, depending on what
has been vetoed above.  Using static back-refs for scoring would
incorrectly update already-vetoed schemas.

The counter-update cost at each field is proportional to the total number of
active back-references across all active-set entries, not just the number of
StateIDs.  Deduplication reduces transition-table lookups and active-list
entry count, but does not reduce counter increments when many schemas are
routing through the same state.

### Complexity Analysis

#### Active set decay model

As the walk progresses through fields at a given depth, the active set
shrinks: a field with tag `T` eliminates every candidate whose current state
does not declare `T`.  Model this as a geometric decay:

```
A(d, k) ≈ N × p^k × q^d
```

where:
- `N` = initial candidate count (~100,000)
- `k` = number of fields processed at depth 0 before first recursion
- `d` = current nesting depth
- `p` = fraction of candidates surviving each field at depth 0
  (1 − per-field elimination rate)
- `q` = additional fraction surviving the transition into each recursion
  level (schemas that do not recurse into the sub-message are suspended, not
  eliminated, but the active set passed into the recursion is smaller)

`p` and `q` depend on the schema corpus and the specific message being
matched.  In a corpus where schemas are structurally similar near the root
(e.g. all share a common prologue of fields), `p` is close to 1 and the
active set decays slowly.  As schemas diverge at deeper levels, `q` becomes
small and the recursion quickly narrows to a handful of candidates.

#### Total walk cost

```
Total cost ≈ N × Σ_{d=0}^{D} p^{k_d} × q^d × F(d)
```

where `F(d)` is the average number of fields at depth `d` and `k_d` is the
number of fields processed at that depth before descending further.

For concrete intuition: with `N = 100,000`, `p = 0.9` (slow decay at the
root — schemas share many top-level fields), `q = 0.1` (only 10% of active
schemas recurse into any given sub-message), and `F = 10` fields per level:

| Depth | Active states |
|---|---|
| 0 | 100,000 |
| 1 | ~34,900  (`100,000 × 0.9^5 × 0.1 × 10`) ... per field of the recursion |
| 2 | ~1,220 |
| 3 | ~43 |
| 4 | < 2 |

The total work is dominated by depth 0, and the recursion cost is small by
comparison.  This holds even with pessimistic parameters, because `q < 1`
ensures geometric attenuation with depth.

**Optional pruning**: if `matches[schema] - unknowns[schema] +
remaining_fields_in_message < current_best_rank`, that schema can be removed
from the active set early (it cannot catch up), accelerating convergence
further.

---

## Part 2 — Graph Representation and Deduplication

### Graph Model

The FDP database is represented as a single directed labeled graph
`G = (V, E, λ)`:

- **Nodes** `V`: all message types across all FDPs, plus a small set of
  canonical leaf nodes (one per wire type).
- **Edges** `E`: fields of a message type.
- **Labels** `λ(e)`: the wire-level tag `(field_number, wire_type)`.
- **Roots**: one root node per FDP (the FDP's top-level message type).

Each node has zero or more outgoing edges.  The graph is **deterministic**:
for a given node and tag, there is at most one outgoing edge.  Recursive
message types produce **cycles**; these are common in real schemas and must
be handled correctly.

### Equivalence Relation

Two nodes `u` and `v` are **equivalent** (`u ~ v`) if they are
bisimulation-equivalent: they generate the same set of tag-path strings.
Formally, `u ~ v` iff for every tag `a`:

- if `u` has an outgoing edge `a → u'`, then `v` has an outgoing edge
  `a → v'` with `u' ~ v'`, and vice versa.

This coinductive (fixpoint) definition handles cycles correctly.

**Wire-level semantics**: equivalence is purely structural — `string` and
`bytes` at the same tag are wire-indistinguishable and will be merged.

### Connection to Part 1

The nodes of the quotient graph `G/~` correspond exactly to the StateIDs of
Part 1.  Deduplication is the pre-compilation step for the matcher:
equivalent `(schema, message_type)` nodes collapse to a single StateID with
a shared transition table entry.

**Static back-references**: each StateID is annotated at compile time with
the set of `(schema_id, message_type)` nodes that were merged into it by
Hopcroft.  This serves one compile-time purpose: initializing the root active
set — one entry `(root_state_id[schema], [schema])` per candidate schema.

Score updates during the walk use **active back-references** — the runtime
per-entry lists that track which initial schemas are currently routing
through a given StateID (see §The Walk Algorithm).  This is necessary because
vetoes can eliminate some schemas from a StateID's contributors before all
fields are processed; static back-references would over-count in that case.

**Example**: StateID `Q` has static back-refs `{(schema_A, PersonMessage),
(schema_B, EmployeeMessage)}`.  The transition table is consulted once per
field.  At the point of the match, if both schema_A and schema_B are still
active (non-vetoed), both `matches[schema_A]` and `matches[schema_B]` are
incremented via the active back-references.  If schema_A was vetoed earlier,
only `matches[schema_B]` is incremented.

### Deduplication Algorithm

The quotient graph `G/~` is computed using **Hopcroft's partition refinement**:

1. **Initialize**: partition `V` into blocks by outgoing-edge signature (set
   of tags).  Leaf nodes are pre-partitioned by wire type.
2. **Refine**: repeatedly split blocks — two nodes `u`, `v` are separated if,
   for some tag `a`, their `a`-successors fall in different blocks.
3. **Iterate** until stable.  The stable partition is the bisimulation
   equivalence `~`.
4. **Build quotient**: replace each class with a representative; redirect
   edges; annotate with back-reference sets.

**Cycles**: handled correctly by the fixpoint argument.  Two mutually
recursive schemas `a → b → a` and `c → d → c` (matching tags) merge to
`{a,c} → {b,d} → {a,c}`.

**Complexity**: O(|Σ| · |E| log |V|) where |Σ| is the number of distinct
tags.

### What Deduplication Achieves — and Its Limits

Deduplication is a **RAM optimization**, not a walk-cost optimization.

It reduces the number of distinct nodes and transition table entries.  It
does not reduce the number of schemas or root nodes.  The score-update cost
is determined by total back-reference count, not StateID count — if K schemas
share a state, one transition-table lookup replaces K, but K score increments
still occur.

**Where deduplication helps most**: deep shared utility types (e.g.
`Timestamp`, `LatLng`) appear across many FDPs with high merge ratios and
small back-reference sets.  Both RAM and active-list iteration benefit.

**Where deduplication has little effect**: at depth 0–1, schemas are
intentionally distinct.  The active set at the root has ~100,000 entries
regardless of deduplication; this shallow region dominates walk cost and is
unaffected.

Deduplication compresses the *tail* of the schema tree.  The *head* (shallow
depths, high active-state count) remains the performance bottleneck.

### Concrete Example

**schema_a.proto**
```proto
message Address { string street = 1; string city = 2; }
message Person  { string name = 1;   Address addr = 2; }
```

**schema_b.proto**
```proto
message Location { string line1 = 1; string line2 = 2; }
message Employee { string name = 1;  Location loc  = 2; }
```

After deduplication:

```
[Person, Employee] --[1,wt=2]--> L2
                   --[2,wt=2]--> [Address, Location] --[1,wt=2]--> L2
                                                      --[2,wt=2]--> L2
```

Two root StateIDs remain (one per schema) with identical transition tables.
The `[Address, Location]` node has static back-ref set of size 2; a match
there increments both `matches[schema_A]` and `matches[schema_B]` via the
active back-references (assuming neither has been vetoed).

---

## Scale and Performance Considerations

### Numbers

- Schemas: ~100,000 FDPs
- Message types per schema: 100–1,000
- Total states before deduplication: ~10M–100M
- Total states after deduplication: unknown; expected to be significantly
  smaller for deep sub-messages, but close to 100,000 at the root level
- Active states at root: one per schema = ~100,000
- Active states at depth d: shrinks rapidly; most schemas become incompatible
  after the first few nesting levels

### Memory Layout (Compiled Once)

```
transition_table[state_id][tag]  →  FieldInfo { is_message, child_state_id }
static_back_refs[state_id]       →  list of (schema_id, message_type)  // compile-time; usually size 1
root_state[schema_id]            →  state_id
matches[schema_id]               →  i64  (per-message scratch; reset per message)
unknowns[schema_id]              →  i64  (per-message scratch; reset per message)
vetoed                           →  set of schema_id  (per-message; reset per message)
```

Transition tables and static back-reference sets are compiled once and reused
across all messages.  The `matches`, `unknowns`, `vetoed` arrays and the
active-set list (with its per-entry back-reference lists) are the only
per-message state.

### Active State Representation

Each active-set entry is a **(StateID, back-reference list)** pair.  The
back-reference list carries the initial schema IDs routing through the
StateID at the current level; it starts at size 1 per schema and shrinks
only when schemas are vetoed.  The number of entries in the active set is
bounded by the number of distinct StateIDs in use at this depth; the
back-reference overhead is proportional to the number of non-vetoed schemas,
not to the number of entries.

The initial entry count (~100,000) and its rapid decay with depth make a
sparse list more efficient than a dense bitset over the full state space.

---

## Non-Goals

- **Semantic type resolution**: `string` and `bytes` at the same tag are
  wire-indistinguishable and scored identically.
- **Optimal match guarantee**: Rank is a heuristic; a higher-ranked schema
  may not be semantically correct.
- **Incremental deduplication**: adding new FDPs requires rerunning Hopcroft.
  Incremental algorithms exist but are not planned initially.
- **Walk-cost reduction via deduplication**: deduplication does not reduce
  work at shallow depths, where most walk cost lives.

---

## Open Questions / Future Work

- **Ranking strategy**: the walk produces `matches[S]` and `unknowns[S]`
  counters for non-vetoed schemas (vetoed schemas are excluded before
  ranking).  The three evidence levels — match (weak positive), unknown
  (moderate negative), veto (definitive) — are ordered by strength, but the
  right way to combine the first two into a rank is not obvious.  Strategies
  worth evaluating: (a) sort primarily by `unknowns` ascending, break ties
  by `matches` descending (respects the asymmetry); (b) apply a hard
  threshold — discard candidates with `unknowns > k` before ranking by
  matches; (c) weighted combination with unknowns penalised more heavily.
  Additional metrics derived from the same counters — e.g.
  `matches / (matches + unknowns)` (precision), `matches /
  schema_field_count` (coverage) — capture schema-size effects.  When
  returning a candidate list, presenting all raw counters alongside any
  derived metric helps the user interpret close results.  The right strategy
  should be validated against labelled examples once a corpus is available.
- **Deduplication ratio on production data**: the reduction in state count is
  not yet quantified.  Measuring this on a real FDP corpus is a prerequisite
  for determining whether deduplication is worth its implementation cost.
- **Shallow-level performance**: depth 0–2 with ~100,000 active states
  dominates walk cost and is unaffected by deduplication.  An inverted index
  from tag to candidate subset could prune the active set cheaply before the
  full walk begins.
- **Decay parameter estimation**: the complexity model above uses `p` and
  `q` as free parameters.  Estimating these empirically from a production
  FDP corpus would allow concrete throughput predictions and inform
  implementation choices (e.g. when to apply early pruning).
- **FDP-level partial order as a scoring supplement**: pairwise FDP content
  comparison (one FDP is a structural subset of another) could provide
  additional signal.  Computationally expensive at scale; deferred.
