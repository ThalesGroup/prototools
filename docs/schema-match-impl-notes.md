<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Schema-match: implementation notes

This document is a guided tour of the `score-graph match` implementation.
It is intended as a standalone introduction for someone reading the source
for the first time, with emphasis on the non-obvious algorithmic choices.

---

## 1. What the command does

`score-graph match <graph.bin> <payload.pb>` answers the question:

> Among all known protobuf message types, which one does this binary blob
> most likely belong to?

It reads a compiled scoring graph (produced offline by
`score-graph build-scoring-graph`) and a binary protobuf payload, walks the
payload once, and returns every candidate message type ranked by score.

---

## 2. Inputs

### 2.1 The compiled scoring graph

The graph is serialized with `rkyv` (zero-copy deserialization) and
contains three tables, all sorted for binary search:

- **Node table** — one entry per state.  Each node records the wire type
  of values that arrive on incoming edges (`VARINT`, `I64`, `LEN`,
  `START_GROUP`, `I32`), a flag for string nodes (UTF-8 check required),
  and an optional enum range index.

- **Transition table** — one entry per schema edge, sorted by
  `(state_id, field_number)`.  Each edge carries the destination state,
  the protobuf field number, and the cardinality label
  (0 = optional, 1 = required, 2 = repeated).

- **Root table** — one entry per candidate message type: a
  fully-qualified name and the graph state that represents its top-level
  message frame.

Multiple distinct message types may share the same root state when their
field structures are bisimilar (e.g. two messages that both have a
`repeated LEN_MSG` at field 3 and nothing else).  Such types are
structurally indistinguishable from the wire alone and will always tie.

### 2.2 The payload

The payload is read as raw bytes.  If it looks like prototext (the
human-readable format), it is first compiled to binary.

---

## 3. Scoring rules

For each field encountered during the walk, the scorer assigns one of three
verdicts:

| Verdict | Condition | Effect |
|---|---|---|
| **Veto** | Wire parse error; wire-type mismatch on a declared field; invalid UTF-8 on a string field; enum value out of declared range; mismatched or open-ended group | Candidate is permanently eliminated |
| **Match** | Field number declared in schema at current state; wire content compatible | `matches += 1` |
| **Unknown** | Field number not declared in schema at current state | `unknowns += 1` |

Non-canonical encodings (overhang bytes on varints, field numbers 0 or
≥ 2²⁹) do not veto by themselves; they accumulate in `non_canonical`.

At the end of each message or group frame, cardinality checks apply
over all transitions from the current state:

- Optional field seen more than once: `non_canonical += count − 1`
- Required field never seen: `mismatches += 1`
- Required field seen more than once: `non_canonical += count − 1`
- Repeated field: no constraint

The final integer score is:

```
score = matches×1 − unknowns×10 − mismatches×10 − non_canonical×20
```

---

## 4. The parallel multi-entry walk

The naive approach — run a separate walk for each of the N candidate types
— reads the payload N times and pays N times the parsing cost.  The
implementation instead does a single pass over the payload while tracking
all candidates simultaneously.

### 4.1 The active set

Candidates are grouped by the graph state they currently occupy into a
`Vec<ActiveEntry>`.  Each `ActiveEntry` holds:

- `state_id` — the shared graph state.
- `entries` — a `SmallVec` of entry indices (into the global scores array)
  that are currently in this state.  Multiple distinct message types may
  share a state; they travel together as long as they receive identical
  verdicts.
- `occurrences` — a sorted `Vec<(field_number, count)>` tracking how many
  times each field has been seen in the current frame, used for
  end-of-frame cardinality checks.

An entry index is a `u16` (capped at 65 535 candidates).

A flat bitset in `WalkState` tracks permanently vetoed entries so that
`propagate_vetoes` can remove them from any `ActiveEntry` in O(n/64).

### 4.2 Per-field iteration

For each field in the payload the loop does, in order:

1. **Parse the wire tag** (field number + wire type).  If malformed, veto
   all active candidates and stop.

2. **Non-canonical bookkeeping** — overhang bytes or out-of-range field
   numbers are charged to every still-active candidate.

3. **Compute per-state verdicts** — for each distinct state in the active
   set, look up `(state_id, field_number)` in the transition table (binary
   search).  If found, compare the expected wire type from the child node
   against the stream wire type:
   - equal → `Found(child_state_id, label)`
   - different → `Mismatch`
   - not found → `Unknown`

4. **Apply vetoes from Mismatch** — any `ActiveEntry` whose state produced
   `Mismatch` has all its entries permanently vetoed and is dropped.

5. **Consume the wire body** (dispatched on wire type) and update scores.

6. **Recurse** for `LEN_MSG` and `START_GROUP` fields with a `Found`
   verdict — see §5 and §6 below.

### 4.3 Why verdicts are keyed by state_id, not by Vec index

After step 4, `active` may have shrunk.  If the verdict buffer were indexed
by position in `active`, positions would shift after every removal.  Keying
by `state_id` — which is stable regardless of how many entries are dropped
— avoids rebuilding the buffer after each veto.

---

## 5. LEN-delimited sub-messages

When a field has a `Found(child)` verdict and the child state has outgoing
transitions (i.e. it is a message node, not a scalar leaf), the scorer
recurses into the length-delimited payload slice.

