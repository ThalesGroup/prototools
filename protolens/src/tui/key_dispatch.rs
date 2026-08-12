// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

/// Whether a key is held with `Control` or `Alt`.
///
/// A character pressed with either held is a *different* key from the
/// same character pressed plainly — `Ctrl-D` is not `d` — but a
/// `KeyCode::Char('d')` match arm carries no modifier condition of its
/// own and so matches both. Every pane therefore gates its plain
/// character arms behind this, in one place per handler, ahead of the
/// match: whatever `Control`/`Alt` bindings that pane really has are
/// spelled out there, and anything else is swallowed rather than
/// falling through to a plain letter's action.
///
/// `Shift` is deliberately absent from the set. It is how the upper
/// half of the keyboard is typed at all, so `H` legitimately arrives as
/// `Char('H')` *with* `SHIFT` set once `DISAMBIGUATE_ESCAPE_CODES` is
/// on (`terminal.rs`'s `push_keyboard_enhancement`), and must keep
/// working. That is also why these gates do not use
/// `modifiers.is_empty()`, which would silently kill every upper-case
/// and shifted-punctuation binding in the app.
pub(super) fn ctrl_or_alt(key: &KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// Whether a key is held with `Shift` *and* `Alt` together (spec 0242
/// S9's horizontal pan).
///
/// `contains(SHIFT)` is true of a `Shift-Alt` chord as well, so an arm
/// guarded by this one must be matched *before* the bare-`Shift`
/// selection arms, or the selection would swallow the pan.
fn shift_alt(key: &KeyEvent) -> bool {
    key.modifiers
        .contains(KeyModifiers::SHIFT | KeyModifiers::ALT)
}

/// Spec 0242 S3: whether a main-pane key is allowed to leave the
/// selection standing — the four `Shift`-motions that extend it, the
/// `Ctrl-c` that copies it, and spec 0268's `w`/`W`, which read it.
/// Every other key clears it.
///
/// The guards mirror the match arms in `handle_main_key` exactly, so
/// that a key and its "does this select?" answer cannot drift apart:
/// `Alt` disqualifies an arrow because `Shift-Alt` is the horizontal pan
/// (S9), and `Control`/`Alt` disqualify a capital because `Ctrl-H` is
/// not `H` (see [`ctrl_or_alt`]).
fn keeps_the_selection(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
            key.modifiers.contains(KeyModifiers::SHIFT)
                && !key.modifiers.contains(KeyModifiers::ALT)
        }
        KeyCode::Char('H' | 'J' | 'K' | 'L') => !ctrl_or_alt(key),
        KeyCode::Char('c') => key.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Char('w' | 'W') => !ctrl_or_alt(key),
        _ => false,
    }
}

/// What a keypress did to the `gg` chord.
pub(super) enum GChord {
    /// Both `g`s seen: jump to the first row.
    Fired,
    /// The first `g`: the key is consumed, and the caller should return
    /// and wait for the next one.
    Armed,
    /// Not a `g`. Any pending arm is now cleared and the key is the
    /// caller's to dispatch.
    Other,
}

impl App {
    /// The vim-style `gg` jump-to-first chord, shared by all three
    /// panes: each arms on the first `g`, fires on the second, and
    /// clears on anything else. Only what it jumps *to* differs, which
    /// is why the caller keeps that and this keeps the state machine.
    ///
    /// `Ctrl-g`/`Alt-g` are not `g` (see `ctrl_or_alt`), so they neither
    /// arm the chord nor fire it — they clear it, like any other key.
    pub(super) fn take_g_chord(&mut self, key: &KeyEvent) -> GChord {
        if key.code != KeyCode::Char('g') || ctrl_or_alt(key) {
            self.pending_g = false;
            return GChord::Other;
        }
        if std::mem::take(&mut self.pending_g) {
            GChord::Fired
        } else {
            self.pending_g = true;
            GChord::Armed
        }
    }

    /// Handle a keypress while the override pane has focus (spec 0114
    /// §2/§3/§4).
    pub(super) fn handle_override_key(&mut self, key: KeyEvent) {
        match self.take_g_chord(&key) {
            GChord::Fired => {
                self.override_highlight = 0;
                self.preview_override_highlight();
                return;
            }
            GChord::Armed => return,
            GChord::Other => {}
        }

        // This pane's entire `Control`/`Alt` character vocabulary, in one
        // place, so that the plain-character arms below — which carry no
        // modifier condition of their own — cannot also answer for it
        // (see `ctrl_or_alt`). Everything else here is swallowed.
        if matches!(key.code, KeyCode::Char(_)) && ctrl_or_alt(&key) {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                // Emacs' own next/previous-line, aliasing `j`/`k` as they
                // do in every pane.
                KeyCode::Char('n') if ctrl => self.move_override_highlight(1),
                KeyCode::Char('p') if ctrl => self.move_override_highlight(-1),
                _ => {}
            }
            return;
        }

