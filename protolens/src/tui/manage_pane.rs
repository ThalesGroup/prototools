// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

/// Character column (within `manage_type_line`'s own "  <marker> ..."
/// layout) the active/inactive radio marker renders at — kept as a named
/// constant since `handle_manage_click` needs to recognize the same
/// column a mouse click landed on to toggle it.
const MANAGE_MARKER_COL: usize = 2;

impl App {
    /// `o`: toggle the override management pane (spec 0117 §3). Closes
    /// it (cancelling) if already open. Otherwise opens it — no
    /// cursor-node-kind precondition, unlike `t` — closing the override
    /// selection pane first if it's open (mutual exclusion, one shared
    /// right-hand UI slot).
    pub(super) fn toggle_manage_pane(&mut self) {
        if self.manage_open {
            self.close_manage_pane();
            return;
        }
        if self.term_width < MIN_OVERRIDE_WIDTH {
            self.message = format!(
                "terminal too narrow for override management pane (need >= \
                 {MIN_OVERRIDE_WIDTH} columns)"
            );
            return;
        }
        if self.override_target.is_some() {
            self.close_override();
        }
        self.manage_open = true;
        self.manage_focus = true;
        self.manage_highlight = self.initial_manage_highlight();
        self.manage_scroll = 0;
        self.last_manage_highlight = None;
        self.manage_pan_offset = 0;
        self.manage_pending_kind = None;
        self.last_manage_click = None;
    }

    /// The manage pane's initial highlight, in priority order (feedback,
    /// 2026-07-17): (1) an entry already active for the cursor node
    /// (`resolve_active_override_entry_index`); else (2)
    /// `first_entry_matching_origin_candidates`'s result; else (3)
    /// simply the first entry in the pane. `0` (a no-op, since there's
    /// nothing to highlight) when the collection is empty.
    fn initial_manage_highlight(&self) -> usize {
        if self.overrides.entries().is_empty() {
            return 0;
        }
        if let Some(i) = self.resolve_active_override_entry_index(self.cursor) {
            return i;
        }
        self.first_entry_matching_origin_candidates(self.cursor)
            .unwrap_or(0)
    }

    /// The first entry in `overrides.entries()`'s own display order
    /// whose origin would resolve against `idx` under *some*
    /// `OverrideKind` (`Path`/`PathField`/`FqdnField`), regardless of
    /// whether that entry is currently active. Shared by
    /// `initial_manage_highlight` (`o` key) and `toggle_override`'s
    /// smart-open logic (`t` key, spec 0139).
    pub(super) fn first_entry_matching_origin_candidates(&self, idx: usize) -> Option<usize> {
        let candidates: Vec<OverrideOrigin> = [
            OverrideKind::Path,
            OverrideKind::PathField,
            OverrideKind::FqdnField,
        ]
        .into_iter()
        .filter_map(|k| self.origin_for_kind(idx, k).ok())
        .collect();
        self.overrides
            .entries()
            .iter()
            .position(|e| candidates.contains(&e.origin))
    }

    /// Close the override management pane (spec 0117 §3).
    pub(super) fn close_manage_pane(&mut self) {
        self.manage_open = false;
        self.manage_focus = false;
    }

    /// Move the management pane's highlighted row by `delta`, clamped to
    /// `0..overrides.entries().len()` (spec 0117 §3's `j`/`k`).
    pub(super) fn move_manage_highlight(&mut self, delta: isize) {
        let len = self.overrides.entries().len();
        if len == 0 {
            self.manage_highlight = 0;
            self.manage_pending_kind = None;
            return;
        }
        self.manage_highlight = clamp_highlight(self.manage_highlight, delta, len - 1);
        self.manage_pending_kind = None;
    }

    /// Vertical pan for the management pane (Ctrl-Up/Ctrl-Down at `step
    /// == PAN_STEP`, plain mouse wheel at `step == WHEEL_PAN_STEP`,
    /// 2026-07-19 feedback items 1/2): scrolls the listing without
    /// moving the highlight, bounded only by the content itself.
    pub(super) fn manage_pan_vertical(&mut self, step: usize, up: bool) {
        let max_scroll = self
            .manage_display_rows()
            .len()
            .saturating_sub(self.manage_list_height);
        pan_vertical_by_step(&mut self.manage_scroll, max_scroll, step, up);
    }

