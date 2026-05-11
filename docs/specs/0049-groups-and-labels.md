<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0049 — GROUP recursion and label-based cardinality checks

**Status:** implemented
**Implemented in:** 2026-05-11
**App:** score-graph

---

## Background

Specs 0042 and 0048 explicitly deferred two scoring features:

- **GROUP recursion**: fields carried on `WT_START_GROUP` / `WT_END_GROUP`
  wire pairs are currently skipped entirely by `parse_group_blind`.  A schema-declared
  group is counted as one match (or unknown), but its interior fields
  contribute nothing — no matches, no unknowns, no veto checks.

- **Label-based cardinality checks**: the compiled graph stores a `label` byte
  on every `TransitionEntry` (0=optional, 1=required, 2=repeated), but the
  walk never reads it.  Required fields that are absent generate no signal;
  optional fields that appear more than once are not penalised.

Both gaps reduce scoring precision.  In a corpus where many types share the
same set of field numbers and wire types, cardinality and group content are
sometimes the only discriminators.  This spec closes both gaps.

---

## Goals

1. **GROUP recursion** — replace `parse_group_blind` with a proper recursive descent
   in both the single-entry walk (`score_message`) and the multi-entry walk
   (`score_message_multi`).  Fields inside a group are scored exactly like
   fields inside a LEN-delimited sub-message.

2. **Label-based cardinality checks** — at the end of each message frame
   (including group frames), apply per-field occurrence counts:

   | Label    | 0 occurrences       | 1 occurrence | >1 occurrences          |
   |----------|---------------------|--------------|-------------------------|
   | Optional | ok                  | ok           | `non_canonical += 1` per extra |
   | Required | `mismatches += 1`   | ok           | `non_canonical += 1` per extra |
   | Repeated | ok                  | ok           | ok                      |

3. **`mismatches` counter** — add a new counter to `MatchScore` and
   `EntryScore`, weighted in the score formula at the same level as `unknowns`.

4. All existing tests (E2E-01 through E2E-15, MT-01 through MT-06) must
   continue to pass without modification.

5. New unit tests covering the new behaviour.

## Non-goals

- Packed-element veto (distinct deferred item from spec 0042).
- Hopcroft runtime deduplication of the active set.
- Changes to the compiled graph format (the `label` field already exists in
  `TransitionEntry`; no format bump is needed).
- Changes to `reproto --emit-scoring-graphs` or the YAML schema.
- Any CLI changes.

---

## Specification

### §1 — Score formula update

Add `mismatches: u64` to both `MatchScore` and `EntryScore`:

```rust
pub struct MatchScore {
    pub matches:       u64,
    pub unknowns:      u64,
    pub mismatches:    u64,  // absent required fields
    pub non_canonical: u64,  // overhang, >1 of optional/required, etc.
    pub vetoed:        bool,
}
```

The score formula gains a `mismatches` term weighted identically to `unknowns`
(−10 each):

```
score = matches×1  −  unknowns×10  −  mismatches×10  −  non_canonical×20
```

Rationale: an absent required field is as structurally suspicious as an
unknown field.  Both indicate the schema partially explains the data.  Using
the same weight keeps the formula easy to reason about.

The output line format for `score-graph score` and `score-graph match` gains a
new column:

```
matches=M unknowns=U mismatches=X non_canonical=C score=S
```

### §2 — `find_transition` extended return type

Currently `find_transition` returns `Option<u32>` (child state id only).
Change it to return both the child state id and the label:

```rust
struct TransitionResult {
    child_state_id: u32,
    label: u8,   // 0=optional, 1=required, 2=repeated
}

fn find_transition(
    graph: &ArchivedCompiledGraph,
    state: u32,
    field_number: u32,
) -> Option<TransitionResult>
```

This is a pure internal refactor: only `find_transition` and its two callers
(`schema_verdict` in the single-entry path and the inline verdict loop in
`score_message_multi`) need to change.  No other public interface is affected.

### §3 — Per-field occurrence tracking within a message frame

Both `score_message` and `score_message_multi` process fields in a loop.  To
support cardinality checks, each call frame maintains a per-field occurrence
map.

#### §3.1 — Single-entry walk

Add to the `score_message` call frame:

```rust
// field_number → occurrence count within this frame
let mut occurrences: HashMap<u32, u64> = HashMap::new();
```

Each time a field is consumed (regardless of verdict — match or unknown),
`*occurrences.entry(field_number).or_insert(0) += 1`.

At frame exit (when `pos == buflen` or when `WT_END_GROUP` terminates the
frame), apply cardinality checks (§3.3).

