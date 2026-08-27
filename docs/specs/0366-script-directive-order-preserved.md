<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0366 — script step directives execute in YAML key order

Status: implemented
Implemented in: 2026-08-27
App: protolens
Refs: docs/specs/0271-a-script-walks-the-reader-through-the-blob.md (the
      script step model this spec extends);
      docs/specs/0357-script-select-and-search-directives.md (introduced
      `select_line:` and `search:`, whose interaction exposed the ordering
      problem)

## Background

`script_apply` processes step directives in a hardcoded sequence regardless
of the order in which the script author wrote them.  The ordering was:

```
folds → node → (select_node) → wire → search → (select_line) → focus → command
```

This caused a concrete bug: a step with

```yaml
node: /1/3/1
select_node: true
search: SomeType
```

selected the node the *search* landed on (inside `/1/3/1`) rather than the
subtree of `/1/3/1` itself, because `select_node` was evaluated after
`search` in the hardcoded sequence, not before it as the YAML ordering
implies.

More generally, the hardcoded order forces the script author to know the
internal evaluation sequence rather than expressing intent through the
written order of keys.

## Goals

- **G1.** Each step is represented internally as an ordered list of
  directives.  The list preserves the order in which the keys appear in
  the YAML source.
- **G2.** `script_apply` executes directives in that list order, so the
  YAML key order is the execution order.
- **G3.** The set of available directives and their semantics are
  unchanged.  Only the dispatch mechanism changes.

## Non-goals

- **N1.** No change to which directives exist or what they accept.
- **N2.** No YAML-level validation of key ordering — the author chooses
  the order; the engine honours it.
- **N3.** No YAML syntax change. Existing scripts require no rewriting;
  the key order they happen to have continues to work correctly because
  the execution order now matches whatever the author wrote.
- **N4.** `text:`, `annotations:`, `heat_cues:`, `advance_when:`, and
  `command:` are not made order-sensitive (see S3).

## Specification

### S1 — Directive enum

A new `Directive` enum in `script.rs` covers every per-step view action:

```rust
pub enum Directive {
    Node(Position),
    Fold(Vec<FoldEntry>),
    Wire(Option<Wire>),        // None clears the wire panel
    SelectLine,
    SelectNode,
    Search(String),
    WireClear,
    // (future directives added here)
}
```

### S2 — `Step` carries an ordered directive list

`Step` gains a field:

```rust
pub directives: Vec<Directive>,
```

The existing named fields (`node`, `fold`, `wire`, `select_line`,
`select_node`, `search`) are removed from `Step`.  Anything that
previously read those fields reads `directives` instead by iterating in
order.  `advance_when`, `set_annotations`, `set_heat_cues`, `prefill`,
and `text` remain as named fields (see S3).

### S3 — Keys that remain position-insensitive

Five keys are applied at fixed points regardless of where they appear
in the YAML, and remain as named fields on `Step`:

- `text:` — the commentary string; always displayed, not a view action.
- `annotations:` and `heat_cues:` — mode switches applied before any
  view directives so heat-cue visibility is correct when the view is
  composed (spec 0356 S8).  Writing them after `node:` in the YAML
  would still apply them first, but doing so would be confusing; script
  authors should keep them before the view directives by convention.
- `advance_when:` — evaluated after the view is composed; its position
  in the YAML has no bearing on when it fires.
- `command:` — prefills the command line after the view is set.

All other keys (`node:`, `fold:`, `wire_line:`, `wire_lines:`,
`wire_node:`, `select_line:`, `select_node:`, `search:`) are
position-sensitive: they become `Directive` variants and execute in
the order written.

### S4 — Deserialisation

`RawStep` is replaced by a two-phase deserialiser:

1. Deserialise the YAML mapping as a `serde_norway::Value` (preserving
   key order, since serde_norway's `Mapping` type is insertion-ordered).
2. Walk the mapping's entries in order.  For each key:
   - Named pre/post-pass keys (`text`, `annotations`, `heat_cues`,
     `advance_when`, `command`) are collected into their respective
     named slots.
   - All other keys are parsed into `Directive` variants and appended
     to `directives` in encounter order.
   - An unrecognised key is a load error (preserving the existing
     `deny_unknown_fields` behaviour).

This replaces the current `#[derive(Deserialize)]` struct with an
explicit `impl Deserialize` (or a `from_value` conversion from a
pre-parsed `serde_norway::Value`).

### S5 — `script_apply` dispatch

The hardcoded helper call sequence in `script_apply` is replaced by a
loop over `step.directives`:

```rust
for directive in &step.directives {
    match directive {
        Directive::Node(pos)    => self.script_apply_cursor(pos, &mut errors),
        Directive::Fold(folds)  => self.script_apply_folds(folds, &mut errors),
        Directive::Wire(wire)   => self.script_apply_wire(wire, &mut errors),
        Directive::SelectLine   => self.script_apply_select_line(),
        Directive::SelectNode   => self.script_apply_select_node(),
        Directive::Search(pat)  => self.script_apply_search(pat, &mut errors),
    }
}
```

`script_focus` and `prefill` still run after the loop, as now.

### S6 — `advance_when` predicate evaluation

Predicates that currently read `self.cursor` (e.g. `Caret`, `Visible`,
`Folded`) are unaffected — they always read current state.  The `Node`
directive within the step sets the cursor; predicates that fire before
a `Node` directive has run in the current step will see the previous
step's cursor, which is the correct behaviour for a step that omits
`node:`.

## Alternatives considered

### Keep hardcoded order, document it

The immediate `select_node` bug could be fixed by placing `select_node`
before `search` in the hardcoded sequence.  Rejected because it
requires the author to know the internal order and offers no way to
express "search first, then select the match" vs "select the node
first, then search within it".

### `serde_norway::Value` with `IndexMap`

serde_norway's `Mapping` already uses `IndexMap` internally (insertion-
ordered).  Deserialising to `serde_norway::Value` and then walking the
mapping preserves key order without writing a custom `Visitor`.  This is
the approach specified in S4.

## Test plan

1. `select_node_before_search_selects_node_subtree` — a step with
   `select_node: true` written before `search:` selects the `node:`
   target's whole subtree, not the search-match line.
2. `search_before_select_node_selects_match_node_subtree` — a step with
   `search:` before `select_node: true` selects the subtree of the node
   the search landed on.
3. `select_node_covers_the_whole_subtree` — `select_node: true` without
   `search:` spans from the node header to the last descendant line
   without overflow (regression for the `usize::MAX` overflow).
4. `select_node_is_not_affected_by_search` — existing test; must pass
   with `select_node` written before `search:`.
5. `directive_order_is_preserved` — a step that moves the cursor twice
   (`node: /1` then `node: /2`, if both are listed) ends on `/2`.
6. `unknown_key_is_still_a_load_error` — unrecognised key is rejected.
7. `reuse lint` passes.
