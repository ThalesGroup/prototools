<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0048 — Multi-entry parallel scoring walk

**Status:** implemented
**Implemented in:** 2026-05-10
**App:** score-graph

---

## Background

Spec 0042 delivered the single-schema scoring walk: `score-graph score <graph>
<entry> <proto>` scores one binary protobuf against one named entry point and
prints a single result line.

The compiled graph produced by `score-graph build-scoring-graph` (spec 0047)
already contains **all** top-level message types across all input YAML files
as named `RootEntry` records.  Scoring them one at a time is wasteful: each
independent call re-loads the graph, re-decodes the prototext if needed, and
traverses the wire bytes from scratch.  Because the scoring walk is
schema-guided, the traversal of a given wire field depends on which schema is
active — but all schemas must still visit every field at depth 0.  A single
joint traversal can therefore amortize the cost of the wire parse across all
candidate entry points simultaneously.

The target use-case is schema identification: given a binary protobuf of
unknown type, rank all known top-level message types by how well they explain
the wire content, and return the best match (or top-k candidates).

This spec describes the data structures and algorithm for the multi-entry
parallel walk, and adds a `score-graph match` subcommand to invoke it.

---

## Goals

1. Add a `score-graph match <graph> <proto>` subcommand that scores **all**
   root entries in the compiled graph simultaneously against a single binary
   protobuf and prints results sorted by score (best first).

2. Implement the parallel walk in `score/walk.rs` as a new entry point
   `score_all(pb, graph) -> Vec<EntryScore>` that performs one joint traversal
   of the wire bytes while maintaining per-entry counters.

3. Non-vetoed entries are printed; vetoed entries are omitted unless `--all` is
   given.

4. A `--top N` flag limits output to the N highest-scoring non-vetoed entries.

5. The existing `score-graph score` subcommand is unchanged; it continues to
   call the single-entry `score()` function.

## Non-goals

- Hopcroft deduplication of the active set (the graph is already deduplicated
  at build time; this spec does not add runtime state-merging on top).
- Incremental graph updates.
- Packed-element veto or group recursion (inherited limitations from spec 0042).
- A Python API or FFI binding.
- Scoring against multiple *proto files* simultaneously (the proto input is
  always one file).

---

## Specification

### §1 — CLI

```
score-graph match [OPTIONS] <GRAPH> <PROTO>

Arguments:
  <GRAPH>   Compiled scoring graph (.bin) from build-scoring-graph.
  <PROTO>   Binary protobuf file to score (prototext format auto-detected).

Options:
  --top <N>      Print only the top N entries by score [default: all]
  --all          Include vetoed entries in output (marked "Vetoed")
  -q, --quiet    Suppress header line
```

Output (one line per non-vetoed entry, sorted by score descending):

```
entry=google.rpc.Status     matches=6 unknowns=0 non_canonical=0 score=6
entry=google.rpc.BadRequest matches=2 unknowns=4 non_canonical=0 score=-38
...
```

With `--all`, vetoed entries appear as:

```
entry=google.logging.LogEntry Vetoed
```

### §2 — Data model

#### §2.1 — Per-entry counters

Each root entry `i` (indexed 0..N-1) has its own counters, identical to the
single-entry `MatchScore`:

```rust
struct EntryScore {
    fqdn:          String,   // FQDN of the root entry
    matches:       u64,
    unknowns:      u64,
    non_canonical: u64,
    vetoed:        bool,
}

impl EntryScore {
    fn score(&self) -> i64 {
        self.matches as i64
            - 10 * self.unknowns as i64
            - 20 * self.non_canonical as i64
    }
}
```

All `N` `EntryScore` objects are allocated at the start of the walk and
indexed by a dense entry index `0..N`.

#### §2.2 — Active set

The active set at a given nesting level is a `Vec<ActiveEntry>`:

