// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::key_dispatch::ctrl_or_alt;
use super::*;

/// `run_export`'s parsed `--binary`/`--prototext`/`--descriptor-binary`/
/// `--descriptor-prototext` flag (spec 0156 G2).
#[derive(PartialEq)]
enum ExportFormat {
    Binary,
    Prototext,
    DescriptorBinary,
    DescriptorPrototext,
}

/// What `App::load_overrides` applied, and what it could not.
///
/// `hash_mismatch` is called out separately rather than left to the
/// caller to pick back out of `warnings`, because the two callers do
/// not agree on how serious it is. Interactively it is exactly what it
/// says — a note that the collection was written against a different
/// target, with the user right there to judge. In batch mode it is
/// worse, and for `--format=descriptor-*` it is fatal: that output is a
/// *schema*, consumed by another tool with nobody watching, and a
/// schema derived from overrides written for some other blob is wrong
/// in a way its consumer cannot detect.
pub(crate) struct OverrideLoad {
    pub(crate) warnings: Vec<String>,
    pub(crate) hash_mismatch: bool,
}

/// Write `contents` to `path` so that `path` is only ever the whole old
/// file or the whole new one, never a truncated mixture of the two.
///
/// `std::fs::write` truncates in place: it destroys the old contents
/// before it has written the new, so a crash, a full disk or a killed
/// process in that window leaves a half-written file where a saved
/// override collection used to be — and the collection it was saving
/// is only in memory. Writing a sibling temp file and renaming over
/// the target instead makes the replacement a single atomic step, and
/// `sync_all` before the rename is what stops the rename from being
/// durable while the bytes it points at are not.
///
/// The temp file is a *sibling*, not a file in the system temp
/// directory: `rename` is only atomic within one filesystem, and
/// nothing says the target shares one with `/tmp`. It carries the
/// process id so that two protolens instances saving the same path do
/// not write over each other's temp file.
pub(super) fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write as _;

    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a file name", path.display()),
        )
    })?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);

    // Every failure past this point leaves the temp file behind, so
    // each one removes it before returning: the target is untouched,
    // and a save that reported an error should not also litter the
    // directory it failed to write into.
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(contents)?;
        f.sync_all()
    });
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

impl App {
    /// Opens the command-line row as `kind`, holding `prefill` with the
    /// cursor at its end.
    ///
    /// All three fields together, so that a new prompt site cannot open
    /// one and forget `command_cursor` — which would leave the caret at
    /// column 0 of a pre-filled buffer, and `Backspace` cancelling
    /// instead of deleting.
    pub(super) fn open_command_line(&mut self, kind: CommandLineKind, prefill: String) {
        self.command_kind = kind;
        self.command_cursor = prefill.chars().count();
        self.command_buffer = Some(prefill);
        // Spec 0235 S6: a `/`/`?` prompt remembers where it was opened
        // from before the first keystroke can move the view.
        if matches!(kind, CommandLineKind::Search { .. }) {
            self.start_search_prompt();
        }
    }

