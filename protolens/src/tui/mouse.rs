// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

impl App {
    /// Handle one mouse event: wheel scroll pans the hovered pane's
    /// viewport (2026-07-19 feedback item 2); a left click on a foldable
    /// node's marker column toggles its fold, a click elsewhere on a
    /// node's line moves the cursor there.
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        // A bare `Moved` event (no button held, no wheel) is pointer-
        // tracking noise, not user input — `EnableMouseCapture` turns on
        // any-motion reporting, so the terminal sends one of these on
        // essentially every pixel the mouse crosses, with no click at
        // all. Nothing in this function does anything with `Moved`
        // itself; without this guard, the side effects below (splash
        // dismissal in particular) fired on the very first stray cursor
        // twitch after startup, well before the user ever saw the splash
        // screen, or read as an unintended cause behind status messages
        // vanishing while the mouse merely hovered over the terminal.
        if event.kind == MouseEventKind::Moved {
            return;
        }

        // Dismiss the splash screen transparently, same as `handle_key`
        // (spec 0113 D22/D28): the mouse event that dismisses it is also
        // processed as a real event, not swallowed.
        self.splash = false;

        self.message.clear();

        // Feedback (2026-07-15): while the `F1` help overlay is open,
        // mouse wheel/Shift-wheel hovering over it scrolls its own text
        // instead of leaking through to whichever pane happens to be
        // drawn underneath — `over_main`/`over_side` below have no idea
        // the overlay exists, so this must be checked first. Shift-wheel
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
                // 2026-07-19 feedback item 4: clamped on the right, same
                // as the main pane's own `pan_right` below. The wheel
                // always pans at `WHEEL_PAN_STEP`, unlike Ctrl-Left/
                // Ctrl-Right's `PAN_STEP` (2026-07-19 feedback).
                if self.manage_open {
                    self.manage_pan_horizontal(WHEEL_PAN_STEP, pan_left);
                } else {
                    self.override_pan_horizontal(WHEEL_PAN_STEP, pan_left);
                }
            } else if over_main {
                // Wheel step, not `PAN_STEP` (2026-07-19 feedback).
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
        // hovering, independent of keyboard focus (2026-07-14 feedback,
        // item 4) — unlike `handle_key`, which always follows focus,
        // since a mouse event already carries its own screen position
        // (`event.column`/`event.row`), making hover-based routing both
        // natural and unambiguous. Pans the hovered pane's viewport
        // (2026-07-19 feedback item 2) rather than moving the cursor/
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

        if let MouseEventKind::Down(MouseButton::Left) = event.kind {
            if main_interactive {
                // A click in the main pane always shifts keyboard focus
                // back to it without closing the side pane (2026-07-14
                // feedback, item 3) — `handle_key` follows `override_focus`/
                // `manage_focus`, so clearing them here is what makes the
                // shift stick for subsequent keystrokes too.
                self.override_focus = false;
                self.manage_focus = false;
                self.handle_click(event.column, event.row);
                let line_idx = self.main_pane_line_idx(event.column, event.row);

                // Double-click detection (feedback, 2026-07-15): crossterm
                // reports `Down` identically for single and double
                // clicks, so recognizing the second click of a pair means
                // comparing this `Down` against the previous one's own
                // timestamp/line ourselves (`is_double_click`, generalized
                // 2026-07-17 to also serve the manage pane's radio-marker
                // double-click). The `Up` handler below is what actually
                // acts on `pending_double_click`.
                self.pending_double_click = match line_idx {
                    Some(l) => is_double_click(&mut self.last_click, l),
                    None => {
                        self.last_click = None;
                        false
                    }
                };

                // Spec 0129 §G1: a click also (re-)seeds the drag
                // selection's anchor/end, replacing any previous one, so
                // a following `Drag` still works. Whether a *non-dragged*
                // click keeps or discards this single-line selection is
                // decided by the `Up` handler below (feedback, 2026-07-15:
                // a plain click now deselects; only a double-click keeps
                // it selected).
                self.select_anchor = line_idx;
                self.select_end = line_idx;
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
                // Spec 0129 §G1: dragging extends the selection's end to
                // the row under the pointer; clamped to the pane's
                // currently-visible rows (no auto-scroll past the top/
                // bottom edge in this first cut — an out-of-bounds drag
                // position simply leaves `select_end` where it was).
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some(line_idx) = self.main_pane_line_idx(event.column, event.row) {
                        self.select_end = Some(line_idx);
                    }
                }
                // Spec 0131 §G1: mouse release intentionally no longer
                // copies by itself — selection state was already
                // finalized by the preceding `Down`/`Drag` handling
                // (§G1/§G3 of spec 0129, unchanged); `Ctrl-C` is now the
                // sole trigger for the actual clipboard write.
                //
                // Feedback (2026-07-15): single vs. double click vs. drag
                // disambiguation. A drag (`select_anchor != select_end`)
                // always keeps its selection, unchanged. Otherwise: a
                // plain single click deselects everything; a double-click
                // (recognized by the `Down` handler above, same line,
                // within `DOUBLE_CLICK_THRESHOLD`) instead keeps the
                // single-line selection `Down` just set, and additionally
                // acts as the same `t`/`o` smart proxy as `Enter` (item 3,
                // spec 0139 follow-up).
                MouseEventKind::Up(MouseButton::Left) => {
                    if self.pending_double_click {
                        self.open_smart_override_or_manage();
                    } else if self.select_anchor == self.select_end {
                        self.select_anchor = None;
                        self.select_end = None;
                    }
                }
                _ => {}
            }
        }
    }

    /// Spec 0129 §G2: the currently-selected main-pane lines' full
    /// (untruncated) text, one `render_line_content` per line in
    /// `min(select_anchor, select_end)..=max(...)`, joined with `\n`,
    /// alongside the line count — `None` if there is no active
    /// selection. Split out from `copy_selection_to_clipboard` so the
    /// text-building logic is testable independent of real OS clipboard
    /// access (unavailable e.g. in headless/CI environments).
    pub(super) fn selected_text(&self) -> Option<(usize, String)> {
        let (Some(anchor), Some(end)) = (self.select_anchor, self.select_end) else {
            return None;
        };
        let (start, stop) = (anchor.min(end), anchor.max(end));
        let text = (start..=stop)
            .map(|i| self.render_line_content(i))
            .collect::<Vec<_>>()
            .join("\n");
        Some((stop - start + 1, text))
    }

    /// Spec 0129 §G2/0131 §G2: copy the currently-selected main-pane
    /// lines to the OS clipboard. No-op if there is no active selection.
    /// `copy_to_clipboard` always attempts an OSC 52 fallback when
    /// `arboard` fails (no reliable ack from the terminal either way),
    /// so a failure here still reports an (optimistic) success message
    /// rather than "clipboard unavailable" — spec 0131 §G2's "safest
    /// default."
    pub(super) fn copy_selection_to_clipboard(&mut self) {
        let Some((count, text)) = self.selected_text() else {
            return;
        };
        self.message = match copy_to_clipboard(&text) {
            Ok(()) => format!("{count} line(s) copied to clipboard"),
            Err(_) => format!("{count} line(s) copied to clipboard (OSC 52 fallback)"),
        };
    }

    /// Spec 0131 §G1: `Ctrl-C` — copies the active drag-selection if one
    /// exists (unchanged `selected_text`/`copy_selection_to_clipboard`
    /// logic), else falls back to the cursor's own current line, treated
    /// as a length-1 selection so the existing range-based copy logic
    /// applies unchanged.
    pub(super) fn copy_current_selection_or_line(&mut self) {
        if self.select_anchor.is_none() {
            let line_idx = self.visible_rows[self.cursor_display_row()];
            self.select_anchor = Some(line_idx);
            self.select_end = Some(line_idx);
        }
        self.copy_selection_to_clipboard();
    }

    /// Whether `(col, row)` falls inside `area` (used for mouse hit-
    /// testing against `main_area`/`side_area`).
    pub(super) fn rect_contains(area: Rect, col: u16, row: u16) -> bool {
        col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
    }

    /// Mouse handling for the override selection pane (spec 0113 D30):
    /// wheel scroll pans the candidate list by one row (2026-07-19
    /// feedback item 2 — it no longer moves the highlight, unlike `j`/
    /// `k`), click moves the highlight to the row under the cursor.
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
        let absolute_row = self.override_scroll + rel_row;
        // Spec 0137's row-0 retirement removed the pinned "raw" row —
        // `override_candidates` is indexed directly now, no `+ 1`.
        if absolute_row < self.override_candidates.len() {
            self.override_highlight = absolute_row;
            // 2026-07-20 feedback: clicking a row must live-preview it in
            // the main pane too, same as `move_override_highlight`'s
            // arrow-key path — this call was missing, so clicking a type
            // moved the highlight but left the old preview on screen.
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
        let rel_row = (row - area.y) as usize;
        self.visible_rows.get(self.scroll_offset + rel_row).copied()
    }

    pub(super) fn handle_click(&mut self, col: u16, row: u16) {
        let Some(line_idx) = self.main_pane_line_idx(col, row) else {
            return;
        };

        if let Some(&idx) = self.line_to_node.get(&line_idx) {
            // A click on a foldable node's own fold marker toggles it
            // without moving the cursor there (2026-07-18 feedback) —
            // the marker is a pure fold control, not a row-selection
            // target, mirroring the common tree-view idiom (e.g. VS
            // Code's file-explorer disclosure triangle). Keyboard
            // focus still shifts to the main pane regardless
            // (`handle_mouse` already clears `override_focus`/
            // `manage_focus` before calling here).
            if self.has_children(idx) {
                let area = self.main_area;
                let rel_col = col - area.x;
                // Column 0 is always the heat-cue gutter (spec 0138 N1:
                // a glyph or a reserved blank, never part of the
                // line's own text) — the marker sits one column
                // further right.
                if rel_col >= 1 && rel_col - 1 == marker_column(&self.lines[line_idx]) {
                    self.toggle_fold(idx);
                    return;
                }
            }

            if idx != self.cursor || self.cursor_footer {
                self.record_jump(self.cursor);
                self.set_cursor(idx);
            }
            return;
        }

        // Spec 0142: a click on a node's own closing `}` line (never in
        // `line_to_node`, only in `footer_line_to_node`) moves the
        // cursor there — no fold-marker check applies, since footer
        // lines never carry a fold glyph.
        if let Some(&idx) = self.footer_line_to_node.get(&line_idx) {
            if idx != self.cursor || !self.cursor_footer {
                self.record_jump(self.cursor);
                self.cursor = idx;
                self.cursor_footer = true;
                self.cursor_moves += 1;
            }
        }
    }

    /// Index of `cursor`'s own currently-displayed line (header or
    /// footer, spec 0142) within `visible_rows`.
    pub(super) fn cursor_display_row(&self) -> usize {
        self.visible_rows
            .binary_search(&self.cursor_line())
            .unwrap_or_else(|i| i)
    }
}
