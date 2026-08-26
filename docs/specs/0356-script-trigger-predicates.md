<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0356 — script advance_when predicates

Status: implemented
Implemented in: 2026-08-26
App: protolens
Refs: docs/specs/0271-a-script-walks-the-reader-through-the-blob.md (the
      script step model this spec extends);
      docs/specs/0355-script-navigation-keybindings-overhaul.md (the
      advance/retreat model this spec augments)

## Background

A script step currently advances only on an explicit `space` / `Backspace`
press. For a live demo this means the presenter must remember to press
`space` after every action they perform — the script commentary says "press
`w`", the presenter presses `w`, then has to press `space` before the text
updates. The extra keystroke breaks the flow and makes the script feel
mechanical.

The fix is to let a step declare an **`advance_when`** condition: a predicate
over the session state. After every user action, protolens evaluates the
predicate; when it becomes true, the step advances automatically. The presenter
performs the action the text invited, and the script responds — no `space`
needed.

## Goals

- **G1.** A step may declare an `advance_when:` list of predicate items. When
  all items hold simultaneously, the step advances to the next step
  automatically.
- **G2.** `space` and `Backspace` remain unconditional: they always
  advance/retreat regardless of whether `advance_when` is defined.
- **G3.** `advance_when` is evaluated immediately after `script_apply` (i.e.
  on step entry). If it is already satisfied at that point the step
  advances without waiting for user input — a script bug, but an honest
  one: the step's own setup satisfied its own exit condition.
- **G4.** Eight predicate keys: `visible`, `folded`, `wire`, `type`,
  `caret`, `annotations`, `heat_cues`, and `not`. Each is a flat
  single-key mapping item; the key names the predicate directly.
- **G5.** `not` takes a nested list of predicate items and negates their
  conjunction; it is itself a predicate item and may be nested.
- **G6.** All items in a list are AND'd; an empty list never fires.
- **G7.** `or` is reserved and rejected at parse time with a clear error
  message. It may be introduced in a future spec if a concrete need arises.
- **G8.** A step may declare `annotations:` and `heat_cues:` directives that
  set the corresponding session mode when the step is applied.

## Non-goals

- **N1.** No event-based predicates (watching for a specific keypress). The
  predicate is purely over observable session state, evaluated after any
  action.
- **N2.** No OR semantics within an `advance_when` list in this spec.
- **N3.** No branching: `advance_when` always advances to the immediately
  following step, same as `space`.
- **N4.** `space` and `Backspace` cannot be predicated away — they remain
  unconditional advance/retreat.
- **N5.** No `state` grouping key. Each state value (`visible`, `folded`,
  `wire`) is its own predicate key. This makes typos parse errors rather
  than runtime failures and removes the need to split a two-token string.

## Specification

### S1 — YAML format

A step may include an `advance_when:` key whose value is a list of predicate
items. Each item is a single-key mapping. The six recognized keys are
`visible`, `folded`, `wire`, `type`, `caret`, and `not`. All items are
AND'd. The `or` key is rejected at parse time.

```yaml
steps:
  - text: |
      Press w to see the wire bytes for this node.
    node: /2/1
    advance_when:
      - wire: /2/1
```

```yaml
  - text: |
      Apply the correct type and make the node visible.
    node: /2
    advance_when:
      - type: /2 google.maps.places.v1.SearchTextRequest
      - visible: /2
```

```yaml
  - text: |
      Move to the shadowed field.
    advance_when:
      - caret: /1
```

```yaml
  - text: |
      Close the wire view for this node.
    node: /2/1
    advance_when:
      - not:
          - wire: /2/1
```

```yaml
  - text: |
      Unfold the root.
    node: /
    advance_when:
      - not:
          - folded: /
```

```yaml
  - text: |
      Move away from this node and close its wire view.
    node: /2/1
    advance_when:
      - not:
          - wire: /2/1
          - caret: /2/1
```

```yaml
  - text: |
      Press `i` to enable heat cues.
    advance_when:
      - heat_cues: findings
```

```yaml
  - text: |
      Toggle annotations off.
    advance_when:
      - annotations: false
```

### S2 — Predicate semantics

Each predicate item is a single-key mapping. All position values are
resolved the same way as `node:` — a positional path or a search string
(spec 0271 S3).

**`visible: <position>`**

The node is rendered and not hidden inside a folded ancestor.

**`folded: <position>`**

The node itself is folded (has children and they are collapsed). False
for a leaf — a node with no children is never folded.

**`wire: <position>`**

The wire span currently covers this node's line.

**`type: <position> <fqdn-or-primitive>`**

The effective type protolens has resolved for the node at `<position>`
(after any user override) matches `<fqdn-or-primitive>` exactly. The
comparison is case-sensitive and against the full type name as displayed
in the status line (e.g. `google.maps.places.v1.SearchTextRequest`).
The value is two whitespace-separated tokens: position then type name.

**`caret: <position>`**

The cursor is currently on the node at `<position>`.

**`annotations: <bool>`**

`true` holds when the annotation pane is visible (`self.annotations == true`);
`false` holds when it is hidden. Accepted values: `true`, `false`.

