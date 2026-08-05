// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

impl App {
    /// Handle one mouse event: wheel scroll pans the hovered pane's
    /// viewport; a left click on a foldable node's marker column toggles
    /// its fold, a click elsewhere on a node's line moves the cursor
    /// there.
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        // A bare `Moved` event (no button held, no wheel) is pointer-
        // tracking noise, not user input — `EnableMouseCapture` turns on
        // any-motion reporting, so the terminal sends one of these on
        // essentially every pixel the mouse crosses, with no click at
        // all. Nothing below acts on `Moved` itself; without this guard
        // the side effects further down (splash dismissal in particular)
        // fire on the first stray cursor twitch after startup, before
        // the user has even seen the splash screen, and status messages
        // vanish while the mouse merely hovers over the terminal.
        if event.kind == MouseEventKind::Moved {
            return;
        }

        // Dismiss the splash screen transparently, same as `handle_key`
        // (spec 0113 D22/D28): the mouse event that dismisses it is also
        // processed as a real event, not swallowed.
        self.splash = false;

        self.message.clear();

        // While the `F1` help overlay is open, mouse wheel/Shift-wheel
        // hovering over it scrolls its own text instead of leaking
        // through to whichever pane happens to be drawn underneath —
        // `over_main`/`over_side` below have no idea the overlay
        // exists, so this must be checked first. Shift-wheel
        // is reported as a plain `ScrollUp`/`ScrollDown` with the `SHIFT`
        // modifier set (matched here regardless), not as a distinct
        // event kind — help has no horizontal content to pan, so there's
        // no separate Shift behavior to give it. Native
        // `ScrollLeft`/`ScrollRight` (a real horizontal wheel/trackpad
        // gesture) is likewise swallowed here rather than panning the
        // pane behind the overlay.
        if self.help_open && Self::rect_contains(self.help_area, event.column, event.row) {
            match event.kind {
                MouseEventKind::ScrollDown => self.help_scroll = self.help_scroll.saturating_add(1),
                MouseEventKind::ScrollUp => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => {}
            }
            return;
        }

        let side_open = self.manage_open || self.override_target.is_some();
        let over_side = side_open && Self::rect_contains(self.side_area, event.column, event.row);
        let over_main = Self::rect_contains(self.main_area, event.column, event.row);
        let over_cmd = self
            .cmd_area
            .is_some_and(|area| Self::rect_contains(area, event.column, event.row));

