<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0308 — a new override is about the field

Status: implemented
Implemented in: 2026-08-16
App: protolens
Refs: docs/specs/0208-….md (S2, which made the default plain `path` —
        this reverses it),
      docs/specs/0117-….md (§2, the three origin kinds and their
        derivation rules),
      docs/specs/0124-….md (G2, the manage pane's `z`/`Z`, which is how
        a reader now narrows an entry back down),
      docs/specs/0200-….md (S3, an existing entry's own kind still wins
        over the default)

## Background

Confirming a type in the override selection pane on a bare main-pane
node creates an entry, and something has to choose that entry's *kind*.
The choice has been made twice before:

- until 2026-07-29, `path:field`;
- since spec 0208 S2, plain `path`, on the ground that a reader points
  at one node and an entry naming that node's *parent* plus a field
  number reads as being about somewhere else.

Both settings share a premise this spec rejects: that a default should
address as few nodes as possible. In practice a reader who retypes a
field means the field. The nodes a narrow default declines to cover are
the other instances of the same field — which are exactly the nodes the
reader would have to walk to and retype one at a time, each producing
another entry in the management pane.

## Goals

- **G1.** A new override is expressed in terms of the field, when the
  document gives it enough to name one.
- **G2.** No node loses the ability to be overridden. Whatever cannot
  name a field is still addressable positionally.

## Non-goals

- **N1.** Removing the narrow kinds. `z`/`Z` in the management pane
  still rotate an entry through all three, and spec 0208's reader — the
  one who wants this node and no other — is now one keystroke away
  rather than zero.
- **N2.** Changing `origin_for_kind`. The per-kind derivations and their
  failure conditions are spec 0117 §2's and are untouched; only which
  kind is asked for changes.
- **N3.** Changing spec 0200 S3. A pane opened from the management pane
  on an existing entry still confirms under *that entry's* kind, which
  is the only thing that retypes it in place rather than shadowing it.
- **N4.** Consulting an already-applicable entry when deriving the
  default. A `t` on a node carrying an inherited `path` entry now
  creates an `fqdn:field` one beside it rather than replacing it. That
  is spec 0200 S3's defect reappearing on a path S3 does not cover, and
  it is a separate question from what the default should *be*.

## Specification

- **S1.** `override_origin_for_kind(idx)` returns the widest kind the
  node can express:

  1. `fqdn:field`, when the node has a field number and its parent's
     type resolves;
  2. `path:field`, when it has a field number;
  3. `path` otherwise.

  The ladder tries rather than re-derives. Rungs 1 and 2 are exactly
  `origin_for_kind`'s two failure modes — no parent, and an unresolved
  parent type — so it calls it and falls through on `Err`, and the two
  cannot drift apart.

  The field-number test is the one condition that is *not* a failure
  there: `field_number == 0` is the virtual-wrapper sentinel, and
  `path:field` would otherwise build an origin naming field 0.

  The wrapper root reaches rung 3 for both reasons at once, which is why
  the fallback is stated unconditionally rather than as "no parent".

  > **Extended by spec 0309 S1** (2026-08-16). The ladder takes the
  > chosen type, and its rungs are a shared predicate that also refuses
  > `fqdn:field` for the `message` keyword. N2 above is reversed there
  > too: `origin_for_kind` now errors on a schema-free parent.

## Alternatives considered

**Keep `path` and let `z`/`Z` widen.** This is spec 0208 S2, and its
argument is about *reading* the entry, not about what the entry does:
`fqdn:field` names a type and a field number, which is how the schema
names the thing the reader pointed at. Widening after the fact also
costs an entry rewrite in the management pane, whereas narrowing after
the fact does not — `z` on a fresh entry is the same one keystroke
either way, so the asymmetry favors starting wide.

**Prefer `path:field` over `fqdn:field`.** `path:field` is the narrower
of the two only by accident — it covers the same field under one parent
instead of under every node of that parent's type — and it is the one
spelling that reads as being about somewhere else, since its first half
is a path to a node the reader did not point at. If the default is going
to be about the field, it should say which type declares it.

**Consult the heat cache or the schema to decide.** Any rule richer than
"what can this node name" makes the default unpredictable from the row
under the cursor, which is the one thing a default has to be.

## Test plan

1. `a_new_overrides_kind_is_the_widest_the_node_can_express` — all three
   rungs, each reached by the condition that disqualifies the one above.
   Two fixtures: a typed one for rung 1 and for the wrapper root, and an
   untyped blob for rung 2, whose records hang off a root with no type.
   A rung is a property of the document, not something to fake by
   writing into a decoded tree.
2. `esc_and_enter_land_in_the_same_place_and_the_default_kind_returns`
   already pins what a `t` from the main pane creates; its expectation
   moves from `Path` to `FqdnField`. Reaching it through the real
   keypress path is what makes it the regression test.

## Measured outcome

**Implemented 2026-08-16.** One function, `override_origin_for_kind`
(`tui/override_apply.rs`); no other production code changed.

1075 protolens unit tests pass — 1074 before, plus test 1. Only one
existing test moved, test 2, which is the one that pinned the old
default; every other use of `override_origin_for_kind` in the suite
creates an entry and looks it up through the same call, so the kind is
transparent to them.

The comment at `tests/profiling.rs`'s nested-commit target selection was
inverted rather than deleted: its sibling-uniqueness filter had become
belt-and-braces under spec 0208's `path` default and is load-bearing
again, since `fqdn:field` reaches further than a positional path.