```rust
struct ActiveEntry {
    state_id: u32,
    /// Entry indices routing through this state at the current nesting level.
    /// Sorted and deduplicated.  Invariant: never empty (entries are removed
    /// when the last remaining entry in a group is vetoed).
    entries: SmallVec<[u16; 4]>,
}
```

`SmallVec<[u16; 4]>` avoids heap allocation for the common case where few
entries share a state.  `u16` suffices as long as the entry count stays under
65,535 (sufficient for the foreseeable corpus size; an assertion guards this
at startup).

**Grouping invariant**: entries that currently occupy the **same** `state_id`
are collected into a single `ActiveEntry`.  This is the key efficiency: one
transition-table lookup serves all of them.

#### §2.3 — Global walk state

```rust
struct WalkState<'a> {
    graph:   &'a ArchivedCompiledGraph,
    entries: &'a mut Vec<EntryScore>,   // length N, indexed by entry index
    vetoed:  FixedBitSet,               // length N; set = permanently vetoed
}
```

`vetoed` is a flat bitset (not per-active-set-entry) so that veto propagation
— which must reach **all** ancestor call frames — can be applied during
recursive unwinding without traversing the active-set data structures.

### §3 — Algorithm

#### §3.1 — Initialization

1. Load the compiled graph (memory-mapped, as in spec 0047).
2. Load and decode the proto bytes (prototext auto-detect, as in spec 0042 §3).
3. Allocate `N` `EntryScore` objects (one per `RootEntry` in the graph), all
   zeroed and non-vetoed.
4. Build the initial active set by grouping root entries by their `state_id`:

   ```
   initial_active = group_by_state(
       graph.roots.iter().enumerate().map(|(i, r)| (r.state_id, i as u16))
   )
   ```

   `group_by_state` produces a `Vec<ActiveEntry>` where every distinct
   `state_id` among the roots has exactly one `ActiveEntry`, and the `entries`
   list contains all entry indices that start at that state.  Entries that
   happen to share a root state (because two FQDNs deduplicated to the same
   state after Hopcroft) are already merged here.

5. Call `score_message_multi(pb, initial_active, None, &mut walk_state)`.

#### §3.2 — Core recursive function

```
fn score_message_multi(
    buf:      &[u8],
    active:   Vec<ActiveEntry>,   // owned; consumed and rebuilt at each recursion level
    my_group: Option<u64>,
    ws:       &mut WalkState,
)
```

The function processes fields one by one.  For each field:

**Step A — Parse wire tag** (wire-level, schema-blind):

- Call `parse_wiretag(buf, pos)`.
- If garbage (invalid wire type, truncated varint): veto **all** currently
  active entries (see §3.4), return.
- Advance `pos`.  Record `overhang` and `out_of_range` flags.

**Step B — Apply wire-level non-canonical penalties**:

Wire-level events (tag overhang, out-of-range field number) are observable by
every candidate regardless of schema.  For each `(state_id, entries)` pair in
`active`, for each entry index `e` in `entries`:

```
if tag.overhang > 0 { ws.entries[e].non_canonical += 1; }
if tag.out_of_range { ws.entries[e].non_canonical += 1; }
```

These increments happen unconditionally for all active entries at this depth,
before the schema-specific step.

**Step C — Schema verdict per active entry group**:

For each `ActiveEntry { state_id, entries }` in `active`:

- Call `find_transition(graph, state_id, field_number)` → `Option<child_state_id>`.
  - `None` → **unknown** for all `e` in `entries`: `ws.entries[e].unknowns += 1`.
  - `Some(child)` → check wire-type compatibility:
    - Wire type mismatch → **veto** for all `e` in `entries` (see §3.4).
    - Wire type match → **found**; classify as message or leaf (see §3.3).

**Step D — Consume wire body** (dispatch on stream wire type):

The wire body is consumed once per field, schema-independently, after all
verdicts are computed.  This is safe because the body length is determined by
the stream wire type alone, independent of schema.

- `WT_VARINT`: `parse_varint`; if garbage → veto all active, return.
  Record `overhang` for value-level non-canonical (see §3.5).
