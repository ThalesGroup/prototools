<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0321 — one pane, one answer

Status: implemented
Implemented in: 2026-08-18
App: protolens
Refs: docs/specs/0236-… (`o` pre-fills the whole `:override`),
        docs/specs/0237-… (S4: the pre-fill takes the applicable
        entry's origin), docs/specs/0185-… (S5 focus lock, S6 the
        overlay must not outlive a splice), docs/specs/0200-…
        (a pane opened from the manage pane returns there),
        docs/specs/0308-… (the widest-first default kind),
        docs/specs/0309-… (the status line says what `Enter` will do)

## Background

The selection pane offers two ways to commit an override, and they
disagree about the most consequential thing an override has — its reach.

`Enter` builds `projected_override_origin()`: the pinned
`override_origin_kind` when there is one, else spec 0308's widest-first
ladder under the highlighted type. Spec 0309 put that same call in the
pane's status line, so the reader is shown the origin `Enter` will
create, character for character.

`o` ignores all of it. Its selection-pane branch takes the origin of
whatever entry currently covers the node, and a bare `path` when none
does (spec 0237 S4). So on an uncovered node the status line reads
`fqdn:field` and the line `o` opens reads `path` — the two exits from
one pane, describing the same keystroke's subject two different ways.

Two smaller faults sit beside it:

- Running `:override` from inside the pane leaves the pane open, still
  asking a question the command has just answered, with a candidate
  highlighted that is no longer the node's type. Worse, the splice in
  `run_override_cmd` runs while the preview overlay is still alive,
  which is exactly what spec 0185 S6 forbids: the overlay's anchor is a
  row position the splice invalidates.
- The status line joins a value and an affordance with a dash —
  `override <origin> as <Type> - i → all types`. The left half is what
  `Enter` will do; the right half is a key you may press. A dash reads
  as joining two peers.

## Goals

- **G1.** In the selection pane, `o` pre-fills the origin the status
  line is projecting — the same one `Enter` would build.
- **G2.** Committing `:override` closes the selection pane.
- **G3.** The status line subordinates its mode hint rather than
  conjoining it.

## Non-goals

- **N1.** The manage pane's `o`. There the subject is an existing
  entry, whose origin is a deliberate choice made with `z`/`Z` (spec
  0124 G2); re-deriving a default over it would be the defect, not the
  fix. Untouched — the branch this changes is reached only from the
  selection pane, since the main pane's `o` opens the manage pane
  (spec 0236 S19) and the manage pane's takes the entry branch.

- **N2.** Keeping spec 0237 S4's anti-narrowing guarantee in the
  selection pane. It is superseded here rather than preserved, and the
  loss is smaller than it looks: the ladder is *widest*-first, so a
  node covered by an `fqdn:field` entry projects that same
  `fqdn:field` and `o`-then-`Enter` remains the no-op S4 was written to
  protect. The case that changes is a node covered by a **narrower**
  entry — a `path` override, say — where `o` now pre-fills the wider
  projection and `Enter` creates a new entry beside the old one instead
  of editing it. That is precisely what `t`-then-`Enter` does today, and
  "the same rules as `t`" is the requirement. `z`/`Z` pins the kind for
  readers who want the narrow one, and the status line shows which is
  in force before either key is pressed.

- **N3.** Dropping the pane's `Tab` binding. `Tab` there is not a route
  to anything; it is an explanatory refusal, setting
  `OVERRIDE_FOCUS_LOCK_MESSAGE` because spec 0185 S5 locks focus to the
  pane and the main pane's `Tab` (focus the manage pane) cannot fire.
  Deleting the arm frees the key for nothing and makes it read as
  broken, which is the specific outcome the arm's comment exists to
  prevent. `o` does not replace it: they answer different questions.

- **N4.** The two further changes implied by the example accompanying
  the request for G3 — dropping the leading `override ` word, and
  spelling the mode label "show all types". Only the dash was asked for
  in the sentence; the prefix is what makes the row a sentence rather
  than a fragment, and "all types" is spec 0309 S5's wording. Both are
  one-line changes if wanted.

