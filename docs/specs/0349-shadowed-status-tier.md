<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0349 — Shadowed status tier and dimmed vertical bars

Status: implemented
Implemented in: 2026-08-24
App: protolens
Refs: docs/specs/0343-the-last-one-wins-and-the-others-say-so.md (B9 —
        shadow bit raises own_status to NonCanonical; this spec replaces
        that with a dedicated tier)

## Background

Spec 0343 B9 makes a shadowed scalar contribute `Status::NonCanonical`
to its node's own status, so the roll-up carries the shadow signal to
every ancestor. The visual result is correct but too loud: a shadowed
scalar and a field with a genuine type annotation on it wear the same
amber filled diamond `◆` and roll up identically through their parents'
fold toggles. Being shadowed is less severe than having an annotation
conflict — it means the field's value is overridden by a later
occurrence, not that anything is structurally wrong. The eye cannot
distinguish the two from the margin alone.

Additionally, the vertical cursor bars (`│`) inherit the full-intensity
amber/red/blue of their owner node, making them as visually prominent as
the fold toggles they support. A bar speaks for a whole ancestry path,
not for a single node; drawing it at the same intensity as the toggle it
descends from overweights it.

## Goals

- **G1.** Introduce `Status::Shadowed` as a new tier between `Unbaked`
  and `NonCanonical`, so the ordering is:
  `Ok < Unbaked < Shadowed < NonCanonical < Invalid`.
- **G2.** A shadowed-only node (one whose worst non-canonical signal is
  solely from shadow bits) displays hollow glyphs in the fold margin:
  `◇` (leaf anomaly mark), `▽` (open fold toggle), `▷` (closed fold
  toggle). A node at `NonCanonical` or higher displays the existing
  filled glyphs.
- **G3.** `Status::Shadowed` uses the same amber color as
  `Status::NonCanonical` in every palette. The glyph shape (hollow vs
  filled) is the only visual distinction.
- **G4.** Vertical cursor bars (`│`) are drawn in a dimmed variant of
  their owner's status color — same hue, lower luminance — for all
  statuses that have a color (Unbaked, Shadowed, NonCanonical, Invalid).
  Fold triangles and anomaly diamonds are unaffected.

## Non-goals

- **N1.** No new annotation keyword or rendered text for `Shadowed`.
  The `shadowed_scalar` keyword introduced by spec 0343 B10 is
  unchanged.
- **N2.** No change to how the shadow bit is set or cleared (spec 0343
  B5/B6/B8 are untouched).
- **N3.** No per-node "shadow-only" flag stored separately. The tier
  ordering handles everything: `Shadowed` propagates up through `max`
  exactly like any other status, and is outranked by `NonCanonical`
  whenever a genuine annotation is present anywhere in the subtree.

## Specification

### Status lattice

- **S1.** `Status` gains a new variant `Shadowed`, inserted between
  `Unbaked` and `NonCanonical`:
  ```
  Ok = 0 < Unbaked = 1 < Shadowed = 2 < NonCanonical = 3 < Invalid = 4
  ```
  All existing comparisons (`>=`, `max`, etc.) continue to work without
  change because `Shadowed` sits strictly between its neighbors.

### own_status

- **S2.** In `own_status`, the shadowed rung (spec 0343 B9) raises to
  `Status::Shadowed` instead of `Status::NonCanonical`:
  ```rust
  if shadowed {
      worst = worst.max(Status::Shadowed);
  }
  ```
  A node that has both a shadow bit and a genuine non-canonical
  annotation reaches `NonCanonical` through `row_status`'s own
  contribution, so no explicit `max` with `NonCanonical` is needed.

### Glyph selection

- **S3.** `margin_glyph_of` selects the anomaly glyph based on the
  node's `status_of`:
  - `>= NonCanonical` → `◆` (ANOMALY_GLYPH, as today)
  - `== Shadowed`     → `◇` (HOLLOW_ANOMALY_GLYPH, U+25C7)
  - `< NonCanonical`  → no glyph (as today)

- **S4.** `fold_marker_of` selects the fold toggle glyph:
  - `status_of(idx) == Shadowed` → `▽` open (U+25BD), `▷` closed
    (U+25B7)
  - otherwise → `▼` open, `▶` closed (as today)

  "Otherwise" covers both `>= NonCanonical` (filled, as today) and
  `<= Unbaked` (no color, but still filled — the glyph shape is not
  determined by severity alone, only by whether the node is specifically
  at `Shadowed`).

- **S5.** Introduce three new constants in `render.rs` alongside the
  existing ones:
  ```rust
  pub(super) const HOLLOW_ANOMALY_GLYPH:   char = '◇'; // U+25C7
  pub(super) const FOLD_GLYPH_OPEN_HOLLOW: char = '▽'; // U+25BD
  pub(super) const FOLD_GLYPH_CLOSED_HOLLOW: char = '▷'; // U+25B7
  ```

### Color

- **S6.** `status_color` (and `status_color_in`) maps `Shadowed` to the
  same color as `NonCanonical` in every palette (RGB dark, RGB light,
  ANSI-16 dark, ANSI-16 light). No new palette entry is needed — the
  function returns `tier_color(Tier::NonCanonical, theme, rgb)` for both.

