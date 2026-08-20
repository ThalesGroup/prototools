<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0339 — one search, three panes

Status: implemented
Implemented in: 2026-08-20
App: protolens
Refs: docs/specs/0276-a-find-steps-through-its-matches.md (amends S9 —
        a side pane now tints, so the find preview is no longer the
        only way its current match is shown), docs/specs/0274-a-match-
        may-cross-a-line.md (the cross-row engine becomes explicitly
        main-pane-only), docs/specs/0246-the-search-prompt-browses-
        history-and-rotates-matches.md (N4 — a side pane's stop is its
        whole entry, which is what makes the tint a per-row rule),
        docs/specs/0117-protolens-override-collection.md (§3 — the
        manage pane's search corpus gains the display name)

## Background

The search *engine* is already one engine. `SearchScope::{Main,
Override, Manage}` runs through one `SearchSweep`, one `SearchPattern`,
one `pick_match`, one `advance_sweep`; both side panes already have
incremental search-as-you-type, history browsing, the `F`/`B` find
prompt, the `n of m` tally, the committed echo, `Ctrl-←`/`Ctrl-→`
rotation, scroll preview and `Esc` restore.

The *display* half is not. `render_override_pane` and
`render_manage_pane` never call `search_highlight_pattern()`, so a `/`
in a side pane shows the reader nothing at all while they type. Spec
0276 S9 records this — "a side pane tints nothing, so its current match
is shown by the highlight or not at all" — and works around it by
moving the highlight for a find prompt, which is why a find previews
there and a `/` does not.

Reproduction: `protolens <any .desc>`, `t`, `/`, type any substring of a
candidate type name. The pane is unchanged until `Enter`.

Four pieces of per-scope duplication survive alongside it, all made
redundant by the shared engine and none load-bearing: three
near-identical `Enter` arms in `handle_command_key` differing only in
which `last_*_search` field they touch; three hand-rolled `n`/`N`
blocks; three one-line trampolines into `run_search` (`jump_to_match`,
`jump_to_override_match`, `jump_to_manage_match`) whose doc comments
still describe implementations deleted long ago; and a manage-pane
haystack that omits the `as "x"` display name the row visibly draws.

## Goals

- **G1.** A `/` or `?` in either side pane tints its matches as it is
  typed, by the same rule the main pane uses: every occurrence in
  `search_match_style`, the one the sweep is standing on in
  `search_current_style`.
- **G2.** The tint lands on the text the reader sees, not on the
  pane's search haystack — the two differ in both panes.
- **G3.** The manage pane's display name is searchable.
- **G4.** One `Enter` arm, one `n`/`N`, no trampolines: the pane a
  search runs against is named once, by `SearchScope`, and never
  re-derived from focus at a call site.

## Non-goals

- **N1.** The manage pane's origin-kind header rows do not become
  search targets. They are not landing targets either — the pane's
  cursor is an *entry* index — so admitting them to the candidate walk
  would mean inventing a highlight that can rest on a header. An
  origin-only match still lands on the entries grouped under it, which
  is what `manage_search_text` keeping the origin label is for.
- **N2.** No cross-entry match in a side pane. A pane entry is matched
  on its own; a pattern is never run across two entries, because a
  list is not a document and the `\n` between two rows of it is an
  artifact of the drawing, not a fact about the data.
- **N3.** Spec 0276 S9's `find: Some(_)` guard in `show_sweep_hit`
  stays. Once a `/` tints, the preview-move may be redundant — but
  removing it changes user-visible find behavior, and belongs in its
  own spec.
- **N4.** Path patterns (`/1/2`) in a side pane still match nothing:
  `find_range` returns `None` for `SearchPattern::Path`, and a list
  index has no positional path to compare against. Left as is,
  silently, as today.
- **N5.** The main pane's tint rule is unchanged. This spec extracts
  its loop; it does not re-specify it.

## Specification

- **S1.** `tint_matches(spans, text, pattern, cells, style_of)` in
  `render.rs`, beside `tint_columns`: walk `find_range_from` over
  `text`, convert each match's byte offsets to character columns, and
  `restyle_range` the drawn cells they occupy. `cells` carries the
  column geometry — the pan, the gutter widths before and after the
  pan is subtracted, and the pane's right edge. `style_of` is asked
  per match, by the match's start column.

  This is the main pane's own single-row loop, lifted. It keeps that
  loop's two rules: `find_range_from` rather than a slice (spec 0273
  S6 — `^` and `\b` depend on what precedes the offset), and stepping
  past each match's *start* rather than its end, which is what keeps
  overlapping occurrences honest.

- **S2.** The main pane passes the fold margin and the heat gutter as
  its leading and trailing widths and a `style_of` that consults
  `search_current_cell()`; both side panes pass zero for each and their
  own pan offset. The path-cell special case (spec 0235 S22) stays at
  the main pane's call site: a path pattern cannot match a side pane.

- **S3.** **The tint re-scans the drawn text; it does not map the
  hit's byte offset.** Neither side pane's haystack is its drawn text.
  The override pane searches the bare FQDN and draws
  `format_fqdn_label(fqdn)`, which may gain a leading `.` (spec 0136
  G6), a ` [enum]` suffix or a `  (score: N)` tail; the manage pane
  searches origin + type + name and draws `  ● type as "name"`.
  Re-scanning is what makes the tint land on what the reader sees, and
  it is the rule the main pane already follows.