#### §3.2 — Multi-entry walk

The multi-entry walk must track occurrences **per entry** (because two entries
may declare the same field number with different labels, or one may not declare
it at all).

Each `ActiveEntry` gains a per-field occurrence map:

```rust
struct ActiveEntry {
    state_id: u32,
    entries:  SmallVec<[u16; 4]>,
    // How many times each declared field number has been seen in this frame.
    // field_number → count.  Only populated for field numbers where at least
    // one entry in this group declared the field (i.e. Found verdict).
    occurrences: HashMap<u32, u64>,
}
```

Alternatively — and preferably for performance — store a `Vec<(u32, u64)>`
sorted by field number, since the total number of distinct seen field numbers
per message frame is typically small.

When a `Found` verdict is resolved for `state_id` and `field_number`,
increment `occurrences[field_number]` for that `ActiveEntry`.

At frame exit, iterate over every `ActiveEntry` that survives to the end of
the buffer (or group) and apply cardinality checks for its `state_id` (§3.3).
Unknown-verdict fields do not appear in the `occurrences` map and are not
subject to cardinality checks (they have no declared label).

#### §3.3 — Cardinality check algorithm

At end of frame, for each surviving entry group (single-entry: one state;
multi-entry: each `ActiveEntry`):

1. Iterate over all transitions from `state_id` in the graph (binary-search
   for the first transition with `state_id`, then scan while `state_id`
   matches).

2. For each transition `(field_number, label, child_state_id)`:

   Let `count = occurrences.get(field_number).copied().unwrap_or(0)`.

   - `label == 0` (Optional):
     - `count > 1`: `non_canonical += count - 1`
   - `label == 1` (Required):
     - `count == 0`: `mismatches += 1`
     - `count > 1`: `non_canonical += count - 1`
   - `label == 2` (Repeated):
     - no cardinality check

3. Checks are applied directly to `MatchScore` (single-entry) or to every
   entry `e` in `ActiveEntry.entries` via `ws.scores[e]` (multi-entry).

Note: cardinality checks are applied **after** the frame loop exits — they
must not trigger a veto, and they do not affect the field-processing loop.

### §4 — GROUP recursion

#### §4.1 — Wire semantics recap

A proto2 group field is encoded as:

```
<START_GROUP tag: field_number, wire_type=3>
  ... fields of the group message ...
<END_GROUP tag: field_number, wire_type=4>
```

The group body is itself a protobuf message with the same encoding rules as
a LEN-delimited sub-message — except the terminator is `WT_END_GROUP` instead
of a byte-counted boundary.

The compiled graph represents a GROUP field as a `TransitionEntry` whose
child node has `wire_type == 3` (WT_START_GROUP).  That child node has its
own outgoing transitions (the fields of the group message), exactly like a
LEN message node.

#### §4.2 — Groups and LEN fields are analogous: wire-determined boundaries

For a LEN field, the length prefix fixes the boundary wire-independently.
For a group, the END_GROUP tag fixes the boundary wire-independently.  In
both cases, schema knowledge affects what happens *inside* the boundary —
not where the boundary is.

The critical invariant for groups: **every entry that has not been vetoed
when the recursive call returns has reached the same `new_pos`**.  Schema-
triggered vetoes (wrong wire type, bad UTF-8, enum out of range) remove
entries from the active set but do not change where the surviving entries
end up.  Wire-level errors (truncated body, garbage tag, mismatched
END_GROUP) veto all active entries and set `new_pos = buflen`.  Either way,
all survivors agree on `new_pos`.

This makes group handling structurally identical to LEN sub-message handling:

| | LEN sub-message | Group |
|---|---|---|
| Boundary determined by | length prefix (wire) | END_GROUP tag (wire) |
| Boundary signal to recursive call | sub-slice `buf[pos..pos+len]` | `my_group = Some(field_number)` |
| `new_pos` for survivors | `pos + len` (fixed before recursion) | returned `usize` from recursive call |
| Schema-triggered veto | removes entry; others continue | same |
| Wire-level error | veto all; `new_pos = buflen` | same |

#### §4.3 — Return `usize` from `score_message` / `score_message_multi`

Change both functions from `-> ()` to `-> usize` (the consumed position),
exactly as `prototext-core`'s `parse_message` already does.

For groups this means: call the recursive function with
`my_group = Some(field_number)`, get back `new_pos`, set `pos = new_pos` in
the outer loop.  The `new_pos` is always past the END_GROUP tag (clean
termination) or `buflen` (veto path) — both are correct to assign to `pos`.

