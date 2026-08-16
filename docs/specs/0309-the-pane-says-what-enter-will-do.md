<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0309 — the pane says what `Enter` will do

Status: implemented
Implemented in: 2026-08-16
App: protolens
Refs: docs/specs/0308-….md (S1, the widest-first default this narrows
        and makes visible),
      docs/specs/0299-….md (the `message` keyword and its schema-free
        synthetic descriptor),
      docs/specs/0117-….md (§2, the three origin kinds),
      docs/specs/0124-….md (G2, the management pane's `z`/`Z`),
      docs/specs/0200-….md (S3, an existing entry's own kind still wins),
      docs/specs/0147-….md (G2, the selection pane's local statusline)

## Background

Spec 0308 made a new override as wide as the node can express —
`fqdn:field` when there is one. Three things followed from that which
the pane never said out loud.

**The origin was invisible.** The selection pane's statusline read
`/1/3 - inferred types`: the node's positional path, which after 0308 is
usually *not* what `Enter` will record. A reader could only find out by
confirming and then opening the management pane.

**There was no way to say "narrower".** 0308 N1 said the reader who
wants this node and no other is "one keystroke away" via the management
pane's `z`/`Z`. They were not: that keystroke is in the *other* pane,
reached by confirming a wide entry first and retyping it. Worse,
`OverrideKind::next`'s doc comment claimed `z` already rotated in the
selection pane. Nothing called it.

**`message` acquired a reach it cannot justify.** Spec 0299's `message`
is what a reader picks when no schema fits these bytes. `fqdn:field`
spreads that verdict across every node of the parent's declared type,
which is a claim about the schema. The same defect appears one level
down: the synthetic descriptor is registered as
`protolens_internal.message`, so a *child* of a `message`-overridden
node derived the origin `protolens_internal.message:1` — a field number
under "unknown", matching every node anyone ever overrode to `message`.
That FQDN exists only because prost-reflect requires every descriptor to
have a name; it is not a type and must not reach an origin.

## Goals

- **G1.** The pane states the override `Enter` would create, before
  `Enter`.
- **G2.** The reader can choose the origin kind where they are choosing
  the type, not in a later pane.
- **G3.** No origin names the schema-free `message` synthetic, in either
  half of a `fqdn:field`.

## Non-goals

- **N1.** Removing `protolens_internal.message`. prost-reflect resolves
  message types by name; the synthetic needs one to be spliceable. What
  0299 promised is that the *keyword* is what the reader types, selects
  and reads — and that holds: `override_candidates` stores the bare
  `message`, `wrapper_target_for` resolves it, and the `#@` annotation
  on the spliced header says `message`. G3 closes the one place the
  internal name escaped.
- **N2.** The management pane's forgiving multi-candidate rotation (spec
  0134 G2 — `manage_pending_kind`, the "try again for …" retry). It
  exists because an entry there can affect several nodes and the right
  one is ambiguous. The selection pane has exactly one target, so the
  rotation just skips what does not fit.
- **N3.** Showing the origin *kind* by name. The origin's own spelling
  already distinguishes all three: `/1/3`, `/1:3`, `pkg.Msg:3`.
- **N4.** Reconsidering spec 0200 S3. A pane opened from the management
  pane still starts pinned to that entry's kind; `z` from there is a
  deliberate change to it, which is the same thing `z` means everywhere
  else.

## Specification

- **S1.** `override_origin_if_it_fits(idx, kind, type)`
  (`tui/override_apply.rs`) is the single predicate behind the default
  ladder, the rotation and the projection, so none of the three can
  offer a kind another refuses. It is `origin_for_kind` plus two
  refusals:

  1. `field_number == 0` — the virtual-wrapper sentinel — rules out
     both field-scoped kinds (spec 0308 S1's guard, moved here);
  2. `type == message` rules out `fqdn:field`.

  G3's other half is not here but in `origin_for_kind`'s `FqdnField`
  arm, which now errors when the parent's resolved type is
  `SCHEMA_FREE_MESSAGE_FQDN`. Spec 0308 N2 left `origin_for_kind`
  untouched; this reverses that, because the refusal is about what an
  origin can *name* and belongs beside the existing "parent's type is
  unresolved" error — a schema-free parent is that same condition,
  declared by the reader instead of discovered. Putting it there also
  covers the management pane's own `z`, which builds origins through
  the same call.

  `override_origin_for_kind` accordingly takes the chosen type and
  walks `[FqdnField, PathField]` through the predicate before falling
  back to `Path`, which always fits.

- **S2.** `z`/`Z` in the selection pane rotate the projected kind one
  step round the three-kind barrel, skipping what S1 refuses, and store
  the result in `override_origin_kind` — the field spec 0200 S3 already
  uses for "the kind this pane will confirm under", rather than a second
  one that would have to agree with it. `close_override` already clears
  it. Landing back on the kind it started from means that kind is the
  only one available, and says so.

- **S3.** `projected_override_origin()` is what `Enter` and the
  statusline both call: the pinned `override_origin_kind` when there is
  one, else S1's ladder under the highlighted type.

- **S4.** The pane's statusline (spec 0147 G2) reads
  `override <origin> as <type> - <mode label>`, where `<origin>` is
  S3's label and `<type>` is the highlighted candidate's last
  `.`-segment. The short name because this row is a reminder of the row
  highlighted a few lines above, not a second place to read a FQDN, and
  a full name would crowd out the mode label on a pane this narrow. With
  no candidate highlighted — a cold heat cache — the ` as <type>` clause
  is omitted rather than filled with a placeholder.

- **S5.** `<mode label>` names the list `i` would switch *to*, not the
  one on screen: `i → all types` while the inferred list is up, and
  `i → inferred types` while it is not. Spec 0147 G2 named the current
  mode, which the reader can already see; what they cannot see is that
  another list exists and which key produces it. The arrow is U+2192,
  one column wide.

## Alternatives considered

**Show the key hints instead: `override /1:3 (Esc/Enter/z/i)`.** Two of
the four keys in the first draft of that list did nothing in this pane,
which is the argument against it: a hint list is a second inventory of
the bindings, drifting against `key_dispatch.rs` exactly as
`help_text.rs` has. State is checkable against the code that produces
it; a key list is not.

**Refuse `message` all the way down to plain `path`.** `path:field`
names the field under one specific parent, which is a positional claim,
not a schema one — the same claim the reader made by pointing at the
node. Only `fqdn:field` generalizes across the schema, so only
`fqdn:field` is wrong here.

**A separate `override_kind_choice` field for the `z` pin.** It would
have to lose to, or beat, `override_origin_kind` on every read, and
every reader of either would need to know which. They mean the same
thing — "confirm under this kind, not the derived one" — so they are one
field.

**Let the rotation report a refusal and stay put** (the management
pane's `manage_pending_kind` dance). N2: with a single target there is
nothing to disambiguate, and a `z` that sometimes moves and sometimes
prints is worse than one that always lands somewhere valid.

## Test plan

1. `z_pins_the_projected_kind_and_enter_builds_it` — the full barrel
   forward, one step back, then `Enter` builds the pinned kind rather
   than 0308's default. Driven through `handle_key`, because the
   binding's existence is half the claim.
2. `message_never_reaches_the_fqdn_field_kind` — the default falls one
   rung and no further; `z` skips `fqdn:field` and comes back round;
   and, after confirming, a child of the reinterpreted node gets
   `path:field` while `origin_for_kind(child, FqdnField)` errors.
3. `the_override_statusline_projects_the_origin_and_the_short_type` —
   the rendered row, clipped to `side_area`'s columns because the main
   pane's own statusline shares the row, before and after `z`.
4. `a_new_overrides_kind_is_the_widest_the_node_can_express` gains the
   `message` rung beside spec 0308's three.
5. `override_statusline_wording_differs_by_sort_mode` moves from spec
   0147 G2's wording to S5's, and from `contains` to an exact tail:
   "all types" is a substring of both wordings, so a `contains` would
   have passed in either mode.

## Measured outcome

**Implemented 2026-08-16.** 1078 protolens unit tests pass — 1075
before, plus tests 1-3. One existing expectation moved, test 5's, and
only because S5 changed the wording; `override_origin_for_kind` gained a
parameter, and every caller in the suite passes the type it is actually
about (or `None`).

S4 and S5 together make the left half about 20 columns longer than spec
0147 G2's. On a 120-column terminal the pane is 59 wide, and
`statusline_text` then truncates the left with a leading `<` to keep the
right-hand `L…/…  …` intact — which is the correct priority, the
viewport counters being the half that cannot be inferred from anything
else on screen. The two rendering tests use a 200-column backend so that
they assert the whole label rather than the truncation.

`help_text.rs` gained the `z`/`Z` entry. It is not checked against the
bindings — `tests/help_text.rs` scans for case-insensitive `Char` keys
only, which is how the drift this spec found survived — so the entry is
a claim, not a test.