The key point: the length prefix in the wire format provides an
**unambiguous byte boundary, independent of any schema**.  Every candidate
— whether it considers the field a sub-message, a byte blob, or a string —
advances to exactly the same position after the LEN field.  Schema only
influences what happens *inside* the payload slice.

Concretely:

- Candidates with `Found(child)` and a message child are collected into
  `child_pairs` and dispatched into a recursive `score_message_multi` call
  on the payload slice.
- Candidates with `Unknown` simply increment `unknowns` and skip the same
  byte range.
- After the recursive call, `propagate_vetoes` removes any candidates
  vetoed inside the sub-message from the parent active set.

---

## 6. GROUP fields — the tricky case

Groups use a different wire encoding: a `START_GROUP` tag (wire type 3)
begins the body, and a matching `END_GROUP` tag (wire type 4, same field
number) ends it.  Unlike LEN fields, **there is no length prefix** — the
boundary is only known after walking the body to its end.

This creates a complication for the multi-entry walk: candidates that
consider the group field `Unknown` cannot skip it without knowing where it
ends, but that end position is determined by walking the body with schema.

The solution exploits a fundamental property of the protobuf wire format:
**all legal parses of a group body must end at the same byte position**.
The group boundary is a structural fact of the wire encoding, not a
schema-dependent interpretation.  Whether a given candidate schemas into
the group or treats it as unknown, if the group is well-formed, every
candidate that does not veto during the body walk ends up at the same
`new_pos`.

The implementation proceeds as follows:

1. Split the active set into `recurse_into` (candidates with `Found`) and
   `stay_out` (candidates with `Unknown`).

2. If `recurse_into` is non-empty, call `score_message_multi` recursively
   with `my_group = Some(field_number)`.  The recursive call walks the
   body applying schema to `recurse_into` candidates, and returns
   `new_pos` (the position after the `END_GROUP` tag).

3. `stay_out` candidates advance to the same `new_pos` without any
   additional walk.  They receive `unknowns += 1`.

4. If all `recurse_into` candidates were vetoed during the body walk,
   `new_pos` is no longer reliable for `stay_out` candidates.  In that
   case `parse_group_blind` is called: a schema-free structural walk that
   finds the `END_GROUP` boundary purely from wire-type rules, recursing
   into any nested groups it encounters.

5. If `recurse_into` is empty from the start (all candidates treat this
   field as unknown), `parse_group_blind` is called immediately.

An **open-ended group** — one where `END_GROUP` is never seen before the
end of the buffer — is a structural error that vetoes all active candidates
in the current frame, regardless of schema.

---

## 7. The END_GROUP sentinel in recursive calls

`score_message_multi` (and its single-entry counterpart `score_message`)
takes a `my_group: Option<u64>` parameter.  When `Some(field_number)` is
passed, the function is operating inside a group body.  The loop exits
cleanly (applying cardinality checks) when it encounters a matching
`END_GROUP` tag.  Two error conditions are detected:

- **EOF before END_GROUP**: the buffer ends while still inside the group →
  open-ended group → veto all active entries.
- **Mismatched END_GROUP field number**: the end tag belongs to a different
  nesting level → veto.

When `my_group` is `None` (top-level message frame), any `END_GROUP` tag
encountered is erroneous → veto.

The function returns the byte position after the `END_GROUP` tag (or
`buflen` on veto), so the caller can advance its own `pos` to the correct
boundary and charge `unknowns` to stay-out candidates.

---

## 8. Cardinality checks and occurrence tracking

Each `ActiveEntry` carries a per-frame `occurrences: Vec<(field_number,
count)>` sorted by field number.  `record_occurrence` does a binary search
to increment the count in O(log k) where k is the number of distinct fields
seen so far.

Crucially, `record_occurrence` is called **after** the field body has been
successfully consumed — at the same point where `matches` is incremented,
and never on a path that ends in a veto.  This ensures that a field which
caused a veto mid-body does not inflate the occurrence count.

At the end of each frame (top-level EOF, or `END_GROUP` match),
`apply_cardinality_multi` scans all transitions out of the current state
and for each one checks the observed count against the label:

- label 0 (optional): count > 1 → `non_canonical += count − 1`
- label 1 (required): count = 0 → `mismatches += 1`; count > 1 →
  `non_canonical += count − 1`
- label 2 (repeated): no constraint

---

## 9. The single-entry walk

`score_message` is a simplified version of `score_message_multi` for the
case where only one candidate is being scored (`score` subcommand).  It
carries a single `MatchScore` instead of a `WalkState`, and a `vetoed`
boolean short-circuits the loop.  The GROUP handling is identical in
structure: `my_group` carries the expected end-tag field number, and
`parse_group_blind` is used for unknown groups.

---

## 10. Relationship to the compiled graph

The graph is produced offline by `build-scoring-graph`, which:

1. Parses all per-file scoring-graph YAMLs (emitted by `reproto`).
2. Merges them into a raw directed graph where nodes are message types and
   edges are field transitions.
3. Runs **Hopcroft DFA minimization** to merge bisimilar states — states
   that cannot be distinguished by any sequence of field observations.
   This reduces graph size (typically 30–40%) without changing which types
   are distinguishable.  Two message types that share a state after
   minimization have identical field structure and will always score
   identically on any input.
4. Serializes the result as a zero-copy `rkyv` binary.

The scorer consumes only the serialized form; it never sees the raw graph
or the YAML source.