- `WT_I64`: advance 8 bytes; if truncated → veto all active, return.
- `WT_LEN`: `parse_varint` for length prefix; if garbage → veto all active,
  return.  Record length-prefix `overhang`.  If `pos + length > buflen` →
  veto all active, return.  Slice `payload = buf[pos..pos+length]`.
- `WT_START_GROUP`: `skip_group`; if error → veto all active, return.
- `WT_END_GROUP`: handled by group termination (see §3.6).
- `WT_I32`: advance 4 bytes; if truncated → veto all active, return.

**Step E — Apply schema verdicts** (matches, unknowns, recursion):

After body consumption:

- For entries that were **found** (wire type matched):
  - Increment `ws.entries[e].matches += 1`.
  - If the child node is a **message node** (has outgoing transitions) and
    `wire_type == WT_LEN`: mark this entry for recursion (see §3.3).
  - If the child node is a **string leaf** and `wire_type == WT_LEN`: schedule
    a UTF-8 check against `payload`.
  - If the child node is an **enum leaf**: apply enum-range and truncated-neg
    checks against the varint value (see §3.5).

- For entries that were **unknown**: `ws.entries[e].unknowns += 1` (already
  done in Step C).

- For entries that were **vetoed**: already removed from `active` (see §3.4).

#### §3.3 — LEN recursion

When `wire_type == WT_LEN`, some active entries declare the field as a message
type (found a message child state) and some do not.  These two groups must be
handled differently.

**At the moment the LEN field is processed** (before advancing `pos` past the
payload):

1. Partition `active` into two sets:
   - `recurse_into`: entries for which `found` and `child` is a message node.
   - `stay_out`: all other non-vetoed entries (unknowns, scalar/string found,
     already-vetoed entries are excluded).

2. For `recurse_into`, build the **child active set**: a `Vec<ActiveEntry>`
   where each `ActiveEntry.state_id` is the child message state, and
   `entries` contains the entry indices that declared this field as a message
   and transition to that child state.  Entries transitioning to the same
   child `state_id` are merged into one `ActiveEntry`.

3. Suspend `stay_out` entries on the call stack (they are implicitly preserved
   by the Rust call stack frame; no explicit stack needed).

4. Call `score_message_multi(payload, child_active, None, ws)` recursively.

5. **On return**: entries that were vetoed inside the recursion are now
   recorded in `ws.vetoed`.  Remove them from the parent `active` set (from
   all entries lists).  This is the **upward veto propagation** step.

6. Rejoin `stay_out` and the (now-possibly-reduced) `recurse_into` entries
   back into `active` for the next field.

The key point: **`stay_out` entries do not enter the child call and therefore
do not receive any updates for fields inside the sub-message**.  They pay
exactly one outcome (match or unknown) at the LEN field itself, then resume
at the next field in the parent message.  This is correct: a schema that
declared `bytes` for this field is not wrong about the sub-message content —
it simply treats it as opaque bytes.

#### §3.4 — Veto handling

A veto is triggered by:
- A wire-level invalid (garbage tag, truncated body, invalid wire type).
- A wire-type / proto-type mismatch for a declared field.
- An invalid UTF-8 sequence on a `string`-declared field.
- An enum varint outside the schema-declared range.
- An invalid group end or open-ended group.

When a veto applies to a **subset** of entries (e.g. wire-type mismatch
vetoes only entries that declared the field, not unknowns):

```
for e in vetoed_entries:
    ws.vetoed.set(e, true);
    ws.entries[e].vetoed = true;
```

Then remove those `e` values from the `entries` list of each affected
`ActiveEntry`.  If an `ActiveEntry.entries` becomes empty, remove the entire
`ActiveEntry` from `active`.

When a veto applies to **all** active entries (wire-level invalids that make
the buffer uninterpretable at this depth):

```
for ae in &active:
    for e in &ae.entries:
        ws.vetoed.set(*e, true);
        ws.entries[*e].vetoed = true;
active.clear();
return;
```

