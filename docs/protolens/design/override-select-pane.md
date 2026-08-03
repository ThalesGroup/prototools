<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Pane: override selection pane

*last verified: 2026-08-03*

## Executive summary

The override selection pane (`t`) is where the user picks a type for the
node under the cursor. It offers a ranked list of plausible candidates —
either scored by the inference graph or sorted alphabetically — plus a
permanently pinned "raw / no type" option, and, distinctively, shows the
effect of each highlighted candidate *live* in the main pane before the
user commits to it. Committing (`Enter`) is the one action in this pane
that actually writes to the [override collection](override-collection.md);
everything else — sorting, scrolling, live preview — is provisional and
freely discardable via `Esc`.

## Technical detail

### Ranking is a thin UI layer over `descriptor-context.md`'s scoring

This pane does no scoring itself. It asks the scoring graph (via
`descriptor-context.md`) for a ranked `(type, score)` list for the
cursor's own byte range, consults the [candidate
cache](caches.md) first, and falls back to lexicographic-only ranking
(every known type FQDN, alphabetically) whenever no scoring graph is
loaded at all. The pinned raw entry is always row 0, deliberately never
the pane's default highlight on open — the default highlight instead
prefers, in order: whatever type is already active for this node (so
reopening the pane on an already-typed node doesn't lose the user's
place), then the top-ranked inferred candidate, then raw only as a last
resort.

### Live preview: an overlay, not a splice

**A preview does not splice** (spec 0185). Every time the highlighted row
changes, the pane calls `render_node_as` — shared verbatim with the
committed path, so the preview is byte-identical to the splice it stands
in for — and holds the result in a read-only `PreviewOverlay`: a first
row, a covered-row count, and the replacement lines. The draw path
substitutes that block for the target's contiguous run of rows by
arithmetic (`committed_row_of`).

Nothing in the document, the arena, the override collection or the
node's `rendered_as` provenance is mutated. So rebuilding a preview is a
plain overwrite, discarding one is a plain assignment, and a preview
that fails to render leaves the committed document on screen with a
message. Nothing downstream reads spliced state anyway: the candidate
list is already computed, scoring runs against raw byte ranges, and
confirming re-derives everything from the entry.

That is a stronger property than it sounds, and what it replaced is
worth knowing. The preview used to splice speculatively and then unwind
— a watermark into the then-growing tree array, truncated back to on
every new highlighted row, plus hand-nulled child pointers, three
`retain`s and a forced `rendered_as` reset. Every field pointing into
the discarded range had to be repaired by hand, and each one missed was
a live defect: a dangling index made valid again by the next splice's
fresh nodes, an out-of-bounds line-map entry read by a later heat cue.
The overlay has none of those obligations because it owns no tree state.

Overlay rows have no node, and therefore no heat cue, no override hint,
no fold marker and no selection.

A preview additionally truncates the candidate node's *interior bytes*
to `override_preview_byte_budget` before handing them to the renderer
(spec 0174); a confirmed override never does. Bounding the input bounds
the decode, the render, the span count and the line count together, so a
huge candidate subtree never gets materialized for a mere preview. The
truncation rewrites the node's own length prefix so the surviving prefix
is still well-framed, which is what lets it render as complete,
correctly-typed, fully nested fields rather than one opaque bytes line;
a truncated preview ends with a literal `...`. That marker is not valid
prototext, which is why the highlighter blanks the row it is on rather
than parsing it. The render cache key includes an `is_preview` flag so a
truncated preview is never mistaken for, or reused as, a full confirmed
render of the same range.

Closing the pane by either route (`Enter` or `Esc`) drops the overlay;
`Enter` additionally runs the real `render_overrides` pass that commits
the choice. Both routes return to whichever pane *opened* this one — the
main pane for `t`, the [management pane](manage-pane.md) when it was
opened from there (spec 0200). `t` itself no longer closes the pane
(spec 0236): `Esc` is the one key that closes any pane, which is only a
usable convention if it is also the *only* one.

`o` here opens a pre-filled `:override` for the pane's **target
node** — deliberately the target and not the main-pane cursor, since
those differ whenever the pane was opened from the management pane, and
the target is what the pane is visibly about. It pre-fills the target's
*current* type rather than whichever candidate is currently
highlighted: picking from the list is what `Enter` does, and `o` exists
for the two dimensions the list cannot express (origin and display
name).

Live preview intentionally does not extend into nested Any/MessageSet
auto-expansion within the previewed subtree — a preview shows the
directly-retyped node's own new shape, not a full recursive re-resolution
of everything beneath it. This is a documented scope limit, not an
oversight: a complete-preview mode was considered and deferred, along
with a "cancel in-flight, latest wins" debounce design for if/when scoring
or rendering a full nested preview ever becomes expensive enough to need
one.

### Capped vs. complete candidate lists

Reopening the pane on a range that's only ever been seen as someone
else's [capped candidate-cache preview](caches.md) initially shows just
that capped list. Scrolling (or jumping via `Home`/`End`) past what's
cached transparently triggers a one-time upgrade to the complete,
freshly-scored list — the user never has to explicitly ask for "show me
more"; the pane notices it's about to run out of cached rows and fetches
the rest first.

### No border — a local statusline, a vertical separator from the main pane

Like every other pane, the override-select pane draws no border of its
own — its area splits into a `Min(0)` candidate-list region above its
own `Length(1)` local statusline, showing the target field's own
positional path and current sort mode (`inferred types` vs. `all types`)
plus a row ruler over the candidate list. When open, the pane sits beside
the main pane, divided from it by a single neutral-styled `'│'` column
rather than a left/right border — focus is conveyed by each side's own
statusline accent, not by the divider.

### Search operates on the FQDN, not the score

The pane's own `/`/`?`/`n` search matches candidate FQDNs **smartcase**
(spec 0195): an all-lowercase pattern matches case-insensitively, a
pattern with any uppercase character matches exactly — vim's rule, and
the same helper the main pane and the management pane use. It is
independent of the sort mode, since it is a text match over the same
underlying strings either way.