    /// Horizontal pan for the management pane (Ctrl-Left/Ctrl-Right,
    /// Shift+wheel/native horizontal scroll, 2026-07-19 feedback item 4):
    /// mirrors the main pane's own `pan_right`, stopping once the
    /// rightmost character of the widest currently-visible row would be
    /// shown — never further.
    pub(super) fn manage_pan_horizontal(&mut self, step: usize, left: bool) {
        let width = self.side_area.width as usize;
        let max_offset = self.manage_max_visible_line_len().saturating_sub(width);
        pan_by_step_clamped(&mut self.manage_pan_offset, max_offset, step, left);
    }

    /// One management-pane display row's rendered text — a `Header`'s own
    /// label, or an `Entry`'s `manage_type_line` (2026-07-19 feedback
    /// item 4, factored out so `manage_max_visible_line_len` matches
    /// exactly what `render_manage_pane` shows).
    pub(super) fn manage_row_text(&self, row: &ManageRow) -> String {
        match row {
            ManageRow::Header(label) => label.clone(),
            ManageRow::Entry(idx) => self.manage_type_line(*idx),
        }
    }

    /// Longest rendered row (in characters) among the management pane's
    /// currently-visible window — the basis for `manage_pan_horizontal`'s
    /// clamp (2026-07-19 feedback item 4), mirroring the main pane's own
    /// `max_visible_line_len`.
    pub(super) fn manage_max_visible_line_len(&self) -> usize {
        let rows = self.manage_display_rows();
        let total = rows.len();
        let start = self.manage_scroll.min(total);
        let end = (self.manage_scroll + self.manage_list_height).min(total);
        rows[start..end]
            .iter()
            .map(|r| self.manage_row_text(r).chars().count())
            .max()
            .unwrap_or(0)
    }

    /// The management pane's grouped-by-origin display rows (spec 0117
    /// §3 amendment): one `Header` row per distinct origin (in the
    /// collection's own sort order — origins never interleave, since
    /// `OverrideCollection::sort` already groups by origin), followed by
    /// one `Entry` row per type recorded under it.
    pub(super) fn manage_display_rows(&self) -> Vec<ManageRow> {
        let mut rows = Vec::new();
        let mut prev_origin: Option<&OverrideOrigin> = None;
        for (idx, entry) in self.overrides.entries().iter().enumerate() {
            if prev_origin != Some(&entry.origin) {
                rows.push(ManageRow::Header(entry.origin.label()));
                prev_origin = Some(&entry.origin);
            }
            rows.push(ManageRow::Entry(idx));
        }
        rows
    }

    /// The manage pane's currently-highlighted display row, resolved
    /// from `manage_highlight`'s entry index — the target row for
    /// `clamp_scroll_to_visible` and for Ctrl-Up/Ctrl-Down vertical
    /// panning (2026-07-18 feedback item 2).
    pub(super) fn manage_highlighted_row(&self) -> usize {
        self.manage_display_rows()
            .iter()
            .position(|r| matches!(r, ManageRow::Entry(idx) if *idx == self.manage_highlight))
            .unwrap_or(0)
    }