    /// Edit the in-progress command-line buffer at `command_cursor`
    /// (a proper single-line text-input model — `Left`/`Right`/`Home`/`End`
    /// move it, `Backspace`/`Delete`/typing act relative to it), or
    /// execute/cancel the buffer. `Backspace` on an empty buffer cancels,
    /// matching vim's own command line.
    pub(super) fn handle_command_key(&mut self, key: KeyEvent) {
        // Any key other than Tab/Shift-Tab ends an in-progress completion
        // cycle (spec 0113 D26) — a fresh Tab press afterward starts a new
        // one from scratch, against whatever the buffer/cursor now are.
        if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.completion = None;
        }
        // Spec 0235 S18: this line's entire `Control`/`Alt` character
        // vocabulary, in one place, so that the plain `Char(c)` arm below
        // — which carries no modifier condition of its own — cannot also
        // answer for it (see `ctrl_or_alt`). Without it an unbound
        // `Ctrl-u` typed a `u` into the pattern, and every future
        // `Control`/`Alt` binding would have to be added defensively
        // rather than usefully.
        if matches!(key.code, KeyCode::Char(_)) && ctrl_or_alt(&key) {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            match key.code {
                // Emacs/readline-style char motion (vim's own command
                // line supports these too), aliasing the plain arrow keys
                // below; `Alt-f`/`Alt-b` are the word-wise pair, aliasing
                // Alt-Left/Alt-Right.
                KeyCode::Char('f') if ctrl => {
                    let len = self.command_buffer_char_len();
                    self.command_cursor = (self.command_cursor + 1).min(len);
                }
                KeyCode::Char('b') if ctrl => {
                    self.command_cursor = self.command_cursor.saturating_sub(1)
                }
                KeyCode::Char('f') if alt => self.command_cursor = self.next_word_boundary(),
                KeyCode::Char('b') if alt => self.command_cursor = self.prev_word_boundary(),
                // Spec 0235 S17: the same two keys mean the same thing on
                // both halves of the screen — spec 0208 S1 already binds
                // them to `^`/`$` in the main pane.
                KeyCode::Char('a') if ctrl => self.command_cursor = 0,
                KeyCode::Char('e') if ctrl => self.command_cursor = self.command_buffer_char_len(),
                // Spec 0237 S9/S10: readline's forward-delete, the one
                // hole left in the set above. Deliberately *not*
                // readline's delete-or-EOF — an empty buffer is not a
                // quit here (spec 0236 G8), so this is exactly the
                // `Delete` arm below and nothing more.
                KeyCode::Char('d') if ctrl => self.delete_char_forward(),
                // readline's `backward-delete-char` and `kill-line`. The
                // first is an alias of `Backspace` — terminals that do not
                // translate `Ctrl-h` themselves deliver it as a character,
                // and it would otherwise fall through this gate as a
                // no-op.
                KeyCode::Char('h') if ctrl => self.backspace(),
                KeyCode::Char('k') if ctrl => self.kill_to_end_of_line(),
                // The paste counterpart of `Ctrl-c`'s copy (spec 0129
                // G2). `Ctrl-v` rather than readline's `Ctrl-y` because
                // the clipboard it reads is the OS one, not a kill
                // ring, and `Ctrl-v` is what every other single-line
                // field on the user's screen answers to.
                KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
                // readline's `previous-history`/`next-history`, aliasing
                // the plain `Up`/`Down` below — and carrying that arm's
                // guard with them, so that like it they stay unbound at
                // a `:` prompt (spec 0246 S14/N1) rather than silently
                // browsing a history it does not have.
                KeyCode::Char('p')
                    if ctrl && matches!(self.command_kind, CommandLineKind::Search { .. }) =>
                {
                    self.browse_search_history(true)
                }
                KeyCode::Char('n')
                    if ctrl && matches!(self.command_kind, CommandLineKind::Search { .. }) =>
                {
                    self.browse_search_history(false)
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Tab if self.command_kind == CommandLineKind::Command => {
                self.handle_tab_key(true)
            }
            KeyCode::BackTab if self.command_kind == CommandLineKind::Command => {
                self.handle_tab_key(false)
            }
            KeyCode::Enter => {
                // Spec 0276 S4: a find's `Enter` steps to the next match
                // and leaves everything else exactly where it is — the
                // buffer, the caret in it, the origin and the highlight.
                // It never reaches the commit below.
                // Spec 0281 S5: `dir` is the active direction, so this
                // steps whichever way the reader last pointed the prompt.
                if let CommandLineKind::Search { dir, find: Some(_) } = self.command_kind {
                    self.rotate_search_match(dir);
                    return;
                }
                let buf = self.command_buffer.take().unwrap_or_default();
                self.command_cursor = 0;
                match self.command_kind {
                    CommandLineKind::Command => self.run_command(&buf),
                    // Vim convention: `/`/`?` confirmed with an empty
                    // pattern re-uses the last active pattern, searching
                    // in the newly chosen direction (which may differ
                    // from the direction that pattern was originally
                    // searched in — unlike `n`, which always repeats in
                    // the same direction as last time).
                    //
                    // Spec 0339 S8: which pane's search runs is
                    // `active_search_scope` — the pane the prompt was
                    // opened from — and which of the three
                    // `last_*_search` fields it remembers is
                    // `set_last_search_for`'s business, not this arm's.
                    // All three panes share the main pane's bar (spec
                    // 0114 §4/0117 §3).
                    CommandLineKind::Search { dir, .. } => {
                        let scope = self.active_search_scope();
                        let pattern = if buf.is_empty() {
                            self.last_search_for(scope)
                                .map(|(_, p)| p.clone())
                                .unwrap_or(buf)
                        } else {
                            buf
                        };
                        self.set_last_search_for(scope, (dir, pattern.clone()));
                        self.commit_search(dir, &pattern);
                    }
                }
            }
            KeyCode::Esc => {
                // Spec 0276 S5: at a find prompt `Esc` is the accept,
                // not the cancel — the gesture's one exit (N1).
                if let CommandLineKind::Search { dir, find: Some(_) } = self.command_kind {
                    self.accept_find(dir);
                    return;
                }
                self.command_buffer = None;
                self.command_cursor = 0;
                self.cancel_search();
            }
            // Word motion: Alt-Left/Alt-Right, whose Alt-f/Alt-b aliases
            // are in the Ctrl/Alt gate above. Must precede the plain
            // Left/Right arms below since match arms are checked in
            // order.
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                self.command_cursor = self.next_word_boundary();
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                self.command_cursor = self.prev_word_boundary();
            }
            // Spec 0246 S17/S24: rotate the preview through the matches
            // of the pattern as typed. Must precede the plain Left/Right
            // arms for the same reason the Alt pair does — the Ctrl/Alt
            // gate above only answers for `Char` keys, so without an arm
            // here a `Ctrl-Right` would move the text cursor. At a `:`
            // prompt they are swallowed rather than passed on, which is
            // what keeps that from being a surprise later (N1).
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if matches!(self.command_kind, CommandLineKind::Search { .. }) {
                    self.step_search_match(SearchDir::Forward);
                }
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if matches!(self.command_kind, CommandLineKind::Search { .. }) {
                    self.step_search_match(SearchDir::Backward);
                }
            }
            // Spec 0281 S3: the find's own pair — *onward* and *back*
            // rather than forward and backward, so that after `B` the
            // key that keeps going is still the right-hand one.
            //
            // The `find` guard is in the pattern rather than in the body
            // (S6): at a `/`, `?` or `:` prompt these must reach the
            // plain Left/Right arms below, which is what they do today.
            // They sit after the Ctrl pair so that a Ctrl-Shift-arrow
            // keeps meaning the Ctrl one.
            KeyCode::Right
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    && matches!(
                        self.command_kind,
                        CommandLineKind::Search { find: Some(_), .. }
                    ) =>
            {
                self.step_find_match(false);
            }
            KeyCode::Left
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    && matches!(
                        self.command_kind,
                        CommandLineKind::Search { find: Some(_), .. }
                    ) =>
            {
                self.step_find_match(true);
            }
            // Spec 0246 S14: the search history. Unbound at a `:` prompt
            // (N1), where these fall through to the catch-all below.
            KeyCode::Up if matches!(self.command_kind, CommandLineKind::Search { .. }) => {
                self.browse_search_history(true)
            }
            KeyCode::Down if matches!(self.command_kind, CommandLineKind::Search { .. }) => {
                self.browse_search_history(false)
            }
            KeyCode::Left => self.command_cursor = self.command_cursor.saturating_sub(1),
            KeyCode::Right => {
                let len = self.command_buffer_char_len();
                self.command_cursor = (self.command_cursor + 1).min(len);
            }
            KeyCode::Home => self.command_cursor = 0,
            KeyCode::End => self.command_cursor = self.command_buffer_char_len(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_char_forward(),
            KeyCode::Char(c) => {
                let byte_idx = self.char_byte_index(self.command_cursor);
                if let Some(buf) = self.command_buffer.as_mut() {
                    buf.insert(byte_idx, c);
                }
                self.command_cursor += 1;
                // Spec 0235 S7: every edit of the pattern replaces the
                // sweep in flight, whatever the edit was.
                self.restart_search_sweep();
            }
            _ => {}
        }
    }