        match key.code {
            // Spec 0185 S5: while the selection pane is open, focus is
            // locked to it — that lock is what keeps the preview
            // overlay's anchor immutable for the overlay's whole
            // lifetime. Say so rather than doing nothing, which reads
            // as a broken key.
            KeyCode::Tab => self.message = OVERRIDE_FOCUS_LOCK_MESSAGE.to_string(),
            // Spec 0236 S18: `Esc` is the only way out. `t` closed this
            // pane too until then — but `t` is the key that *opens* it
            // from the main pane, and a pane that locks focus (spec
            // 0185 S5) is better served by one unambiguous exit than by
            // a toggle. Spec 0200 S1's reasoning for leaving `q`
            // unbound here now holds for every letter: an exit that
            // silently discards the highlighted candidate should not be
            // one keystroke away, and `Esc` is the keystroke users
            // already reach for when they mean "never mind".
            KeyCode::Esc => self.close_override(),
            // Spec 0185 S5/G4: the main pane can still be panned while
            // the preview is up. Ctrl-arrows already pan this pane's own
            // candidate list (below), so the main pane gets Alt-arrows.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => self.pan_vertical_up(),
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => self.pan_vertical_down(),
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => self.pan_left(),
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => self.pan_right(),
            // Horizontal pan, mirroring the main pane's own Ctrl-Left/
            // Ctrl-Right (spec 0113 D24) and the mouse's Shift-wheel/
            // native horizontal-scroll pan over this pane
            // (`handle_mouse`) — clamped on the right so the rightmost
            // character of the widest visible row is the limit (see
            // `App::override_pan_horizontal`).
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.override_pan_horizontal(PAN_STEP, true)
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.override_pan_horizontal(PAN_STEP, false)
            }
            // Vertical pan: scrolls the candidate list without moving
            // the highlight, bounded only by the content itself and not
            // by the highlighted row (see `App::override_pan_vertical`).
            // Must precede the plain `Up`/`Down` arms below, same
            // "modifier-guard first" convention as the horizontal pan
            // above.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.override_pan_vertical(PAN_STEP, true)
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.override_pan_vertical(PAN_STEP, false)
            }
            // Spec 0271 S7: a second spelling, added when script
            // navigation still took the Ctrl-arrows and this was the one
            // pane gesture with no alternative left. The 2026-08-12
            // amendment moved the script to the bare arrows and gave the
            // Ctrl pair back, so this is now only a mirror of the
            // horizontal pan above, which has had its `Alt` pair all
            // along. Unreachable as written — the main pane's own
            // `Alt-Up`/`Alt-Down` arms are matched first.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                self.override_pan_vertical(PAN_STEP, true)
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                self.override_pan_vertical(PAN_STEP, false)
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_override_highlight(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_override_highlight(-1),
            // Spec 0236 S16: `f`/`b` page here exactly as they do in the
            // main pane — `less`'s pager idiom, and the spelling that
            // works on a keyboard with no `PageDown`.
            KeyCode::PageDown | KeyCode::Char('f') => {
                self.move_override_highlight(self.override_list_height.max(1) as isize)
            }
            KeyCode::PageUp | KeyCode::Char('b') => {
                self.move_override_highlight(-(self.override_list_height.max(1) as isize))
            }
            KeyCode::Home => {
                self.override_highlight = 0;
                self.preview_override_highlight();
            }
            KeyCode::End | KeyCode::Char('G') => {
                if !self.override_candidates_complete && self.override_sort == SortMode::Inferred {
                    self.upgrade_active_override_to_complete();
                }
                self.override_highlight = self.override_candidates.len().saturating_sub(1);
                self.preview_override_highlight();
            }
            KeyCode::Char('i') => {
                self.override_sort = match self.override_sort {
                    SortMode::Lexicographic => SortMode::Inferred,
                    SortMode::Inferred => SortMode::Lexicographic,
                };
                self.recompute_override_candidates();
            }
            // In-pane search (spec 0114 §4): reuses the shared bottom
            // command/message bar as the search prompt, same mechanism
            // as the main pane's own `/`/`?` (`handle_command_key`'s
            // `Enter` arm dispatches to `jump_to_override_match` while
            // `override_focus` is set).
            KeyCode::Char('/') => {
                self.open_command_line(CommandLineKind::search(SearchDir::Forward), String::new())
            }
            KeyCode::Char('?') => {
                self.open_command_line(CommandLineKind::search(SearchDir::Backward), String::new())
            }
            KeyCode::Char('n') => {
                if let Some((dir, pattern)) = self.last_override_search.clone() {
                    self.jump_to_override_match(dir, &pattern);
                }
            }
            KeyCode::Char('N') => {
                if let Some((dir, pattern)) = self.last_override_search.clone() {
                    self.jump_to_override_match(dir.reverse(), &pattern);
                }
            }
            // Spec 0276 S2: the find prompt — this pane's last pattern
            // pre-filled, `Enter` stepping to the next match and `Esc`
            // accepting the one on screen.
            KeyCode::Char('F') => self.open_find(SearchDir::Forward),
            KeyCode::Char('B') => self.open_find(SearchDir::Backward),
            // Spec 0236 S15: edit the target node's override — type,
            // origin and display name at once — as a pre-filled
            // `:override`. The pane picks a type from a list; `o` is
            // the way out to the other two dimensions it cannot express.
            KeyCode::Char('o') => self.prefill_override_cmd(),
            KeyCode::Enter => {
                let Some(idx) = self.override_target else {
                    return;
                };
                // Spec 0137 §G4: `override_candidates` is indexed
                // directly. In alphabetic mode index `0` is always the
                // `None` sentinel, which resolves to raw
                // (`splice_override`'s sentinel arm).
                let new_fqdn = match self
                    .override_candidates
                    .get(self.override_highlight)
                    .map(|(fqdn, _)| fqdn.clone())
                {
                    Some(fqdn) => Some(fqdn),
                    None => {
                        self.message = "cannot apply override: no candidate selected".to_string();
                        return;
                    }
                };
                // Spec 0117 §2: per-kind origin — errors (wrapper root,
                // unresolved parent FQDN) abort before either the
                // collection or 0114's splice-render is touched.
                //
                // Spec 0200 S3: when the pane was opened on an existing
                // entry, that entry's kind wins over the default (plain
                // `path`, spec 0208 S2). An entry's kind is deliberate —
                // the manage pane's `z`/`Z` set it (spec 0124 G2) — and
                // deriving the default kind instead would not retype the
                // entry at all: `activate` deactivates only entries with
                // the *same* origin, so the old one would stay active
                // and a second would appear beside it.
                //
                // The error path is deliberately not softened into a
                // fallback. If the kind no longer resolves, the parent's
                // type has become unresolved since the entry was
                // created, and silently retyping under a different kind
                // is precisely the defect above.
                let origin = match self.override_origin_kind {
                    Some(kind) => self.origin_for_kind(idx, kind),
                    None => self.override_origin_for_kind(idx),
                };
                let origin = match origin {
                    Ok(origin) => origin,
                    Err(e) => {
                        self.message = format!("cannot create override: {e}");
                        return;
                    }
                };
                // Spec 0118 §6: any kind's activation triggers the
                // recursive render pass — `path`/`path-field`/
                // `fqdn-field` alike.
                self.overrides.activate(origin.clone(), new_fqdn.clone());
                // Spec 0185 S6: the overlay must not be alive while a
                // splice runs — its anchor is a row position the splice
                // is about to invalidate.
                self.preview_overlay = None;
                self.render_overrides(self.first_node);
                // Spec 0200 S2: land in the management pane and
                // highlight the entry just created/reactivated (spec
                // 0119 G3), but only when the management pane is where
                // this pane was opened from. Every exit returns to the
                // caller, so a `t` from the main pane must not end in a
                // pane the user never asked for, covering the document
                // they just changed. `activate` guarantees at most one
                // entry per origin is active, so this origin/type pair
                // unambiguously identifies it.
                //
                // `manage_open`/`manage_focus` are not set here at all:
                // `close_override` sets them from
                // `override_opened_from_manage`, and clears that flag as
                // it goes — hence reading it out first.
                let target_highlight = self
                    .overrides
                    .entries()
                    .iter()
                    .position(|e| e.origin == origin && e.r#type == new_fqdn);
                let returning_to_manage = self.override_opened_from_manage;
                self.close_override();
                if returning_to_manage {
                    self.manage_highlight = target_highlight.unwrap_or(0);
                    self.manage_scroll = PaneScroll::default();
                    self.last_manage_highlight = None;
                    self.manage_pan_offset = 0;
                }
            }
            _ => {}
        }
    }

    /// Spec 0194 S10: push the position being left, not just the node —
    /// `Ctrl-o` returns to a *place*. The line within the node is part
    /// of that, so a jump back from a node's `}` returns to the `}` and
    /// not to its `{`.
    pub(super) fn record_jump(&mut self) {
        self.back_stack.push(self.cursor_pos());
        self.fwd_stack.clear();
    }

    /// The cursor's whole current position (spec 0194 S10).
    pub(super) fn cursor_pos(&self) -> CursorPos {
        CursorPos {
            node: self.cursor,
            line_in_node: self.cursor_line_in_node,
            column: self.cursor_column,
        }
    }

    /// Restore a position pushed by `record_jump`.
    ///
    /// The column is clamped rather than trusted: the row may have
    /// shrunk since it was recorded — a fold toggled under it, an
    /// override respliced the subtree — and a stale column would
    /// otherwise point past the row's end. It is not restored as a
    /// *desired* column: a jump back reinstates a position, and
    /// `desired_column` follows it as after any other horizontal move.
    /// The caret anchor is forfeited for the same reason (spec 0199 S10):
    /// a restored position carries no intent, so it must not arm a fold.
    fn restore_cursor_pos(&mut self, pos: CursorPos) {
        self.set_cursor(pos.node);
        self.unfold_ancestors(pos.node);
        self.cursor_line_in_node = pos.line_in_node;
        self.cursor_column = pos.column;
        self.clamp_caret_column();
        self.desired_column = self.cursor_column;
        self.caret_anchor = CaretAnchor::Free;
    }

    /// Propose a default `:export`/`x` path, in the same directory as the
    /// original blob: `<blob_stem>.<raw_start>-<raw_end>.<short_type>.pb`.
    /// The byte range ties the filename back to the status line's
    /// `bytes[..]` display (and keeps repeated extracts from the same blob
    /// collision-free); the short type name (the FQDN's last `.`-segment)
    /// adds readability. Always `.pb`, regardless of format (0113 D23) —
    /// binary and `#@ prototext` are both "a protobuf-shaped payload"; the
    /// extension shouldn't leak which one was chosen.
    pub(super) fn default_extract_path(&self) -> String {
        let stem = self
            .blob_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "extract".to_string());
        let short_type = self
            .fqdns
            .get(self.tree[self.cursor].span.type_fqdn)
            .and_then(|f| f.rsplit('.').next())
            .unwrap_or("node");
        let range = self.display_range(self.cursor);
        let filename = format!("{stem}.{}-{}.{short_type}.pb", range.start, range.end);
        match self.blob_path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => {
                dir.join(filename).to_string_lossy().into_owned()
            }
            _ => filename,
        }
    }

    /// Pre-fills the command line with `export <flag> <path>` and opens
    /// it (spec 0156 G3) — shared by all four chord resolutions.
    fn prefill_export(&mut self, flag: &str, path: String) {
        self.open_command_line(CommandLineKind::Command, format!("export {flag} {path}"));
    }

    /// Propose a default `xdb`/`xdp`/`:export --descriptor-*` path (spec
    /// 0156 G5): same `<blob_stem>`/`<short_type>` construction as
    /// `default_extract_path`, but `.desc` extension. If the cursor node
    /// (root or not) has an active override entry with a rename (`f` in
    /// the manage pane), that name alone is used as the segment, with no
    /// `short_type` suffix. Otherwise the segment is `no range` at the
    /// document root, or the cursor node's schema field name when
    /// resolvable — falling back to the numeric range only when neither
    /// applies — and the `short_type` suffix is kept.
    pub(super) fn default_export_descriptor_path(&self) -> String {
        let stem = self
            .blob_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "extract".to_string());
        let renamed = self
            .resolve_active_override_entry(self.cursor)
            .and_then(|e| e.name.clone());
        let filename = if let Some(name) = renamed {
            format!("{stem}.{name}.desc")
        } else {
            let short_type = self
                .fqdns
                .get(self.tree[self.cursor].span.type_fqdn)
                .and_then(|f| f.rsplit('.').next())
                .unwrap_or("node");
            let segment = if self.cursor == self.first_node {
                "no range".to_string()
            } else if let Some(field) = self.parent_field(self.cursor) {
                field.name().to_string()
            } else {
                let range = self.display_range(self.cursor);
                format!("{}-{}", range.start, range.end)
            };
            format!("{stem}.{segment}.{short_type}.desc")
        };
        match self.blob_path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => {
                dir.join(filename).to_string_lossy().into_owned()
            }
            _ => filename,
        }
    }

    /// Handle one key event, mutating cursor/fold/scroll/jumplist state.
    /// No `ratatui` rendering happens here — see spec 0111 §4.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Dismiss the splash screen transparently: the key that dismisses
        // it is also processed as a real command, same as if there had
        // been no splash screen at all (spec 0113 D22 amendment).
        self.splash = false;

        // Spec 0147 G5: every keypress, regardless of which pane has
        // focus, dismisses a stale `self.message` before its own handler
        // runs — matching `handle_mouse`'s own unconditional clear at the
        // top of every mouse event. Placed ahead of every dispatch branch
        // below, so a handler that sets a message of its own still wins.
        self.message.clear();
        // Spec 0278 S3: the search echo goes the same way and at the
        // same moment, so the pattern and spec 0277's count are never
        // on the row without each other. A handler that re-echoes —
        // `n`, `N`, a commit, an accepted find — sets it again below.
        self.search_echo = None;

        // `Ctrl-Z` suspends the process (spec 0113 D31, Unix only) —
        // checked centrally here, ahead of every other dispatch, so it
        // applies uniformly regardless of focus/mode. Left unbound on
        // non-Unix platforms (no `SIGTSTP` equivalent).
        #[cfg(unix)]
        if key.code == KeyCode::Char('z') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_suspend = true;
            return;
        }

        // `F1` opens the help overlay regardless of current focus (spec
        // 0126 G1) — checked centrally here, same tier as `Ctrl-Z`
        // above, ahead of every focus-specific dispatch.
        // Closing it again is still handled by `handle_help_key`'s own
        // `F1` arm below (`self.help_open` branch), which fires first
        // once help is open, so this only ever needs to *open* it.
        if !self.help_open && key.code == KeyCode::F(1) {
            self.help_open = true;
            self.help_scroll = 0;
            return;
        }

        if self.help_open {
            self.handle_help_key(key);
            return;
        }
        if self.command_buffer.is_some() {
            self.handle_command_key(key);
            return;
        }
        // `:` opens the command line regardless of which pane has focus
        // — checked centrally here, ahead of every focus-specific
        // dispatch below, same tier as `F1`/`Ctrl-Z` above, so `:quit`
        // (and every other command) is reachable from the override/
        // manage panes too, not just the main pane.
        if key.code == KeyCode::Char(':') && !ctrl_or_alt(&key) {
            self.open_command_line(CommandLineKind::Command, String::new());
            return;
        }
        // `v` jumps to the FQDN under focus's declaration in a handed-off
        // Neovim (spec 0144 G1) — checked centrally here, ahead of every
        // focus-specific dispatch, so it works identically in the override
        // pane, the manage pane, and the main pane. Unix-only (mirrors
        // `Ctrl-Z` above): no terminal job-control equivalent elsewhere.
        #[cfg(unix)]
        if key.code == KeyCode::Char('v') && !ctrl_or_alt(&key) {
            self.open_definition();
            return;
        }
        // Spec 0271 S7: the script keys, in the same tier as `F1`/`:`/`v`
        // above. Below `command_buffer`'s early return, so `space` is
        // still a literal space at the `:` prompt; above the two
        // side-pane dispatches, so they work regardless of focus.
        //
        // `space` is stolen for the whole session whenever a script is
        // loaded, because it is the toggle and so cannot be conditional
        // on the state it toggles. Paging keeps `f`/`PageDown`, and
        // `Shift-Space` is a different chord and is left alone. The bare
        // arrows are taken only while navigation is on — that is what
        // handing navigation to the pane *means*, and every main-pane
        // gesture they displace has a letter spelling (`h`/`l`/`j`/`k`)
        // already. Modified arrows are left alone, so the selection, the
        // pan and the sibling-level fold all still reach the main pane.
        if self.script.is_some() {
            if key.code == KeyCode::Char(' ')
                && !ctrl_or_alt(&key)
                && !key.modifiers.contains(KeyModifiers::SHIFT)
            {
                self.script_toggle();
                return;
            }
            if self.script_active()
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT)
            {
                match key.code {
                    KeyCode::Left => {
                        self.script_advance(false);
                        return;
                    }
                    KeyCode::Right => {
                        self.script_advance(true);
                        return;
                    }
                    KeyCode::Up => {
                        self.script_scroll_by(false);
                        return;
                    }
                    KeyCode::Down => {
                        self.script_scroll_by(true);
                        return;
                    }
                    _ => {}
                }
            }
        }
        if self.override_focus {
            self.handle_override_key(key);
            return;
        }
        if self.manage_open && self.manage_focus {
            self.handle_manage_key(key);
            return;
        }

        // An empty tree (e.g. reopening an extracted `google.protobuf.Empty`,
        // or any all-default submessage — decoding zero bytes legitimately
        // yields zero fields, see spec 0113) has no cursor node to index
        // into, and every key below touches `self.tree`. The keys that
        // still have to work here — `:` above all, which is now the only
        // way to quit (spec 0236 S20) — are dispatched centrally above.
        if self.tree.is_empty() {
            return;
        }

        // Spec 0242 S3: everything that is not one of the four selection
        // keys — nor the `Ctrl-c` that copies what they selected — drops
        // the selection, so a plain motion does not drag it along behind
        // the caret. One check here rather than a call in each of the
        // dozens of arms below, which is also what keeps the rule true
        // of arms added later.
        if !keeps_the_selection(&key) {
            self.clear_selection();
        }

        match self.take_g_chord(&key) {
            GChord::Fired => {
                self.move_home();
                return;
            }
            GChord::Armed => return,
            GChord::Other => {}
        }

        // This pane's entire `Control`/`Alt` character vocabulary, in one
        // place, so that the plain-character arms further below — which
        // carry no modifier condition of their own — cannot also answer
        // for it (see `ctrl_or_alt`). Everything else here is swallowed.
        //
        // Placed ahead of the `x` chord below so that `Ctrl-x` cannot arm
        // an export either; that also makes these the only keys that can
        // run while the chord is armed, hence the explicit disarm — the
        // chord's own arms below reset it the same way before acting.
        if matches!(key.code, KeyCode::Char(_)) && ctrl_or_alt(&key) {
            self.pending_x = ExportChord::None;
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            match key.code {
                // Emacs' next/previous-line, aliasing `j`/`k`.
                KeyCode::Char('n') if ctrl => self.move_down(),
                KeyCode::Char('p') if ctrl => self.move_up(),

                // The readline layer, spelled here exactly as the `:`
                // command line spells it (`command_line.rs`): `Ctrl-b`/
                // `Ctrl-f` a character, `Alt-b`/`Alt-f` a word,
                // `Ctrl-a`/`Ctrl-e` to the ends (spec 0208 S1) — as on
                // every other text surface the user touches, the shell
                // included. `Alt-h`/`Alt-l` remain a second word-motion
                // spelling, aliasing the Alt-arrows below.
                //
                // These are *not* vim's `Ctrl-F`/`Ctrl-B` page keys:
                // paging is `b`/`f` unmodified, in the plain arms below.
                KeyCode::Char('b') if ctrl => self.caret_left(),
                KeyCode::Char('f') if ctrl => self.caret_right(),
                KeyCode::Char('b') if alt => self.caret_word_left(),
                KeyCode::Char('f') if alt => self.caret_word_right(),
                KeyCode::Char('a') if ctrl => self.caret_to_line_start(),
                KeyCode::Char('e') if ctrl => self.caret_to_line_end(),
                KeyCode::Char('h') if alt => self.caret_word_left(),
                KeyCode::Char('l') if alt => self.caret_word_right(),

                // Spec 0242 S8: the four tree gestures that `H`/`J`/`K`/
                // `L` used to spell, moved onto `Control` now that the
                // plain shifted letters select. These are the letter
                // spellings of the Ctrl-arrows in the plain-key match
                // further below, and each keeps the direction it had:
                // `h`/`l` fold and unfold the sibling level, `j`/`k`
                // skip to the next and previous sibling.
                KeyCode::Char('h') if ctrl => self.fold_all_siblings(),
                KeyCode::Char('l') if ctrl => self.unfold_all_siblings(),
                KeyCode::Char('j') if ctrl => self.next_sibling_move(),
                KeyCode::Char('k') if ctrl => self.prev_sibling_move(),

                // Navigation history.
                KeyCode::Char('o') if ctrl => {
                    if let Some(pos) = self.back_stack.pop() {
                        self.fwd_stack.push(self.cursor_pos());
                        self.restore_cursor_pos(pos);
                    } else {
                        self.message = "jumplist: at oldest position".to_string();
                    }
                }
                KeyCode::Char('i') if ctrl => {
                    if let Some(pos) = self.fwd_stack.pop() {
                        self.back_stack.push(self.cursor_pos());
                        self.restore_cursor_pos(pos);
                    } else {
                        self.message = "jumplist: at newest position".to_string();
                    }
                }

                // Spec 0131 §G1: `Ctrl-C` is the single, explicit copy
                // key — copies the active drag-selection if one exists,
                // else the cursor's own current line. Mouse release does
                // not copy by itself (see the no-op
                // `Up(MouseButton::Left)` arm in `handle_mouse`).
                KeyCode::Char('c') if ctrl => self.copy_current_selection_or_line(),
                _ => {}
            }
            return;
        }

        // `x<b|p|d<b|p>>` chord (export-format leader key, spec 0156 G3):
        // a first `x` press arms `ExportChord::Leader` silently; the next
        // keypress either fires a data export (`b`/`p`), arms
        // `ExportChord::Descriptor` (`d`, no fire yet), or cancels the
        // chord (falls through unswallowed) — same pattern as the `gg`
        // chord above, adapted for a two/three-key selection rather than
        // a repeated key.
        match self.pending_x {
            ExportChord::Leader => {
                self.pending_x = ExportChord::None;
                match key.code {
                    KeyCode::Char('b') => {
                        let path = self.default_extract_path();
                        self.prefill_export("--binary", path);
                        return;
                    }
                    KeyCode::Char('p') => {
                        let path = self.default_extract_path();
                        self.prefill_export("--prototext", path);
                        return;
                    }
                    KeyCode::Char('d') => {
                        self.pending_x = ExportChord::Descriptor;
                        return;
                    }
                    _ => {} // falls through, processed normally below
                }
            }
            ExportChord::Descriptor => {
                self.pending_x = ExportChord::None;
                match key.code {
                    KeyCode::Char('b') => {
                        let path = self.default_export_descriptor_path();
                        self.prefill_export("--descriptor-binary", path);
                        return;
                    }
                    KeyCode::Char('p') => {
                        let path = self.default_export_descriptor_path();
                        self.prefill_export("--descriptor-prototext", path);
                        return;
                    }
                    _ => {} // falls through, processed normally below
                }
            }
            ExportChord::None => {
                if key.code == KeyCode::Char('x') {
                    self.pending_x = ExportChord::Leader;
                    return;
                }
            }
        }

        match key.code {
            // Horizontal pan (spec 0242 S9). `Shift-Alt` must be matched
            // before the bare `Shift` selection arms below, since
            // `contains(SHIFT)` is true for both — the same
            // "modifier-guard first" convention the rest of this match
            // runs on, with the most-modified arm first.
            KeyCode::Up if shift_alt(&key) => self.pan_left(),
            KeyCode::Down if shift_alt(&key) => self.pan_right(),

            // Vertical pan: scrolls the viewport without moving the
            // cursor (see `App::pan_vertical`). On `Alt` since spec 0242
            // S9 gave the Ctrl-arrows to the sibling moves.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => self.pan_vertical_up(),
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => self.pan_vertical_down(),

            // Spec 0242 S4: the four selection keys. `Shift-h` *is* `H`,
            // so each pair is one gesture with two spellings.
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => self.select_down(),
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => self.select_up(),
            KeyCode::Char('J') => self.select_down(),
            KeyCode::Char('K') => self.select_up(),

            // Sibling-skip move (spec 0126 G2), on `Ctrl` since spec
            // 0242 S8 gave `Shift`/`J`/`K` to the selection. The
            // `Ctrl-j`/`Ctrl-k` letter spellings are in the Ctrl/Alt
            // gate above.
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.next_sibling_move()
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prev_sibling_move()
            }

            // Document-order move.
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),

            // Jump to first/last visible node.
            KeyCode::Home => self.move_home(),
            KeyCode::End | KeyCode::Char('G') => self.move_end(),

            // Page move, in three spellings. `PageDown`/`PageUp` are the
            // literal ones; `Space`/`Shift-Space` are the pager idiom
            // (`less`, `man`), which is what a reader is most likely to
            // try first; `f`/`b` are the *other* pager idiom — `less`
            // binds them too — and are the pair to reach for when the
            // others are unavailable, since a compact keyboard often has
            // no `PageDown` key at all and `Shift-Space` needs the
            // terminal to report the modifier.
            //
            // The Shift arm must precede the plain one: without
            // `DISAMBIGUATE_ESCAPE_CODES` (`terminal.rs`'s
            // `push_keyboard_enhancement`) a terminal reports
            // `Shift-Space` as a bare `Space`, so there it silently pages
            // *down*. That is why `b` matters — an unmodified letter
            // needs no modifier reporting at all, so it is the one
            // page-up spelling that always works.
            //
            // `b` is safe next to the `x` export chord above (`xb`,
            // `xdb`): that match runs first and returns, so a bare `b`
            // only reaches here with no chord armed.
            KeyCode::PageDown | KeyCode::Char('f') => self.move_page_down(),
            KeyCode::PageUp | KeyCode::Char('b') => self.move_page_up(),
            KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.move_page_up()
            }
            KeyCode::Char(' ') => self.move_page_down(),

            // Sibling-level fold (spec 0199 S9), on `Ctrl` since spec
            // 0242 S8 gave `Shift`/`H`/`L` to the selection. Checked
            // before the Shift-guarded and plain Left/Right arms below,
            // since `Ctrl` and `Shift` are independent modifier checks.
            // These reuse the very functions `zC`/`zO` call.
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.fold_all_siblings()
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.unfold_all_siblings()
            }

            // Word motion (spec 0199 S8), over the same word definition
            // the `:` prompt uses. Checked before the plain arms below.
            // Their `Alt-h`/`Alt-l` aliases are in the Ctrl/Alt gate
            // above.
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => self.caret_word_left(),
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => self.caret_word_right(),

            // Spec 0242 S4: the horizontal half of the selection. `Shift`
            // no longer widens a fold — spec 0199 S9's organizing rule
            // now reads **motion happens in the text until the text runs
            // out and then continues into the tree; `Shift` selects;
            // `Ctrl` widens a fold to the sibling level; `z` folds.**
            KeyCode::Char('H') => self.select_left(),
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => self.select_left(),
            KeyCode::Char('L') => self.select_right(),
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => self.select_right(),

            // Caret motion (spec 0194 S6, amended by spec 0199 S5/S6). No
            // wrapping at either end (N5), matching vim's default
            // `whichwrap`; but at a *voluntary* end the motion continues
            // into the tree. `Backspace` joins them because vim's `<BS>`
            // is a plain left motion in normal mode.
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.caret_left(),
            KeyCode::Char('l') | KeyCode::Right => self.caret_right(),
            // `0` and `^` are one destination here: column zero is the
            // fold gutter and unreachable (S3), so vim's two motions
            // coincide. Their `Ctrl-a`/`Ctrl-e` aliases (spec 0208 S1)
            // are in the Ctrl/Alt gate above.
            KeyCode::Char('0') | KeyCode::Char('^') => self.caret_to_line_start(),
            KeyCode::Char('$') => self.caret_to_line_end(),
            KeyCode::Char('%') => self.jump_matching_brace(),

            // Fold/unfold. `z` toggles the cursor node; `Z` toggles it
            // and forces every descendant into whichever state it just
            // took, so a subtree opens or closes all the way down in one
            // keystroke.
            //
            // `z` was vim's fold *prefix* here (`za`/`zc`/`zo` and their
            // sibling-wide capitals). Folding is the single most-used
            // gesture in this pane, so it is spelled with one key rather
            // than two; the sibling level keeps `H`/`L` below, which is
            // where it was already reachable without a chord. A bare `z`
            // is safe because `Ctrl-Z` (suspend) is handled and returned
            // well above this match.
            KeyCode::Char('z') => self.toggle_cursor_fold(),
            KeyCode::Char('Z') => self.toggle_cursor_fold_recursive(),

            // Toggle main-pane annotation display (spec 0133 G3) — a
            // pure display attribute, distinct from the override pane's
            // own `a` (candidate sort toggle) and the manage pane's own
            // `a` (entry active toggle), both gated behind their own
            // focus checks and unreachable here.
            KeyCode::Char('a') => self.annotations = !self.annotations,

            // Show the bytes under the lines the reader is looking at
            // (spec 0268 S2), or under the whole subtree they are in
            // (S3) — like `a`, a pure display attribute that invalidates
            // no cache and bumps no `structural_version`. It does change
            // how many lines the pane holds, so the scroll and pan
            // offsets are clamped against the geometry it just changed.
            KeyCode::Char('w') => self.wire_lines(),
            KeyCode::Char('W') => self.wire_subtree(),

            // Toggle the main-pane inference-mismatch heat cue (spec
            // 0138) — hides/shows the cue without discarding the
            // heat-cue caches, distinct from the override pane's own `i`
            // (candidate sort toggle), gated behind its own focus check
            // and unreachable here. `Ctrl-i` (jumplist "forward") is in
            // the Ctrl/Alt gate above.
            KeyCode::Char('i') => self.heat_cues_hidden = !self.heat_cues_hidden,

            // In-pane search (spec 0114 §4, extended to the main pane):
            // reuses the command-line row as the search prompt. Only
            // reachable with main-pane focus — `override_focus` is checked
            // earlier in `handle_key`, and the override pane has its own
            // `/`/`?`/`n` in `handle_override_key`.
            KeyCode::Char('/') => {
                self.open_command_line(CommandLineKind::search(SearchDir::Forward), String::new())
            }
            KeyCode::Char('?') => {
                self.open_command_line(CommandLineKind::search(SearchDir::Backward), String::new())
            }
            KeyCode::Char('n') => {
                if let Some((dir, pattern)) = self.last_search.clone() {
                    self.jump_to_match(dir, &pattern);
                }
            }
            KeyCode::Char('N') => {
                if let Some((dir, pattern)) = self.last_search.clone() {
                    self.jump_to_match(dir.reverse(), &pattern);
                }
            }
            // Spec 0276 S2: the find prompt — the last pattern
            // pre-filled, `Enter` stepping to the next match and `Esc`
            // accepting the one on screen, caret on its last character.
            KeyCode::Char('F') => self.open_find(SearchDir::Forward),
            KeyCode::Char('B') => self.open_find(SearchDir::Backward),

            // Override pane (spec 0114 §1/§2): `t` opens it; `Esc`
            // closes it (focus is the main pane here, since
            // `override_focus` is checked earlier in `handle_key`).
            // Spec 0236 S18 dropped `t`'s own close arm inside the
            // pane, so from here `t` is only ever an open.
            KeyCode::Char('t') => self.toggle_override(),

            // `Enter` on a main-pane node (spec 0139): a smart proxy for
            // `t`/`o`, mirroring double-click's own behavior below in
            // `handle_mouse`.
            KeyCode::Enter => self.open_smart_override_or_manage(),
            KeyCode::Esc if self.override_target.is_some() => self.close_override(),
            // `Esc` also closes the override management pane when it's
            // open but the main pane has focus (consistent with the
            // override-select pane's own arm just above).
            KeyCode::Esc if self.manage_open => self.close_manage_pane(),
            // Spec 0129 §G3: `Esc` clears an active main-pane line
            // selection, alongside whatever else it already clears above.
            KeyCode::Esc => {
                self.clear_selection();
                // Spec 0235 S15/N5: and the search highlight — vim's
                // `:nohlsearch` without the command. A highlight the
                // user cannot dismiss leaves a third of a document
                // tinted after a search for `id`.
                self.clear_search_highlight();
            }

            // Spec 0185 S5: there is deliberately no `Tab`-into-the-
            // override-pane arm — the pane's focus lock means the main
            // pane never holds focus while it is open, so such an arm
            // would be unreachable.

            // Override management pane (spec 0117 §3): `o` opens it,
            // mirroring `t`. `Tab` moves focus back into it while it's
            // open (Q2: the management pane deliberately does *not* get
            // the selection pane's focus lock — it performs actual
            // splices, so its main-pane content is committed content and
            // nothing there depends on an immutable anchor).
            //
            // Spec 0236 S19: `o` keeps this meaning in the main pane and
            // means "edit this override" only *inside* the two override
            // panes, where there is a single obvious entry to edit. In
            // the main pane there is not — hence `:override` typed in
            // full, which is what `o` opens a pane to avoid.
            KeyCode::Char('o') => self.toggle_manage_pane(),
            KeyCode::Tab if self.manage_open => self.manage_focus = true,

            _ => {}
        }
    }

    /// Resolve the type FQDN currently under focus (G2) — the override
    /// candidate pane (either sort mode), the manage pane, or the main
    /// pane, whichever currently holds focus. `None` when the focused row
    /// has nothing to jump to: an empty candidate list/tree, the `None`
    /// sentinel or a primitive keyword row (spec 0137), or the internal,
    /// non-real `decode::MESSAGE_SET_ITEM_FQDN` placeholder (spec
    /// 0120/0135) — never registered as a real message in the pool, so
    /// it has no declaration of its own to jump to.
    ///
    /// The main-pane branch mirrors `status_type_label`'s own fallback
    /// chain: `span.type_fqdn` is `None` for every scalar node,
    /// including enum-typed ones, so an enum field under the cursor
    /// falls back to its currently effective type (an active override if
    /// one applies, else `natural_type`) — the same FQDN the status line
    /// already shows as "type: ...". A primitive-keyword result has no
    /// declaration to jump to either.
    #[cfg(unix)]
    fn fqdn_under_focus(&self) -> Option<String> {
        if self.override_focus {
            let (fqdn, _) = self.override_candidates.get(self.override_highlight)?;
            if fqdn == "protolens_internal.None"
                || decode::ALL_PRIMITIVE_KEYWORDS.contains(&fqdn.as_str())
            {
                return None;
            }
            return Some(fqdn.clone());
        }
        let fqdn = if self.manage_open && self.manage_focus {
            self.overrides
                .entries()
                .get(self.manage_highlight)?
                .r#type
                .clone()?
        } else {
            let idx = self.cursor;
            self.tree.get(idx)?;
            match self.fqdns.get(self.tree[idx].span.type_fqdn) {
                Some(fqdn) => fqdn.to_owned(),
                None => {
                    let effective = match self.resolve_active_override(idx) {
                        Some(inner) => inner?,
                        None => self.natural_type(idx)?,
                    };
                    decode::primitive_type_for_keyword(&effective)
                        .is_none()
                        .then_some(effective)?
                }
            }
        };
        (fqdn != decode::MESSAGE_SET_ITEM_FQDN).then_some(fqdn)
    }

    /// `v` (G1): resolve the FQDN under focus (G2), look up its
    /// declaration (G3), resolve it against `proto_root` (G4), and — if
    /// everything checks out — arm `pending_editor_open` so `run_loop` can
    /// hand off to Neovim (G5) once it regains control of the `Terminal`.
    /// Any failure along the way is reported via `self.message` (the
    /// existing auto-dismissing bottom-bar notice) and stops here.
    #[cfg(unix)]
    fn open_definition(&mut self) {
        let Some(fqdn) = self.fqdn_under_focus() else {
            self.message = "no declaration to jump to here".to_string();
            return;
        };
        // JIT-load the name's file before locating it (spec 0197 §S5): on
        // the lazy branch nothing outside the root closure is in the pool
        // yet, and `locate_declaration` reads the pool only.
        self.ctx.message(&fqdn);
        let Some((rel_path, line, col)) = neovim::locate_declaration(self.ctx.pool(), &fqdn) else {
            self.message = format!("unknown type: {fqdn}");
            return;
        };
        let Some(proto_root) = &self.proto_root else {
            self.message =
                "no proto root configured; set one with :proto-root <dir> or -I/--proto-root"
                    .to_string();
            return;
        };
        let abs_path = proto_root.join(&rel_path);
        if !abs_path.is_file() {
            self.message = format!(
                "proto source not found: {} (under proto-root {})",
                rel_path.display(),
                proto_root.display()
            );
            return;
        }
        self.pending_editor_open = Some(neovim::EditorRequest {
            path: abs_path,
            line,
            col,
        });
    }

    /// Scroll/close the `F1` help overlay.
    pub(super) fn handle_help_key(&mut self, key: KeyEvent) {
        // This overlay's entire `Control`/`Alt` character vocabulary, in
        // one place, so that the plain-character arms below — which carry
        // no modifier condition of their own — cannot also answer for it
        // (see `ctrl_or_alt`). Everything else here is swallowed.
        if matches!(key.code, KeyCode::Char(_)) && ctrl_or_alt(&key) {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                // Emacs' own next/previous-line, aliasing `j`/`k` as they
                // do in every pane.
                KeyCode::Char('n') if ctrl => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::Char('p') if ctrl => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => {}
            }
            return;
        }
        match key.code {
            // Spec 0236 S20: `q` is unbound app-wide, this overlay
            // included — after it stops quitting and stops closing
            // panes, `q` still closing exactly one overlay reads as a
            // leftover rather than a convention.
            KeyCode::Esc | KeyCode::F(1) => self.help_open = false,
            KeyCode::Char('j') | KeyCode::Down => {
                self.help_scroll = self.help_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_scroll = self.help_scroll.saturating_sub(1)
            }
            // Spec 0236 S16: `f`/`b` page here too, as in every other
            // pane.
            KeyCode::PageDown | KeyCode::Char('f') => {
                self.help_scroll = self.help_scroll.saturating_add(10)
            }
            KeyCode::PageUp | KeyCode::Char('b') => {
                self.help_scroll = self.help_scroll.saturating_sub(10)
            }
            _ => {}
        }
    }
}
