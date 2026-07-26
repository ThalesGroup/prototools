<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0176 — reproto: an open enum has no range

Status: implemented
Implemented in: 2026-07-26
App: reproto
Refs: docs/scoring-flaws.md (C6),
      docs/protolens/rendering-worklist.md (D-g, W30),
      docs/specs/0045-reproto-emit-graph.md (§ kind mapping),
      docs/specs/0077-varint-range-veto.md

## Background

In proto3 — and under editions with `features.enum_type = OPEN` — enums are
**open**: a value outside the declared set is legal, is preserved on
round-trip, and is exactly what a newer sender emits to an older reader.
Forward compatibility is the point.

`reproto` does not ask. `_scoring_kind` (`phases.py:1862-1866`) computes the
extent of the declared values and emits it unconditionally:

```python
if TYPE == FD.TYPE_ENUM:
    values = list(field.enum_type.values_by_number.keys())
    return 'enum', None, (min(values), max(values))
```

The builder turns that into a `Range` leaf (`load.rs:200`, `graph.rs:90-93`)
and the walk tests every value against it (`walk.rs:995-1018`), charging
`non_canonical` — or vetoing, under `strict_ranges` — for anything outside.
So ordinary forward-compatible traffic is scored as evidence against the
schema that defines it.

### D-g scoped this as a format change. It is not one.

Decision D-g (`rendering-worklist.md:79`, `:112-130`) scoped the fix as one
open/closed bit per enum in the compiled graph, plus a graph format version
bump, and deferred it on that basis.

That is more machinery than the problem needs, because **an open enum has no
range**. Every 32-bit value is legal. The correct emission for an open enum
is therefore `type: int32` — not an approximation but a precise statement of
what an enum is on the wire: a 32-bit two's-complement varint, negatives
sign-extended to ten bytes. `ScoringKind::Int32` already exists and already
documents itself as *"2's-complement 32-bit; veto if wire value in invalid
gap"* (`load.rs:64-65`), which is exactly the one check that should survive.

So this is a **reproto-only change**: no YAML schema change, no builder
change, no walk change, no format bump. D-g is *answered*, not implemented.

### Finding 1 — the CLI vetoes open enums by default, and the worklist says it does not

`rendering-worklist.md:503-504` records, of spec 0172's decision to keep
`strict_ranges` as an opt-in knob:

> Nothing in this workspace sets it, so the shipped behavior is what this
> item asked for.

That is false. All three scoring entry points in the CLI set it
(`prototext/src/run.rs:423`, `:500`, `:532`):

```rust
strict_ranges: !relax_ranges,
```

`relax_ranges` is a bare `clap` boolean (`prototext/src/lib.rs:188-193`), so
it is `false` unless the user passes `--relax-ranges`. **The CLI's default is
`strict_ranges: true`.**

Spec 0172 S3 flipped `ScoringOpts::default()` to `false`, which is what
`protolens` uses (`decode.rs:192`, `override_pane.rs:60`) — but the
`prototext` CLI never reads that default. C6 is therefore still live on the
primary user-facing path: a proto3 enum value from a newer sender still
eliminates the correct FQDN, absorbingly, in `prototext`.

S1 closes that without touching the flag, and closes it at the source: after
S1 an open enum has **no range at all**, so there is nothing for
`strict_ranges` to be strict about.

### Finding 2 — after S1, `strict_ranges` means something defensible

Once open enums carry no range, the knob reaches only bool and *closed*
enums, where an out-of-set value is genuine evidence against the candidate:
a conformant writer of that schema would not produce it. That is what the
knob is for, and it is what makes the CLI's strict default acceptable rather
than merely tolerated. The knob stays.

### Penalty is not veto, and the penalty here is deliberate

`ScoringKind::Range` conflates bool with enum (`load.rs:200`), and a bool's
range is `(0, 1)`. On the wire **any nonzero varint is a legal `bool`** —
generated parsers evaluate `value != 0`. So a bool of `2` is legal, and it is
also something no conformant writer emits.