    /// Spec 0275 S2: put the caret on the character the reader clicked.
    ///
    /// This is `render_command_row`'s mapping read backwards, and the
    /// two have to move together: that mapping draws character `pos` of
    /// `cmd_text` — the prefix (`:`, `/`, `?`) followed by the buffer —
    /// at `cmd_area.x + (pos - command_pan_offset)`, so a click at
    /// `col` names `pos = (col - cmd_area.x) + command_pan_offset`, and
    /// the `- 1` below drops the prefix to get back to a buffer index.
    ///
    /// The pan is carried by that one added term and nothing else. Both
    /// it and the caret are counted in *characters* — `pan_spans` skips
    /// characters and `set_cursor_position` advances one column per
    /// character — so this inverts exactly rather than approximately.
    ///
    /// Clamped at both ends rather than rejected (S4): a click on the
    /// prefix means the start, a click past the text means the end, and
    /// a click that visibly did nothing would read as a lost one.
    ///
    /// No pan follows. `render_command_row` re-pans only when the caret
    /// is outside the visible window, and a caret derived from a
    /// visible column is inside it.
    pub(super) fn command_click(&mut self, col: u16) {
        let Some(area) = self.cmd_area else {
            return;
        };
        // Spec 0275 N2: a status message shares this row but is output,
        // not a field — there is no caret to place.
        if self.command_buffer.is_none() {
            return;
        }
        let pos = (col.saturating_sub(area.x)) as usize + self.command_pan_offset;
        self.command_cursor = pos.saturating_sub(1).min(self.command_buffer_char_len());
    }

    /// `Ctrl-v`: insert the OS clipboard's text at the cursor.
    ///
    /// A failed read is reported through `self.message`, which the open
    /// command line is currently covering — but nothing was inserted,
    /// and that is the feedback the user actually reads. The message is
    /// there for when the line closes.
    fn paste_from_clipboard(&mut self) {
        match clipboard_text() {
            Ok(text) => self.insert_pasted(&text),
            Err(e) => self.message = format!("clipboard unavailable: {e}"),
        }
    }

    /// Insert `text` at the cursor as if it had been typed, minus what
    /// cannot be.
    ///
    /// The command line is one line and holds no control characters, so
    /// a paste is cut at the first line break and stripped of the rest.
    /// Cutting rather than joining: a two-line clipboard concatenated
    /// into `foobar` is a plausible-looking command nobody asked for,
    /// while a truncated one is visibly incomplete.
    pub(super) fn insert_pasted(&mut self, text: &str) {
        let cut = text.find(['\n', '\r']).unwrap_or(text.len());
        let cleaned: String = text[..cut].chars().filter(|c| !c.is_control()).collect();
        if cleaned.is_empty() {
            return;
        }
        let byte_idx = self.char_byte_index(self.command_cursor);
        if let Some(buf) = self.command_buffer.as_mut() {
            buf.insert_str(byte_idx, &cleaned);
        }
        self.command_cursor += cleaned.chars().count();
        // Spec 0235 S7, same as the typed-character arm: any edit of the
        // pattern replaces the sweep in flight.
        self.restart_search_sweep();
    }

    /// Delete the character before the cursor, shared by `Backspace` and
    /// `Ctrl-h`. On an already-empty buffer it closes the line instead —
    /// backspacing past the `:`/`/` prompt is how vim leaves it.
    fn backspace(&mut self) {
        let empty = match &self.command_buffer {
            Some(buf) => buf.is_empty(),
            None => true,
        };
        if empty {
            self.command_buffer = None;
            self.command_cursor = 0;
            self.cancel_search();
        } else if self.command_cursor > 0 {
            self.command_cursor -= 1;
            self.remove_char_at(self.command_cursor);
            self.restart_search_sweep();
        }
    }

    /// readline's `kill-line`: drop everything from the cursor to the end
    /// of the buffer. Unlike [`Self::backspace`] an empty buffer is not a
    /// close — there is nothing to kill, so nothing happens.
    fn kill_to_end_of_line(&mut self) {
        let byte_idx = self.char_byte_index(self.command_cursor);
        let killed = match self.command_buffer.as_mut() {
            Some(buf) if byte_idx < buf.len() => {
                buf.truncate(byte_idx);
                true
            }
            _ => false,
        };
        if killed {
            self.restart_search_sweep();
        }
    }

    /// Delete the character under the cursor, shared by `Delete` and
    /// `Ctrl-d` (spec 0237 S9). A no-op at the end of the buffer.
    fn delete_char_forward(&mut self) {
        if self.command_cursor < self.command_buffer_char_len() {
            self.remove_char_at(self.command_cursor);
            self.restart_search_sweep();
        }
    }

    /// `Tab` (`forward`)/`Shift-Tab` (`!forward`) in the command line (spec
    /// 0113 D26): continue an already-cycling completion, or start a new
    /// one against the current token.
    pub(super) fn handle_tab_key(&mut self, forward: bool) {
        if let Some(state) = &self.completion {
            if state.candidates.len() > 1 {
                let n = state.candidates.len();
                let new_index = match state.index {
                    Some(i) if forward => (i + 1) % n,
                    Some(i) => (i + n - 1) % n,
                    None if forward => 0,
                    None => n - 1,
                };
                let candidate = state.candidates[new_index].clone();
                let token_start = state.token_start;
                let suffix = state.suffix.clone();
                self.replace_token(token_start, &suffix, &candidate);
                if let Some(state) = &mut self.completion {
                    state.index = Some(new_index);
                }
                return;
            }
        }
        self.start_tab_completion();
    }

    /// Complete the token the cursor currently sits in: the first token
    /// (the command name, before any space) always; a token beginning
    /// with `-` against the command's own flags (spec 0236 S22); and
    /// otherwise whatever that command's arguments are — an
    /// `:override` origin, type or field name, a `:save-overrides`/
    /// `:restore-overrides` path, a `:proto-root` directory.
    /// Anywhere else is a silent no-op.
    ///
    /// The flag check comes first and is command-independent, so a `-`
    /// never reaches a value completer that would try to read it as a
    /// path or an FQDN. It is safe there precisely because no value
    /// this line accepts begins with `-`.
    pub(super) fn start_tab_completion(&mut self) {
        let buf = self.command_buffer.clone().unwrap_or_default();
        let cursor_byte = self.char_byte_index(self.command_cursor);
        let prefix = &buf[..cursor_byte];
        let Some((cmd, rest)) = prefix.split_once(' ') else {
            self.complete_command_name(prefix);
            return;
        };
        let Ok(resolved) = resolve_command(cmd) else {
            return;
        };
        let token_byte = rest.rfind(' ').map_or(0, |i| i + 1);
        if rest[token_byte..].starts_with('-') {
            let token_start = cmd.chars().count() + 1 + rest[..token_byte].chars().count();
            self.complete_flag(resolved, token_start, &rest[token_byte..]);
            return;
        }
        match resolved {
            "override" => self.complete_override_cmd(cmd, rest),
            "save-overrides" | "restore-overrides" => self.complete_fs_path(cmd, rest),
            "proto-root" => self.complete_dir_path(cmd, rest),
            _ => {}
        }
    }

