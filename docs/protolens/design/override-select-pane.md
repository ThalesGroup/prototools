<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Pane: override selection pane

*last verified: 2026-07-25*

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

### Live preview: cheap, provisional, and self-reverting

Every time the highlighted row changes, the pane immediately splices the
highlighted candidate into the main pane using the same
[`splice_override`](document-tree.md) primitive a real commit would use —
but deliberately does *not* touch the override collection, and
deliberately *invalidates* (rather than sets) the node's own `rendered_as`
provenance after each preview splice. That second detail is what makes
`Esc` able to cleanly revert: because a preview splice never claims to be
the node's real resolved state, a later real render pass never
mistakenly concludes "nothing changed, no need to re-splice" just because
a previewed row happened to coincide with the node's actual effective
type. Closing the pane (by any route — `Enter`, `Esc`, or toggling `t`
again) always ends with one real `render_overrides` pass from the target
node, which is what actually settles the display back to the collection's
true state, live-preview history notwithstanding.

Each preview splice is called outside any `render_overrides` batch, so it
is its own standalone, immediately self-finalizing batch — it pays the
full [batch-finalization cost](override-collection.md) on every
highlighted row, not just on commit; on a very large document this is the
dominant cost of browsing candidates (see spec 0163's non-goals). To keep
the splice itself affordable independent of that, preview splices alone
truncate the candidate node's *interior bytes* to
`OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT` before handing them to the
renderer (spec 0174) — bounding the input bounds the decode, the render,
the span count and the line count together, so a huge candidate subtree
never gets materialized for a mere preview. The truncation rewrites the
node's own length prefix so the surviving prefix is still well-framed,
which is what lets it render as complete, correctly-typed, fully nested
fields rather than one opaque bytes line; a truncated preview ends with a
literal `...`. The render cache key includes an `is_preview` flag
specifically so a truncated preview is never mistaken for, or reused as,
a full confirmed render of the same range.

Because the same node gets re-spliced repeatedly as the user browses
candidates, the pane tracks a `preview_tree_watermark`: the tree's own
length recorded on the *first* preview of a pane-open session. Every
subsequent preview (a new highlighted row, or an automatic re-trigger as
background candidate scoring streams in) first truncates the tree array
— and the caches indexed by it — back to that watermark, discarding the
previous preview's now-superseded subtree before splicing the new one in,
so successive previews don't leak orphaned nodes indefinitely. The
truncation invalidates the target node's own `first_child`/`last_child`
(both pointed into the just-discarded range), which are simply nulled;
its `doc_next` pointed into that range too — at the previous preview's
own first child — and so must be *recomputed* to the post-subtree seam
**before** the truncation runs, while the descendant set it is derived
from still exists. Assuming instead that `doc_next` always points outside
the subtree left a dangling index that the next splice's freshly-pushed
nodes made valid again, wiring a cycle into the document-order chain; see
[document-tree.md](document-tree.md)'s seam section for the full account.

The truncation also scrubs the two line-index maps (`line_to_node`,
`footer_line_to_node`) of any entry at or past the watermark: an
unrelated full `render_overrides` pass landing between two previews (e.g.
background root-type resolution) may have rebuilt them against a tree
that has since been truncated, and a later `heat_cue_for` lookup would
otherwise feed an out-of-bounds index straight into an array index.

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

The pane's own `/`/`?`/`n` search matches candidate FQDNs
case-insensitively, independent of whichever sort mode is currently
active — searching works the same whether the list is inferred-order or
alphabetical, since it's a text match over the same underlying strings
either way.