#### §4.4 — `stay_out` entries and the group interior

For a LEN sub-message, a `stay_out` entry (Unknown verdict) does not enter
the recursive call; it receives exactly one `unknowns += 1` at the outer
level and the outer `pos` advances past the sub-message using the pre-known
length.  The group analogy holds: a `stay_out` entry receives one
`unknowns += 1` at the outer level and needs `new_pos` past the END_GROUP.

Since `new_pos` is wire-determined and shared by all survivors, a `stay_out`
entry can use the same `new_pos` returned by the `recurse_into` recursive
call — provided at least one `recurse_into` entry survived to the END_GROUP.
If at least one survives, the recursive call reached the END_GROUP cleanly,
so its `new_pos` is the correct position past the group for everyone.

If no `recurse_into` entry survives (either because `recurse_into` was empty
to begin with, or because all were vetoed before reaching the END_GROUP), the
recursive call's `new_pos` cannot be trusted as a group boundary.  In that
case `parse_group_blind` is called to obtain `new_pos`; veto all on `None`.

Summary:

| Case | `pos` source for `stay_out` |
|---|---|
| Only `stay_out` | `parse_group_blind` |
| Only `recurse_into` | recursive call (no `stay_out` to advance) |
| Mixed, ≥1 `recurse_into` survivor | recursive call `new_pos` (reached END_GROUP) |
| Mixed, 0 `recurse_into` survivors | `parse_group_blind` |

No `GroupBounds` struct.  `parse_group_blind` return type is unchanged.

#### §4.5 — Single-entry: updated `WT_START_GROUP` arm

```rust
WT_START_GROUP => {
    match verdict {
        SchemaVerdict::Unknown => {
            // No recurse_into entries; use parse_group_blind for new_pos.
            match parse_group_blind(buf, pos, field_number) {
                None => { s.veto(); return; }
                Some(new_pos) => pos = new_pos,
            }
            s.unknowns += 1;
        }
        SchemaVerdict::Found(child) => {
            s.matches += 1;
            pos = score_message(buf, pos, child, Some(field_number), graph, s);
        }
        SchemaVerdict::WireTypeMismatch => unreachable!(),
    }
}
```

#### §4.6 — Multi-entry: updated `WT_START_GROUP` arm

1. Compute verdicts for all active entries (same as any other field).
2. Apply wire-type mismatch vetoes (same as any other field).
3. Count `matches += 1` for `recurse_into`; `unknowns += 1` for `stay_out`.
4. If `recurse_into` non-empty: build child active set; call
   `score_message_multi(buf, pos, child_active, Some(field_number), ws)`
   returning `new_pos`; apply `propagate_vetoes`.
5. Determine authoritative `new_pos` for `stay_out`:
   - If `stay_out` is empty: use `new_pos` from step 4 (no `stay_out` to advance).
   - If `stay_out` is non-empty and at least one `recurse_into` entry survived
     (is not vetoed after step 4): use `new_pos` from step 4 — it reached
     the END_GROUP cleanly and is valid for everyone.
   - Otherwise (`recurse_into` was empty, or all vetoed in step 4): call
     `parse_group_blind(buf, pos, field_number)`; veto all on `None`;
     `new_pos = result`.
6. Set `pos = new_pos`.

#### §4.7 — Cardinality checks inside groups

A group body is a complete message frame.  Cardinality checks (§3.3) apply
inside groups exactly as they do inside LEN-delimited messages: at the end of
the `score_message` / `score_message_multi` call that processed the group
body.

The recursive call receives a fresh `occurrences` map (an empty
`HashMap::new()` at the top of each call frame), so group-interior field
counts are independent of the enclosing message's counts.

### §5 — Impact on existing scoring rules

The following existing rules are **unchanged**:

- Wire parse errors (garbage tag, truncated body) veto all active entries.
- Wire-type / proto-type mismatch vetoes matching entries.
- UTF-8 check on string fields.
- Enum range check.
- `non_canonical` for tag/value overhang.
- LEN recursion for sub-messages.
- `WT_END_GROUP` at top level (no enclosing group) vetoes all active entries.

The only **additions** are:

1. GROUP recursion (fields inside groups now contribute to the score).
2. Cardinality checks at end of each frame (new `mismatches` counter;
   optional/required overflow counts toward `non_canonical`).
3. `mismatches` column in output.

### §6 — Impact on Hopcroft deduplication

#### §6.1 — The problem with the current implementation

The current `hopcroft.rs` treats `label` as invisible to bisimulation:

