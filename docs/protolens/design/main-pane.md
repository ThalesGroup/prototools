<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Pane: main pane

*last verified: 2026-07-31*

## Executive summary

The main pane is the single scrollable view of the whole document tree —
the pane every other pane is a satellite of. Its job is threefold: let the
user move a cursor around the document tree in a way that matches how a
human actually thinks about the structure (next/previous sibling, into/out
of a container, not just "next line of text"); let them fold away
uninteresting subtrees; and render each visible line with syntax
highlighting and, where relevant, a visual cue that an override is in
effect. None of this pane's own code decides *what* to render — that's the
[document tree](document-tree.md)'s and the
[override collection](override-collection.md)'s job; the main pane only
decides *which* of the tree's already-rendered lines are currently on
screen and how to draw them.

## Technical detail

### Two movement granularities

Plain document-order stepping (`j`/`k`) passes through every visible
descendant of a container before reaching its next sibling;
sibling-skip movement (`J`/`K`) jumps directly to the next/previous node
sharing the same parent — the more useful move once a subtree has been
inspected. Both respect fold state identically: a folded node's
descendants are simply never visited by either.

Neither is a pointer chase. The arena is in level order, so a sibling is
`idx ± 1` bounded by the parent's child block, and a document-order step
is `tui/lines.rs`'s `next_visible`/`prev_visible` — O(1), carrying the
absolute line with it. See [arena-and-batch.md](arena-and-batch.md).

### Fold state is a `HashSet`, visibility is derived

Folding doesn't remove or hide anything in the tree itself — it only
adds a node's index to a `folded` set. Each node carries `lines_visible`
alongside `lines_total`, so the number of rows a subtree draws with
folds applied is already known and a fold toggle corrects only the
folded node's own count and its ancestors'. There is no visible-row list
to rebuild; a frame descends once to its first row and then steps.

A node's own opening line stays visible even when it's folded — it gains
a `{ ... }` collapse summary, and its glyph in the fold gutter flips.
Folding a node changes how much of *its own* line is shown, never
whether it appears at all.

The gutter is a real left margin (spec 0193): `fold_margin_of` subsumes
the line's own leading indentation into it, so a foldable line's first
token lands in the same column as a non-foldable one's at the same
depth. An all-blank row gets no margin, because padding it would only
lengthen the row that horizontal panning measures against.

### Selection, copy, and the double-click distinction

Mouse-driven line selection (click, drag, release) and cursor movement
are tracked independently: a selection never moves the cursor, and moving
the cursor never touches an active selection. This independence exists
because a selection's purpose (marking a range of *text* to copy) and the
cursor's purpose (marking the *node* the rest of the UI acts on) are
genuinely different operations that a user may want to combine — e.g.
selecting several lines while the cursor stays on an earlier node whose
override is still being decided.

Copying is a single, explicit, always-available action (`Ctrl-C`) rather
than tied to mouse release, because terminal mouse-release events are
ambiguous by nature: crossterm cannot distinguish a single click from the
first half of a double-click at the protocol level, so protolens
disambiguates them itself (comparing consecutive click timestamps and
positions) purely to decide whether a plain click should clear an
existing selection or a double-click should preserve one — a decision
that has nothing to do with when the actual clipboard write happens.
`Ctrl-C` with no active selection falls back to the cursor's current
line, so the key is always meaningful, never a silent no-op.

Clipboard access itself degrades gracefully: an OS clipboard write
(`arboard`) is attempted first, and an OSC 52 terminal escape sequence is
always emitted as a fallback whenever it fails — the common case being a
plain SSH session with no X11/Wayland provider available. Because OSC 52
has no acknowledgment protocol, the UI can never distinguish "the
terminal actually honored it" from "the terminal silently ignored it," so
the status message is deliberately optimistic rather than trying to
report a definitive success/failure.

### No border — a local statusline instead

The main pane draws no border of its own. Its area splits into a
`Min(0)` content region above its own `Length(1)` local statusline row,
showing the cursor's own positional path, resolved type, byte range, and
line number — dropped down to just the path and type once a side pane is
open, since precise byte-range/line-ruler detail is secondary while
focus is elsewhere and remains recoverable by closing the side pane.
Focus is conveyed purely by that row's own accent color, the same way
vim distinguishes an active window's statusline from an inactive one —
there is no separate border color to also track. See
[help-and-chrome.md](help-and-chrome.md) for how this local statusline
relates to the one global command/message row shared by every pane.

### Syntax highlighting is a display transform over already-rendered text

The main pane doesn't highlight *data* — it highlights protolens's own
already-rendered `#@`-annotated textproto output, using a tree-sitter
grammar for that same textproto syntax. This means highlighting is
entirely decoupled from the decode/override pipeline: any text protolens
can render, it can highlight, with no per-override-kind special-casing
needed in the highlighter itself. See `colorize.rs`/`theme.rs` for the
highlighting pipeline's own internal structure.

It is recomputed **per frame, for the window only** (spec 0187) — there
is no stored per-line style buffer. The window has to be wrapped in a
synthetic brace frame first, or tree-sitter's error recovery eats
captures; see [rendering.md](rendering.md)'s stage 3.