- **S7.** The `no_two_fold_margin_colors_share_a_neighborhood` test
  exempts the `(Shadowed, NonCanonical)` pair, as it already exempts the
  `NonCanonical` amber from self-collision. Both statuses share one hue
  by design; the hollow/filled glyph distinction is the differentiator,
  not the color.

### Vertical bars

- **S8.** Introduce `bar_status_color(status, theme) -> Option<Color>` in
  `theme.rs`. It returns a dimmed variant of `status_color` for statuses
  that have a color, `None` for `Ok`. Target luminance: approximately
  60% of the full-intensity value. Concrete RGB values (dark theme):

  | Status | Full color | Dimmed bar color |
  |---|---|---|
  | Unbaked | `#808080` | `#303030` |
  | Shadowed | `#EFB94E` | `#8F6E2E` |
  | NonCanonical | `#EFB94E` | `#8F6E2E` |
  | Invalid | `#E05C5C` | `#8A3636` |
  | Unknown (blue) | `#4D8FFF` | `#2D5599` |

  ANSI-16 fallbacks: `DarkGray`, `DarkYellow`, `DarkYellow`, `DarkRed`,
  `DarkBlue` respectively.

- **S9.** `bar_style` in `render.rs` calls `bar_status_color` instead of
  `margin_glyph_color`. Fold triangles, anomaly diamonds, and preview
  bars are unaffected.

## Alternatives considered

### Separate `is_shadow_only` flag

A per-node `is_shadow_only(idx)` computed from `own_status(idx, …,
false) < NonCanonical && is_shadowed(idx)` was the first design. It
requires calling `own_status` twice per node during `rebuild_status`, and
the logic is duplicated at every glyph call site. The new tier encodes
the same information in the existing status value at no extra cost.

### Dimmed amber for Shadowed, full amber for NonCanonical

Giving `Shadowed` its own lighter amber (e.g. 70% luminance) in
`status_color` was considered. Rejected: the glyph shape already carries
the distinction, and a second amber hue close to the first is harder to
read than one amber in two shapes. Two distinguishable signals (shape and
color) pointing at the same axis of severity is one too many.

### No new Status variant — use a second rolled array

A `status_rolled_ignoring_shadow` array, computed in parallel with
`status_rolled`, was the alternative to a new tier. It doubles the memory
and the pass work, and the consumer logic (`>= NonCanonical but
ignoring_shadow < NonCanonical`) is more complex than `== Shadowed`.

## Test plan

1. `shadowed_own_status_is_shadowed` — a slot with only a shadow bit
   reaches `Status::Shadowed`, not `NonCanonical`.
2. `shadowed_plus_annotation_is_non_canonical` — a slot that is both
   shadowed and carries a genuine non-canonical annotation reaches
   `NonCanonical`.
3. `shadowed_rolls_up_to_parent` — a parent of a shadowed-only leaf
   shows `Shadowed` in its rolled status.
4. `shadowed_outranked_by_non_canonical` — a parent with one shadowed
   child and one genuinely non-canonical child shows `NonCanonical`.
5. `hollow_anomaly_glyph_for_shadowed_leaf` — a shadowed leaf renders
   `◇` in the fold column.
6. `filled_anomaly_glyph_for_non_canonical_leaf` — an annotated leaf
   renders `◆` in the fold column.
7. `hollow_fold_toggle_for_shadowed_subtree` — a bracketed node whose
   subtree is entirely shadowed renders `▽`/`▷`.
8. `filled_fold_toggle_when_subtree_has_non_canonical` — a bracketed
   node with at least one non-canonical descendant renders `▼`/`▶`.
9. `bar_color_is_dimmed` — a `NonCanonical` node's cursor bar is drawn
   in the dimmed amber, not the full amber.
10. `shadowed_bar_color_matches_non_canonical_bar_color` — `Shadowed`
    and `NonCanonical` nodes produce identical bar colors.
11. `no_two_fold_margin_colors_share_a_neighborhood` — the existing test
    passes with `Shadowed` added to the margin hues list and the
    `(Shadowed, NonCanonical)` pair added to the exemptions.

## Measured outcome

- `Status::Shadowed` inserted at ordinal 2; `Unknown` shifted to 3,
  `NonCanonical` to 4, `Invalid` to 5.
- `own_status` raises to `Shadowed` (not `NonCanonical`) for shadow-only slots.
- Hollow glyphs (`◇ ▽ ▷`) render for `== Shadowed` nodes; filled glyphs for
  `>= NonCanonical` as before.
- `bar_status_color` returns ~60% luminance of the full status color; bars
  use it while toggles and diamonds are unaffected.
- Test-plan item 3 (purely-Shadowed parent via pipeline) is unreachable:
  the shadow sweep only marks scalars, and a repeated scalar always gets
  `repeated_singular` (NonCanonical) on the winning occurrence, so a
  parent's rolled status is always at least NonCanonical when a shadowed
  scalar is present. Item 3 is covered at the lattice level instead
  (`shadowed_rolls_up_via_max_and_is_outranked_by_non_canonical`).
- 1214 tests pass (7 new).