- **S4.** **The current match is chosen by row, not by column.** Spec
  0246 N4 makes a side pane's stop its whole entry, so on the entry the
  sweep is standing on *every* occurrence is `search_current_style`,
  and on any other row every occurrence is `search_match_style`.
  `search_current_index()` answers which entry, reading the sweep's
  `found` cursor — mirroring `search_current_cell`.

  Not `override_highlight` / `manage_highlight`: a `/` prompt
  deliberately does not move those (spec 0235 S15), and a tint that
  tracked the highlight would therefore never move while the reader
  typed, which is the whole point of having one.

- **S5.** A side pane tints only when it is the pane the search belongs
  to. `search_highlight_pattern()` returns the live `command_buffer`
  without asking which pane opened the prompt, so each pane renderer
  gates on `active_search_scope()` being its own.

- **S6.** `begin_search_sweep` states N2 rather than relying on it:
  a `SearchPattern::Multi` builds a segment queue only for
  `SearchScope::Main`. A pattern admitting `\n` still compiles to
  `Multi` — `\s` admits one, so `foo\s+bar` is a pattern readers type
  without meaning anything by it — and is still matched *per entry*
  through `find_range`, which the `Multi` arm supports. What is refused
  is reading the list as one joined haystack.

  Correspondingly the pane tint never reaches for
  `multi_row_occurrences`: that pass is a window construction over
  document-adjacent rows and has no meaning over a list.

- **S7.** `manage_search_text(idx)` is origin label + type label + the
  entry's display name. The origin label stays in it (N1). Consequence,
  stated at the function: an origin-only match tints nothing on the
  entry row, because the matched text is drawn on the header row above
  it. The row is still the landing and the highlight still moves there.

- **S8.** One `Enter` arm for `CommandLineKind::Search`, over
  `active_search_scope()` — a new one-liner lifting the idiom
  `accept_find` already uses: the prompt's own origin scope, falling
  back to `search_scope()`. The pane a search commits against is the
  pane it was opened from, stated once rather than at two call sites.
  `last_search_for` / `set_last_search_for` carry the per-pane field
  choice, as they already do for the find prompt.

- **S9.** One `repeat_search(back)` — vim's `n`/`N` — bound in all
  three panes. `jump_to_match`, `jump_to_override_match` and
  `jump_to_manage_match` are deleted: S8 and this remove their last
  production callers, and each is a single call into `run_search`
  already.

## Alternatives considered

**Map the sweep hit's byte range onto the drawn row.** It is what the
main pane's *current-hit* path does, and it is wrong here for the
reason S3 gives: the hit's offsets are into the haystack, and neither
pane's haystack is its drawn row. It would also tint nothing at all
while the reader types, since only the standing hit has offsets — the
other occurrences would need the scan anyway.

**Tint the entry the highlight rests on.** Cheaper — no scan — and it
is what a find prompt effectively shows today via spec 0276 S9. But a
`/` prompt does not move the highlight, so the tint would be frozen at
the caret while the pattern narrowed underneath it, and the reader
would still be typing blind.

**Give the side panes the cross-row engine too.** Rejected as N2: it
requires a document order over a list and a meaning for the `\n`
between two entries, neither of which exists.

## Test plan

1. `a_slash_tints_the_override_pane` — the matched characters of a
   candidate row carry `search_current_style`, and the same substring
   on a different row carries `search_match_style`.
2. `the_override_tint_lands_on_the_drawn_text` — a candidate whose
   drawn row differs from its FQDN (an enum's ` [enum]` suffix): the
   tinted columns sit on the FQDN part.
3. `a_slash_tints_the_manage_pane` — the type label tints, at the column
   the `  ● ` marker puts it at rather than the one it occupies in the
   haystack. Its second half, `an_origin_only_match_lands_without_
   tinting`, checks that an origin-only match tints nothing while still
   moving the highlight to the entry.
4. `the_manage_display_name_is_searchable` — an entry renamed
   `as "x"`; `/x` finds it.
5. `a_manage_header_is_never_a_landing` — with a pattern matching only
   an origin label, one full `n` cycle stops exactly once per matching
   *entry*.
6. `no_match_crosses_two_entries` — a pattern ending in `\s+` plus a
   prefix of the next entry finds nothing; the same pattern within one
   entry finds it. Pins S6.
7. `esc_still_restores_a_side_pane` — scroll and pan return to the
   origin and the highlight never moved, now that a `/` visibly changes
   the pane.
8. `n_repeats_its_own_panes_search` — `last_search`,
   `last_override_search` and `last_manage_search` stay independent
   through `repeat_search`.

The sandbox has no `COLORTERM`, so assertions read `Modifier` or named
colors, never RGB.

## Measured outcome

Implemented 2026-08-20. Both side panes tint as the reader types, and
the three trampolines, the three `Enter` arms and the three `n`/`N`
blocks are one each.

The change is a net simplification in production code: `render.rs` grew
by the extracted `tint_matches` and `RowCells` and by two call sites,
while `command_line.rs`, `key_dispatch.rs` and `override_select.rs` each
shrank. Nine new tests, all four gates clean (`cargo fmt --all --check`;
`cargo clippy --no-default-features --workspace -- -D warnings`;
`cargo test --release --no-default-features --workspace`, run twice,
with and without `COLORTERM`; `reuse lint`), 1194 tests passing.

Two things the plan did not anticipate, both recorded above as they
were found:

- **S5 had to be added.** `search_highlight_pattern()` answers with the
  live command buffer regardless of which pane opened the prompt, so a
  bare call from a side-pane renderer tints that pane during a *main*
  pane's search. Each side pane gates on
  `active_search_scope() == <its own scope>`.
- **The tint geometry had to become a struct.** The plan's eight-argument
  `tint_matches` exceeds clippy's `too_many_arguments` threshold of
  seven. Grouping the four column values into `RowCells` fixes the cause
  rather than suppressing the lint, and names the thing they describe.
