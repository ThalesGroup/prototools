// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

/// Which of a main-pane row's two click zones a `Down` landed in
/// (spec 0284 S6).
///
/// A double-click pair has to agree on its zone, so this is half of
/// `last_click`'s key: a click on the text followed by a click on the
/// cue beside it is two single clicks, not a gesture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ClickZone {
    /// The row's own text, whose double-click selects the line.
    Text,
    /// The heat cue's `[…]` suffix, whose double-click opens the
    /// override selection pane.
    Cue,
}

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
        // all. It is answered here and returns immediately; without this
        // guard the side effects further down (splash dismissal in
        // particular) fire on the first stray cursor twitch after
        // startup, before the user has even seen the splash screen, and
        // status messages vanish while the mouse merely hovers over the
        // terminal. Spec 0280 S8 keeps the arm exactly where spec 0223
        // put it, for exactly that reason.
        if event.kind == MouseEventKind::Moved {
            // Spec 0280 S9: arming a dwell is not a visible change, and
            // the frame the dwell eventually needs is bought by
            // `hover_deadline` through `ui_deadline`. Only tearing down
            // a box already on screen owes this event a frame — which is
            // what keeps a pointer crossing the pane from redrawing at
            // motion rate (G5).
            self.event_changed_nothing = !self.handle_hover(event.column, event.row);
            return;
        }

        // Spec 0280 S16: anything that is not the pointer holding still
        // takes the box down, so there is no dismiss binding to learn.
        self.dismiss_popup();

        // Dismiss the splash screen transparently, same as `handle_key`
        // (spec 0113 D22/D28): the mouse event that dismisses it is also
        // processed as a real event, not swallowed.
        self.splash = false;

        self.message.clear();
        // Spec 0278 S3: a wheel scroll dismisses the search echo — and
        // spec 0277's count with it — exactly as a keypress does.
        self.search_echo = None;

        // An open context menu takes the mouse before anything else,
        // for the same reason it takes the keyboard: it is the innermost
        // modal, and the panes below it have no idea it exists. A click
        // inside picks a row, a click anywhere else dismisses without
        // acting — the universal behavior for a menu, and the one that
        // makes an accidental right-click cost nothing.
        if self.menu.is_some() {
            match event.kind {
                MouseEventKind::ScrollDown => self.move_menu_selection(1),
                MouseEventKind::ScrollUp => self.move_menu_selection(-1),
                MouseEventKind::Down(_) => {
                    let hit = self
                        .menu
                        .as_ref()
                        .and_then(|m| m.item_at(event.column, event.row));
                    match hit {
                        Some(idx) => self.activate_menu_item(idx),
                        None => self.menu = None,
                    }
                }
                _ => {}
            }
            return;
        }

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

        // `input-bindings-review.md` C9: right-click opens the context
        // menu for whatever is under the pointer. Placed beside the
        // `Down(Left)` arm and routed by the same hit tests, including
        // `main_interactive` — while the override pane locks focus, a
        // right-click is refused and named exactly as a left-click is.
        if let MouseEventKind::Down(MouseButton::Right) = event.kind {
            let anchor = (event.column, event.row);
            if main_interactive {
                // The focus claim is unconditional, unlike the caret
                // move it used to be nested in: every row of either menu
                // acts on the main pane, so the pane has to have the
                // keyboard by the time one runs.
                self.override_focus = false;
                self.manage_focus = false;
                // A click past the end of the document names no node, and
                // that is a surface in its own right rather than a miss:
                // it is the pane, so it gets the pane's own settings.
                let items = if self.caret_to_point(event.column, event.row) {
                    self.main_menu_items()
                } else {
                    self.pane_menu_items()
                };
                self.open_menu(items, anchor);
            } else if over_main {
                self.message = OVERRIDE_FOCUS_LOCK_MESSAGE.to_string();
            } else if over_side && self.manage_open {
                // The override selection pane has no per-candidate
                // actions to offer — `Enter` applies and `Esc` closes,
                // and both are already on the pane's own statusline — so
                // only the manage pane answers a right-click.
                if let Some(idx) = self.manage_row_at(event.column, event.row) {
                    self.manage_focus = true;
                    self.set_manage_highlight(idx);
                    let items = self.manage_menu_items();
                    self.open_menu(items, anchor);
                }
            }
            return;
        }

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
                //
                // The heat cue is a control too, but it acts only on the
                // second click of a pair, so it does not need that rule
                // and pairs normally (spec 0284 S7). It does need its
                // own half of the key: the two zones' double-clicks mean
                // different things, so a pair may not straddle them.
                self.pending_double_click = match line_idx {
                    Some(l) if !on_marker => {
                        let zone = match self.heat_cue_at_point(event.column, event.row) {
                            Some(_) => ClickZone::Cue,
                            None => ClickZone::Text,
                        };
                        is_double_click(&mut self.last_click, (l, zone)).then_some(zone)
                    }
                    _ => {
                        self.last_click = None;
                        None
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
            } else if over_cmd {
                // Spec 0275 S1: the command line answers a click like
                // every other text with a caret in it. Last in the
                // chain, and safely so — the three areas are disjoint.
                //
                // No focus claim, unlike the two arms above: while a
                // command line is open `handle_key` routes to it ahead
                // of any focus-specific dispatch, so it is modal rather
                // than focused and there is nothing to claim (N3).
                self.command_click(event.column);
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
                //
                // On the heat cue it is the other way round (spec 0284
                // S4). The cue asks *some other type fits these bytes
                // better*, and the override pane is the answer, so a
                // double-click there opens it and selects nothing — the
                // row's text is not what was pointed at, and spec 0242
                // S11 already puts the suffix in no selection. The
                // first click of the pair has put the caret on that
                // line, and `toggle_override` acts on the cursor, so the
                // gesture needs no positioning step of its own. Its
                // close arm is unreachable from here: with the pane open
                // `main_interactive` is false and this whole block is
                // skipped.
                MouseEventKind::Up(MouseButton::Left) => match self.pending_double_click {
                    Some(ClickZone::Text) => self.select_current_line(),
                    Some(ClickZone::Cue) => {
                        self.clear_selection();
                        self.toggle_override();
                    }
                    // No drag happened, so the click selected nothing —
                    // drop the anchor it armed rather than leave it
                    // standing.
                    None if !self.select_engaged => self.clear_selection(),
                    None => {}
                },
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
        self.main_pane_line_part(col, row).map(|(line, _)| line)
    }

    /// The same, keeping *which* terminal row of the pair it was
    /// (spec 0282 S2): 0 is the document row, 1 the wire row below it.
    ///
    /// A click does not care — spec 0225 S8 has a click anywhere in the
    /// pair select the line, because the row is taller and not two
    /// controls. A hover does: the two rows hold different things, and
    /// asking about a thing is not the same as choosing a line.
    pub(super) fn main_pane_line_part(&self, col: u16, row: u16) -> Option<(usize, usize)> {
        let area = self.main_area;
        if !Self::rect_contains(area, col, row) {
            return None;
        }
        // Which rows are which is spec 0268 S4's map rather than a
        // division.
        //
        // Spec 0230: `scroll.skip` shifts the whole pane by a terminal
        // row, so it is added back before the division. A negative skip
        // draws blank rows above the document, and a click on one of
        // those names no line at all.
        let rel_row = (row - area.y) as isize + self.scroll.skip;
        if rel_row < 0 {
            return None;
        }
        let heights = self.row_heights();
        let (content_row, part) =
            heights.row_at(heights.offset(self.scroll.index) + rel_row as usize);
        self.visible_row_pos(content_row)
            .map(|(_, line)| (line, part))
    }

    /// Spec 0284 S2: the line whose drawn heat suffix — ` [3/7]`,
    /// ` [2@7]`, ` [?/7]` or ` [?]` — covers `(col, row)`, or `None`
    /// where no suffix is drawn there.
    ///
    /// The suffix begins one column past the row's text: column 0 is the
    /// reserved heat gutter (spec 0138 N1) and the text between them is
    /// as long as the pan left it. **No `pan_offset` is added back**,
    /// unlike the fold marker's hit test above: `render` pushes the
    /// suffix span *after* `pan_spans`, so the suffix does not scroll.
    ///
    /// Only the document row of a wire pair is a target, since only it
    /// draws a suffix. A cue hidden by `i`, or a line that draws none,
    /// is `None` — the target is exactly what is on screen.
    pub(super) fn heat_cue_at_point(&mut self, col: u16, row: u16) -> Option<usize> {
        let (line_idx, part) = self.main_pane_line_part(col, row)?;
        if part != 0 {
            return None;
        }
        let pos = self.line_pos(line_idx)?;
        let display = self.heat_cue_at(pos);
        let (_, suffix) = self.heat_chrome(&display);
        let width = suffix?.content.chars().count();
        let drawn = self.committed_row_at(line_idx, pos);
        let start = 1 + self
            .row_content(drawn)
            .chars()
            .count()
            .saturating_sub(self.pan_offset);
        let x = (col - self.main_area.x) as usize;
        (start..start + width).contains(&x).then_some(line_idx)
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
        let x = col.saturating_sub(self.main_area.x) as usize;
        self.cursor_column = if x == 0 {
            // Spec 0284 S5: the gutter is one zone of its own, ahead of
            // the two below, and means the row's first non-blank at any
            // pan. Setting 0 is enough — the `clamp_caret_column` at the
            // end of this function raises it to `caret_bounds().0`,
            // which is that character by definition.
            //
            // Letting the saturation below decide instead gave the same
            // answer only while unpanned: with `pan_offset > 0` it
            // landed on the leftmost *visible* character. The gutter
            // does not scroll, so a click in it must not mean two
            // things depending on how far the view has.
            //
            // The anchor stays `Free` (N3): this places the caret, it
            // does not declare `CaretAnchor::Home` the way `0`/`^` does.
            0
        } else if x - 1 < panned {
            (x - 1 + self.pan_offset).saturating_sub(render::FOLD_FIELD_WIDTH)
        } else {
            text_chars + (x - 1 - panned)
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
        self.select_engaged = self.select_anchor.is_some();
        self.caret_to_point(col, row);
    }

    /// Moves the caret to the character under a point, and does nothing
    /// else — no fold toggle, no selection, no focus claim.
    ///
    /// This is what a right-click needs: the menu it opens is about the
    /// node under the pointer, so the caret has to go there first (every
    /// binding the menu replays reads the caret), but the click itself
    /// must not be an edit.
    fn caret_to_point(&mut self, col: u16, row: u16) -> bool {
        let Some(line_idx) = self.main_pane_line_idx(col, row) else {
            return false;
        };
        let Some(pos) = self.line_pos(line_idx) else {
            return false;
        };
        self.cursor = pos.node;
        self.cursor_line_in_node = pos.line_in_node;
        self.cursor_moves += 1;
        self.set_caret_from_click(col, line_idx, pos);
        true
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
        self.pending_double_click = None;
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
