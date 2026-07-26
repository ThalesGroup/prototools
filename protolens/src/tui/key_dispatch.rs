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
            // Spec 0185 S5: `Tab` no longer hands focus to the main
            // pane. While the selection pane is open, focus is locked to
            // it — that lock is what makes the preview overlay's anchor
            // (`first_row`/`covered_rows`, positions in `visible_rows`)
            // immutable for the overlay's whole lifetime. It falls
            // through to the catch-all arm below.
            KeyCode::Esc | KeyCode::Char('t') | KeyCode::Char('q') => self.close_override(),
            // Spec 0185 S5/G4: the main pane can still be panned while
            // the preview is up. Ctrl-arrows already pan this pane's own
            // candidate list (below), so the main pane gets Alt-arrows.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => self.pan_vertical_up(),
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => self.pan_vertical_down(),
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => self.pan_left(),
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => self.pan_right(),
            // Horizontal pan (item 14 of 2026-07-17 feedback), mirroring
            // the main pane's own Ctrl-Left/Ctrl-Right (spec 0113 D24)
            // and the mouse's Shift-wheel/native horizontal-scroll pan
            // over this pane (`handle_mouse`) — clamped on the right so
            // the rightmost character of the widest visible row is the
            // limit (2026-07-19 feedback item 4, see
            // `App::override_pan_horizontal`).
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.override_pan_horizontal(PAN_STEP, true)
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.override_pan_horizontal(PAN_STEP, false)
            }
            // Vertical pan (2026-07-19 feedback item 1): scrolls the
            // candidate list without moving the highlight, bounded only
            // by the content itself, no longer by the highlighted row
            // (see `App::override_pan_vertical`). Must precede the
            // plain `Up`/`Down` arms below, same "modifier-guard first"
            // convention as the horizontal pan above.
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
            // In-pane search (spec 0114 §4, spec-0133-adjacent rework):
            // reuses the shared bottom command/message bar as the search
            // prompt, same mechanism as the main pane's own `/`/`?`
            // (`handle_command_key`'s `Enter` arm dispatches to
            // `jump_to_override_match` while `override_focus` is set).
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
            KeyCode::Char('p') => {
                if let Some((dir, pattern)) = self.last_override_search.clone() {
                    self.jump_to_override_match(dir.reverse(), &pattern);
                }
            }
            KeyCode::Enter => {
                let Some(idx) = self.override_target else {
                    return;
                };
                // Spec 0137 §G4: `override_candidates` is indexed
                // directly — no more pinned row 0. In alphabetic mode,
                // index `0` is always the `None` sentinel, resolving to
                // raw exactly as the old row 0 did (splice_override's
                // sentinel arm).
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
                let origin = match self.override_origin_for_kind(idx) {
                    Ok(origin) => origin,
                    Err(e) => {
                        self.message = format!("cannot create override: {e}");
                        return;
                    }
                };
                // Spec 0118 §6: any kind's activation triggers the
                // recursive render pass — `path`/`path-field`/`fqdn-field`
                // alike, not just `path` (unlike the old one-shot
                // `apply_override`, which only ever fired for `path`).
                self.overrides.activate(origin.clone(), new_fqdn.clone());
                // Spec 0185 S6: the overlay must not be alive while a
                // splice runs — its anchor is a position in
                // `visible_rows`, which the splice is about to rebuild.
                self.preview_overlay = None;
                self.render_overrides(self.first_node);
                // Spec 0119 G3: land in the management pane, highlighting
                // the entry just created/reactivated, instead of just
                // closing the pane. `activate` guarantees at most one
                // entry per origin is active, so this origin/type pair
                // unambiguously identifies it.
                let target_highlight = self
                    .overrides
                    .entries()
                    .iter()
                    .position(|e| e.origin == origin && e.r#type == new_fqdn);
                self.close_override();
                self.manage_open = true;
                self.manage_focus = true;
                self.manage_highlight = target_highlight.unwrap_or(0);
                self.manage_scroll = 0;
                self.last_manage_highlight = None;
                self.manage_pan_offset = 0;
            }
            _ => {}
        }
    }

    pub(super) fn record_jump(&mut self, from: usize) {
        self.back_stack.push(from);
        self.fwd_stack.clear();
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
        let node = &self.tree[self.cursor].span;
        let short_type = node
            .type_fqdn
            .as_deref()
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
            let node = &self.tree[self.cursor].span;
            let short_type = node
                .type_fqdn
                .as_deref()
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
        // Item 9 (2026-07-17 feedback): `:` opens the command line
        // regardless of which pane currently has focus — checked
        // centrally here, ahead of every focus-specific dispatch below,
        // same tier as `F1`/`Ctrl-Z` above, so `:quit` (and every other
        // command) is reachable from the override/manage panes too, not
        // just the main pane.
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

            // Vertical pan (2026-07-18 feedback item 2): scrolls the
            // viewport without moving the cursor, bounded so the
            // cursor's own row never leaves view. Checked before the
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

            // Fold all siblings of the cursor node (alias for `H`).
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.fold_all_siblings()
            }

            // Parent move / fold (nvim-tree-style: fold an expanded
            // foldable node first; a second press then moves to parent).
            // At the root (no parent to move to), fold all root-level
            // siblings instead — same effect as `H`.
            KeyCode::Char('h') | KeyCode::Left => {
                if self.has_children(self.cursor) && !self.folded.contains(&self.cursor) {
                    self.toggle_fold(self.cursor);
                } else if let Some(parent) = self.tree[self.cursor].parent {
                    self.record_jump(self.cursor);
                    self.set_cursor(parent);
                } else {
                    self.fold_all_siblings();
                }
            }

            // Fold all siblings of the cursor node (its level under the
            // same parent, or all root-level nodes if the cursor is at the
            // root — sibling links are unconditional, see sibling_range).
            KeyCode::Char('H') => self.fold_all_siblings(),

            // Unfold all siblings of the cursor node.
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.unfold_all_siblings()
            }

            // Unfold / child move (ARIA tree-view pattern, symmetric with
            // `h`/`Left`: open a closed foldable node first, cursor stays;
            // a second press then moves to the first child).
            KeyCode::Char('l') | KeyCode::Right => {
                if self.has_children(self.cursor) && self.folded.contains(&self.cursor) {
                    self.toggle_fold(self.cursor);
                } else if let Some(child) = self.tree[self.cursor].first_child {
                    self.record_jump(self.cursor);
                    self.set_cursor(child);
                } else {
                    self.message = "no children".to_string();
                }
            }

            // Fold/unfold toggle.
            KeyCode::Char('z') | KeyCode::Char(' ') => {
                if self.has_children(self.cursor) {
                    self.toggle_fold(self.cursor);
                } else {
                    self.message = "not foldable".to_string();
                }
            }

            // Toggle main-pane annotation display (spec 0133 G3) — a
            // pure display attribute, distinct from the override pane's
            // own `a` (candidate sort toggle) and the manage pane's own
            // `a` (entry active toggle), both gated behind their own
            // focus checks and unreachable here.
            KeyCode::Char('a') => self.annotations = !self.annotations,

            // Toggle the main-pane inference-mismatch heat cue (spec
            // 0138, item 12 of 2026-07-17 feedback) — hides/shows the
            // cue without discarding the heat-cue caches, distinct from the
            // override pane's own `i` (candidate sort toggle), gated
            // behind its own focus check and unreachable here. Guarded
            // to plain `i` so `Ctrl-i` (jumplist "forward", below) is
            // unaffected.
            KeyCode::Char('i') if key.modifiers.is_empty() => {
                self.heat_cues_hidden = !self.heat_cues_hidden
            }

            // Navigation history.
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(pos) = self.back_stack.pop() {
                    self.fwd_stack.push(self.cursor);
                    self.set_cursor(pos);
                    self.unfold_ancestors(pos);
                } else {
                    self.message = "jumplist: at oldest position".to_string();
                }
            }
            KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(pos) = self.fwd_stack.pop() {
                    self.back_stack.push(self.cursor);
                    self.set_cursor(pos);
                    self.unfold_ancestors(pos);
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
            KeyCode::Char('p') => {
                if let Some((dir, pattern)) = self.last_search.clone() {
                    self.jump_to_match(dir.reverse(), &pattern);
                }
            }

            // Override pane (spec 0114 §1/§2): `t` opens/closes it; `Tab`
            // moves focus into it while it's open; `Esc` closes it
            // (focus is main pane here, since `override_focus` is
            // checked earlier in `handle_key`) — same "works regardless
            // of focus" treatment as `t`.
            KeyCode::Char('t') => self.toggle_override(),

            // `Enter` on a main-pane node (item 3, spec 0139 follow-up):
            // a smart proxy for `t`/`o`, mirroring double-click's own
            // behavior below in `handle_mouse`.
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
            // cursor's own current line. Mouse release no longer copies
            // by itself (see the no-op `Up(MouseButton::Left)` arm in
            // `handle_mouse`).
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.copy_current_selection_or_line()
            }
            // Spec 0185 S5: there is no `Tab`-into-the-override-pane arm
            // any more — it was unreachable once the pane's focus lock
            // meant the main pane never holds focus while it is open.

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
    /// non-real `decode::MESSAGE_SET_ITEM_FQDN` placeholder (spec 0120/
    /// 0135) — never registered as a real message in the pool, so it has
    /// no declaration of its own to jump to.
    ///
    /// The main-pane branch mirrors `status_type_label`'s own fallback
    /// chain (2026-07-18 feedback): `span.type_fqdn` is `None` for every
    /// scalar node, including enum-typed ones, so an enum field under
    /// the cursor falls back to its currently effective type (an active
    /// override if one applies, else `natural_type`) — the same FQDN the
    /// status line already shows as "type: ...". A primitive-keyword
    /// result has no declaration to jump to either.
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
            match self.tree[idx].span.type_fqdn.clone() {
                Some(fqdn) => fqdn,
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