- The outgoing-signature used for the initial partition is `Vec<field_number>`
  — label is absent.
- The refinement splitters are `(block_id, field_number)` pairs — label is
  not a splitter dimension.

Two states that share identical field-number sets but differ on a label (e.g.
one has `optional int32 id = 1`, the other `required int32 id = 1`) land in
the same initial block and are never split.  Then `compile()` collapses their
edges by `(src_block, field_number)` and silently merges the labels using
"most permissive wins".  The compiled graph ends up with a single edge for
that field carrying `optional`, discarding the `required` constraint entirely.

With cardinality checks now reading `label` at scoring time, this loss is
no longer harmless: an entry that declared `required` would never fire its
`mismatches` check, because the compiled graph stores `optional` for that
edge.

#### §6.2 — Fix: include label in the bisimulation key

The edge identity must be `(field_number, label)` jointly, not `field_number`
alone.  Two states are bisimulation-equivalent only if, for every
`(field_number, label)` pair, they transition to nodes in the same block.

Concretely, in `hopcroft.rs`:

1. **Reverse adjacency** — change the key from `field_number: u32` to
   `(field_number, label): (u32, u8)`:

   ```rust
   // before:
   rev[di].push((si, edge.field_number));
   // after:
   rev[di].push((si, edge.field_number, edge.label));
   ```

2. **Outgoing signature** — change from `Vec<u32>` to `Vec<(u32, u8)>`:

   ```rust
   // before:
   sig[si].push(edge.field_number);
   // after:
   sig[si].push((edge.field_number, edge.label));
   ```

   Sort and dedup as before (now on pairs).

3. **Splitters** — change from `(block_id, field_number)` to
   `(block_id, field_number, label)`.  The worklist is seeded with all
   `(block, field_number, label)` triples seen in the edge set.  The
   predecessor query filters on both `field_number` and `label`.

4. **`compile()`** — with label now part of the bisimulation key, two raw
   edges with the same `(src, field_number)` but different labels always
   target different blocks (they were split in §6.2 steps 1–3).  The
   `(src_block, field_number)` collision that required "most permissive wins"
   can no longer occur.  Remove the label-merge logic; each
   `(src_block, field_number, label)` triple produces exactly one
   `TransitionEntry`.  The compiled key remains `(src_block, field_number)`
   — label is a property of the edge, not part of the lookup key — but
   it is now always unambiguous.

#### §6.3 — Effect on deduplication rate

States that previously merged (same field numbers, different labels on some
fields) are now kept separate.  This slightly reduces the deduplication ratio:
in a corpus where many schemas share field numbers but differ in `required` vs
`optional`, more distinct states will appear in the compiled graph.  In
practice `required` fields are uncommon in proto3 (proto3 has no `required`)
and moderately common in proto2 but rarely the sole distinguishing attribute
between two otherwise-identical message types, so the practical impact on
graph size is expected to be small.

### §7 — Source changes

```
score-graph/src/build_scoring_graph/
  hopcroft.rs
    - rev[] entries: (src, field_number) → (src, field_number, label)
    - sig[] entries: field_number → (field_number, label)
    - worklist seed: (block, field_number) → (block, field_number, label)
    - predecessor filter: field_number match → (field_number, label) match
  graph.rs
    - compile(): remove "most permissive wins" label-merge logic;
      each (src_block, field_number, label) produces one TransitionEntry
      unambiguously

score-graph/src/score/
  walk.rs
    - score_message: return type () → usize (consumed pos)
    - score_message_multi: return type () → usize (consumed pos)
    - find_transition: return type Option<u32> → Option<TransitionResult>
      where TransitionResult { child_state_id: u32, label: u8 }
    - schema_verdict: SchemaVerdict::Found carries label alongside child_state_id
    - score_message: occurrences HashMap per frame; end-of-frame cardinality
      checks; WT_START_GROUP Unknown arm calls parse_group_blind (None → veto,
      Some(new_pos) → pos = new_pos); WT_START_GROUP Found arm recurses
      with my_group=Some(field_number), sets pos = returned usize
    - score_message_multi: ActiveEntry gains occurrences field;
      WT_START_GROUP arm partitions into recurse_into / stay_out;
      recurse_into non-empty: recursive call returns new_pos (shared by
      stay_out); recurse_into empty: parse_group_blind returns new_pos;
      end-of-frame cardinality checks for each surviving ActiveEntry
    - MatchScore and EntryScore gain mismatches: u64
    - score()/EntryScore::score() formula updated (−10 per mismatch)
  mod.rs
    - Print mismatches column in run() and run_match() output
  tests.rs
    - New unit tests (see §8)

score-graph/tests/
  score_e2e.rs
    - Update expected output strings to include mismatches=0
      (all existing fixtures have zero mismatches)
```