    /// First-token (command-name) completion — see `start_tab_completion`.
    pub(super) fn complete_command_name(&mut self, prefix: &str) {
        let mut matches = complete_prefix(prefix, COMMANDS.iter().copied());
        if matches.is_empty() {
            self.message = format!("no command matches '{prefix}'");
            return;
        }
        matches.sort_unstable();
        let candidates: Vec<String> = matches.into_iter().map(String::from).collect();
        self.apply_completion(0, prefix.chars().count(), candidates);
    }

    /// Flag completion (spec 0236 S22): a token beginning with `-`
    /// completes against `command_flags`, the way a shell's completion
    /// does. A single `-` matches every flag, since every flag begins
    /// with one.
    fn complete_flag(&mut self, resolved: &str, token_start: usize, prefix: &str) {
        let mut matches: Vec<String> =
            complete_prefix(prefix, command_flags(resolved).iter().copied())
                .into_iter()
                .map(String::from)
                .collect();
        if matches.is_empty() {
            self.message = format!("{resolved}: no option matches '{prefix}'");
            return;
        }
        matches.sort_unstable();
        self.apply_completion(token_start, prefix.chars().count(), matches);
    }

    /// Type-argument completion (spec 0114 §7, spec 0236 S12) —
    /// candidates are `all_type_fqdns` (the same session-global,
    /// lexicographically-sorted list §3.2/§6 already compute and cache),
    /// reused here rather than recomputed, plus (spec 0135 §G4, spec
    /// 0299) the override keywords wire-compatible with the cursor
    /// node's current wire type (a packed element's own effective wire
    /// type is always `WT_LEN`, per its reconstructed record — spec
    /// 0135 §G1).
    ///
    /// Takes an explicit `subject` node and token start rather than
    /// assuming the cursor and "just past the command name":
    /// `:override`'s `--as` can sit anywhere on the line, and speaks
    /// about whichever node its origin names — which in the manage pane
    /// is routinely not the cursor.
    pub(super) fn complete_type_at(
        &mut self,
        subject: usize,
        token_start: usize,
        arg_prefix: &str,
    ) {
        let wire_type = decode::effective_wire_type(&self.tree[subject].span);
        // Collected into owned `String`s upfront (rather than borrowing
        // `self.all_type_fqdns` for `matches`'s lifetime) so the
        // subsequent `self.replace_token`/`self.completion = ...` calls
        // below aren't blocked by a live immutable borrow of `self`.
        // Spec 0315 S7: declared types are chained in explicitly rather
        // than recomputing `all_type_fqdns` to include them — that list
        // is built once, before any override exists, and that timing is
        // the only thing keeping every synthetic wrapper out of it.
        let candidates = decode::override_keywords_for_wire_type(wire_type)
            .iter()
            .copied()
            .chain(self.ctx.created_types().iter().map(String::as_str))
            .chain(self.all_type_fqdns.iter().map(String::as_str));
        let mut matches: Vec<String> = complete_prefix(arg_prefix, candidates)
            .into_iter()
            .map(String::from)
            .collect();
        if matches.is_empty() {
            self.message = format!("no type matches '{arg_prefix}'");
            return;
        }
        matches.sort_unstable();
        self.apply_completion(token_start, arg_prefix.chars().count(), matches);
    }

    /// `:save-overrides`/`:restore-overrides` argument completion (spec
    /// 0117 §4) — candidates are `std::fs::read_dir`'s entries for the
    /// argument's directory portion (everything up to and including its
    /// last `/`, or the current directory if there is none), filtered by
    /// its final path segment; directory entries get a trailing `/`
    /// appended, so a further Tab press descends into them. No
    /// `!arg_prefix.contains(' ')` guard, unlike `complete_type_as_fqdn` —
    /// a path argument is everything after the command name's single
    /// space, embedded spaces included.
    pub(super) fn complete_fs_path(&mut self, cmd: &str, arg_prefix: &str) {
        self.complete_path(cmd, arg_prefix, false);
    }

    /// `:proto-root <dir>`'s argument completion (spec 0144 G4) — same
    /// shape as `complete_fs_path`, but directory entries only: a file is
    /// never a valid `:proto-root` argument.
    pub(super) fn complete_dir_path(&mut self, cmd: &str, arg_prefix: &str) {
        self.complete_path(cmd, arg_prefix, true);
    }

    /// The body of both path completions. `dirs_only` is the whole of
    /// the difference between them: it drops non-directory entries, and
    /// with them the trailing-`/` conditional (everything left is a
    /// directory) and the wording of the no-match message.
    fn complete_path(&mut self, cmd: &str, arg_prefix: &str, dirs_only: bool) {
        let (dir_part, file_prefix) = match arg_prefix.rfind('/') {
            Some(i) => (&arg_prefix[..=i], &arg_prefix[i + 1..]),
            None => ("", arg_prefix),
        };
        let read_dir_path = if dir_part.is_empty() {
            Path::new(".")
        } else {
            Path::new(dir_part)
        };
        let entries = match std::fs::read_dir(read_dir_path) {
            Ok(rd) => rd,
            Err(e) => {
                self.message = format!("cannot list '{}': {e}", read_dir_path.display());
                return;
            }
        };
        let mut matches: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if dirs_only && !is_dir {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(file_prefix) {
                continue;
            }
            let mut candidate = format!("{dir_part}{name}");
            if is_dir {
                candidate.push('/');
            }
            matches.push(candidate);
        }
        if matches.is_empty() {
            let what = if dirs_only { "directory" } else { "path" };
            self.message = format!("no {what} matches '{arg_prefix}'");
            return;
        }
        matches.sort_unstable();
        let token_start = cmd.chars().count() + 1;
        self.apply_completion(token_start, arg_prefix.chars().count(), matches);
    }

