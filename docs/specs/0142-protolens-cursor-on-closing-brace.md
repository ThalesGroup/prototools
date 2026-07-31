<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0142 — protolens: let the cursor rest on a node's own closing `}` line

Status: implemented — but nothing below describes the mechanism any
        more. The cursor does still rest on a closing `}`, which is
        this spec's whole point. Two things changed underneath.
        (1) The cursor is no longer a node index plus a footer flag:
        it is a `LinePos { node, line_in_node }` (spec 0194's caret),
        so a node has as many stops as it has lines and the footer is
        merely the last of them — `cursor_on_footer()` survives only
        as the derived predicate `line_in_node > 0 && is_bracketed()`
        (`navigation.rs:360`). (2) Spec 0210 deleted `line_to_node`,
        `footer_line_to_node` and `visible_rows` outright, because a
        node now stores its own line *counts* and a position is
        derived. Read every "look the line up in the map" passage
        below as "walk to the line", and every "the footer is a second
        stop on the node" passage as "every line is a stop".
Implemented in: 2026-07-18
Refs: docs/specs/0194-the-cursor-is-a-caret.md (replaces the node-index
      + footer-flag cursor with a per-line caret),
      docs/specs/0210-a-node-counts-its-own-lines.md (deletes the two
      line maps and `visible_rows` this spec extends),
      docs/specs/0111-protolens-v1-decode-navigate-extract.md (`doc_next`/
      `doc_prev`, `visible_rows`, cursor-as-node-index), docs/specs/
      0113-protolens-tui-refinements.md (D33: `footer_line_to_node`'s
      original introduction, for the override bold-hint only),
      docs/specs/0135-protolens-override-raw-tag-rewrap.md (`splice_
      override`'s unified splice mechanic, `idx` keeping its own
      tree-array identity across a retype), 2026-07-18 feedback item 1,
      2026-07-18 feedback (empty-message navigation/fold glitches found
      during this spec's own implementation), 2026-07-18 feedback
      (status line reporting the header's line number for a
      footer-resting cursor)
App: protolens

## Background

The main pane's keyboard/mouse cursor is a node index
(`App.cursor: usize`), and `navigation.rs`'s `move_down`/`move_up` walk
`doc_next`/`doc_prev` node links (skipping folded-away nodes via
`is_hidden`). Each node has exactly one cursor stop: its own header
line (`span.text_range.start`). A message/group node's closing `}`
line (`span.text_range.end - 1`) is not a node in its own right — it is
never a `doc_next`/`doc_prev` target — so it is currently unreachable
by `Down`/`Up`/`PageDown`/`PageUp`/`End`, and a mouse click on it is
silently ignored (`handle_click` looks the clicked line up in
`line_to_node`, which only maps header lines).

The user's request (verbatim):

> In the main pane, currently it is impossible to have the cursor on a
> line with a closing curly bracket: the browsing behavior skip those.
> This is a bit awkward when the rendering produces output larger than
> the pane: when you `Down` to the bottom of the rendered protobuf, you
> stop at the last non-`}` node, and you never see the end of the
> rendering, which feels awkward. [...] Find a way for the cursor to be
> able to sit on the `}` lines and manage the UI consequences (e.g. any
> action when the cursor is on a `}` is interpreted as if the cursor
> was on the corresponding opening bracket) [...]

Concretely: the whole document is wrapped in a single virtual
encompassing message node (`decode`'s "virtual encompassing wrapper",
spec 0111/0114 §1.1) which is itself a real tree node (`App.first_node`
has `doc_prev == None`). Its own closing `}` is always the very last
line of the document, and today it can never be reached — this is
exactly the "never see the end of the rendering" symptom.

A `footer_line_to_node: HashMap<usize, usize>` map already exists
(spec 0113 D33), built in `App::new` and rebuilt in lockstep by
`splice_override` (`override_apply.rs`), mapping each *currently
expanded, non-folded* parent node's closing-line index back to that
node's own tree index. Its only consumer today is `render.rs`'s
`line_has_active_override`, for override-highlight styling of closing
lines — it has no bearing on cursor navigation.

### Nodes that change shape under `t` (message ⇄ scalar retype)

The override selection pane can retype any node, including a message/
group node, to a primitive scalar (spec 0135/0137), and vice versa
(a scalar retyped to a message FQDN). `splice_override` always
re-renders `idx`'s whole subtree from scratch but **keeps `idx`'s own
tree-array identity** (spec 0135 Background) — only its `span` and
children set change. Two consequences for a footer-resting cursor:

- If `idx == self.cursor` and the retype turns a message into a
  scalar, `idx` no longer has children, so `footer_line_to_node` (fully
  cleared and rebuilt at the end of `splice_override`) no longer holds
  an entry for it — the footer line the cursor was resting on no
  longer exists.
- The reverse (scalar → message) is harmless: the cursor was
  necessarily on `idx`'s header (a childless node never has a footer
  stop), and stays there; the node's new footer line simply becomes a
  reachable stop the next time `Down` is pressed, same as any other
  freshly-expanded node.

### Folding the very node the cursor's footer is resting on

`toggle_fold(idx)` (bound to `Enter`/`f` on `self.cursor`, or clicking
a header's fold marker) hides `idx`'s whole body, including its own
footer line (`rebuild_visible_rows`: hidden range is
`text_range.start + 1 .. text_range.end`, which includes
`text_range.end - 1` whenever the node has a nonempty body). If the
cursor is currently resting on `idx`'s footer at the moment `idx`
itself gets folded, that line stops being visible while the cursor
notionally remains "on" it.

Both cases reduce to the same invariant violation: **the cursor must
never claim to be resting on a footer line that is not currently a
member of `footer_line_to_node`** (i.e. not currently an expanded
parent's visible closing line). Both are handled by the same one-line
fix (G6 below).

### Empty-but-bracketed messages (discovered live during implementation)

A message decoded with zero populated fields still renders as two
distinct lines (e.g. `"inner {"` then `"}"` on the next line — a
one-line-empty body, not a collapsed `"inner {}"`), i.e.
`text_range` spans more than one line, but `first_child == None`.
Two pre-existing code sites used `first_child.is_some()` as a proxy
for "this node has a real, distinct footer line" / "this node is
foldable", which is wrong for exactly this case:

- `footer_line_to_node`'s construction (`App::new` and
  `splice_override`'s rebuild) omitted a footer entry for such nodes,
  so `move_down`/`move_up` would land on the visible `}` line via
  `visible_rows` but fail to resolve it back to a cursor stop —
  silently getting stuck (no visible effect, cursor frozen).
- `has_children()` (`navigation.rs`), which gates the fold-marker
  glyph (`render.rs`), marker-click folding (`mouse.rs`), and the
  `Left`/`Right`/`Enter` fold keys (`key_dispatch.rs`), returned
  `false` for such nodes — no fold handle shown, un-foldable via
  keyboard either.

Both are fixed by replacing the `first_child.is_some()` gate with a
line-span check (`text_range.end - 1 > text_range.start`) at both
sites — a strict superset of the old condition: any node with real
children necessarily spans more than one line, and the only case that
newly becomes `true` is exactly the empty-but-bracketed message. See
G8.

### Status line reporting the wrong line number for a footer cursor

The status line's `L<n>/<total>` field (`render.rs`) read
`node.text_range.start + 1` unconditionally — a leftover oversight
from before this spec: a footer-resting cursor's own currently
*displayed* line is `cursor_line()` (`text_range.end - 1` when
`cursor_footer`), not the header. Fixed by using `self.cursor_line()`
directly (G9), consistent with G4's `cursor_display_row()` already
doing the equivalent for scroll-into-view.

## Goals

- G1: new field `App.cursor_footer: bool` (default `false`): `true`
  when the cursor is visually resting on `self.cursor`'s own closing
  `}` line rather than its header line. `self.cursor` itself stays a
  node index, unchanged in meaning — every existing action reading
  `self.cursor` directly (fold/unfold, `t`/override editing, status
  line, `y`ank/copy, positional path, etc.) keeps addressing the same
  node regardless of `cursor_footer`, which is exactly the "interpreted
  as if the cursor was on the corresponding opening bracket" behavior
  the user asked for — no extra redirection logic needed at those call
  sites.

- G2: `move_down`/`move_up` (`navigation.rs`) reworked to step through
  `self.visible_rows` by line position instead of walking
  `doc_next`/`doc_prev` node links:
  1. Compute the cursor's current line: `text_range.end - 1` if
     `cursor_footer` else `text_range.start`.
  2. `binary_search` it in `visible_rows` (always present, since the
     cursor's current line is always visible by construction) to get
     its position.
  3. Step that position `+1`/`-1`; if in bounds, resolve the landed-on
     line back to a `(node, footer)` pair via `line_to_node` (footer
     `false`) or, failing that, `footer_line_to_node` (footer `true`) —
     the two maps never collide on the same line (a footer line only
     exists for a node with a nonempty body, so its closing line always
     differs from its own header line).
  4. Set `self.cursor`/`self.cursor_footer` to the resolved pair and
     bump `cursor_moves`, mirroring `set_cursor`'s existing contract.

  `move_page_down`/`move_page_up` are unchanged (they already just
  call `move_down`/`move_up` `pane_height` times) and transparently
  gain footer stops as a byproduct.

- G3: `move_end` (`End`/`G`) targets the true last visible line
  (`visible_rows.last()`, resolved via G2 step 3's helper) instead of
  the last node's header — so `End` now reaches the final `}` (e.g. the
  virtual encompassing wrapper's own closing line) whenever the
  document's last visible row is a footer. `move_home` (`Home`/`gg`)
  is unchanged: the first visible line is always a header (the
  document can never open on a `}`).

- G4: `cursor_display_row()` (`mouse.rs`, drives scroll-into-view and
  the reversed cursor-highlight row in `render.rs`) becomes
  footer-aware: `text_range.end - 1` when `cursor_footer` else
  `text_range.start`, matching G2 step 1. This is the only rendering
  change needed — `render.rs`'s highlight logic already keys off this
  helper's return value, not off the node index directly.

- G5: `handle_click` (`mouse.rs`) gains a fallback: if the clicked
  line isn't in `line_to_node`, try `footer_line_to_node`; on a hit,
  move the cursor there with `cursor_footer = true` (via `record_jump`
  + the same cursor-set path as a header click). No fold-toggle check
  runs for a footer click — footer lines never carry a fold-marker
  glyph, so the existing `rel_col - 1 == marker_column(...)` check
  simply doesn't apply and is skipped entirely for this branch.

- G6: invariant enforcement for the two shape-changing cases identified
  in Background — both force `cursor_footer` back to `false` (falling
  back to the node's header, i.e. exactly the "corresponding opening
  bracket" the user asked for) the moment the footer line it was
  resting on stops existing/being visible:
  1. End of `splice_override`: if `self.cursor == idx` (the spliced
     node) and `self.cursor_footer` is `true` but `idx` no longer has
     children, set `self.cursor_footer = false`.
  2. `toggle_fold`: right after folding (not unfolding) `idx`, if
     `idx == self.cursor` and `self.cursor_footer` is `true`, set
     `self.cursor_footer = false`.

- G7: `set_cursor(idx)` (the existing single mutation path for
  `self.cursor`, used by every other cursor-setting call site — sibling
  skip `J`/`K`, `Home`/`gg`, jump-history back/forward, in-pane search,
  header-line mouse clicks, override/manage pane hand-offs) explicitly
  resets `cursor_footer = false`. This is a no-op for every existing
  caller (none of them currently produce a footer position — untouched
  behavior) and keeps `set_cursor`'s contract simple: "go to node
  `idx`'s own header row."

- G8: `footer_line_to_node`'s construction (`App::new`,
  `splice_override`'s rebuild) and `has_children()` both switch their
  gate from `first_child.is_some()` to `text_range.end - 1 >
  text_range.start` — see "Empty-but-bracketed messages" above. Makes
  an empty-but-bracketed message a fully-navigable, fully-foldable
  two-line node like any other message, instead of an inert dead spot.

- G9: the status line's `L<n>` field (`render.rs`) uses
  `self.cursor_line()` instead of the node's own header line
  unconditionally — see "Status line reporting the wrong line number"
  above.

## Non-goals

- N1: no change to `App.cursor`'s type or meaning (still a node index)
  — no move to a line-indexed or otherwise reworked cursor
  representation.
- N2: no change to `folded`/`rebuild_visible_rows`/`is_hidden` fold
  bookkeeping itself — folding continues to hide a node's whole body
  including its footer line, unchanged; this spec only makes sure the
  cursor never gets stranded on a line that folding just hid (G6.2).
- N3: ~~no change to `line_to_node`/`footer_line_to_node`'s own
  construction or maintenance~~ — **superseded by G8**: the
  empty-but-bracketed-message bug (found live during this spec's own
  implementation) required a small gate fix at both construction
  sites. `line_to_node`'s own construction is still untouched — only
  `footer_line_to_node`'s gate condition changed.
- N4: no change to jump history (`back_stack`/`fwd_stack`/
  `record_jump`) beyond what G5 already implies — it continues to
  store/restore node indices only (header position); jumping back to a
  node that was left on its footer restores to that node's header, not
  its footer. Not worth the extra bookkeeping for a rarely-exercised
  corner of a corner case.
- N5: no change to `next_sibling_move`/`prev_sibling_move` (`J`/`K`) —
  sibling-skip always lands on a sibling's header, unchanged (G7 covers
  this for free, since both already go through `set_cursor`).
- N6: no change to how many on-screen rows a footer line occupies (one,
  like any other line) or to `max_visible_line_len`/pan logic, which
  already iterate `visible_rows` directly rather than per-node.

## Specification

### `App` struct (`mod.rs`)

Add, next to `cursor: usize`:

```rust
/// `true` when the cursor is visually resting on `cursor`'s own
/// closing `}` line rather than its header line (spec 0142). `cursor`
/// itself is unaffected — still the node whose bracket pair the
/// cursor belongs to, so every existing node-indexed action (fold,
/// override edit, status line, etc.) already treats a footer-resting
/// cursor exactly like its header, satisfying the "acts as if on the
/// opening bracket" requirement with no extra redirection.
cursor_footer: bool,
```

Initialize to `false` in `App::new`'s struct literal.

### `navigation.rs`

New private helper (used by `move_down`/`move_up`/`move_end`/G4/G5):

```rust
/// `self.cursor`'s own currently-displayed line: its footer line
/// (`text_range.end - 1`) if `cursor_footer`, else its header line
/// (`text_range.start`).
pub(super) fn cursor_line(&self) -> usize {
    let span = &self.tree[self.cursor].span;
    if self.cursor_footer {
        span.text_range.end - 1
    } else {
        span.text_range.start
    }
}

/// Resolve a visible line back to a `(node, is_footer)` cursor stop —
/// `line_to_node` (header) checked first, `footer_line_to_node`
/// (footer) as fallback; the two never overlap for the same line.
fn resolve_cursor_line(&self, line: usize) -> Option<(usize, bool)> {
    if let Some(&idx) = self.line_to_node.get(&line) {
        return Some((idx, false));
    }
    self.footer_line_to_node.get(&line).map(|&idx| (idx, true))
}
```

`move_down`/`move_up` become:

```rust
pub(super) fn move_down(&mut self) {
    let cur = self.cursor_line();
    if let Ok(pos) = self.visible_rows.binary_search(&cur) {
        if let Some(&line) = self.visible_rows.get(pos + 1) {
            if let Some((idx, footer)) = self.resolve_cursor_line(line) {
                self.cursor = idx;
                self.cursor_footer = footer;
                self.cursor_moves += 1;
            }
        }
    }
}

pub(super) fn move_up(&mut self) {
    let cur = self.cursor_line();
    if let Ok(pos) = self.visible_rows.binary_search(&cur) {
        if pos > 0 {
            if let Some((idx, footer)) = self.resolve_cursor_line(self.visible_rows[pos - 1]) {
                self.cursor = idx;
                self.cursor_footer = footer;
                self.cursor_moves += 1;
            }
        }
    }
}
```

`move_end` becomes:

```rust
pub(super) fn move_end(&mut self) {
    let Some(&last_line) = self.visible_rows.last() else {
        return;
    };
    if let Some((idx, footer)) = self.resolve_cursor_line(last_line) {
        if self.cursor != idx || self.cursor_footer != footer {
            self.record_jump(self.cursor);
            self.cursor = idx;
            self.cursor_footer = footer;
            self.cursor_moves += 1;
        }
    }
}
```

(`last_node()`/its `is_hidden` fallback become unused by `move_end`
after this change; keep `last_node()` only if another caller still
needs it — otherwise remove it.)

`set_cursor` gains one line:

```rust
pub(super) fn set_cursor(&mut self, idx: usize) {
    self.cursor = idx;
    self.cursor_footer = false;
    self.cursor_moves += 1;
}
```

`toggle_fold` gains G6.2's guard:

```rust
pub(super) fn toggle_fold(&mut self, idx: usize) {
    if !self.folded.remove(&idx) {
        self.folded.insert(idx);
        if idx == self.cursor && self.cursor_footer {
            self.cursor_footer = false;
        }
    }
    self.rebuild_visible_rows();
}
```

### `mouse.rs`

`cursor_display_row` (G4):

```rust
pub(super) fn cursor_display_row(&self) -> usize {
    self.visible_rows
        .binary_search(&self.cursor_line())
        .unwrap_or_else(|i| i)
}
```

`handle_click` (G5) — after the existing `line_to_node` lookup fails,
try the footer map before giving up:

```rust
pub(super) fn handle_click(&mut self, col: u16, row: u16) {
    let Some(line_idx) = self.main_pane_line_idx(col, row) else {
        return;
    };
    if let Some(&idx) = self.line_to_node.get(&line_idx) {
        if idx != self.cursor || self.cursor_footer {
            self.record_jump(self.cursor);
            self.set_cursor(idx);
        }
        if self.has_children(idx) {
            let area = self.main_area;
            let rel_col = col - area.x;
            if rel_col >= 1 && rel_col - 1 == marker_column(&self.lines[line_idx]) {
                self.toggle_fold(idx);
            }
        }
        return;
    }
    if let Some(&idx) = self.footer_line_to_node.get(&line_idx) {
        if idx != self.cursor || !self.cursor_footer {
            self.record_jump(self.cursor);
            self.cursor = idx;
            self.cursor_footer = true;
            self.cursor_moves += 1;
        }
    }
}
```

### `override_apply.rs`

At the end of `splice_override`, after `footer_line_to_node` is
rebuilt (G6.1) — the retyped node no longer has a footer if it's no
longer a message/group:

```rust
if self.cursor_footer && !self.has_children(self.cursor) {
    self.cursor_footer = false;
}
```

(Placed unconditionally at the very end of the function, after the
rebuild, so it applies regardless of which node was spliced — cheap,
and correct even for the packed-record `idx` reassignment case, since
it only ever acts when `self.cursor` itself lost its children.)

Also (G8), the same file's `footer_line_to_node` rebuild loop switches
its gate condition:

```rust
// was: if self.tree[c].first_child.is_some() { ... }
if self.tree[c].span.text_range.end - 1 > self.tree[c].span.text_range.start {
    self.footer_line_to_node
        .insert(self.tree[c].span.text_range.end - 1, c);
}
```

### `mod.rs` (G8, G9)

`App::new`'s `footer_line_to_node` construction loop gets the same
gate-condition switch as `override_apply.rs` above.

`render.rs`'s status line uses `self.cursor_line()` instead of
`node.text_range.start + 1` for its `L<n>` field (G9).

### `navigation.rs` (G8)

`has_children` becomes:

```rust
pub(super) fn has_children(&self, idx: usize) -> bool {
    let span = &self.tree[idx].span;
    span.text_range.end - 1 > span.text_range.start
}
```

(`next_visible`/`prev_visible`/`is_hidden` become entirely unused once
`move_down`/`move_up`/`move_end` are rewritten per G2/G3 and are
removed.)

## Test plan

1. `Down` from the last non-`}` node of a small message reaches that
   node's own closing `}` line next (was previously a no-op / skipped
   straight to the parent's next sibling).
2. Repeated `Down` from the document's first node reaches the virtual
   encompassing wrapper's own final `}` as the very last stop — the
   document's true last line becomes reachable.
3. `Up` from a footer line returns to the last child inside that node
   (not to the node's own header) — footer-to-body symmetry with (1).
4. `End`/`G` lands on the document's true last visible line (a footer,
   in the common case where the outermost node isn't the only content).
5. A fold-toggle action (`Enter`/`f`/marker click) invoked while the
   cursor rests on `idx`'s own footer folds `idx` and snaps the cursor
   back to `idx`'s header (G6.2) — no stale footer-cursor referencing a
   now-hidden line.
6. Retyping (`t`) a message node the cursor's footer is resting on into
   a scalar leaves the cursor on that node's (now sole) line with
   `cursor_footer == false` (G6.1) — no stale footer flag surviving a
   shape change.
7. Retyping a scalar node into a message FQDN, then `Down`, reaches the
   newly-created footer line as a new stop (confirms G2 naturally picks
   up newly-expanded footers with no special-casing needed).
8. Clicking directly on a `}` line places the cursor there
   (`cursor_footer == true`, node = the line's owning node) without
   toggling that node's fold state.
9. Any node-indexed action invoked while `cursor_footer` is `true`
   (status line, `t`/override editing, `y`ank/copy-line, positional
   path display) behaves identically to invoking it from that same
   node's header — confirms the "acts as if on the opening bracket"
   requirement (G1).
10. `cargo fmt --check`, `cargo clippy --all-targets`, full test suite
    pass.
11. `Down`/`Up` correctly pass through an empty-but-bracketed message
    (zero populated fields, still rendered as `Name {` / `}` on two
    lines): both the header and the footer are reachable stops (G8).
12. An empty-but-bracketed message shows a fold marker and is foldable
    via marker click and via `Left` (G8).
13. The status line's `L<n>` reports the footer's own 1-based line
    number when `cursor_footer` is `true` (G9).