        // Spec 0127 §G2: Shift+wheel and native ScrollLeft/ScrollRight pan
        // whichever pane is under the pointer, instead of the vertical
        // scroll the plain wheel dispatches to below — checked first so
        // it takes priority over the vertical-scroll branch.
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);
        let (pan_left, pan_right) = match event.kind {
            MouseEventKind::ScrollLeft => (true, false),
            MouseEventKind::ScrollRight => (false, true),
            MouseEventKind::ScrollUp if shift => (true, false),
            MouseEventKind::ScrollDown if shift => (false, true),
            _ => (false, false),
        };
        if pan_left || pan_right {
            if over_side {
                // Clamped on the right, same as the main pane's own
                // `pan_right` below. The wheel always pans at
                // `WHEEL_PAN_STEP`, unlike Ctrl-Left/Ctrl-Right's
                // `PAN_STEP`.
                if self.manage_open {
                    self.manage_pan_horizontal(WHEEL_PAN_STEP, pan_left);
                } else {
                    self.override_pan_horizontal(WHEEL_PAN_STEP, pan_left);
                }
            } else if over_main {
                // Wheel step, not `PAN_STEP`.
                if pan_left {
                    self.wheel_pan_left();
                } else {
                    self.wheel_pan_right();
                }
            } else if over_cmd {
                pan_by_step(&mut self.command_pan_offset, WHEEL_PAN_STEP, pan_left);
            }
            return;
        }

        // Wheel scroll routes to whichever pane the mouse is currently
        // hovering, independent of keyboard focus — unlike `handle_key`,
        // which always follows focus, since a mouse event already
        // carries its own screen position (`event.column`/`event.row`),
        // making hover-based routing both natural and unambiguous. Pans
        // the hovered pane's viewport rather than moving the cursor/
        // highlight, same distinction Shift+wheel already makes for
        // horizontal scrolling above.
        if matches!(
            event.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            if over_side {
                if self.manage_open {
                    self.handle_manage_mouse(event);
                } else {
                    self.handle_override_mouse(event);
                }
            } else if over_main {
                match event.kind {
                    MouseEventKind::ScrollDown => self.wheel_pan_down(),
                    MouseEventKind::ScrollUp => self.wheel_pan_up(),
                    _ => unreachable!(),
                }
            }
            return;
        }

        // Spec 0185 S5: while the override selection pane is open, focus
        // is locked to it — a main-pane click moves no focus, no cursor,
        // and starts no selection, and a drag extends nothing. That lock
        // is what keeps the preview overlay's anchor valid. Wheel events
        // are deliberately *not* gated on this: they route by geometry
        // rather than by focus, and panning must keep working (G4).
        let main_interactive = over_main && self.override_target.is_none();

        // `Ctrl` is an alias for `Shift` on a main-pane click, and the
        // one that actually arrives: holding `Shift` is the long-
        // standing terminal convention for "do your *own* selection,
        // ignore the application's mouse reporting" (xterm, VTE, Kitty
        // and others all honor it), so a `Shift`-click is usually eaten
        // before protolens ever sees it — the same reason the manage
        // pane's radio marker offers a double-click alternative to its
        // `Shift`-click. The `Shift` arm is kept for the terminals that
        // do forward it.
        let extend_click = shift || event.modifiers.contains(KeyModifiers::CONTROL);

        if let MouseEventKind::Down(MouseButton::Left) = event.kind {
            if main_interactive && extend_click {
                self.shift_click_to(event.column, event.row);
            } else if main_interactive {
                // A click in the main pane always shifts keyboard focus
                // back to it without closing the side pane —
                // `handle_key` follows `override_focus`/`manage_focus`,
                // so clearing them here is what makes the shift stick
                // for subsequent keystrokes too.
                self.override_focus = false;
                self.manage_focus = false;
                let on_marker = self.handle_click(event.column, event.row);
                let line_idx = self.main_pane_line_idx(event.column, event.row);

                // Double-click detection: crossterm reports `Down`
                // identically for single and double clicks, so
                // recognizing the second click of a pair means comparing
                // this `Down` against the previous one's own timestamp/
                // line ourselves (`is_double_click`, shared with the
                // manage pane's radio-marker double-click). The `Up`
                // handler below is what acts on `pending_double_click`.
                //
                // A click that `handle_click` spent on a fold marker is
                // never half of a pair, and clears `last_click` so that
                // no pair forms across the two zones either, in
                // whichever order they are clicked. The marker is a
                // *control*: it must act on every click that reaches
                // it, and a run of four fast clicks on it must be four
                // toggles, not three plus a gesture.
                self.pending_double_click = match line_idx {
                    Some(l) if !on_marker => is_double_click(&mut self.last_click, l),
                    _ => {
                        self.last_click = None;
                        false
                    }
                };

                // Spec 0242 S10: a click (re-)seeds the selection's
                // anchor, replacing any previous one, so a following
                // `Drag` still works. `handle_click` has already put the
                // caret exactly where the click landed, and the caret is
                // the selection's moving end — so anchoring on it is the
                // whole of what starting a selection means.
                //
                // It does *not* engage the selection, which is what
                // keeps a click from selecting the character under it: a
                // drag or a double-click is the mouse saying so, a bare
                // click is not.
                self.select_anchor = line_idx.is_some().then(|| self.cursor_pos());
                self.select_engaged = false;
            } else if over_main {
                // Locked out by `main_interactive` above. Same reasoning
                // as `handle_override_key`'s `Tab` arm: a click that does
                // visibly nothing reads as a bug, so name the lock.
                self.message = OVERRIDE_FOCUS_LOCK_MESSAGE.to_string();
            } else if over_side {
                // Symmetric with the main-pane case above: clicking the
                // side pane (re-)claims keyboard focus for it too.
                if self.manage_open {
                    self.manage_focus = true;
                    self.handle_manage_click(event.column, event.row, shift);
                } else {
                    self.override_focus = true;
                    self.handle_override_click(event.column, event.row);
                }
            }
            return;
        }

        if main_interactive {
            match event.kind {
                // Spec 0242 S10: dragging moves the *caret* to the
                // character under the pointer, and the caret is the
                // selection's moving end — so a drag and a run of
                // `Shift`-motions to the same place leave identical
                // state (G3). Clamped to the pane's currently-visible
                // rows: an out-of-bounds drag position simply leaves the
                // caret where it was (N3, no auto-scroll).
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.drag_caret_to(event.column, event.row);
                }
                // Spec 0131 §G1: mouse release deliberately does not copy
                // by itself — selection state is already finalized by
                // the preceding `Down`/`Drag` handling (spec 0129 §G1/
                // §G3), and `Ctrl-C` is the sole trigger for the actual
                // clipboard write.
                //
                // Single vs. double click vs. drag: a drag always keeps
                // the selection its caret motion built; a plain single
                // click deselects everything; a double-click on a line's
                // text (recognized by the `Down` handler above, same
                // line, within `DOUBLE_CLICK_THRESHOLD`) selects the
                // whole row it landed on.
                //
                // The double-click is the one mouse gesture that names a
                // *line* rather than a character, which is why it says so
                // here rather than relying on where the pointer happened
                // to be: `Down` anchored on the clicked character, and a
                // second click on the same character would otherwise
                // select just that character.
                //
                // It selects, and does nothing else. It used to also act
                // as the `t`/`o` smart proxy `Enter` is (spec 0139), but
                // a gesture that both selects text and opens a side pane
                // is two gestures wearing one name — and the pane is the
                // more disruptive of the two to get by accident.
                MouseEventKind::Up(MouseButton::Left) => {
                    if self.pending_double_click {
                        self.select_current_line();
                    } else if !self.select_engaged {
                        // No drag happened, so the click selected
                        // nothing — drop the anchor it armed rather than
                        // leave it standing.
                        self.clear_selection();
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether `(col, row)` falls inside `area` (used for mouse hit-
    /// testing against `main_area`/`side_area`).
    pub(super) fn rect_contains(area: Rect, col: u16, row: u16) -> bool {
        col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
    }

    /// Mouse handling for the override selection pane (spec 0113 D30):
    /// wheel scroll pans the candidate list by one row without moving
    /// the highlight (unlike `j`/`k`); click moves the highlight to the
    /// row under the cursor.
    pub(super) fn handle_override_mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::ScrollDown => self.override_pan_vertical(WHEEL_PAN_STEP, false),
            MouseEventKind::ScrollUp => self.override_pan_vertical(WHEEL_PAN_STEP, true),
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_override_click(event.column, event.row)
            }
            _ => {}
        }
    }

    pub(super) fn handle_override_click(&mut self, col: u16, row: u16) {
        let area = self.side_area;
        if !Self::rect_contains(area, col, row) {
            return;
        }
        let rel_row = (row - area.y) as usize;
        if rel_row >= self.override_list_height {
            return;
        }
        // Spec 0244 S9: an over-panned pane draws blank rows above its
        // first candidate, and a click on one of those names no row.
        let rel_row = rel_row as isize + self.override_scroll.skip;
        if rel_row < 0 {
            return;
        }
        let absolute_row = self.override_scroll.index + rel_row as usize;
        // Spec 0137: `override_candidates` is indexed directly, with no
        // pinned "raw" row to offset past.
        if absolute_row < self.override_candidates.len() {
            self.override_highlight = absolute_row;
            // Clicking a row must live-preview it in the main pane too,
            // same as `move_override_highlight`'s arrow-key path;
            // without this the highlight moves but the old preview stays
            // on screen.
            self.preview_override_highlight();
        }
    }

    /// `line_idx` of the main-pane row under `(col, row)`, or `None` if
    /// the position is outside `main_area` or past the last visible row
    /// (spec 0129 §G1) — shared by `handle_click` and the drag-select
    /// tracking in `handle_mouse`.
    pub(super) fn main_pane_line_idx(&self, col: u16, row: u16) -> Option<usize> {
        let area = self.main_area;
        if !Self::rect_contains(area, col, row) {
            return None;
        }
        // Spec 0225 S8: in wire mode each document line is two terminal
        // rows thick, so a click anywhere in the pair selects the line —
        // the rows are simply taller, not separately clickable.
        //
        // Spec 0230: `scroll.skip` shifts the whole pane by a terminal
        // row, so it is added back before the division. A negative skip
        // draws blank rows above the document, and a click on one of
        // those names no line at all.
        let rel_row = (row - area.y) as isize + self.scroll.skip;
        if rel_row < 0 {
            return None;
        }
        let rel_row = rel_row as usize / self.row_height();
        self.visible_row_pos(self.scroll.index + rel_row)
            .map(|(_, line)| line)
    }

    /// Dispatch one main-pane left-click: on a fold marker it toggles
    /// the fold, anywhere else it moves the caret under the pointer.
    ///
    /// Returns whether the click was spent on a fold marker. The caller
    /// uses that to keep such a click out of its double-click pairing
    /// entirely: a marker is a control, and clicking a control n times
    /// must mean acting on it n times, however fast they arrive.
    pub(super) fn handle_click(&mut self, col: u16, row: u16) -> bool {
        let Some(line_idx) = self.main_pane_line_idx(col, row) else {
            return false;
        };

        let Some(pos) = self.line_pos(line_idx) else {
            return false;
        };

        // A click on a foldable node's own fold marker toggles it, and
        // `toggle_fold` puts the cursor on the node it toggled — the
        // click named that node twice over, by pointing at it and by
        // asking to change it. Keyboard focus still shifts to the main
        // pane regardless (`handle_mouse` already clears
        // `override_focus`/`manage_focus` before calling here).
        //
        // The caret lands at the row's `Home` rather than under the
        // pointer, because `set_cursor` is what a node-level jump uses
        // and the marker is left of every column the caret may rest
        // on.
        //
        // Spec 0142: a footer line carries no fold glyph, so the check
        // is confined to the rows a node draws its own content on.
        if !self.is_footer(pos) && self.has_children(pos.node) {
            // Column 0 is always the heat-cue gutter (spec 0138 N1: a
            // glyph or a reserved blank, never part of the line's own
            // text) — the marker sits one column further right.
            //
            // `pan_offset` is added back for the same reason
            // `set_caret_from_click` adds it: the fold margin is part of
            // the row's content, so `pan_spans` drops it along with
            // everything else the pan scrolls off the left edge. Without
            // this the marker's hit target stays at the column it
            // occupied *before* the pan, so on a panned view a click on
            // the visible glyph places the caret and a click on some
            // unrelated character folds the node.
            //
            // The hit target is the marker *and* the blank column
            // `fold_margin` draws beside it — the whole two-column fold
            // field, never any of the row's text (the field is
            // `FOLD_FIELD_WIDTH` wide by construction and the first
            // non-blank starts after it). A one-column target is too
            // small to click repeatedly: a fast run of clicks drifts by
            // a cell and one of its presses silently becomes a caret
            // placement instead of a toggle, which reads as a lost
            // click.
            let rel_col = (col - self.main_area.x) as usize;
            let line = self.line_text(pos);
            let marker = render::marker_column(&line) as usize;
            let field = marker..marker + render::FOLD_FIELD_WIDTH;
            if rel_col >= 1 && field.contains(&(rel_col - 1 + self.pan_offset)) {
                self.toggle_fold(pos.node);
                return true;
            }
        }

        if (self.cursor, self.cursor_line_in_node) != (pos.node, pos.line_in_node) {
            self.record_jump();
            self.cursor = pos.node;
            self.cursor_line_in_node = pos.line_in_node;
            self.cursor_moves += 1;
        }
        // Overwrites the column, the desired column and the anchor
        // outright, which is why the cursor is moved by hand above
        // rather than through `set_cursor`: its caret reset would be
        // invisible.
        self.set_caret_from_click(col, line_idx, pos);
        false
    }

    /// Spec 0194 S7: invert S1's column-to-screen mapping and put the
    /// caret where the click landed.
    ///
    /// The two zones invert differently, because only one of them pans:
    /// a click on the row's own text adds `pan_offset` back and drops
    /// the fold field, while a click past the panned text's right edge
    /// is in the heat suffix, which is appended after panning and so
    /// does not move. A click left of the first non-blank — anywhere in
    /// the gutter — clamps to it rather than being rejected (S3).
    ///
    /// Called after the cursor has been moved, since the reachable range
    /// it clamps into is the *new* row's. Sets `desired_column` too,
    /// exactly as a horizontal key would.
    ///
    /// The caret anchor is always forfeited (spec 0199 S10), even when the
    /// click lands squarely on an end of the row: a click expresses
    /// *where*, never *why*, so it must not arm a fold.
    fn set_caret_from_click(&mut self, col: u16, line_idx: usize, pos: LinePos) {
        let row = self.committed_row_at(line_idx, pos);
        let text_chars = self.row_text(row).chars().count();
        let panned = self
            .row_content(row)
            .chars()
            .count()
            .saturating_sub(self.pan_offset);
        // Column 0 of the pane is the heat glyph's reserved gutter (spec
        // 0138 N1), which is never a caret stop.
        let x = (col.saturating_sub(self.main_area.x) as usize).saturating_sub(1);
        self.cursor_column = if x < panned {
            (x + self.pan_offset).saturating_sub(render::FOLD_FIELD_WIDTH)
        } else {
            text_chars + (x - panned)
        };
        self.clamp_caret_column();
        self.desired_column = self.cursor_column;
        self.caret_anchor = CaretAnchor::Free;
    }

    /// Spec 0242 S10: move the caret to the character under a dragging
    /// pointer, without any of the other things a *click* does.
    ///
    /// No fold-marker toggle: a drag across the gutter would otherwise
    /// fold every node it passed. No `record_jump` either — a drag is
    /// one continuous gesture, and filling the jumplist with a stop per
    /// reported pointer position would make `Ctrl-o` useless.
    ///
    /// A drag *engages* the selection, and stays engaged even if the
    /// pointer comes back to the character it started on: dragging out
    /// and back is how the mouse asks for a single character, the
    /// counterpart of `Shift-Right` `Shift-Left`.
    fn drag_caret_to(&mut self, col: u16, row: u16) {
        let Some(line_idx) = self.main_pane_line_idx(col, row) else {
            return;
        };
        let Some(pos) = self.line_pos(line_idx) else {
            return;
        };
        self.select_engaged = self.select_anchor.is_some();
        self.cursor = pos.node;
        self.cursor_line_in_node = pos.line_in_node;
        self.cursor_moves += 1;
        self.set_caret_from_click(col, line_idx, pos);
    }

    /// Spec 0242 S10: `Shift`-click extends the selection to the clicked
    /// character — the mouse's spelling of holding `Shift` and pressing
    /// an arrow until the caret gets there.
    ///
    /// With nothing engaged yet, the fixed end is wherever the caret
    /// already is: the gesture selects the span between the caret and
    /// the click. The anchor a bare click armed names that same cell,
    /// but re-anchoring here is what makes the gesture work right after
    /// a *keyboard* motion too. With a selection already engaged the
    /// anchor stays put and only the caret moves, so the one gesture
    /// extends or contracts depending on which side of the anchor the
    /// click lands.
    ///
    /// It toggles no fold — a `Shift`-click on the fold field puts the
    /// caret on the row's first character, like any other gutter click —
    /// and forms no double-click pair, since a `Shift`-double-click
    /// would then have to choose between the span and the line.
    fn shift_click_to(&mut self, col: u16, row: u16) {
        // Same focus claim as the plain click below.
        self.override_focus = false;
        self.manage_focus = false;
        self.last_click = None;
        self.pending_double_click = false;
        if !self.select_engaged {
            self.select_anchor = Some(self.cursor_pos());
        }
        // Engages, exactly as a drag to the same place would — the two
        // gestures differ only in whether the button stayed down.
        self.drag_caret_to(col, row);
    }

    /// Which drawn row `cursor`'s own currently-displayed line (header
    /// or footer, spec 0142) is on.
    pub(super) fn cursor_display_row(&self) -> usize {
        self.visible_row_of_line(self.cursor_line()).unwrap_or(0)
    }
}