**`heat_cues: <mode>`**

Holds when `self.heat_cues` matches `<mode>`. Accepted values: `off`,
`findings`, `all` (case-sensitive, matching `HeatCueMode`'s display names).

**`not: <list>`**

`<list>` is a conjunction of predicate items with the same recursive
structure. `not` holds iff that conjunction is false — i.e. at least one
item in the list is false. `not: []` is always false (negation of the
empty conjunction, which is vacuously true).

`not` may appear inside another `not` list; double negation is legal and
equivalent to the inner predicate.

### S3 — Evaluation

`advance_when` is evaluated:

1. Immediately after `script_apply` completes for the current step (G3).
2. After every `handle_key` call that does not itself cause a step advance.
3. After every `handle_mouse` call.

If all predicates hold, `script_advance(true)` is called. The advance
itself triggers another `script_apply`, which may in turn satisfy the next
step's `advance_when` — a chain of instantly-satisfied steps is legal and
terminates at the last step.

Evaluation is skipped while navigation is off (`!script_active()`).

### S4 — Parse errors

The following are hard errors at script load time, reported the same way
as other step errors (spec 0271 S13):

- An `or` key present anywhere in an `advance_when` list.
- An unknown key in a predicate item (any key other than `visible`,
  `folded`, `wire`, `type`, `caret`, `annotations`, `heat_cues`, `not`).
- A predicate item mapping with more than one key.
- A `type` value with fewer than two whitespace-separated tokens.
- A `not` value that is not a list (e.g. a scalar).
- A `heat_cues` predicate or directive value that is not one of `off`,
  `findings`, `all` — caught by serde's `HeatCueMode` deserializer.

Resolution errors (position names no node) are runtime, not load-time,
matching spec 0271 S13.

### S5 — Unresolvable positions

An `advance_when` predicate whose position resolves to no node at evaluation
time is treated as **false** (the predicate is not satisfied). This is
consistent with spec 0271 S13's principle that a broken position degrades
gracefully rather than stopping the script. The step can still be advanced
manually with `space`.

### S6 — Step struct extension

**`HeatCueMode` visibility (DRY):** `tui::heat_cue::HeatCueMode` is promoted
from `pub(super)` to `pub(crate)` and gains `#[derive(Deserialize, PartialEq)]`
so that `script.rs` can use it directly without a parallel definition.
`annotations` is a plain `bool` — no new type needed.

`Step` gains an `advance_when: Vec<Predicate>` field (empty by default, never
fires), plus two mode-directive fields:

```rust
pub enum Predicate {
    Visible     { position: Position },
    Folded      { position: Position },
    Wire        { position: Position },
    Type        { position: Position, fqdn: String },
    Caret       { position: Position },
    Annotations { on: bool },
    HeatCues    { mode: HeatCueMode },
    /// Conjunction of `inner` is negated.
    /// `inner` empty → negation of vacuous truth → always false.
    Not         { inner: Vec<Predicate> },
}
```

`Not` is recursive: `Predicate` contains `Not { inner: Vec<Predicate> }`,
requiring `Box` or indirection on `inner` in the compiled form.

`RawStep` gains an `advance_when: Option<Vec<RawPredicate>>` field plus two
directive fields:

```rust
annotations: Option<bool>,           // step directive: set annotations mode
heat_cues:   Option<HeatCueMode>,    // step directive: set heat-cue mode
```

`into_step` converts them to the `Step` struct fields `set_annotations:
Option<bool>` and `set_heat_cues: Option<HeatCueMode>`.

`RawPredicate` is an untagged enum whose arms are recognized by which key is
present. `HeatCueMode` derives `Deserialize` so it can be used directly:

```rust
#[serde(untagged)]
enum RawPredicate {
    Visible     { visible: String },
    Folded      { folded: String },
    Wire        { wire: String },
    Type        { r#type: String },
    Caret       { caret: String },
    Annotations { annotations: bool },
    HeatCues    { heat_cues: HeatCueMode },
    Not         { not: Vec<RawPredicate> },
}
```

`deny_unknown_fields` on each arm catches unknown keys at parse time.

### S8 — Step directives for mode control

A step may include `annotations:` and `heat_cues:` keys at the step level
(not inside `advance_when:`). These are applied by `script_apply` in the same
pass as `node:`, `fold:`, and `command:`.

```yaml
  - text: |
      Press `i` to enable heat cues.
    annotations: true
    heat_cues: off
    advance_when:
      - heat_cues: findings
```

**`annotations: <bool>`** — set `self.annotations` to `true` or `false`.

**`heat_cues: <mode>`** — set `self.heat_cues` to `off`, `findings`, or `all`.

Conflict with the same-named `advance_when` predicate keys is intentional and
explicit: the directive sets the mode on step entry; the predicate checks the
mode (possibly after the user changes it) as an exit condition.

### S7 — Evaluation helpers

A new `App` method:

```rust
fn script_advance_when_satisfied(&self) -> bool
```

Returns true iff every predicate in the current step's `advance_when` holds.
Called from `script_apply` (post-apply check, G3) and from the tail of
`handle_key` / `handle_mouse` (post-action check, S3).

Each predicate kind delegates to existing `App` queries:
- `Visible`: no ancestor of the node is folded.
- `Folded`: `is_folded(idx) && has_children(idx)`.
- `Wire`: the node's line range intersects `self.wire`.
- `Type`: `self.effective_type(idx)` matches the fqdn string exactly.
- `Caret`: `self.cursor == idx`.
- `Annotations { on }`: `self.annotations == on`.
- `HeatCues { mode }`: `self.heat_cues == mode`.
- `Not`: evaluate `inner` as a conjunction and negate.

## Alternatives considered

### Event-based predicates (watch for a specific keypress)

Rejected (N1): a key press and its observable effect are not the same
thing. `w` toggles wire bytes — if the step is entered with wire already
on (because the previous step left it there and `script_reset` cleared
it), pressing `w` turns it *off*, and the `advance_when` condition is still
wire bytes being visible. State-based predicates are correct in all cases;
key-based ones require the script author to reason about entry state,
which contradicts spec 0271's "a step declares a view" principle.

### Arm flag to prevent immediate firing

An `armed` boolean set after step entry, cleared after the first user
action, would prevent G3's immediate advance. Rejected: since `script_apply`
fully defines the starting state (spec 0271 S6), an `advance_when` satisfied
at entry is unambiguously a script bug — the step's own directives satisfy
its own exit condition. Advancing immediately is the correct and honest
response, and makes the bug visible rather than hiding it behind an
artificial delay.

### `state: <position> <value>` grouping key

An earlier draft grouped `visible`, `folded`, and `wire` under a single
`state` key with a two-token value (`state: /x wire`). Rejected: the
value token is parsed at runtime from a free string, so a typo (`wrie`)
silently becomes a runtime false rather than a parse error. Flat keys
(`wire: /x`) are validated at load time by serde's untagged enum
dispatch, catching the mistake immediately.

### `invisible` and `unfolded` as additional flat keys

Would be symmetric with `visible` and `folded` but redundant — two names
for the same bit. `not: [{visible: /x}]` and `not: [{folded: /x}]` are
unambiguous and apply the same `not` mechanism that works for `wire`,
`type`, and `caret` too.

### OR semantics within an `advance_when` list

Reserved as `or` for a future spec. `not` already gives De Morgan access
to OR: `[{not: [{not: [A]}, {not: [B]}]}]` = A OR B, but that form is
too verbose to encourage for real use cases.

## Test plan

1. `advance_when_wire_advances_on_w` — a step with `wire: /1`; pressing `w`
   satisfies the predicate and the step index increments without `space`.
2. `advance_when_not_satisfied_does_not_advance` — same setup; pressing a key
   that does not affect wire state leaves the step unchanged.
3. `advance_when_fires_immediately_if_satisfied_at_entry` — a step whose
   `script_apply` already satisfies its own `advance_when` skips to the next
   step on entry (G3).
4. `advance_when_caret_predicate` — `caret: /2` fires when the cursor moves
   to `/2`, not before.
5. `advance_when_type_predicate` — `type: /2 some.Type` fires after an
   override sets that type, not before.
6. `advance_when_visible_and_folded_predicates` — `visible: /1` and
   `folded: /1` hold and fail at the right times; a leaf is never folded.
7. `advance_when_all_predicates_must_hold` — an `advance_when` with two items
   does not fire when only one holds.
8. `advance_when_unresolvable_position_is_false` — a predicate whose position
   names no node evaluates to false; `space` still advances.
9. `advance_when_not_inverts_predicate` — `[{not: [{wire: /1}]}]` fires when
   wire is off, not when it is on.
10. `advance_when_not_of_conjunction` — `[{not: [{wire: /1}, {caret: /1}]}]`
    fires when at least one sub-predicate is false (De Morgan).
11. `advance_when_not_is_recursive` — `not` inside `not` double-negates; the
    result is equivalent to the inner predicate.
12. `advance_when_or_key_is_a_load_error` — a step with an `or` item in its
    `advance_when` list fails at parse time with a message naming the key.
13. `advance_when_unknown_key_is_a_load_error` — `dancing: /1` in an
    `advance_when` list fails at parse time with a message naming the key.
14. `space_always_advances_regardless_of_advance_when` — with `advance_when`
    not yet satisfied, `space` still advances the step (G2).
15. `advance_when_annotations_predicate` — `annotations: true` fires when
    the annotation pane is visible; `annotations: false` fires when hidden.
16. `advance_when_heat_cues_predicate` — `heat_cues: findings` fires when
    `HeatCueMode::Findings` is active; does not fire for `Off` or `All`.
17. `step_directive_annotations_sets_mode` — a step with `annotations: false`
    disables the annotation pane on entry; `annotations: true` re-enables it.
18. `step_directive_heat_cues_sets_mode` — a step with `heat_cues: all` sets
    `HeatCueMode::All` on entry; `heat_cues: off` sets `HeatCueMode::Off`.
19. `step_directive_heat_cues_bad_value_is_load_error` — `heat_cues: maybe`
    in a step fails at parse time.
20. `reuse lint` passes.

## Measured outcome

(To be filled in at implementation.)