    /// Shared tail of Tab-completion (spec 0114 §7): `candidates` (already
    /// filtered/sorted by the caller) either replaces the in-progress
    /// token outright (a single candidate) or extends it to the longest
    /// common prefix, stashing `candidates` in `self.completion` for a
    /// subsequent Tab press to cycle through (spec 0113 D26). `prefix_len`
    /// is the char length of what the user already typed, used to decide
    /// whether the LCP actually extends it.
    pub(super) fn apply_completion(
        &mut self,
        token_start: usize,
        prefix_len: usize,
        candidates: Vec<String>,
    ) {
        let cursor_byte = self.char_byte_index(self.command_cursor);
        let buf = self.command_buffer.clone().unwrap_or_default();
        let suffix = buf[cursor_byte..].to_string();
        if candidates.len() == 1 {
            self.replace_token(token_start, &suffix, &candidates[0]);
            return;
        }
        let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        let lcp = longest_common_prefix(&refs);
        if lcp.chars().count() > prefix_len {
            self.replace_token(token_start, &suffix, &lcp);
        }
        self.completion = Some(CompletionState {
            token_start,
            suffix,
            candidates,
            index: None,
        });
    }

    /// An *unfiltered* rotation completion (spec 0237 S6/S7), as
    /// opposed to `apply_completion`'s prefix match: `candidates` are
    /// alternatives to each other rather than entries in a namespace,
    /// so the token is replaced outright and no common prefix is
    /// computed.
    ///
    /// The first Tab lands on the candidate *after* whichever one the
    /// token already spells, so rotating away from a pre-filled value
    /// moves on the first press; on a token spelling none of them it
    /// lands on the first. This is the whole difference from
    /// `apply_completion`, which leaves `index: None` so that its first
    /// Tab only primes the cycle.
    pub(super) fn apply_rotation(
        &mut self,
        token_start: usize,
        prefix: &str,
        candidates: Vec<String>,
    ) {
        let index = match candidates.iter().position(|c| c == prefix) {
            Some(i) => (i + 1) % candidates.len(),
            None if candidates.is_empty() => return,
            None => 0,
        };
        let cursor_byte = self.char_byte_index(self.command_cursor);
        let buf = self.command_buffer.clone().unwrap_or_default();
        let suffix = buf[cursor_byte..].to_string();
        self.replace_token(token_start, &suffix, &candidates[index]);
        self.completion = Some(CompletionState {
            token_start,
            suffix,
            candidates,
            index: Some(index),
        });
    }

    /// Replace `command_buffer[token_start..command_cursor]` with
    /// `replacement`, re-appending `suffix` (the text that originally
    /// followed the token) verbatim, and move the cursor to just past the
    /// replacement.
    pub(super) fn replace_token(&mut self, token_start: usize, suffix: &str, replacement: &str) {
        // A directory-path `replacement` always ends in `/` (G4). If the
        // cursor sat right before an already-present `/` when completion
        // was triggered (e.g. after Left-arrow-ing back into an earlier
        // path segment to edit it), `suffix` starts with that same `/`
        // and splicing them back-to-back would double it up. Drop the
        // redundant leading separator from `suffix` in that case.
        let suffix = if replacement.ends_with('/') && suffix.starts_with('/') {
            &suffix[1..]
        } else {
            suffix
        };
        let start_byte = self.char_byte_index(token_start);
        let mut new_buf = String::with_capacity(start_byte + replacement.len() + suffix.len());
        if let Some(buf) = &self.command_buffer {
            new_buf.push_str(&buf[..start_byte]);
        }
        new_buf.push_str(replacement);
        new_buf.push_str(suffix);
        self.command_cursor = token_start + replacement.chars().count();
        self.command_buffer = Some(new_buf);
    }

    pub(super) fn command_buffer_char_len(&self) -> usize {
        self.command_buffer
            .as_deref()
            .map(|b| b.chars().count())
            .unwrap_or(0)
    }

    /// `prev_word_boundary` over the command buffer (Alt-b/Alt-Left).
    pub(super) fn prev_word_boundary(&self) -> usize {
        let chars: Vec<char> = self
            .command_buffer
            .as_deref()
            .unwrap_or("")
            .chars()
            .collect();
        prev_word_boundary(&chars, self.command_cursor)
    }

    /// `next_word_boundary` over the command buffer (Alt-f/Alt-Right).
    pub(super) fn next_word_boundary(&self) -> usize {
        let chars: Vec<char> = self
            .command_buffer
            .as_deref()
            .unwrap_or("")
            .chars()
            .collect();
        next_word_boundary(&chars, self.command_cursor)
    }

    /// Byte offset in `command_buffer` of the `char_idx`-th character (or
    /// the buffer's end, if `char_idx` is at/past its length).
    pub(super) fn char_byte_index(&self, char_idx: usize) -> usize {
        let buf = self.command_buffer.as_deref().unwrap_or("");
        buf.char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(buf.len())
    }

    /// Remove the character at char index `char_idx` from `command_buffer`.
    pub(super) fn remove_char_at(&mut self, char_idx: usize) {
        let byte_idx = self.char_byte_index(char_idx);
        if let Some(buf) = self.command_buffer.as_mut() {
            if byte_idx < buf.len() {
                buf.remove(byte_idx);
            }
        }
    }

    pub(super) fn run_command(&mut self, cmd: &str) {
        let mut tokens = cmd.split_whitespace();
        let Some(name) = tokens.next() else {
            return;
        };
        match resolve_command(name) {
            Ok("export") => self.run_export(tokens.collect()),
            // Spec 0236 S20: `:quit` — or any unambiguous prefix, e.g.
            // `:q`, since no other command starts with `q` — is the only
            // way to quit. The `q`-then-`q` confirmation it replaced
            // existed because `q` was one keystroke from an accidental
            // exit; typing a command out and pressing Enter is already
            // deliberate.
            Ok("quit") => self.should_quit = true,
            // Spec 0236 S23: the same overlay `F1` opens, reachable by
            // name — the one binding a newcomer cannot guess is the one
            // that lists the others.
            Ok("help") => self.help_open = true,
            Ok("override") => self.run_override_cmd(tokens.collect()),
            Ok("save-overrides") => self.run_save_overrides(tokens.collect()),
            Ok("restore-overrides") => self.run_restore_overrides(tokens.collect()),
            Ok("proto-root") => self.run_proto_root(tokens.collect()),
            // Reachable, notwithstanding that `COMMANDS` and the arms
            // above currently agree: a name added to the registry gets
            // resolution and Tab-completion for free but not an arm
            // here, and that omission is a missing feature, not grounds
            // for taking the session down mid-edit.
            Ok(other) => self.message = format!("command not implemented: {other}"),
            Err(e) => self.message = e,
        }
    }