    /// The type label for management-pane entry `idx`: the entry's own
    /// `r#type`, or `"<raw / no type>"` if unset — except the internal,
    /// globally-shared `decode::MESSAGE_SET_ITEM_FQDN` is never shown
    /// to the user directly, replaced by the friendly, MessageSet-
    /// specific FQDN instead (2026-07-18 feedback item 4).
    pub(super) fn manage_entry_type_label(&self, idx: usize) -> String {
        let e = &self.overrides.entries()[idx];
        let Some(fqdn) = e.r#type.as_deref() else {
            return "<raw / no type>".to_string();
        };
        if fqdn == decode::MESSAGE_SET_ITEM_FQDN {
            if let OverrideOrigin::Path { path } = &e.origin {
                if let Some(node_idx) = self.resolve_path(path) {
                    if let Some(display) = self.message_set_item_display_fqdn(node_idx) {
                        return display;
                    }
                }
            }
        }
        fqdn.to_string()
    }

    /// One management-pane type row's display text: indented, a
    /// radio-button-style active marker (`●`/`○`) leading — clickable, see
    /// `handle_manage_click` — then the type label (spec 0117 §3
    /// amendment), plus the display-name override when set (spec 0119
    /// §G4).
    pub(super) fn manage_type_line(&self, idx: usize) -> String {
        let e = &self.overrides.entries()[idx];
        let marker = if e.active { '●' } else { '○' };
        let type_label = self.manage_entry_type_label(idx);
        match &e.name {
            Some(name) => format!("  {marker} {type_label} as \"{name}\""),
            None => format!("  {marker} {type_label}"),
        }
    }

    /// Search corpus for management-pane entry `idx` (spec 0117 §3's
    /// `/`/`?`/`n`) — origin label plus type label, so searching for
    /// either the origin or the type finds it, independent of how the
    /// grouped display happens to lay them out across rows.
    pub(super) fn manage_search_text(&self, idx: usize) -> String {
        let e = &self.overrides.entries()[idx];
        let type_label = self.manage_entry_type_label(idx);
        format!("{} {type_label}", e.origin.label())
    }

    /// Find the next management-pane entry (spec 0117 §3's `/`/`?`/`n`)
    /// whose search text (`manage_search_text`) contains `pattern`
    /// (case-insensitive), searching in `dir` from just past the current
    /// highlight, wrapping around. Moves the highlight there on success;
    /// otherwise leaves it unchanged and sets a status-line message.
    pub(super) fn jump_to_manage_match(&mut self, dir: SearchDir, pattern: &str) {
        let n = self.overrides.entries().len();
        if pattern.is_empty() || n == 0 {
            return;
        }
        let needle = pattern.to_lowercase();
        // Convert to the 0-based `start` convention `search_wrap` expects
        // (index to check first, not "current + 1" or "current - 1").
        let start = match dir {
            SearchDir::Forward => (self.manage_highlight + 1) % n,
            SearchDir::Backward => (self.manage_highlight + n - 1) % n,
        };
        match search_wrap(n, start, dir, |i| {
            self.manage_search_text(i).to_lowercase().contains(&needle)
        }) {
            Some(i) => {
                self.manage_highlight = i;
                self.manage_pending_kind = None;
            }
            None => self.message = format!("pattern not found: {pattern}"),
        }
    }

    /// Origins derivable under `kind` from every node in `affected`, in
    /// document order, deduplicated by `OverrideOrigin` equality (spec
    /// 0134 G2 step 4).
    pub(super) fn manage_kind_candidates(
        &self,
        affected: &[usize],
        kind: OverrideKind,
    ) -> Vec<OverrideOrigin> {
        let mut result: Vec<OverrideOrigin> = Vec::new();
        for &node in affected {
            if let Ok(origin) = self.origin_for_kind(node, kind) {
                if !result.contains(&origin) {
                    result.push(origin);
                }
            }
        }
        result
    }

    /// Document-order list of main-pane node indices whose origin
    /// matches `origin` exactly (spec 0124 G1, also reused by G2's `z`
    /// membership test). `Path` has at most one match, via
    /// `resolve_path`; `PathField` scans the one parent's children;
    /// `FqdnField` has no shortcut — a message type of a given FQDN can
    /// recur anywhere in the tree, so this is a full document-order walk.
    pub(super) fn manage_affected_nodes(&self, origin: &OverrideOrigin) -> Vec<usize> {
        match origin {
            OverrideOrigin::Path { path } => self.resolve_path(path).into_iter().collect(),
            OverrideOrigin::PathField { path, field } => {
                let Some(parent) = self.resolve_path(path) else {
                    return Vec::new();
                };
                let mut result = Vec::new();
                let mut child = self.tree[parent].first_child;
                while let Some(c) = child {
                    if self.tree[c].span.field_number == *field {
                        result.push(c);
                    }
                    child = self.tree[c].next_sibling;
                }
                result
            }
            OverrideOrigin::FqdnField { fqdn, field } => {
                let mut result = Vec::new();
                let mut cur = Some(self.first_node);
                while let Some(c) = cur {
                    let parent_fqdn = self.tree[c]
                        .parent
                        .and_then(|p| self.tree[p].span.type_fqdn.as_deref());
                    if self.tree[c].span.field_number == *field
                        && parent_fqdn == Some(fqdn.as_str())
                    {
                        result.push(c);
                    }
                    cur = self.tree[c].doc_next;
                }
                result
            }
        }
    }

    /// Handle a keypress while the override management pane is open (spec
    /// 0117 §3) — always focused while open, no separate focus check
    /// (unlike `handle_override_key`).
    pub(super) fn handle_manage_key(&mut self, key: KeyEvent) {
        if let Some(buffer) = &mut self.manage_rename {
            match key.code {
                KeyCode::Esc => self.manage_rename = None,
                KeyCode::Enter => {
                    let buffer = self.manage_rename.take().expect("checked above");
                    let name = if buffer.is_empty() {
                        None
                    } else {
                        Some(buffer)
                    };
                    let was_active = self.overrides.entries()[self.manage_highlight].active;
                    self.overrides.rename(self.manage_highlight, name);
                    if was_active {
                        self.render_overrides(self.first_node);
                    }
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(c) => buffer.push(c),
                _ => {}
            }
            return;
        }
        // `gg` chord (vim-style jump-to-first), mirroring the main
        // pane's own `handle_key` chord: a first `g` press arms
        // `pending_g`; a second `g` press immediately after jumps to
        // the first entry. Any other key clears the pending state.
        if key.code == KeyCode::Char('g') {
            if self.pending_g {
                self.pending_g = false;
                self.manage_highlight = 0;
                self.manage_pending_kind = None;
            } else {
                self.pending_g = true;
            }
            return;
        }
        self.pending_g = false;

        match key.code {
            KeyCode::Tab => self.manage_focus = false,
            KeyCode::Esc | KeyCode::Char('o') | KeyCode::Char('q') => self.close_manage_pane(),
            // Item 11 (2026-07-17 feedback): opens the selection pane on
            // the highlighted entry to change its type, instead of
            // closing the pane — falls back to the old close behavior
            // when there's nothing to select.
            KeyCode::Enter => {
                if self.overrides.entries().is_empty() {
                    self.close_manage_pane();
                } else {
                    self.open_override_from_manage();
                }
            }
            // Interactive feedback, 2026-07-17: Shift-Up/Shift-Down move
            // the highlight like Up/Down, but also activate the
            // destination entry — deactivating any other entry sharing
            // its origin, per the usual per-origin invariant
            // (`OverrideCollection::set_active`) — a combined "move and
            // select" gesture. Unlike Shift-Space, terminals report
            // Shift-arrow reliably via the modifier bit even without the
            // Kitty keyboard protocol, so no `KITTY_KEYBOARD_ENHANCED`
            // gate is needed here. Must precede the plain `Down`/`Up`
            // arms below, since an unguarded arm there would otherwise
            // shadow it.
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.move_manage_highlight(1);
                if let Some(entry) = self.overrides.entries().get(self.manage_highlight) {
                    if !entry.active {
                        self.overrides.set_active(self.manage_highlight);
                        self.render_overrides(self.first_node);
                    }
                }
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.move_manage_highlight(-1);
                if let Some(entry) = self.overrides.entries().get(self.manage_highlight) {
                    if !entry.active {
                        self.overrides.set_active(self.manage_highlight);
                        self.render_overrides(self.first_node);
                    }
                }
            }
            // Vertical pan (2026-07-19 feedback item 1): scrolls the
            // list without moving the highlight, bounded only by the
            // content itself, no longer by the highlighted row (see
            // `App::manage_pan_vertical`). Must precede the plain
            // `Up`/`Down` arms below, same "modifier-guard first"
            // convention as the horizontal pan below.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.manage_pan_vertical(PAN_STEP, true)
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.manage_pan_vertical(PAN_STEP, false)
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_manage_highlight(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_manage_highlight(-1),
            KeyCode::PageDown => {
                self.move_manage_highlight(self.manage_list_height.max(1) as isize)
            }
            KeyCode::PageUp => {
                self.move_manage_highlight(-(self.manage_list_height.max(1) as isize))
            }
            KeyCode::Home => {
                self.manage_highlight = 0;
                self.manage_pending_kind = None;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.manage_highlight = self.overrides.entries().len().saturating_sub(1);
                self.manage_pending_kind = None;
            }
            // Horizontal pan (item 14 of 2026-07-17 feedback), mirroring
            // the main pane's own Ctrl-Left/Ctrl-Right (spec 0113 D24)
            // and the mouse's Shift-wheel/native horizontal-scroll pan
            // over this pane (`handle_mouse`) — clamped on the right so
            // the rightmost character of the widest visible row is the
            // limit (2026-07-19 feedback item 4, see
            // `App::manage_pan_horizontal`). Must precede the plain
            // `Left`/`Right` arms below, since an unguarded arm there
            // would otherwise shadow it.
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.manage_pan_horizontal(PAN_STEP, true)
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.manage_pan_horizontal(PAN_STEP, false)
            }
            // Spec 0124 G1: circulate the main-pane cursor among the
            // fields the highlighted entry's origin currently matches,
            // without touching focus. No-op on zero matches; if the
            // cursor isn't currently one of the matches, jumps to the
            // first (Right) or last (Left) match.
            KeyCode::Left => self.manage_circulate_cursor(false),
            KeyCode::Right => self.manage_circulate_cursor(true),
            // In-pane search (spec 0117 §3, spec-0133-adjacent rework):
            // reuses the shared bottom command/message bar as the search
            // prompt, mirroring the override pane's own `/`/`?`
            // (`handle_command_key`'s `Enter` arm dispatches to
            // `jump_to_manage_match` while `manage_open && manage_focus`).
            KeyCode::Char('/') => {
                self.command_kind = CommandLineKind::Search(SearchDir::Forward);
                self.command_buffer = Some(String::new());
                self.command_cursor = 0;
            }
            KeyCode::Char('?') => {
                self.command_kind = CommandLineKind::Search(SearchDir::Backward);
                self.command_buffer = Some(String::new());
                self.command_cursor = 0;
            }
            KeyCode::Char('n') => {
                if let Some((dir, pattern)) = self.last_manage_search.clone() {
                    self.jump_to_manage_match(dir, &pattern);
                }
            }
            KeyCode::Char('p') => {
                if let Some((dir, pattern)) = self.last_manage_search.clone() {
                    self.jump_to_manage_match(dir.reverse(), &pattern);
                }
            }
            // Spec 0119 §G4: edit the highlighted entry's display-name
            // override, pre-filled with its current value (empty when
            // unset).
            KeyCode::Char('f') => {
                if !self.overrides.entries().is_empty() {
                    let current = self.overrides.entries()[self.manage_highlight]
                        .name
                        .clone()
                        .unwrap_or_default();
                    self.manage_rename = Some(current);
                }
            }
            // Interactive feedback, 2026-07-17: `A`/Shift-Space is
            // `toggle_active`'s cascading sibling — same toggle, but also
            // applied to every entry whose origin sits at-or-under the
            // highlighted entry's own origin (`toggle_active_cascading`).
            // Terminals report Shift-`a` as the uppercase char directly
            // (no modifier check needed, same convention as `J`/`K`
            // elsewhere), but Space has no uppercase form, so Shift-Space
            // is only distinguishable via its modifier bit — which
            // legacy terminal escape sequences don't carry for printable
            // keys at all (unlike arrows/function keys), so this arm only
            // fires on terminals `push_keyboard_enhancement` (`mod.rs`)
            // successfully negotiated Kitty-protocol enhancement with; on
            // every other terminal, Shift-Space is indistinguishable from
            // plain Space and falls through to the arm below instead —
            // `A` remains the universally reliable keyboard trigger. The
            // guarded `Char(' ')` arm here must precede the plain `a`/
            // Space arm below, since an unguarded `Char(' ')` there would
            // otherwise shadow it.
            KeyCode::Char('A') => {
                if !self.overrides.entries().is_empty() {
                    self.overrides
                        .toggle_active_cascading(self.manage_highlight);
                    self.render_overrides(self.first_node);
                }
            }
            KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !self.overrides.entries().is_empty() {
                    self.overrides
                        .toggle_active_cascading(self.manage_highlight);
                    self.render_overrides(self.first_node);
                }
            }
            // Spec 0118 §6: toggling active status changes the active set
            // (possibly for a sibling too), so it always triggers a
            // recursive render pass.
            KeyCode::Char('a') | KeyCode::Char(' ') => {
                if !self.overrides.entries().is_empty() {
                    self.overrides.toggle_active(self.manage_highlight);
                    self.render_overrides(self.first_node);
                }
            }
            // Spec 0134 G2/G3: forgiving multi-candidate resolution —
            // works out the mutated origin from every node the entry
            // currently affects, only falling back to the main-pane
            // cursor/message line when genuinely ambiguous, and never
            // gets stuck repeating an unresolvable rotation (advances one
            // more step down the 3-kind barrel on a same-key retry with
            // an unchanged cursor). `Z` rotates in reverse.
            KeyCode::Char('z') | KeyCode::Char('Z') => {
                if let Some(entry) = self.overrides.entries().get(self.manage_highlight) {
                    let origin = entry.origin.clone();
                    let r#type = entry.r#type.clone();
                    let was_active = entry.active;
                    let entry_kind = origin.kind();
                    let reverse = key.code == KeyCode::Char('Z');
                    let affected = self.manage_affected_nodes(&origin);

                    let attempt_kind = match &self.manage_pending_kind {
                        Some((pending_origin, kind, last_cursor_moves))
                            if *pending_origin == origin =>
                        {
                            if self.cursor_moves == *last_cursor_moves {
                                if reverse {
                                    kind.prev()
                                } else {
                                    kind.next()
                                }
                            } else {
                                *kind
                            }
                        }
                        _ => {
                            if reverse {
                                entry_kind.prev()
                            } else {
                                entry_kind.next()
                            }
                        }
                    };

                    let candidates = self.manage_kind_candidates(&affected, attempt_kind);

                    if candidates.is_empty() {
                        let other_kind = Self::third_kind(entry_kind, attempt_kind);
                        let other_candidates = self.manage_kind_candidates(&affected, other_kind);
                        if other_candidates.is_empty() {
                            self.message =
                                format!("z: no {} override target", attempt_kind.label());
                            self.manage_pending_kind = None;
                        } else {
                            self.message = format!(
                                "z: no {} override target, try again for {} override",
                                attempt_kind.label(),
                                other_kind.label()
                            );
                            self.manage_pending_kind =
                                Some((origin, attempt_kind, self.cursor_moves));
                        }
                    } else {
                        let cursor_origin = if affected.contains(&self.cursor) {
                            self.origin_for_kind(self.cursor, attempt_kind).ok()
                        } else {
                            None
                        };
                        let resolved = cursor_origin
                            .or_else(|| (candidates.len() == 1).then(|| candidates[0].clone()));

                        match resolved {
                            Some(new_origin) => {
                                self.manage_highlight = self
                                    .overrides
                                    .rotate_origin(self.manage_highlight, new_origin.clone());
                                if was_active {
                                    self.render_overrides(self.first_node);
                                    // `render_overrides` can auto-seed
                                    // brand-new entries elsewhere in the
                                    // tree (Any/MessageSet auto-expansion),
                                    // which re-sorts the whole collection
                                    // and can invalidate the index
                                    // `rotate_origin` just returned above
                                    // — relocate the rotated entry by
                                    // identity instead of trusting the
                                    // pre-render index (feedback,
                                    // 2026-07-16).
                                    if let Some(idx) =
                                        self.overrides.entries().iter().rposition(|e| {
                                            e.origin == new_origin && e.r#type == r#type
                                        })
                                    {
                                        self.manage_highlight = idx;
                                    }
                                }
                                self.manage_pending_kind = None;
                            }
                            None => {
                                self.message = "z: pick an override target (<-/->)".to_string();
                                self.manage_pending_kind =
                                    Some((origin, attempt_kind, self.cursor_moves));
                            }
                        }
                    }
                }
            }
            // Spec 0124 G3: duplicate the highlighted entry as a new,
            // always-inactive copy. Bound to `D` (interactive feedback,
            // 2026-07-17) — `d` now deletes, matching most other
            // list-oriented tools' convention.
            KeyCode::Char('D') => {
                if !self.overrides.entries().is_empty() {
                    self.manage_highlight = self.overrides.duplicate(self.manage_highlight);
                    self.manage_pending_kind = None;
                    self.render_overrides(self.first_node);
                }
            }
            // Spec 0125 §G2: an in-scope `auto` entry is deactivated
            // instead of removed — deleting it would just make
            // `render_overrides`'s next pass re-seed an identical entry.
            // `d` (interactive feedback, 2026-07-17) is an alias for
            // Delete/Backspace, swapped with the former `d`-as-duplicate
            // (now `D`).
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                if let Some(entry) = self.overrides.entries().get(self.manage_highlight).cloned() {
                    if entry.auto && self.auto_entry_in_scope(&entry) {
                        if entry.active {
                            self.overrides.toggle_active(self.manage_highlight);
                            self.render_overrides(self.first_node);
                        }
                        self.message = "auto-derived override deactivated (still in scope \
                            — delete would just recreate it; use 'a' or wait for it to go \
                            out of scope)"
                            .to_string();
                    } else {
                        // Spec 0118 §6: only re-render when the removed
                        // entry was active — removing an inactive entry
                        // cannot change any node's resolved override.
                        let was_active = entry.active;
                        self.overrides.remove(self.manage_highlight);
                        let len = self.overrides.entries().len();
                        if self.manage_highlight >= len {
                            self.manage_highlight = len.saturating_sub(1);
                        }
                        self.manage_pending_kind = None;
                        if was_active {
                            self.render_overrides(self.first_node);
                        }
                    }
                }
            }
            KeyCode::Char('s') => {
                let buf = format!("save {}", self.default_save_overrides_path());
                self.command_kind = CommandLineKind::Command;
                self.command_cursor = buf.chars().count();
                self.command_buffer = Some(buf);
            }
            KeyCode::Char('r') => {
                let buf = "restore ".to_string();
                self.command_kind = CommandLineKind::Command;
                self.command_cursor = buf.chars().count();
                self.command_buffer = Some(buf);
            }
            _ => {}
        }
    }

    /// Mouse handling for the override management pane (spec 0113 D30):
    /// wheel scroll pans the listing by one row (2026-07-19 feedback item
    /// 2 — it no longer moves the highlight), click moves the
    /// highlight to the entry under the cursor (header rows under the
    /// click are ignored, same as clicking whitespace) and, when the
    /// click lands on that entry's own radio marker, also toggles it
    /// active/inactive — the mouse equivalent of `a`/Space, or of `A`/
    /// Shift-Space (cascading) when Shift is held, or when the marker is
    /// double-clicked (interactive feedback, 2026-07-17 — most terminal
    /// emulators intercept Shift-click for native text selection before
    /// it ever reaches the app, so double-click is the reliable mouse
    /// trigger for the cascading toggle; Shift-click is kept too, for
    /// terminals that do pass it through).
    pub(super) fn handle_manage_mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::ScrollDown => self.manage_pan_vertical(WHEEL_PAN_STEP, false),
            MouseEventKind::ScrollUp => self.manage_pan_vertical(WHEEL_PAN_STEP, true),
            MouseEventKind::Down(MouseButton::Left) => self.handle_manage_click(
                event.column,
                event.row,
                event.modifiers.contains(KeyModifiers::SHIFT),
            ),
            _ => {}
        }
    }

    /// Spec 0124 G1's `Left`/`Right` logic, factored out so the
    /// management pane's own click handler (item 10, 2026-07-17
    /// feedback) can trigger the same "next/previous impacted node"
    /// jump that the arrow keys already do.
    pub(super) fn manage_circulate_cursor(&mut self, forward: bool) {
        if let Some(entry) = self.overrides.entries().get(self.manage_highlight) {
            let origin = entry.origin.clone();
            let affected = self.manage_affected_nodes(&origin);
            if !affected.is_empty() {
                let next = match affected.iter().position(|&i| i == self.cursor) {
                    Some(pos) if forward => affected[(pos + 1) % affected.len()],
                    Some(pos) => affected[(pos + affected.len() - 1) % affected.len()],
                    None if forward => affected[0],
                    None => affected[affected.len() - 1],
                };
                self.record_jump(self.cursor);
                self.set_cursor(next);
            }
        }
    }

    pub(super) fn handle_manage_click(&mut self, col: u16, row: u16, shift: bool) {
        let area = self.side_area;
        if !Self::rect_contains(area, col, row) {
            return;
        }
        let rel_row = (row - area.y) as usize;
        if rel_row >= self.manage_list_height {
            return;
        }
        let absolute_row = self.manage_scroll + rel_row;
        let rows = self.manage_display_rows();
        if let Some(&ManageRow::Entry(idx)) = rows.get(absolute_row) {
            let was_current = idx == self.manage_highlight;
            self.manage_highlight = idx;
            self.manage_pending_kind = None;
            // Content-space column the click landed on, undoing the
            // pane's own horizontal pan (`pan_spans` strips the first
            // `manage_pan_offset` chars before display, so screen column
            // 0 is content column `manage_pan_offset`) — spec 0118 §6:
            // clicking the radio marker itself toggles active status,
            // same as `a`/Space on the highlighted entry (or `A`/Shift-
            // Space's cascading toggle, when Shift is held or the marker
            // is double-clicked).
            let content_col = (col - area.x) as usize + self.manage_pan_offset;
            if content_col == MANAGE_MARKER_COL {
                // Double-click detection (2026-07-17 feedback), same
                // technique as the main pane's own `last_click`/
                // `pending_double_click` (generalized as `is_double_click`)
                // — only tracked for marker clicks specifically, since
                // that's the only click this handler ever turns into a
                // state change; a marker click on one entry followed by
                // one on a different entry's marker never counts.
                //
                // There is no timer to defer to in this synchronous event
                // loop, so by the time a second click is recognized as
                // "double", the first click has *already* applied its own
                // plain toggle. Undoing that toggle first, then applying
                // the cascading one, reproduces exactly what a single
                // Shift-click/`A` would have done from the state *before*
                // the first click — not two independent toggles stacked
                // on top of each other.
                if is_double_click(&mut self.last_manage_click, idx) {
                    self.overrides.toggle_active(idx);
                    self.overrides.toggle_active_cascading(idx);
                } else if shift {
                    self.overrides.toggle_active_cascading(idx);
                } else {
                    self.overrides.toggle_active(idx);
                }
                self.render_overrides(self.first_node);
            } else if is_double_click(&mut self.last_manage_row_click, idx) {
                // Item 11 (2026-07-17 feedback): double-clicking an
                // entry outside its marker column opens the selection
                // pane on it, same as `Enter` — tracked via its own
                // `last_manage_row_click`, separate from the marker
                // column's `last_manage_click` above.
                self.open_override_from_manage();
            } else if was_current {
                // Item 10 (2026-07-17 feedback): a single click on the
                // entry that was already highlighted (anywhere outside
                // the marker column) does the same as pressing `Right`
                // — jump the main-pane cursor to the next node this
                // override impacts.
                self.manage_circulate_cursor(true);
            }
        }
    }

    /// Override management pane (spec 0117 §3) — always focused while
    /// open, lists the whole `OverrideCollection` in its canonical sort
    /// order.
    pub(super) fn render_manage_pane(&mut self, frame: &mut Frame, area: Rect) {
        let style = pane_focus_style(self.manage_focus, self.theme);

        // Spec 0147 G1/G2: no border — content splits into a `Min(0)`
        // area above its own `Length(1)` local statusline row.
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        let inner = split[0];
        self.side_area = inner;

        // Neither the rename buffer nor the `/`/`?` search buffer reserves
        // a row here — both render in the global command/message row
        // instead (`render`, spec 0147 G4), which also gives them a real
        // cursor.
        let list_height = inner.height as usize;
        self.manage_list_height = list_height;

        let rows = self.manage_display_rows();
        let total_rows = rows.len();
        let highlighted_row = self.manage_highlighted_row();
        // 2026-07-19 feedback item 3: auto-pan into view only on genuine
        // highlight movement, mirroring the main pane's own
        // `last_cursor_row` gate (`render.rs`).
        if self.last_manage_highlight != Some(highlighted_row) {
            clamp_scroll_to_visible(&mut self.manage_scroll, highlighted_row, list_height);
            self.last_manage_highlight = Some(highlighted_row);
        }
        let end = (self.manage_scroll + list_height).min(total_rows);
        let start = self.manage_scroll.min(total_rows);

        let origin_path = self
            .overrides
            .entries()
            .get(self.manage_highlight)
            .map(|e| e.origin.label())
            .unwrap_or_default();
        let left = format!("{origin_path} - type overrides");
        // Spec 0193 S4.
        let right = format!(
            "L{}/{}  {}",
            highlighted_row + 1,
            total_rows,
            viewport_label(start, list_height, total_rows),
        );
        let text = statusline_text(&left, Some(&right), split[1].width as usize);
        frame.render_widget(Paragraph::new(Line::styled(text, style)), split[1]);

        let mut lines: Vec<Line> = Vec::new();
        for row in &rows[start..end] {
            match row {
                // Spec 0127 §G1: pan the manage pane's own rows
                // independently of the main pane's `pan_offset`.
                // Origin-path header rows render in the same dark-blue
                // style as a `true`/`false` value in the main pane
                // (`SyntaxRole::Boolean`), distinguishing them at a
                // glance from the type rows grouped underneath
                // (restyled 2026-07-18).
                ManageRow::Header(label) => lines.push(Line::from(pan_spans(
                    vec![Span::styled(
                        label.clone(),
                        theme::style_for(SyntaxRole::Boolean, self.theme),
                    )],
                    self.manage_pan_offset,
                ))),
                ManageRow::Entry(idx) => {
                    let text = self.manage_type_line(*idx);
                    // Spec 0130 §G1 (restyled 2026-07-18): auto-derived
                    // entries render in `Comment`'s muted-green color;
                    // manual entries render in the plain terminal
                    // default, so only auto-derived entries stand out.
                    let auto = self.overrides.entries()[*idx].auto;
                    let base_style = theme::manage_entry_style(auto, self.theme);
                    // Feedback (2026-07-16): the highlighted row's
                    // `REVERSED` modifier applies only starting at the
                    // type label's own first character, not the radio
                    // marker or the single space separating it from the
                    // label — reverse video on the marker's own cell would
                    // wash out the filled-vs-hollow shape that's the whole
                    // point of showing active/inactive state there,
                    // defeating it on exactly the row (the cursor's own)
                    // where a user is most likely to be checking it, and
                    // starting the reversed block right at the label reads
                    // cleaner than leaving one un-reversed space in the
                    // middle of it.
                    let split = text
                        .char_indices()
                        .nth(MANAGE_MARKER_COL + 2)
                        .map_or(text.len(), |(byte, _)| byte);
                    let (marker_part, rest_part) = text.split_at(split);
                    let rest_style = if *idx == self.manage_highlight {
                        base_style.add_modifier(Modifier::REVERSED)
                    } else {
                        base_style
                    };
                    lines.push(Line::from(pan_spans(
                        vec![
                            Span::styled(marker_part.to_string(), base_style),
                            Span::styled(rest_part.to_string(), rest_style),
                        ],
                        self.manage_pan_offset,
                    )));
                }
            }
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
}