### §8 — Tests

#### §8.1 — Unit tests (new)

All new tests live in `score/tests.rs` and use the existing
`build_graph` / `build_two_entry_graph` helpers or new helpers as needed.

**GL-01 — Group recursion, single-entry, known group field**

Schema: `Outer { repeated group G = 1 { int32 x = 1; } }`.
Fixture: one occurrence of group G containing field x=42.
Expected: `matches=2 unknowns=0 mismatches=0 non_canonical=0 score=2`.
(1 match for the group field itself, 1 match for x inside.)

**GL-02 — Group recursion, single-entry, unknown group field**

Schema: `Outer { }` (no fields declared).
Fixture: one occurrence of a group with field_number=1.
Expected: `matches=0 unknowns=1 mismatches=0 non_canonical=0 score=-10`.
(Group is unknown, content is skipped.)

**GL-03 — Group recursion, single-entry, veto inside group**

Schema: `Outer { repeated group G = 1 { string s = 1; } }`.
Fixture: group G with s=1 carrying invalid UTF-8.
Expected: `vetoed=true`.

**GL-04 — Required field present — no mismatch**

Schema: `Outer { required int32 id = 1; }`.
Fixture: field 1 = 42.
Expected: `matches=1 unknowns=0 mismatches=0 non_canonical=0 score=1`.

**GL-05 — Required field absent — mismatch**

Schema: `Outer { required int32 id = 1; }`.
Fixture: empty message (no fields).
Expected: `matches=0 unknowns=0 mismatches=1 non_canonical=0 score=-10`.

**GL-06 — Optional field twice — non_canonical**

Schema: `Outer { optional int32 id = 1; }`.
Fixture: field 1 appears twice (= two wire tags for field_number=1).
Expected: `matches=2 unknowns=0 mismatches=0 non_canonical=1 score=-18`.
(2 matches; 1 non_canonical for the extra occurrence.)

**GL-07 — Required field twice — non_canonical**

Schema: `Outer { required int32 id = 1; }`.
Fixture: field 1 appears twice.
Expected: `matches=2 unknowns=0 mismatches=0 non_canonical=1 score=-18`.

**GL-08 — Repeated field many times — no penalty**

Schema: `Outer { repeated int32 xs = 1; }`.
Fixture: field 1 appears four times.
Expected: `matches=4 unknowns=0 mismatches=0 non_canonical=0 score=4`.

**GL-09 — Multi-entry: group recursion vetoes one entry, not the other**

Two entries:
- `A`: `{ repeated group G = 1 { string s = 1; } }`
- `B`: `{ }` (no fields declared)

Fixture: one group field_number=1 containing s=1 with invalid UTF-8,
terminated by a matching END_GROUP.
Expected: A is vetoed (UTF-8 check on declared string inside group fails);
B scores `unknowns=1` (field 1 unknown, group walked blind, no veto).

**GL-10 — Multi-entry: required absent vetoes no-one but adds mismatch**

Two entries:
- `A`: `{ required int32 id = 1; optional string name = 2; }`
- `B`: `{ optional int32 id = 1; }`

Fixture: field 2 = "hello" only (field 1 absent).
Expected:
- A: `matches=1 unknowns=0 mismatches=1 non_canonical=0` (name matched; id
  absent-required).
- B: `matches=0 unknowns=1 mismatches=0 non_canonical=0` (field 2 unknown).

#### §8.2 — E2E test updates

All existing E2E tests (E2E-01 through E2E-15) must have their expected output
strings updated to include the new `mismatches=0` column.  No fixture `.pb`
files need to change (the existing fixtures have no required fields, so
`mismatches` is 0 for all of them).

Example: E2E-01 expected line changes from

```
matches=6 unknowns=0 non_canonical=0 score=6
```

to

```
matches=6 unknowns=0 mismatches=0 non_canonical=0 score=6
```

---

## Open questions

1. **`mismatches` score weight**: the choice of −10 (same as `unknowns`) is
   justified by symmetry, but could be tuned once real-corpus scoring data
   is available.

2. **Cardinality for `oneof` fields**: proto2/proto3 `oneof` semantics allow
   at most one field from the set to be present.  The compiled graph does not
   currently encode `oneof` membership.  Violations (two fields from the same
   `oneof` both present) are not detected.  This is out of scope for this
   spec.