    /// The single free-form argument these commands take, rejoined, or
    /// `None` — having already set `message` to `missing` — when the
    /// command was given none.
    ///
    /// The rejoin is half the point: a path or an FQDN containing a
    /// space arrives here already split on whitespace, and every one of
    /// these commands wants it back whole.
    fn require_arg(&mut self, args: &[&str], missing: &str) -> Option<String> {
        if args.is_empty() {
            self.message = missing.to_string();
            return None;
        }
        Some(args.join(" "))
    }

    /// `export [--binary|--prototext|--descriptor-binary|
    /// --descriptor-prototext] <path>` — default format is `#@ prototext`
    /// text (0113 D21). The two `--descriptor-*` flags build and write a
    /// `FileDescriptorSet` per spec 0156 G6/G7, instead of slicing the
    /// node's own data.
    pub(super) fn run_export(&mut self, args: Vec<&str>) {
        let mut format = ExportFormat::Prototext;
        let mut path_parts = Vec::new();
        for a in args {
            match a {
                "--binary" => format = ExportFormat::Binary,
                "--prototext" => format = ExportFormat::Prototext,
                "--descriptor-binary" => format = ExportFormat::DescriptorBinary,
                "--descriptor-prototext" => format = ExportFormat::DescriptorPrototext,
                other => path_parts.push(other),
            }
        }
        if path_parts.is_empty() {
            self.message = "export: missing path".to_string();
            return;
        }
        let path = path_parts.join(" ");
        match format {
            ExportFormat::DescriptorBinary | ExportFormat::DescriptorPrototext => {
                // Spec 0261 S4: the descriptor formats read the *shape*
                // of the cursor's children, and `child_slots` reports a
                // stop as having none — so an unbaked node would export
                // as a message with no fields at all.
                if !self.drain_for_export() {
                    return;
                }
                let as_prototext = format == ExportFormat::DescriptorPrototext;
                self.message = match self.export_descriptor(&path, as_prototext) {
                    Ok(()) => format!("exported to {path}"),
                    Err(e) => e,
                };
            }
            ExportFormat::Binary | ExportFormat::Prototext => {
                let extract_format = if format == ExportFormat::Binary {
                    ExtractFormat::Binary
                } else {
                    ExtractFormat::Text
                };
                // Spec 0261 N4: `--binary` slices the blob by the node's
                // raw range and never reads a rendered line, so it has
                // nothing to wait for — and making it wait would turn a
                // free root export into a multi-second one.
                if extract_format == ExtractFormat::Text && !self.drain_for_export() {
                    return;
                }
                let lines = self.subtree_lines(self.cursor);
                match extract::extract(
                    Path::new(&path),
                    extract_format,
                    &self.blob,
                    &lines,
                    &self.tree[self.cursor],
                    0..lines.len(),
                ) {
                    Ok(()) => self.message = format!("exported to {path}"),
                    Err(e) => self.message = format!("export error: {e}"),
                }
            }
        }
    }

    /// Spec 0261 S1/S5: make the cursor's subtree whole before a format
    /// that reads the render gets hold of it, and say whether the export
    /// may proceed.
    ///
    /// A refusal writes no file. Silence here would be the defect this
    /// spec exists to remove — a document that looks complete and is
    /// not — so the one case the drain cannot fix is the one case the
    /// user is told about. `expand_auto_fold` has already put the
    /// underlying splice failure in `message`, which is the useful half
    /// of the news, so this wraps it rather than overwriting it.
    fn drain_for_export(&mut self) -> bool {
        if self.bake_subtree(self.cursor) {
            return true;
        }
        self.message = format!(
            "export refused: this node is still incomplete and could not be \
             finished ({})",
            self.message
        );
        false
    }

    /// Shared core of `xdb`/`xdp` (TUI) and batch's
    /// `--format=descriptor-binary`/`descriptor-prototext` (spec 0156 G6/
    /// G7): resolves the cursor node's synthetic fields (G6a-c) and
    /// builds the `FileDescriptorSet` (`export_descriptor::build`), as
    /// raw bytes, or (`as_prototext`) rendered through the `#@ prototext`
    /// pipeline against G7's located meta-schema.
    pub(crate) fn export_descriptor_bytes(
        &mut self,
        as_prototext: bool,
    ) -> Result<Vec<u8>, String> {
        let span = &self.tree[self.cursor].span;
        if span.kind != NodeKind::Message {
            return Err("export --descriptor: cursor node is not a message/group".to_string());
        }
        let message_name =
            export_descriptor::synthetic_message_name(&self.field_name_for(self.cursor));
        let fields = self.resolve_export_fields(self.cursor)?;
        let fds = export_descriptor::build(&message_name, fields);
        use prost_reflect::prost::Message as _;
        let bytes = fds.encode_to_vec();
        if as_prototext {
            // The meta-schema is one specific message, not a namespace
            // scan (spec 0197 §S4): JIT-load it by name first, then let
            // `locate_file_descriptor_set_type` search the pool.
            self.ctx.message("google.protobuf.FileDescriptorSet");
            let fds_type = export_descriptor::locate_file_descriptor_set_type(self.ctx.pool())
                .ok_or_else(|| {
                    "export --descriptor --prototext: no descriptor.proto (with \
                     FileDescriptorSet/FileDescriptorProto messages) found in the \
                     loaded --descriptor-set; use --descriptor-binary instead, or \
                     rebuild the schema-db so descriptor.proto is included"
                        .to_string()
                })?;
            let opts = DecodeRenderOpts {
                annotations: true,
                indent_size: self.indent_size,
                emit_header: true,
                ..DecodeRenderOpts::default()
            };
            Ok(decode_and_render(&bytes, Some(&fds_type), opts))
        } else {
            Ok(bytes)
        }
    }