**Upward propagation**: when `score_message_multi` returns after recursing
into a sub-message, the caller checks `ws.vetoed` for each entry in
`recurse_into` and removes newly vetoed entries from the parent `active` set.
Because `ws.vetoed` is a flat bitset indexed by entry index, this check is
`ws.vetoed.contains(e)` — O(1) per entry, no traversal of inner data
structures needed.

#### §3.5 — Value-level non-canonical and enum checks

These checks apply only to entries for which the field was **found** (the
entry declared this field, and the wire type matched).

**Value varint overhang** (wire type 0):
- `parse_varint` returns `overhang > 0`.
- For each found entry `e`: `ws.entries[e].non_canonical += 1`.

**Length-prefix overhang** (wire type 2):
- The length-prefix varint itself has `overhang > 0`.
- For each active entry `e` at this depth (not just found):
  `ws.entries[e].non_canonical += 1`.
- Rationale: the overhang is observable by all schemas active here,
  regardless of how they declare the field.

**Enum range check** (wire type 0, child is enum leaf):
- `val >= 0x8000_0000 && val <= 0xFFFF_FFFF`: truncated negative.
  For this entry `e`: `ws.entries[e].non_canonical += 1`.  Then proceed to
  range check (truncated negatives are still in-range for schemas that accept
  negative enums).
- `val >= 2^32 && val < 2^64`: always a veto (too-large varint, same as
  `INVALID_VARINT` for enum).
- After sign-extension to `i32`: value outside `[enum_min, enum_max]` → veto
  this entry.

**UTF-8 check** (wire type 2, child is string leaf):
- If `std::str::from_utf8(payload).is_err()`: veto this entry.

#### §3.6 — Group termination

Group handling is unchanged from the single-entry walk: `WT_END_GROUP`
terminates the current call frame if `field_number == my_group`; mismatched or
unexpected group end vetoes all active entries and returns.

Since this spec does not add GROUP recursion (deferred, as in spec 0047),
schema-declared GROUP fields (where the schema has `wire_type == WT_START_GROUP`
for a message child) are treated the same as the single-entry walk: the entire
group content is skipped by `skip_group`, and all entries that declared the
field as a group get a match (no recursion into it).

### §4 — Output

After `score_message_multi` returns:

1. Collect all `EntryScore` objects.
2. Sort non-vetoed entries by `score()` descending; break ties by `fqdn`
   ascending (stable, deterministic output).
3. Apply `--top N` truncation.
4. Print each non-vetoed entry:
   ```
   entry=<fqdn> matches=M unknowns=U non_canonical=C score=S
   ```
5. If `--all`: also print vetoed entries (after non-vetoed), sorted by `fqdn`:
   ```
   entry=<fqdn> Vetoed
   ```

### §5 — Source layout

Changes are confined to `score-graph/src/`:

```
score-graph/src/
  score/
    mod.rs       — add ScoreAllArgs; dispatch to score_all(); print results
    walk.rs      — add ActiveEntry, WalkState, score_all(), score_message_multi()
                   (existing score() and score_message() unchanged)
    tests.rs     — new multi-entry unit tests
  main.rs        — add Match subcommand alongside existing Build/Score
```

No changes to `build_scoring_graph/` or the compiled graph format.

### §6 — Implementation notes

#### Active set construction: `group_by_state`

```rust
fn group_by_state(pairs: impl Iterator<Item = (u32, u16)>) -> Vec<ActiveEntry> {
    // Sort by state_id, then group.
    let mut v: Vec<(u32, u16)> = pairs.collect();
    v.sort_unstable_by_key(|&(s, _)| s);
    let mut result = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let state_id = v[i].0;
        let mut entries = SmallVec::new();
        while i < v.len() && v[i].0 == state_id {
            entries.push(v[i].1);
            i += 1;
        }
        result.push(ActiveEntry { state_id, entries });
    }
    result
}
```