## Specification

- **S1.** `prefill_override_cmd`'s selection-pane branch takes its
  origin from `projected_override_origin()`. On error it falls back to
  the subject's bare `path`, matching the status line's own fallback
  (spec 0309 S4) so that the two still agree when no origin projects.

  The pre-filled `--field-name` continues to come from the entry that
  the chosen origin names, if one exists — the origin decides which
  entry is being described, so the name must follow it and not the
  covering entry it may no longer be.

  `--as` is unchanged: the target's *current* type, not the highlighted
  candidate (spec 0236 S15). Picking from the list is what `Enter` is
  for.

- **S2.** `run_override_cmd` closes the selection pane, if open, after
  `activate`/`rename` and **before** `render_overrides`. Before,
  because `close_override` drops the preview overlay and spec 0185 S6
  requires that to happen ahead of the splice. Closing also restores
  the manage pane when the selection pane was opened from it (spec 0200
  S2), which is why the highlight is set from `entry_index_of` *after*
  the close — `manage_open` is not true until then.

  Only on the success path. Every early return above leaves the pane
  up, since a refused command has not answered anything.

- **S3.** The status line parenthesizes its mode label:
  `override <origin> as <Type> (i → all types)`.

## Alternatives considered

**Pin `override_origin_kind` when `t` lands on a covered node.** This
would make `o` and `Enter` agree by moving both onto the covering
entry's kind, preserving spec 0237 S4 for `o` and extending it to
`Enter`. Rejected because it changes `t`-then-`Enter`, which was not
asked for and which spec 0308 settled deliberately: the default kind is
derived from the node, and a pin is a thing the reader does with `z`/`Z`.
It also silently makes the pin's provenance ambiguous — `close_override`
clears the kind precisely so a pin cannot leak into the next opening.

**Close the pane on `o` rather than on the command's commit.** Tempting
— the command line is a second editor for the same subject — but `o` is
abandonable with `Esc`, and closing the pane would make an abandoned
`o` destroy the candidate list the user spent a scroll finding.

## Test plan

1. `o_in_the_selection_pane_prefills_the_projected_origin` — on an
   uncovered node, the line `o` opens names the same origin the status
   line shows, and `z` moves both together. S1.
2. `o_prefills_the_applicable_entry_origin` — the existing spec 0237 S4
   test. Still passes unchanged: the covering entry there is
   `fqdn:field`, which is the ladder's first rung. Its comment is
   restated in terms of N2.
3. `committing_override_closes_the_selection_pane` — with the pane open,
   `:override` on the target: `override_target` is `None` afterwards and
   the entry exists. S2.
4. `a_refused_override_leaves_the_selection_pane_open` — a
   wire-incompatible keyword: the pane is still up and the message is
   the refusal. S2's success-path scoping.
5. The two existing status-line assertions in `tests/override_select.rs`
   are re-pinned to the parenthesized form. S3.

## Measured outcome

Not a performance change; the outcome is behavioral and pinned by the
tests above. The workspace suite passes (1110 protolens tests, with and
without `COLORTERM`), as do `cargo fmt --all --check`, `cargo clippy
--no-default-features --workspace -- -D warnings` and `reuse lint`.

Two things the implementation confirmed that the design only argued:

- **N1 needs no gate in the code.** The branch S1 changes is already
  unreachable from anywhere but the selection pane — the main pane's `o`
  is `toggle_manage_pane` and the manage pane's takes the entry branch
  above it — so `projected_override_origin()` is only ever called there,
  where `override_target` is `Some`. The fallback in S1 covers a
  projection failure, not a missing pane.
- **N2's claim about spec 0237 S4 held.** Its test
  (`o_prefills_the_applicable_entry_origin`) passes **unchanged** under
  the new rule, because the covering entry it uses is `fqdn:field` and
  that is the ladder's first rung. Only its doc comment moved.