That is precisely the case `non_canonical` exists for. The scoring heuristic
**deliberately penalizes suspicious serialization as much as erroneous
serialization** — a voluntary posture, confirmed 2026-07-26 — and the
governing principle in `docs/scoring-flaws.md` ("veto only for what the wire
format makes impossible; score everything that is merely unlikely")
constrains **veto**, not penalty. The bool penalty is therefore correct and
wanted, not a residual.

This is exactly what separates it from the open enum. An out-of-set value on
an *open* enum is not suspicious at all — it is the designed
forward-compatibility mechanism, emitted by conformant writers as a matter of
course. It should cost nothing, which is why S1 is a fix rather than a change
of posture.

What remains genuinely open is narrower: whether the two surviving
`strict_ranges` **vetoes** — bool, and closed enum — should exist at all,
since neither value is impossible on the wire (proto2 moves an unrecognized
enum value to the unknown-field set rather than failing the parse). They are
opt-out-able and they are strong evidence, so they stay. Recorded as a flaw
entry for revisiting, not fixed here.

## Goals

1. A value outside an **open** enum's declared set costs nothing — not a
   veto, not `non_canonical`. It is ordinary forward-compatible traffic.
2. A **closed** enum keeps its range, and so keeps its discriminating power.
3. No compiled-graph format change and no version bump.
4. The 32-bit gap veto (spec 0172 S2, flaw C5) still applies to open enums:
   a varint that is neither a valid `u32` nor a sign-extended `i32` is
   impossible for *any* enum, open or closed.

## Non-goals

- **The per-enum open/closed bit in the compiled graph** — D-g as originally
  scoped. Not needed; see Background.
- **Removing `ScoringOpts::strict_ranges` or `--relax-ranges`.** Finding 2 is
  the argument for keeping both.
- **The bool and closed-enum residuals** above. Documented, not fixed.
- **`prototext`'s renderer.** It annotates enum values from the descriptor,
  not from the scoring graph, and is unaffected.

## Specification

### S1 — `reproto`: an open enum is an `int32`

In `_scoring_kind` (`reproto/src/reproto/phases.py:1862-1866`), consult the
enum's openness. Closed → unchanged (`'enum'` plus `(min, max)`). Open →
`return 'int32', None, None`.

Use `field.enum_type.is_closed`, not a test on the declaring file's `syntax`.
It resolves the edition feature `features.enum_type`, so it is correct for
proto2, proto3 and editions alike, whereas a `syntax` test silently gets
editions wrong. Verified present in the pinned runtime (protobuf 6.33.1),
where it is a **property**, not a method — calling it raises
`TypeError: 'bool' object is not callable`.

There is no option to emit `'enum'` with the range omitted: the `Range` kind
*requires* a range (`load.rs:166-173`, and `leaf_for_field` allocates its
sentinel from the pair), so "an enum with no range" has no representation.
`int32` is the representation.

**Hopcroft consequence.** Open-enum fields now share `LEAF_INT32` with
`int32` fields, so states differing only in that respect will merge. They are
wire-equivalent, so the merge is correct — and it *narrows* A, the number of
distinct states in the active set and the quantity `score_all`'s cost scales
with (spec 0173). This is the opposite direction from spec 0175 S1, which
splits states that were wrongly merged.

### S2 — documentation

- `docs/scoring-flaws.md` C6: record that spec 0172's interim was incomplete
  on the CLI path (Finding 1), and that this spec closes it at the source
  rather than by flipping a default.
- `docs/scoring-flaws.md`, cross-cutting section: state the posture
  explicitly next to the governing principle — the principle bounds **veto**;
  `non_canonical` deliberately penalizes legal-but-suspicious serialization as
  heavily as erroneous serialization, and that is a chosen stance rather than
  an accident. Without this written down, every future reader will read the
  governing principle as forbidding the bool penalty.
- `docs/scoring-flaws.md`: add the surviving bool / closed-enum **vetoes** as
  a new numbered entry, scoped to the veto question only.
- `docs/protolens/rendering-worklist.md`: correct the false claim at
  `:503-504`; rewrite the D-g row (`:79`) and Deferred item 2 (`:112-130`) to
  record that D-g is answered **without** a format bump, and why.
- `docs/specs/0045-reproto-emit-graph.md`: the enum row of the kind-mapping
  table now depends on `is_closed`.

## Test plan

### `reproto`

- **Open enum** — `packed_proto3.proto` declares `enum Status` (proto3, so
  open) and `repeated Status enums_default = 6`. Assert `type == "int32"` and
  that no `range` key is emitted. Together with spec 0175 S1 this field is
  also `label: repeated`, so the one assertion covers both specs.
- **Closed enum** — `field_comprehensive.proto` is proto2; `req_enum = 4`
  must still be `type == "enum"` with `range == [min, max]` over `Status`.
  The existing test asserts only the absence of `child`
  (`test_emit_scoring_graphs.py:110`); add the type and the range.
- **Editions** — if `editions_rendering.proto` (or another editions fixture)
  declares an enum, assert `is_closed`'s verdict is honored there too.
  That is the case a `syntax` test would get wrong, so it is the one worth
  pinning. If no editions fixture has an enum, say so rather than adding a
  fixture for it.
- The full 240-case pytest suite stays green.

### `prototext-graph`

No code changes, so no new Rust tests. The existing strict-mode tests
(`tests.rs:734`, `:957`, `:970`, `:1462`) hand-build graphs with `enum` plus
a range and continue to exercise the closed-enum path; they are independent
of what `reproto` chooses to emit.

### End to end — the test W30 asked for and did not get

Score a blob carrying a proto3 enum value outside the declared set against
its own true schema and assert the true FQDN wins. This must run through the
**CLI's** options: asserting it at the `score_all` level would pick up
`ScoringOpts::default()`, where `strict_ranges` is already `false`, and pass
vacuously while the shipped binary still failed. Before S1 it loses; after,
it wins.

**Venue: `reproto/src/reproto/tests/test_open_enum_scoring.py`, not
`prototext/tests/`.** The blob has to be scored against a graph that
`reproto` emitted, and only reproto's own pytest suite can build one: every
CLI scoring path requires a `hopcroft.rkyv` sibling, which is produced by
`reproto --schema-db-out`. `prototext/tests/e2e.rs` drives only
`decode`/`encode` with a plain `--descriptor-set` and has no way to reach
reproto. The precedent is
`test_scoring_graph_namespace_consistency.py` (spec 0166), which chains the
same two subprocesses for the same reason. What the spec actually demands —
that the assertion pass through the shipped binary's own option defaults
rather than `ScoringOpts::default()` — is satisfied either way.

Three cases:

- **E1** `score --type openenum.Paint` on `0x08 0x63` (`color: 99`, proto3):
  not vetoed, `matches: 1`.
- **E2** `list-schemas` on the same blob names `openenum.Paint`, i.e. the true
  FQDN survives to be ranked rather than being eliminated first.
- **E3** *control* — the identical bytes against a proto2 **closed** enum
  still report `vetoed: true`, and its FQDN is absent from `list-schemas`.
  Without E3, E1 and E2 would also pass if the CLI had simply stopped
  checking ranges; E3 is what pins the veto as live on this exact path.

### Build

`nix-build -A ci` green — it runs the reproto pytest suite, which is where
this change lands.
