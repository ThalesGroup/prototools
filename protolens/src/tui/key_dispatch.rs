// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

impl App {
    /// Handle a keypress while the override pane has focus (spec 0114
    /// §2/§3/§4).
    pub(super) fn handle_override_key(&mut self, key: KeyEvent) {
        // `gg` chord (vim-style jump-to-first), mirroring the main
        // pane's own `handle_key` chord: a first `g` press arms
        // `pending_g`; a second `g` press immediately after jumps to
        // the first candidate. Any other key clears the pending state.
        if key.code == KeyCode::Char('g') {
            if self.pending_g {
                self.pending_g = false;
                self.override_highlight = 0;
                self.preview_override_highlight();
            } else {
                self.pending_g = true;
            }
            return;
        }
        self.pending_g = false;

        match key.code {
            // Spec 0185 S5: while the selection pane is open, focus is
            // locked to it — that lock is what keeps the preview
            // overlay's anchor immutable for the overlay's whole
            // lifetime. Say so rather than doing nothing, which reads
            // as a broken key.
            KeyCode::Tab => self.message = OVERRIDE_FOCUS_LOCK_MESSAGE.to_string(),
            // Spec 0200 S1: `q` is deliberately *not* bound here. In the
            // main pane it is `request_quit`, with a confirmation behind
            // it; this pane locks focus (spec 0185 S5), so binding `q`
            // to an exit would silently discard the highlighted
            // candidate with no prompt. `Esc` and `t` are the ways out.
            // It falls to `_ => {}` below with no message: `Tab` gets
            // one because a user has a specific expectation of it that
            // the focus lock defeats, whereas `q` has no meaning here to
            // explain.
            KeyCode::Esc | KeyCode::Char('t') => self.close_override(),
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
            KeyCode::Char('j') | KeyCode::Down => self.move_override_highlight(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_override_highlight(-1),
            KeyCode::PageDown => {
                self.move_override_highlight(self.override_list_height.max(1) as isize)
            }
            KeyCode::PageUp => {
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
                if let Some((dir, pattern)) = self.last_override_search.clone() {
                    self.jump_to_override_match(dir, &pattern);
                }
            }
            KeyCode::Char('N') => {
                if let Some((dir, pattern)) = self.last_override_search.clone() {
                    self.jump_to_override_match(dir.reverse(), &pattern);
                }
            }
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
                    self.manage_scroll = 0;
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
        let buf = format!("export {flag} {path}");
        self.command_kind = CommandLineKind::Command;
        self.command_cursor = buf.chars().count();
        self.command_buffer = Some(buf);
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

    /// First `q` press: arm `quit_confirm` and prompt; meaningless once
    /// already armed (the top-of-`handle_key` check in that case handles
    /// the second press directly, before dispatch ever reaches here).
    pub(super) fn request_quit(&mut self) {
        self.quit_confirm = true;
        self.message = "quit? press q again to confirm, any other key cancels".to_string();
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
        // below, including `quit_confirm`'s own prompt: on the keypress
        // that calls `request_quit()`, this clear fires before
        // `request_quit()` sets the prompt, so it doesn't erase its own
        // prompt; on the *following* keypress, this clear fires first and
        // wipes the prompt unconditionally, whether that keypress
        // confirms, cancels, or does neither.
        self.message.clear();

        // `Ctrl-Z` suspends the process (spec 0113 D31, Unix only) —
        // checked centrally here, ahead of every other dispatch, so it
        // applies uniformly regardless of focus/mode, same as
        // `quit_confirm` below. Left unbound on non-Unix platforms
        // (no `SIGTSTP` equivalent). Doesn't touch `quit_confirm`, so a
        // pending quit confirmation survives a suspend/resume cycle.
        #[cfg(unix)]
        if key.code == KeyCode::Char('z') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_suspend = true;
            return;
        }

        // A prior `q` press is awaiting confirmation (see `request_quit`):
        // resolve it here, ahead of every other dispatch, so it applies
        // uniformly regardless of which mode/pane has focus. A second `q`
        // quits; any other key cancels without otherwise acting on it.
        if self.quit_confirm {
            self.quit_confirm = false;
            if key.code == KeyCode::Char('q') {
                self.should_quit = true;
            }
            return;
        }

        // `F1` opens the help overlay regardless of current focus (spec
        // 0126 G1) — checked centrally here, same tier as `Ctrl-Z`/
        // `quit_confirm` above, ahead of every focus-specific dispatch.
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
        if key.code == KeyCode::Char(':') {
            self.command_kind = CommandLineKind::Command;
            self.command_buffer = Some(String::new());
            self.command_cursor = 0;
            return;
        }
        // `v` jumps to the FQDN under focus's declaration in a handed-off
        // Neovim (spec 0144 G1) — checked centrally here, ahead of every
        // focus-specific dispatch, so it works identically in the override
        // pane, the manage pane, and the main pane. Unix-only (mirrors
        // `Ctrl-Z` above): no terminal job-control equivalent elsewhere.
        #[cfg(unix)]
        if key.code == KeyCode::Char('v') && key.modifiers.is_empty() {
            self.open_definition();
            return;
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
        // into: only allow the keys that don't touch `self.tree`.
        if self.tree.is_empty() {
            if key.code == KeyCode::Char('q') {
                self.request_quit();
            }
            return;
        }

        // `gg` chord (vim-style jump-to-first): a first `g` press arms
        // `pending_g`; a second `g` press immediately after fires
        // `move_home()`. Any other key clears the pending state.
        if key.code == KeyCode::Char('g') {
            if self.pending_g {
                self.pending_g = false;
                self.move_home();
            } else {
                self.pending_g = true;
            }
            return;
        }
        self.pending_g = false;

        // Spec 0194 S6: `z` is vim's fold prefix. `za`/`zc`/`zo` act on
        // the cursor node, their capitals on the whole sibling level.
        // Same shape as the `gg` chord above; any other key cancels.
        if self.pending_z {
            self.pending_z = false;
            match key.code {
                KeyCode::Char('a') => self.toggle_cursor_fold(),
                KeyCode::Char('c') => self.fold_cursor(),
                KeyCode::Char('o') => self.unfold_cursor(),
                KeyCode::Char('A') => self.toggle_all_siblings(),
                KeyCode::Char('C') => self.fold_all_siblings(),
                KeyCode::Char('O') => self.unfold_all_siblings(),
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Char('z') {
            self.pending_z = true;
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
            KeyCode::Char('q') => self.request_quit(),

            // Sibling-skip move (spec 0126 G2: Shift-Down/Shift-Up alias
            // `J`/`K` — checked before the plain Down/Up arms below, same
            // "modifier-guard first" convention as Ctrl/Shift-Left/Right
            // above).
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.next_sibling_move()
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => self.prev_sibling_move(),
            KeyCode::Char('J') => self.next_sibling_move(),
            KeyCode::Char('K') => self.prev_sibling_move(),

            // Vertical pan: scrolls the viewport without moving the
            // cursor (see `App::pan_vertical`). Checked before the
            // plain Up/Down arms below, same "modifier-guard first"
            // convention as the horizontal pan below.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => self.pan_vertical_up(),
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.pan_vertical_down()
            }

            // Document-order move.
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),

            // Jump to first/last visible node.
            KeyCode::Home => self.move_home(),
            KeyCode::End | KeyCode::Char('G') => self.move_end(),

            // Page move.
            KeyCode::PageDown => self.move_page_down(),
            KeyCode::PageUp => self.move_page_up(),

            // Horizontal pan (spec 0113 D24). Checked before the
            // Shift-guarded and plain Left/Right arms below, since `Ctrl`
            // and `Shift` are independent modifier checks.
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => self.pan_left(),
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => self.pan_right(),

            // Word motion (spec 0199 S8), over the same word definition
            // the `:` prompt uses. Checked before the plain arms below.
            // `Alt-h`/`Alt-l` alias the arrows, as everywhere else in
            // this table.
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.caret_word_left()
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.caret_word_right()
            }

            // Spec 0199 S9's amendment of spec 0194 S6's organizing rule:
            // **motion happens in the text until the text runs out and
            // then continues into the tree; `Shift` widens a fold to the
            // sibling level; `z` folds.** `Shift-h` *is* `H`, so the two
            // spellings of one gesture must agree — and neither is `h`.
            // These reuse the very functions `zC`/`zO` call.
            KeyCode::Char('H') => self.fold_all_siblings(),
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.fold_all_siblings()
            }
            KeyCode::Char('L') => self.unfold_all_siblings(),
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.unfold_all_siblings()
            }

            // Caret motion (spec 0194 S6, amended by spec 0199 S5/S6). No
            // wrapping at either end (N5), matching vim's default
            // `whichwrap`; but at a *voluntary* end the motion continues
            // into the tree. `Backspace` joins them because vim's `<BS>`
            // is a plain left motion in normal mode.
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.caret_left(),
            KeyCode::Char('l') | KeyCode::Right => self.caret_right(),
            // `0` and `^` are one destination here: column zero is the
            // fold gutter and unreachable (S3), so vim's two motions
            // coincide.
            //
            // Spec 0208 S1: `Ctrl-a`/`Ctrl-e` alias them, as every other
            // text surface the user touches (the shell, the `:` command
            // line) binds those two to the same destinations. Separate
            // guarded arms rather than `|`-alternatives, because the
            // unmodified spellings are accepted under any modifier state
            // and there is no reason to tighten that. `Ctrl-a` is free
            // because the annotation toggle below is guarded to a bare
            // `a` (spec 0199 S9); `Ctrl-e` is not bound at all.
            KeyCode::Char('0') | KeyCode::Char('^') => self.caret_to_line_start(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.caret_to_line_start()
            }
            KeyCode::Char('$') => self.caret_to_line_end(),
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.caret_to_line_end()
            }
            KeyCode::Char('%') => self.jump_matching_brace(),

            // Fold/unfold toggle. `Space` exists alongside the `z` fold
            // prefix so the common case still costs one key.
            KeyCode::Char(' ') => self.toggle_cursor_fold(),

            // Toggle main-pane annotation display (spec 0133 G3) — a
            // pure display attribute, distinct from the override pane's
            // own `a` (candidate sort toggle) and the manage pane's own
            // `a` (entry active toggle), both gated behind their own
            // focus checks and unreachable here. Guarded to plain `a`
            // (spec 0199 S9), leaving `Ctrl-a` to the caret.
            KeyCode::Char('a') if key.modifiers.is_empty() => self.annotations = !self.annotations,

            // Toggle the main-pane inference-mismatch heat cue (spec
            // 0138) — hides/shows the cue without discarding the
            // heat-cue caches, distinct from the override pane's own `i`
            // (candidate sort toggle), gated behind its own focus check
            // and unreachable here. Guarded to plain `i` so `Ctrl-i`
            // (jumplist "forward", below) is unaffected.
            KeyCode::Char('i') if key.modifiers.is_empty() => {
                self.heat_cues_hidden = !self.heat_cues_hidden
            }

            // Navigation history.
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(pos) = self.back_stack.pop() {
                    self.fwd_stack.push(self.cursor_pos());
                    self.restore_cursor_pos(pos);
                } else {
                    self.message = "jumplist: at oldest position".to_string();
                }
            }
            KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(pos) = self.fwd_stack.pop() {
                    self.back_stack.push(self.cursor_pos());
                    self.restore_cursor_pos(pos);
                } else {
                    self.message = "jumplist: at newest position".to_string();
                }
            }

            // In-pane search (spec 0114 §4, extended to the main pane):
            // reuses the command-line row as the search prompt. Only
            // reachable with main-pane focus — `override_focus` is checked
            // earlier in `handle_key`, and the override pane has its own
            // `/`/`?`/`n` in `handle_override_key`.
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
                if let Some((dir, pattern)) = self.last_search.clone() {
                    self.jump_to_match(dir, &pattern);
                }
            }
            KeyCode::Char('N') => {
                if let Some((dir, pattern)) = self.last_search.clone() {
                    self.jump_to_match(dir.reverse(), &pattern);
                }
            }

            // Override pane (spec 0114 §1/§2): `t` opens/closes it;
            // `Esc` closes it (focus is the main pane here, since
            // `override_focus` is checked earlier in `handle_key`).
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
                self.select_anchor = None;
                self.select_end = None;
            }

            // Spec 0131 §G1: `Ctrl-C` is the single, explicit copy key —
            // copies the active drag-selection if one exists, else the
            // cursor's own current line. Mouse release does not copy by
            // itself (see the no-op `Up(MouseButton::Left)` arm in
            // `handle_mouse`).
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.copy_current_selection_or_line()
            }
            // Spec 0185 S5: there is deliberately no `Tab`-into-the-
            // override-pane arm — the pane's focus lock means the main
            // pane never holds focus while it is open, so such an arm
            // would be unreachable.

            // Override management pane (spec 0117 §3): `o` opens/closes
            // it, mirroring `t`. `Tab` moves focus back into it while
            // it's open (Q2: the management pane deliberately does *not*
            // get the selection pane's focus lock — it performs actual
            // splices, so its main-pane content is committed content and
            // nothing there depends on an immutable anchor).
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
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::F(1) => self.help_open = false,
            KeyCode::Char('j') | KeyCode::Down => {
                self.help_scroll = self.help_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_scroll = self.help_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(10),
            KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
            _ => {}
        }
    }
}
