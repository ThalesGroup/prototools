<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0233 — the caret is drawn the same wherever it rests

Status: implemented
Implemented in: 2026-08-03
App: protolens
Refs: docs/specs/0194-the-cursor-is-a-caret.md (S2, the caret; S4, the
        brace pair — this spec revises S4's half of it and leaves the
        rest standing)

## Background

Spec 0194 S4 made the caret's rendering conditional: when it rests on
one of the cursor node's braces and the partner is drawn, the *partner*
takes `caret_style`'s inversion and the caret demotes to a background
tint. Reported from use as disruptive, and it is: the caret's appearance
becomes a function of the document rather than of the last keypress, so
after every motion onto a brace the user has to re-derive which of two
differently-styled cells is theirs.

No editor does this. vim, emacs, VS Code, Helix and kakoune all leave
the cursor untouched by match highlighting, and all of them draw the
match as a background rather than as a second inversion.

## Goals

- **G1.** The caret is drawn identically wherever it rests.
- **G2.** A matched brace is a background lift, not an inversion —
  inversion is the caret's own idiom and is not shared.
- **G3.** The pair lights only while the caret is *on* a brace and the
  partner is drawn.

## Non-goals

- **N1.** Emacs's "on, or just after, a bracket" rule. See the
  alternatives.
- **N2.** Any cue for a match that is off screen. Silence still carries
  the fact (spec 0194 S4), and saying more is a message, not a color.
- **N3.** Lighting both members of the pair. See S4.
- **N4.** New bracket kinds. `render_text` emits `{`/`}` as its only
  delimiters, and `cursor_brace_pair` is unchanged.

## Specification

- **S1.** `theme::caret_paired_style` becomes `theme::brace_match_style`,
  and `caret_rgb`'s `DARK_PAIRED`/`LIGHT_PAIRED` become
  `DARK_MATCH`/`LIGHT_MATCH`. The colors are unchanged: they were
  already chosen to sit clearly above `cursor_row_style` on the caret's
  own row, which is exactly what a match tint needs.

- **S2.** The caret's cell always carries `theme::caret_style()`. The
  conditional in `render` goes, and `apply_caret` loses its style
  parameter — `REVERSED` is now the only style it is ever given, and
  leaving the parameter would leave the door it exists to close.

- **S3.** The partner brace carries `brace_match_style`, applied through
  `restyle_char` rather than `apply_caret`: a background patch composes
  with the character's syntax foreground instead of displacing it, and
  it needs none of `apply_caret`'s reversal-cancelling. It is drawn over
  `cursor_row_style` whenever the partner shares the caret's row — which
  a folded node's `{ ... }` always does — so the two backgrounds must
  stay distinguishable from each other and not merely from the page.

- **S4.** Only the partner is lit. Under S2 the caret's own member would
  take the inversion *on top of* the match tint, so its background would
  become its foreground and the caret would once again look different on
  a brace than off one — the defect this spec exists to remove. Lighting
  one member is what keeps the caret invariant, and it costs nothing
  visible: the caret already marks the member it stands on.

- **S5.** "On a brace" means the caret's cell is the brace's cell.
  Unchanged from spec 0194, and restated because the neighboring rule in
  emacs is not this one (N1).

Everything else about the pair is untouched: which braces pair
(`cursor_brace_pair` and its bracketed-node test), the per-frame
resolution of whether the partner is drawn, and `%`.

## Alternatives considered

### Keep spec 0194 S4's swap

Its argument — invert the answer, not the question — is sound about
*information* and wrong about *the cursor*. A cursor's rendering is the
user's proprioception; it has to be findable by shape without thought,
which it cannot be if the document decides what shape it has. The extra
emphasis on the match is not worth that, and use bore it out.

### Emacs's "on, or just after, a bracket"

`show-paren-mode` fires in both positions because emacs's insert-mode
cursor is a bar between characters: you cannot stand on the `)` you just
typed, so the rule is the only way to confirm it. protolens is a
read-only viewer with a block caret, every brace is reachable, and the
rule would only add a highlight with the caret on the space after a `{`
— noise while scanning. Its companion "on wins over just-after" clause
exists solely to resolve the ambiguity the rule itself creates.

### Light both members, and let the caret override its own

Vim and emacs do this. Degenerate here — see S4: with the caret always
inverted, its own member's tint is either invisible or actively harmful.

### Say where an off-screen match is

protolens knows exactly where the partner is; vim's `matchparen` limits
its search to the window because it cannot afford to know. "The match is
340 lines below" is a cue a plain editor cannot offer and this one
could. It is a message rather than a color, so it is not this spec.

## Test plan

1. `a_brace_pairs_with_its_match_only_when_the_caret_is_on_it` — spec
   0194's three states with the two styles the other way round: off a
   brace and with the match out of the window, one inverted cell and no
   tint; with the match drawn, the caret still inverted and the *match*
   tinted.
2. `the_match_highlight_is_resolved_every_frame` — spec 0194's
   `losing_sight_of_the_match_returns_the_strong_cue_to_the_caret`,
   renamed because the caret never had the cue to give back. Panning the
   match off the edge and back changes the tint and leaves the caret
   alone, with no key pressed.
3. `a_folded_node_pairs_its_synthetic_closing_brace_on_the_same_row` —
   both members on the caret's row, the caret inverted and the synthetic
   `}` tinted, which is also where S3's "the two backgrounds must
   differ" is load-bearing.

Together those three assert G1 directly: the caret's cell is
`caret_style` in every state the pair can be in.

## Measured outcome

`cargo test -p protolens` passes 621, plus 25 in `tests/batch_export.rs`
— the same counts as before, since all three tests already existed and
were rewritten rather than added to.

Net −8 lines of implementation. `apply_caret` shed its parameter and its
`reversing` guard along with it, since the guard existed only to keep
the conditional's non-reversing branch from misbehaving; the partner
went to a bare `restyle_char` for the same reason. Spec 0194's S4 and
its G2 are amended in place, and `docs/protolens/design/main-pane.md`
follows.
