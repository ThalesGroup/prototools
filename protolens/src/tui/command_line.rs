// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

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
        match key.code {
            KeyCode::Tab if self.command_kind == CommandLineKind::Command => {
                self.handle_tab_key(true)
            }
            KeyCode::BackTab if self.command_kind == CommandLineKind::Command => {
                self.handle_tab_key(false)
            }
            KeyCode::Enter => {
                let buf = self.command_buffer.take().unwrap_or_default();
                self.command_cursor = 0;
                match self.command_kind {
                    CommandLineKind::Command => self.run_command(&buf),
                    // Vim convention: `/`/`?` confirmed with an empty
                    // pattern re-uses the last active pattern, searching
                    // in the newly chosen direction (which may differ
                    // from the direction that pattern was originally
                    // searched in — unlike `n`, which always repeats in
                    // the same direction as last time). Which pane's
                    // search actually runs is determined by whichever
                    // pane has focus right now — `override_focus`/
                    // `manage_focus` are untouched by typing into
                    // `command_buffer`. All three panes share the main
                    // pane's bar (spec 0114 §4/0117 §3).
                    CommandLineKind::Search(dir) if self.override_focus => {
                        let pattern = if buf.is_empty() {
                            self.last_override_search
                                .as_ref()
                                .map(|(_, p)| p.clone())
                                .unwrap_or(buf)
                        } else {
                            buf
                        };
                        self.last_override_search = Some((dir, pattern.clone()));
                        self.jump_to_override_match(dir, &pattern);
                    }
                    CommandLineKind::Search(dir) if self.manage_open && self.manage_focus => {
                        let pattern = if buf.is_empty() {
                            self.last_manage_search
                                .as_ref()
                                .map(|(_, p)| p.clone())
                                .unwrap_or(buf)
                        } else {
                            buf
                        };
                        self.last_manage_search = Some((dir, pattern.clone()));
                        self.jump_to_manage_match(dir, &pattern);
                    }
                    CommandLineKind::Search(dir) => {
                        let pattern = if buf.is_empty() {
                            self.last_search
                                .as_ref()
                                .map(|(_, p)| p.clone())
                                .unwrap_or(buf)
                        } else {
                            buf
                        };
                        self.last_search = Some((dir, pattern.clone()));
                        self.jump_to_match(dir, &pattern);
                    }
                }
            }
            KeyCode::Esc => {
                self.command_buffer = None;
                self.command_cursor = 0;
            }
            // Emacs/readline-style char motion (vim's own command-line mode
            // supports these too): Ctrl-f/Ctrl-b as alternatives to the
            // plain arrow keys below.
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let len = self.command_buffer_char_len();
                self.command_cursor = (self.command_cursor + 1).min(len);
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.command_cursor = self.command_cursor.saturating_sub(1)
            }
            // Word motion: Alt-f/Alt-b and their Alt-Left/Alt-Right
            // equivalents. Must precede the plain Left/Right arms below
            // since match arms are checked in order.
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.command_cursor = self.next_word_boundary();
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                self.command_cursor = self.next_word_boundary();
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.command_cursor = self.prev_word_boundary();
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                self.command_cursor = self.prev_word_boundary();
            }
            KeyCode::Left => self.command_cursor = self.command_cursor.saturating_sub(1),
            KeyCode::Right => {
                let len = self.command_buffer_char_len();
                self.command_cursor = (self.command_cursor + 1).min(len);
            }
            KeyCode::Home => self.command_cursor = 0,
            KeyCode::End => self.command_cursor = self.command_buffer_char_len(),
            KeyCode::Backspace => {
                let empty = match &self.command_buffer {
                    Some(buf) => buf.is_empty(),
                    None => true,
                };
                if empty {
                    self.command_buffer = None;
                    self.command_cursor = 0;
                } else if self.command_cursor > 0 {
                    self.command_cursor -= 1;
                    self.remove_char_at(self.command_cursor);
                }
            }
            KeyCode::Delete => {
                if self.command_cursor < self.command_buffer_char_len() {
                    self.remove_char_at(self.command_cursor);
                }
            }
            KeyCode::Char(c) => {
                let byte_idx = self.char_byte_index(self.command_cursor);
                if let Some(buf) = self.command_buffer.as_mut() {
                    buf.insert(byte_idx, c);
                }
                self.command_cursor += 1;
            }
            _ => {}
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
    /// (the command name, before any space) always; the second token, once
    /// exactly one space precedes the cursor, only when the first token
    /// has already unambiguously resolved to `type-as` (spec 0114 §7) — an
    /// FQDN argument, completed against `all_type_fqdns`. Anywhere else
    /// (past `type-as`'s single argument, or a second token following any
    /// other command) is a silent no-op.
    pub(super) fn start_tab_completion(&mut self) {
        let buf = self.command_buffer.clone().unwrap_or_default();
        let cursor_byte = self.char_byte_index(self.command_cursor);
        let prefix = &buf[..cursor_byte];
        match prefix.split_once(' ') {
            None => self.complete_command_name(prefix),
            Some((cmd, arg_prefix))
                if !arg_prefix.contains(' ') && resolve_command(cmd) == Ok("type-as") =>
            {
                self.complete_type_as_fqdn(cmd, arg_prefix);
            }
            Some((cmd, arg_prefix))
                if matches!(resolve_command(cmd), Ok("save") | Ok("restore")) =>
            {
                self.complete_fs_path(cmd, arg_prefix);
            }
            Some((cmd, arg_prefix)) if resolve_command(cmd) == Ok("proto-root") => {
                self.complete_dir_path(cmd, arg_prefix);
            }
            Some(_) => {}
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

    /// `:type-as <FQDN>`'s argument completion (spec 0114 §7) — candidates
    /// are `all_type_fqdns` (the same session-global, lexicographically-
    /// sorted list §3.2/§6 already compute and cache), reused here rather
    /// than recomputed, plus (spec 0135 §G4) the primitive type keywords
    /// wire-compatible with the cursor node's current wire type (a packed
    /// element's own effective wire type is always `WT_LEN`, per its
    /// reconstructed record — spec 0135 §G1).
    pub(super) fn complete_type_as_fqdn(&mut self, cmd: &str, arg_prefix: &str) {
        let wire_type = decode::effective_wire_type(&self.tree[self.cursor].span);
        // Collected into owned `String`s upfront (rather than borrowing
        // `self.all_type_fqdns` for `matches`'s lifetime) so the
        // subsequent `self.replace_token`/`self.completion = ...` calls
        // below aren't blocked by a live immutable borrow of `self`.
        let candidates = decode::primitive_keywords_for_wire_type(wire_type)
            .iter()
            .copied()
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
        let token_start = cmd.chars().count() + 1;
        self.apply_completion(token_start, arg_prefix.chars().count(), matches);
    }

    /// `:save`/`:restore`'s argument completion (spec
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
    fn apply_completion(&mut self, token_start: usize, prefix_len: usize, candidates: Vec<String>) {
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
            // `:quit` (or any unambiguous prefix, e.g. `:q` — no other
            // command starts with `q`) quits directly, same effect as
            // confirming `q` twice: typing the full command out is
            // itself the deliberate action `qq`'s second press
            // otherwise confirms.
            Ok("quit") => self.should_quit = true,
            Ok("type-as") => self.run_type_as(tokens.collect()),
            Ok("type-as-raw") => self.run_type_as_raw(),
            Ok("save") => self.run_save_overrides(tokens.collect()),
            Ok("restore") => self.run_restore_overrides(tokens.collect()),
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

    /// `type-as <FQDN>` — apply `FQDN` as the cursor node's type override,
    /// bypassing the override pane entirely (spec 0114 Goal 4/§5/§7). Same
    /// validation/application as picking a ranked candidate from the pane.
    pub(super) fn run_type_as(&mut self, args: Vec<&str>) {
        let Some(fqdn) = self.require_arg(&args, "type-as: missing type FQDN") else {
            return;
        };
        self.message = match self.type_as(Some(&fqdn)) {
            Ok(()) => format!("overridden as {fqdn}"),
            Err(e) => e,
        };
    }

    /// `type-as-raw` — mark the cursor node's range as explicitly raw/
    /// unschema'd (spec 0114 §3.1/§5/§7), bypassing the override pane.
    pub(super) fn run_type_as_raw(&mut self) {
        self.message = match self.type_as(None) {
            Ok(()) => "overridden as raw".to_string(),
            Err(e) => e,
        };
    }

    /// Shared application logic for `type-as`/`type-as-raw` (spec 0114 §5
    /// step 1, spec 0118 §6): validates the cursor is on an eligible node
    /// (`can_override`, §1) — same refusal `t` gives — then activates a
    /// `Path`-kind override for the cursor node's positional path and
    /// runs the recursive `render_overrides` pass (§4/§6), without ever
    /// opening the override pane. The override is persisted in the
    /// collection, not spliced one-shot.
    ///
    /// A primitive type keyword (spec 0135 §G3/§G4) is rejected upfront,
    /// before `render_overrides` runs, when it isn't wire-compatible with
    /// the cursor node's current wire type (a packed element's own
    /// effective wire type is always `WT_LEN`, per its reconstructed
    /// record — spec 0135 §G1). Catching it early is what makes it a
    /// clear message-line error instead of being masked by
    /// `run_type_as`'s own success message. A message-FQDN target falls
    /// back to deferred resolution inside `splice_override`.
    pub(super) fn type_as(&mut self, new_fqdn: Option<&str>) -> Result<(), String> {
        if !self.can_override(self.cursor) {
            return Err(
                "cannot override: not a message/group or length-delimited field".to_string(),
            );
        }
        if let Some(name) = new_fqdn {
            if decode::primitive_type_for_keyword(name).is_some() {
                let wire_type = decode::effective_wire_type(&self.tree[self.cursor].span);
                if !decode::primitive_keywords_for_wire_type(wire_type).contains(&name) {
                    return Err(format!(
                        "type '{name}' not wire-compatible with this node's wire type"
                    ));
                }
            }
        }
        let origin = OverrideOrigin::Path {
            path: self.positional_path(self.cursor),
        };
        self.overrides.activate(origin, new_fqdn.map(String::from));
        self.render_overrides(self.first_node);
        Ok(())
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
        if !span.is_message {
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

    /// Propose a default `:save` path — same directory/stem as
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
    /// pool and that message to declare the given field number.
    pub(super) fn origin_resolves(&mut self, origin: &OverrideOrigin) -> bool {
        match origin {
            OverrideOrigin::Path { path } => self.resolve_path(path).is_some(),
            OverrideOrigin::PathField { path, field } => self
                .resolve_path(path)
                .is_some_and(|idx| self.children_with_field(idx, *field).next().is_some()),
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
        let Some(path) = self.require_arg(&args, "save: missing path") else {
            return;
        };
        let (blob_sha256, descriptor_set_sha256) = match self.target_hashes() {
            Ok(hashes) => hashes,
            Err(e) => {
                self.message = format!("save error: {e}");
                return;
            }
        };
        let yaml = self.overrides.to_yaml(blob_sha256, descriptor_set_sha256);
        match write_atomically(Path::new(&path), yaml.as_bytes()) {
            Ok(()) => self.message = format!("saved overrides to {path}"),
            Err(e) => self.message = format!("save error: {e}"),
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

    /// Shared core of `restore`/batch `--load-overrides` (spec
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
        let (mut collection, target) = override_pane::OverrideCollection::from_yaml(&text)?;
        let dropped = collection.retain_resolvable(|origin| self.origin_resolves(origin));
        let (blob_sha256, descriptor_set_sha256) = self.target_hashes()?;
        let mut warnings = Vec::new();
        let mut hash_mismatch = false;
        if target.blob_sha256 != blob_sha256 {
            warnings.push("blob hash mismatch".to_string());
            hash_mismatch = true;
        }
        if target.descriptor_set_sha256 != descriptor_set_sha256 {
            warnings.push("descriptor-set hash mismatch".to_string());
            hash_mismatch = true;
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
        self.manage_scroll = 0;
        self.last_manage_highlight = None;
        self.manage_pan_offset = 0;
        Ok(OverrideLoad {
            warnings,
            hash_mismatch,
        })
    }

    /// `restore <path>` (spec 0117 §4): replaces the collection wholesale
    /// with `<path>`'s contents — see `load_overrides`.
    pub(super) fn run_restore_overrides(&mut self, args: Vec<&str>) {
        let Some(path) = self.require_arg(&args, "restore: missing path") else {
            return;
        };
        self.message = match self.load_overrides(&path) {
            Ok(load) if load.warnings.is_empty() => format!("restored overrides from {path}"),
            Ok(load) => format!(
                "restored overrides from {path} (warning: {})",
                load.warnings.join(", ")
            ),
            Err(e) => format!("restore error: {e}"),
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
