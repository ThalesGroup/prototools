<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Pane: override management pane

*last verified: 2026-08-03*

## Executive summary

The management pane (`o` from the main pane) is the user-facing view of the
[override collection](override-collection.md) as a whole — every override
ever created this session, whether from the select pane, `:override`, or
auto-seeded Any/MessageSet expansion, grouped by origin and listed in the
collection's own canonical order. Where the select pane is about
*choosing* a type for one node, the management pane is about *auditing
and adjusting* everything already decided: activating/deactivating,
renaming a field's display label, rotating how broadly an override's
origin matches, duplicating, or deleting. It never itself decides what a
node should render as — every mutating action here ends the same way,
by handing off to the collection's own `render_overrides` pass.

## Technical detail

### Grouped display, not a flat list

Entries are shown as origin header rows (unindented, unselectable) each
followed by their own indented type rows, rather than repeating the
origin on every row — a purely cosmetic transform over the collection's
own already-origin-sorted entry order (the collection guarantees origins
never interleave, so grouping is a single linear pass, not a re-sort).
Auto-derived entries render in a muted style, manual entries in a bolder
one, so provenance is visible at a glance without needing to open a
detail view — this coloring is a `manage_entry_style`-only concept,
independent of the syntax-highlighting `SyntaxRole`s used elsewhere, since
"is this override auto or manual" has no corresponding grammar capture to
piggyback on.

### Kind rotation (`z`/`Z`) is forgiving, not exact

Rotating an entry's origin among `Path`/`PathField`/`FqdnField` (see
[override-collection.md](override-collection.md) for what each means)
sounds like it should be a simple three-way cycle, but the pane's actual
behavior is deliberately more forgiving than that, for a concrete reason:
a `PathField` or `FqdnField` origin can match *several* nodes at once, so
"rotate to `FqdnField`" is ambiguous whenever more than one candidate
node in the document would satisfy it. The pane resolves this ambiguity
by preferring whichever candidate the main-pane cursor is currently
sitting on; only when the cursor isn't one of the candidates *and* there
is more than one does it prompt the user to disambiguate by moving the
cursor first (`Left`/`Right` circulate among an entry's currently-affected
nodes for exactly this purpose). A repeated `z`/`Z` press with the cursor
genuinely unchanged since the last attempt advances to the *next* kind
instead of retrying the same one forever — tracked via a real "did the
cursor move" counter rather than comparing cursor positions, since a
round-trip move (e.g. down then back up) would otherwise be
indistinguishable from "never moved" and falsely count as no movement.

### It can call the selection pane, and gets returned to

Retyping an entry from here opens the
[selection pane](override-select-pane.md) over the same area, and all
three of its exits — `Enter`, `Esc`, `t` — come back *here* rather than
to the main pane, with the just-retyped entry highlighted (spec 0200).
The two panes are mutually exclusive on screen, but that is a layout
fact, not a lifecycle one: the manage pane remains the caller
throughout, which is also what decides the confirmed override's origin
kind — the kind of the entry being edited, rather than the `path:field`
default a `t` from the main pane would get.

### Deleting an in-scope auto entry deactivates instead

An auto-derived entry whose governing context is still "in scope" (its
Any/MessageSet source would still re-derive the same type right now) is
deactivated rather than removed on `Delete`/`Backspace` — removing it
outright would be immediately undone by the very next render pass, which
would simply re-seed an identical entry from scratch. Deactivating,
by contrast, sticks: the render pass respects an explicitly-deactivated
auto entry the same way it respects any other deactivated override, and
only resumes auto-deriving it once the entry genuinely falls out of
scope (its governing ancestor context changes).

### No border — a local statusline, a vertical separator from the main pane

Like the override-select pane it's mutually exclusive with (spec 0117
§3), the management pane draws no border of its own — its area splits
into a `Min(0)` entry-list region above its own `Length(1)` local
statusline, showing the currently-highlighted entry's (or header row's)
origin path plus a row ruler over `manage_display_rows()`. The same
neutral `'│'` vertical separator divides it from the main pane when open
— see [override-select-pane.md](override-select-pane.md) for that
column's own details, since both side panes share it identically.

### Editing an entry is one command, not a sub-mode

`o` on a highlighted entry opens the global command/message row
pre-filled with the `:override` that describes that entry exactly —
type, origin and display name, every argument present. Editing the
entry is then editing that text, and `Enter` on the unedited line is a
no-op. This replaced a bespoke inline rename sub-mode that could change
only the display name, and only by way of a second text-entry
implementation with no cursor, no completion and no history (spec
0236).

Neither that command nor this pane's own `/`/`?` search reserves a row
inside the side pane itself — both are typed into the same global
command/message row the main pane and the override-select pane already
use (see [command-line.md](command-line.md)). This wasn't the original
design; folding text entry into one shared row (rather than an ad hoc
spot inside each side pane) was adopted specifically because only the
shared row gets a real terminal text cursor, which a side-pane-local
input never had — a genuine usability gap in the pane's own space that
reusing the existing row closed for every pane at once, not just this
one.