    /// `path`-writing counterpart of `export_descriptor_bytes`, used by
    /// the TUI's `:export --descriptor-*` command (spec 0156 G6/G7).
    pub(super) fn export_descriptor(
        &mut self,
        path: &str,
        as_prototext: bool,
    ) -> Result<(), String> {
        let bytes = self.export_descriptor_bytes(as_prototext)?;
        std::fs::write(path, bytes).map_err(|e| format!("export error: {e}"))
    }

    /// `idx`'s extracted rendering, in the requested format — the
    /// byte-vector counterpart to `run_extract`'s file-writing TUI
    /// command, for a caller with no `Path` to write to (spec 0123's
    /// batch mode, writing to stdout or an explicit `-o`/`--output`).
    pub(crate) fn extract_bytes(&self, idx: usize, format: ExtractFormat) -> Vec<u8> {
        let lines = self.subtree_lines(idx);
        extract::extract_bytes(format, &self.blob, &lines, &self.tree[idx], 0..lines.len())
    }

    /// Propose a default `:save-overrides` path — same directory/stem as
    /// the target blob, `.yaml` extension (spec 0117 §4, mirroring
    /// `default_extract_path`).
    pub(super) fn default_save_overrides_path(&self) -> String {
        let stem = self
            .blob_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "overrides".to_string());
        let filename = format!("{stem}.yaml");
        match self.blob_path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => {
                dir.join(filename).to_string_lossy().into_owned()
            }
            _ => filename,
        }
    }

    /// SHA-256 hex digests of the currently-loaded blob/descriptor set,
    /// canonicalized-binary bytes (spec 0117 §4's `blob_sha256`/
    /// `descriptor_set_sha256`) — the caller's original (pre-wrap) blob,
    /// and the descriptor set's own canonicalized bytes (re-read from disk
    /// on demand, spec 0197 §S6). `Err` when that re-read fails.
    pub(super) fn target_hashes(&self) -> Result<(String, String), String> {
        let blob_sha256 = override_pane::sha256_hex(&self.blob[self.wrapper_offset..]);
        let descriptor_sha256 = self.ctx.descriptor_sha256().map_err(|e| e.to_string())?;
        Ok((blob_sha256, descriptor_sha256))
    }

    /// Inverse of `positional_path`: resolves a canonical `/1/2/3`-style
    /// path (or bare `/` for the wrapper root) back to a tree index.
    /// `None` if any segment doesn't parse as a 1-based position, or
    /// doesn't resolve against the current tree (spec 0117 §4 restore-time
    /// validation). `pub(crate)`: also reused by `main.rs`'s batch `extract`
    /// subcommand (spec 0123) to resolve its `path` argument.
    pub(crate) fn resolve_path(&self, path: &str) -> Option<usize> {
        // Spec 0188 G1: `self.first_node`, not a search for the arena's
        // parentless node. That search is a linear scan of the whole
        // arena — 382 k nodes on a 25 MB descriptor — and it would run
        // once per `Path` override entry per batch.
        let root = self.first_node;
        if path == "/" {
            return Some(root);
        }
        let mut cur = root;
        // Spec 0216 S17: one add and a bounds check per segment. The
        // positions are 1-based on the wire, `nth_child` is 0-based, and
        // `checked_sub` is what rejects a `/0/` segment.
        for seg in path.trim_start_matches('/').split('/') {
            let pos: usize = seg.parse().ok()?;
            cur = self.nth_child(cur, pos.checked_sub(1)?)?;
        }
        Some(cur)
    }

    /// Whether `origin` resolves against the currently-loaded tree/
    /// descriptor pool (spec 0117 §4 restore-time validation): `Path`
    /// needs the path to resolve to a node; `PathField` additionally
    /// needs that node to have at least one child with the given field
    /// number; `FqdnField` needs the FQDN to resolve in the descriptor
    /// pool and that message to declare the given field number — unless
    /// the FQDN is a declared type (spec 0315 S13), which by
    /// construction declares no fields at all.
    pub(super) fn origin_resolves(&mut self, origin: &OverrideOrigin) -> bool {
        match origin {
            OverrideOrigin::Path { path } => self.resolve_path(path).is_some(),
            OverrideOrigin::PathField { path, field } => self
                .resolve_path(path)
                .is_some_and(|idx| self.children_with_field(idx, *field).next().is_some()),
            // The declared-field test is right for a real type — an
            // origin naming a field the published schema does not have
            // is stale — and meaningless for a declared one, where the
            // set of `fqdn:field` entries *is* the schema.
            OverrideOrigin::FqdnField { fqdn, .. } if self.ctx.is_declared_type(fqdn) => true,
            // A restored collection names types the current render may
            // never have touched, so this JIT-loads (spec 0197 §S5).
            OverrideOrigin::FqdnField { fqdn, field } => self
                .ctx
                .message(fqdn)
                .and_then(|m| m.get_field(*field as u32))
                .is_some(),
        }
    }

    /// `save <path>` (spec 0117 §4): writes the entire collection, plus
    /// the current target's hashes, to `<path>` as YAML — atomically
    /// (`write_atomically`), since the collection being saved has no
    /// other copy.
    pub(super) fn run_save_overrides(&mut self, args: Vec<&str>) {
        let Some(path) = self.require_arg(&args, "save-overrides: missing path") else {
            return;
        };
        let (blob_sha256, descriptor_set_sha256) = match self.target_hashes() {
            Ok(hashes) => hashes,
            Err(e) => {
                self.message = format!("save-overrides error: {e}");
                return;
            }
        };
        let yaml = self.overrides.to_yaml(
            blob_sha256,
            descriptor_set_sha256,
            self.ctx.created_types().to_vec(),
        );
        match write_atomically(Path::new(&path), yaml.as_bytes()) {
            Ok(()) => self.message = format!("saved overrides to {path}"),
            Err(e) => self.message = format!("save-overrides error: {e}"),
        }
    }

    /// `proto-root <dir>` (spec 0144 G4): validates and sets `proto_root`
    /// dynamically, overriding `-I`/`--proto-root` for the rest of the
    /// session. Invalid input leaves the previous value untouched.
    pub(super) fn run_proto_root(&mut self, args: Vec<&str>) {
        let Some(arg) = self.require_arg(&args, "proto-root: missing directory") else {
            return;
        };
        let dir = PathBuf::from(arg);
        if !dir.is_dir() {
            self.message = format!("not a directory: {}", dir.display());
            return;
        }
        self.message = format!("proto-root set to {}", dir.display());
        self.proto_root = Some(dir);
    }

    /// Shared core of `:restore-overrides`/batch `--load-overrides` (spec
    /// 0117 §4, spec 0123 G4): loads and parses the YAML override
    /// collection at `path`, silently drops any entry that doesn't
    /// resolve against the current tree/descriptor pool, then replaces
    /// `self.overrides` wholesale and re-renders (spec 0118 §6:
    /// replacing the whole collection can change the resolved override
    /// for any node). Returns an `OverrideLoad` describing what was and
    /// was not applied on success, or `Err(diagnostic)` if the file
    /// couldn't be read or parsed as valid YAML in the first place,
    /// which the two callers (`run_restore_overrides`, batch mode) treat
    /// differently: the TUI just displays it and keeps running; batch
    /// mode (spec 0123 G4) treats it as a hard error.
    pub(crate) fn load_overrides(&mut self, path: &str) -> Result<OverrideLoad, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let (mut collection, target, created_types) =
            override_pane::OverrideCollection::from_yaml(&text)?;
        // Spec 0315 S10/S11: before `retain_resolvable`, which is what
        // would otherwise drop every entry naming one of these. It
        // *refers* to the types rather than re-declaring them, so S4's
        // already-exists refusal is not applied: if the real type has
        // since appeared in the descriptor set, using it is the better
        // outcome, and the descriptor-set hash mismatch below already
        // tells the reader the set has moved.
        for fqdn in &created_types {
            let _ = self.ctx.declare_type(fqdn);
        }
        let dropped = collection.retain_resolvable(|origin| self.origin_resolves(origin));
        let mut warnings = Vec::new();
        let mut hash_mismatch = false;
        // Spec 0354 S4: a targetless file (both hashes empty) is explicitly
        // portable — skip all hash comparisons, no warnings emitted.
        if !target.blob_sha256.is_empty() || !target.descriptor_set_sha256.is_empty() {
            let (blob_sha256, descriptor_set_sha256) = self.target_hashes()?;
            if target.blob_sha256 != blob_sha256 {
                warnings.push("blob hash mismatch".to_string());
                hash_mismatch = true;
            }
            if target.descriptor_set_sha256 != descriptor_set_sha256 {
                warnings.push("descriptor-set hash mismatch".to_string());
                hash_mismatch = true;
            }
        }
        // Spec 0221 S6: dropped entries used to vanish without a word,
        // which reads exactly like a file that was applied. Reported
        // through the warnings both callers already print or show,
        // rather than a channel of its own — this is the same class of
        // notice as the two hash mismatches above.
        if dropped > 0 {
            warnings.push(format!(
                "{dropped} override(s) dropped: their origin does not resolve against this blob"
            ));
        }
        // The file is external input, and nothing in it enforces the
        // per-origin invariant that every in-process mutator maintains
        // (see `enforce_single_active`). A hand-merged file with two
        // active entries for one origin otherwise loads into a state no
        // downstream code is written for, and the node resolves to no
        // override at all while the pane shows both entries checked.
        // Reported for the same reason the drop above is: a repair the
        // user did not ask for is one they should hear about.
        let deactivated = collection.enforce_single_active();
        if deactivated > 0 {
            warnings.push(format!(
                "{deactivated} override(s) deactivated: an origin cannot have more than one \
                 active entry"
            ));
        }
        // The document root's own type is external input (CLI `--type`,
        // auto-inference, or an interactive retype) — unlike every other
        // node, it's never re-derivable from the schema once lost, since
        // `natural_type` infers a node's type by walking up to its
        // *parent's* resolved field descriptor, and root has no parent.
        // It must therefore survive a wholesale collection replace as a
        // persistent baseline entry — otherwise root (and, transitively,
        // every schema-typed descendant whose own `natural_type` walks
        // back up through it) silently reverts to raw rendering, even
        // though the loaded file's own explicit overrides are all
        // individually intact. Preserve the currently-resolved root type
        // unless the loaded file defines its own active root entry.
        let root_origin = OverrideOrigin::Path {
            path: "/".to_string(),
        };
        let has_root_entry = collection
            .entries()
            .iter()
            .any(|e| e.active && e.origin == root_origin);
        let current_root_type = self.resolve_active_override(self.first_node).flatten();
        self.overrides = collection;
        if !has_root_entry {
            self.overrides.seed_root(current_root_type);
        }
        self.render_overrides(self.first_node);
        self.set_manage_highlight(0);
        self.manage_scroll = PaneScroll::default();
        self.last_manage_highlight = None;
        self.manage_pan_offset = 0;
        Ok(OverrideLoad {
            warnings,
            hash_mismatch,
        })
    }

    /// `:restore-overrides <path>` (spec 0117 §4, spec 0354 S1): replaces
    /// the collection wholesale with `<path>`'s contents — see `load_overrides`.
    pub(super) fn run_restore_overrides(&mut self, args: Vec<&str>) {
        let Some(path) = self.require_arg(&args, "restore-overrides: missing path") else {
            return;
        };
        self.message = match self.load_overrides(&path) {
            Ok(load) if load.warnings.is_empty() => format!("loaded overrides from {path}"),
            Ok(load) => format!(
                "loaded overrides from {path} (warning: {})",
                load.warnings.join(", ")
            ),
            Err(e) => format!("restore-overrides error: {e}"),
        };
    }
}

/// Char index of the start of the whitespace-delimited word `from` sits in
/// or just after: skip any whitespace immediately behind it, then skip back
/// over the non-whitespace run before it.
///
/// Spec 0199 S8: shared with the main pane's `Alt`-arrow caret motion
/// rather than restated there — a main pane that broke words differently
/// from the `:` prompt on the same screen would be a defect, not a feature.
pub(super) fn prev_word_boundary(chars: &[char], from: usize) -> usize {
    let mut i = from.min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// Char index just past the end of the whitespace-delimited word `from`
/// sits in or just before: skip any whitespace at/after it, then skip
/// forward to the end of the non-whitespace run.
///
/// The mirror of `prev_word_boundary`; see its note on sharing.
pub(super) fn next_word_boundary(chars: &[char], from: usize) -> usize {
    let len = chars.len();
    let mut i = from.min(len);
    while i < len && chars[i].is_whitespace() {
        i += 1;
    }
    while i < len && !chars[i].is_whitespace() {
        i += 1;
    }
    i
}