This is called once at initialization and once per LEN recursion (to build the
child active set from the entries that recurse into the sub-message).

#### Removing vetoed entries from the parent active set after recursion

After `score_message_multi` returns from a sub-message recursion, the caller
iterates over the `recurse_into` group (which is part of `active`) and removes
newly vetoed entries:

```rust
for ae in active.iter_mut() {
    ae.entries.retain(|&e| !ws.vetoed.contains(e as usize));
}
active.retain(|ae| !ae.entries.is_empty());
```

This is the complete upward propagation mechanism.  Because `ws.vetoed` is a
flat bitset, the inner `retain` closure is a simple bit test — no hash lookup
or recursion needed.

#### Memory

Peak memory beyond the compiled graph (which is memory-mapped):

- `entries: Vec<EntryScore>`: N × ~40 bytes.  For N = 100,000: ~4 MB.
- `vetoed: FixedBitSet`: N / 8 bytes.  For N = 100,000: ~12 KB.
- Active set at depth 0: at most N `ActiveEntry` records.  In the degenerate
  case (all N entries have distinct root states), this is N × (8 + 4) bytes ≈
  1.2 MB.  In practice, after Hopcroft deduplication, many entries share
  root states and the active set is smaller.
- Active set at depth ≥ 1: much smaller; decays rapidly as schemas are vetoed
  and only recursing entries enter sub-messages.

Total additional memory is bounded by a few tens of MB in the worst case,
well within reason.

### §7 — Tests

**Unit tests** (`score/tests.rs`):

- **MT-01**: two entry points with identical root state (deduplicated by
  Hopcroft); both receive the same match/unknown counts from a single walk.
- **MT-02**: two entry points with different root states; one is vetoed by a
  wire-type mismatch, the other scores normally.  After the walk, the vetoed
  entry has `vetoed = true`; the other has a correct score.
- **MT-03**: veto inside a sub-message propagates upward.  Entry A recurses
  into a sub-message and is vetoed there.  Entry B does not recurse (declares
  the field as `bytes`).  After the walk, A is vetoed, B is not.
- **MT-04**: non-canonical tag overhang increments `non_canonical` for all
  active entries at that depth.
- **MT-05**: length-prefix overhang increments `non_canonical` for all active
  entries at that depth (both recursing and non-recursing).
- **MT-06**: enum out-of-range vetoes only the entry with the enum leaf, not
  an entry that declares the same field as `uint32` (varint, no range check).

**E2E test** (in `score-graph/tests/score_e2e.rs`):

- **E2E-15**: run `score-graph match` on `e01.pb` against the fixture graph.
  Assert that `Outer` appears first in the output with
  `matches=6 unknowns=0 non_canonical=0 score=6`, and that `Inner` appears
  later with a lower score.

---

## Relation to schema-match.md

This spec implements Part 1 of `docs/schema-match.md` for the compiled graph
format of spec 0047, without Hopcroft runtime deduplication of the active set
(Part 2 of schema-match.md describes that further optimization).

The key correspondences:

| schema-match.md concept | This spec |
|---|---|
| Candidate | Root entry (one per `RootEntry` in the graph) |
| State | `state_id` in `ActiveEntry` |
| Active set | `Vec<ActiveEntry>` (local to each recursion frame) |
| Active back-references | `ActiveEntry.entries` (entry indices routing through this state) |
| Static back-references | Not used here (no Hopcroft runtime merging) |
| Propagate upward | `ws.vetoed` bitset + `retain` after recursion returns |
| Veto | `ws.vetoed.set(e)` + remove from `ActiveEntry.entries` |

The "active back-references" in schema-match.md correspond to `entries` in
`ActiveEntry`.  The distinction from static back-references is important: two
entries A and B may share a `state_id` (because Hopcroft merged their root
message types), but A may have been vetoed at an outer recursion level.  Using
`entries` (which was pruned when A was vetoed) correctly excludes A from
further score updates; using the static back-references of the shared node
would incorrectly continue updating A's counters.
