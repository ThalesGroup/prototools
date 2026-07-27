// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

use prototext_core::serialize::encode_text::annotation_start;

/// Stand-in for a row that has no styles at all.
static NO_STYLES: LineStyles = Vec::new();

/// Spec 0174 §S4's truncation marker, trimmed. Not prototext (see
/// `window_text`).
const TRUNCATION_MARKER: &str = "...";

/// The activity dot's glyph (spec 0190 S5/S6) — deliberately its own
/// constant rather than a reuse of `heat_cue::HEAT_GLYPH`, which carries
/// its own meaning (this row's inferred type disagrees with the
/// assigned one). The two say unrelated things and must be free to
/// diverge; that they currently share a character is a separate,
/// narrower fact.
///
/// That shared character is not an aesthetic choice: `●` and its
/// plausible alternatives are East-Asian Ambiguous width, so in a
/// CJK-configured terminal they render double-width and would overflow
/// the dot's one-cell slot. Picking the glyph the app already depends on
/// everywhere means the dot cannot break in a configuration the heat
/// cues survive.
pub(super) const ACTIVITY_GLYPH: &str = "●";

/// A rendered line with its trailing `#@ ...` annotation removed (spec
/// 0187 S1) — the part of the line the *format* considers a value.
/// Used both for the display-time annotation hiding of spec 0133 G4 and,
/// in `window_styles_for`, for depth arithmetic that must not be
/// confused by a brace inside an annotation.
fn code_part(line: &str) -> &str {
    match annotation_start(line) {
        Some(pos) => &line[..pos],
        None => line,
    }
}

/// Spec 0187 S3: syntax hints for one window's worth of rendered lines,
/// parsed *inside a synthetic enclosing context* so that each row is
/// highlighted at its true nesting depth.
///
/// Parsing a window on its own is not equivalent to parsing it inside
/// the document: a window scrolled into the middle typically starts on a
/// nested field and contains a bare `}` with no matching `{`, which
/// drives tree-sitter into error recovery — and error recovery in this
/// grammar swallows *following* siblings, losing their captures (see
/// `colorize.rs::bare_decimal_field_name_does_not_corrupt_sibling_
/// captures`). Highlighting would visibly degrade whenever the user is
/// not scrolled to the top.
///
/// The repair is cheap because the rendering is deterministic in
/// indentation: `render_text` takes every line's prefix from
/// `push_indent`, which writes exactly `indent_size * level` spaces, and
/// emits no continuation or wrapped lines. So the window's own opening
/// depth is readable off its first line, and the depth it leaves open is
/// that plus the net brace delta. Wrapping the window in that many
/// synthetic `_ {` openers and `}` closers makes it a syntactically
/// complete document; the synthetic rows' own buckets are then dropped.
///
/// `render_text` emits `{`/`}` as its only message delimiters — the
/// grammar also has `<`/`>` and multi-line `[...]`, so the brace rule
/// below is complete for this renderer rather than general.
pub(super) fn window_styles_for(text: &[String], indent_size: usize) -> Vec<LineStyles> {
    let indent_size = indent_size.max(1);
    // Blank rows carry no indentation to read a depth from — `window_
    // text` blanks non-grammar rows, and a comment-only line's value
    // part is empty by branch 2 of the annotation rule. Skipping to the
    // first row that does carry one is exact except when the window
    // *starts* on such a row, where it is off by at most the difference
    // between that row's depth and the next's.
    let d0 = text
        .iter()
        .map(|l| code_part(l))
        .find(|c| !c.trim().is_empty())
        .map_or(0, |c| {
            let indent = c.len() - c.trim_start().len();
            indent / indent_size + usize::from(c.trim_start().starts_with('}'))
        });
    let mut depth = d0 as isize;
    for line in text {
        let c = code_part(line).trim();
        if c.ends_with('{') {
            depth += 1;
        } else if c == "}" {
            depth -= 1;
        }
    }
    let dn = depth.max(0) as usize;

    let mut framed: Vec<String> = Vec::with_capacity(d0 + text.len() + dn);
    for level in 0..d0 {
        framed.push(format!("{}_ {{", " ".repeat(level * indent_size)));
    }
    framed.extend_from_slice(text);
    for level in (0..dn).rev() {
        framed.push(format!("{}}}", " ".repeat(level * indent_size)));
    }
    let joined = framed.join("\n");
    let mut buckets = colorize::hints_by_line(&framed, &colorize::colorize(&joined));
    buckets.truncate(buckets.len() - dn);
    buckets.drain(..d0);
    buckets
}

impl App {
    /// Spec 0185 S2: total number of rows the main pane draws — the
    /// committed `visible_rows`, adjusted by however many rows the
    /// preview overlay adds or removes.
    pub(super) fn composed_row_count(&self) -> usize {
        match &self.preview_overlay {
            Some(o) => (self.visible_rows.len() + o.lines.len()) - o.covered_rows,
            None => self.visible_rows.len(),
        }
    }

    /// Spec 0185 S2: resolve a display row to the line it draws. Exactly
    /// one contiguous span of rows is ever substituted, so this is
    /// arithmetic rather than a rebuilt vector. `None` past the last row.
    pub(super) fn display_row(&self, d: usize) -> Option<DisplayRow> {
        let committed = |l: usize| self.visible_rows.get(l).copied().map(DisplayRow::Committed);
        let Some(o) = &self.preview_overlay else {
            return committed(d);
        };
        if d < o.first_row {
            committed(d)
        } else if d < o.first_row + o.lines.len() {
            Some(DisplayRow::Overlay(d - o.first_row))
        } else {
            committed((d - o.lines.len()) + o.covered_rows)
        }
    }

    /// Spec 0185 S2: `cursor_display_row()` carried into composed row
    /// space, which is what the render loop's `REVERSED` comparison
    /// needs — it compares against rows it is drawing, not against
    /// `visible_rows`. A cursor inside the block the overlay stands in
    /// for (the usual case: the preview's target *is* the node the
    /// cursor is on) resolves to the overlay's own first row.
    pub(super) fn cursor_composed_row(&self) -> usize {
        let row = self.cursor_display_row();
        let Some(o) = &self.preview_overlay else {
            return row;
        };
        if row < o.first_row {
            row
        } else if row < o.first_row + o.covered_rows {
            o.first_row
        } else {
            (row + o.lines.len()) - o.covered_rows
        }
    }

    /// Spec 0185 S2: the `(content, node)` pair a display row draws
    /// from — the single point where committed and overlay rows
    /// converge, so that everything downstream of it is shared. An
    /// overlay row has no node (S4): no heat cue, no active-override
    /// hint, no fold marker, no selection.
    fn display_row_source(&self, row: DisplayRow) -> (&str, Option<usize>) {
        match row {
            DisplayRow::Committed(l) => (
                self.lines.get(l).map(String::as_str).unwrap_or(""),
                self.node_at_header_line(l),
            ),
            DisplayRow::Overlay(i) => {
                let o = self
                    .preview_overlay
                    .as_ref()
                    .expect("an Overlay row is only ever produced while an overlay is held");
                (&o.lines[i], None)
            }
        }
    }

    /// Spec 0187 S2: the rows currently on screen, as the highlighter
    /// sees them — the committed line or overlay line each `DisplayRow`
    /// draws, untruncated and unfolded (fold markers and the annotation
    /// transform are display insertions applied downstream, and must not
    /// reach the parser).
    ///
    /// Rows that are not prototext at all are emitted as `""`. `lines`
    /// is not purely prototext: spec 0174 §S4's `...` truncation marker
    /// is a literal row in a truncated preview's lines, and `...` is not
    /// in the grammar. That used to be harmless because the marker was
    /// spliced in after `colorize()` had already run; highlighting at
    /// draw time removes that protection, and a syntax error here would
    /// silently strip the color off every row *beneath* the marker via
    /// tree-sitter's error recovery. Substituting a blank line rather
    /// than dropping the row keeps the result index-parallel with
    /// `window`, so the row's own bucket comes out empty by construction
    /// and no index surgery is needed.
    fn window_text(&self, window: &[DisplayRow]) -> Vec<String> {
        window
            .iter()
            .map(|&row| {
                let line = self.display_row_source(row).0;
                if line.trim() == TRUNCATION_MARKER {
                    String::new()
                } else {
                    line.to_string()
                }
            })
            .collect()
    }

    /// Spec 0187 S3: recompute `window_styles` for the rows this frame
    /// is about to draw. Called once per `render()`, from `render()`
    /// only — the result is never retained across frames and is never
    /// document-sized.
    pub(super) fn refresh_window_styles(&mut self, window: &[DisplayRow]) {
        let text = self.window_text(window);
        self.window_styles = window_styles_for(&text, self.indent_size);
    }

    /// A foldable node's line, with its fold marker inserted right after
    /// the line's own leading indentation (kept intact — not shortened by
    /// one column to make room) and immediately before the first
    /// non-blank token, with no extra space either side — see
    /// `marker_column`. Lines with no associated foldable node are
    /// returned unchanged.
    ///
    /// When `self.annotations` is off, the line's trailing `#@ ...`
    /// annotation (and the whitespace that used to separate it from the
    /// value) is hidden — a purely cosmetic, display-time transform (spec
    /// 0133 G4); the underlying `self.lines` always carries the full
    /// annotation regardless.
    pub(super) fn render_line_content(&self, line_idx: usize) -> String {
        self.row_content(DisplayRow::Committed(line_idx))
    }

    /// Spec 0185 S2: `render_line_content` for any display row, overlay
    /// rows included — there must not be a second line-rendering path.
    pub(super) fn row_content(&self, row: DisplayRow) -> String {
        let (full_content, node) = self.display_row_source(row);
        let content = if !self.annotations {
            code_part(full_content)
        } else {
            full_content
        };
        let Some(idx) = node else {
            return content.to_string();
        };
        if !self.has_children(idx) {
            return content.to_string();
        }
        let folded = self.folded.contains(&idx);
        let marker = if folded { '▸' } else { '▾' };
        let indent_len = content.len() - content.trim_start().len();
        let mut s = format!(
            "{}{marker}{}",
            &content[..indent_len],
            &content[indent_len..]
        );
        if folded {
            match s.rfind('{') {
                Some(pos) => s.insert_str(pos + 1, " ... }"),
                None => s.push_str(" ... }"),
            }
        }
        s
    }

    /// Styled counterpart of `row_content` (spec 0116 §7/§9): applies the
    /// row's syntax-highlighting spans via `theme::style_for`, then
    /// splices in the same fold-marker / `" ... }"` collapse-summary text
    /// `row_content` inserts — as unstyled spans, so highlighting and
    /// folding compose cleanly.
    ///
    /// Follows the same display-time annotation-hiding truncation as
    /// `row_content` (spec 0133 G4) — any hint extending past the
    /// truncated length is clipped/dropped before `segment_line` runs,
    /// since `segment_line` doesn't bounds-check hint ranges against
    /// `content`.
    ///
    /// Spec 0185 S2: takes a display row, so overlay rows come through
    /// here too. **There must not be a second line-rendering path** — a
    /// preview and the commit that follows it are required to be
    /// byte-identical (G3), and both reaching this same function is how
    /// that is met.
    ///
    /// Spec 0187 S3: `window_index` is the row's position in the window
    /// `refresh_window_styles` was last called with, which is the
    /// coordinate system `self.window_styles` lives in.
    pub(super) fn row_spans(&self, row: DisplayRow, window_index: usize) -> Vec<Span<'static>> {
        let (full_content, node) = self.display_row_source(row);
        let full_hints = self.window_styles.get(window_index).unwrap_or(&NO_STYLES);
        let (content, hints): (&str, LineStyles) =
            match (!self.annotations, annotation_start(full_content)) {
                (true, Some(pos)) => {
                    let truncated = &full_content[..pos];
                    let clipped = full_hints
                        .iter()
                        .filter(|(r, _)| r.start < truncated.len())
                        .map(|(r, role)| (r.start..r.end.min(truncated.len()), *role))
                        .collect();
                    (truncated, clipped)
                }
                _ => (full_content, full_hints.to_vec()),
            };
        let segments = segment_line(content, &hints);

        let Some(idx) = node else {
            return self.spans_with_insertions(content, segments, Vec::new());
        };
        if !self.has_children(idx) {
            return self.spans_with_insertions(content, segments, Vec::new());
        }
        let folded = self.folded.contains(&idx);
        let marker = if folded { '▸' } else { '▾' };
        let indent_len = content.len() - content.trim_start().len();

        let mut insertions = vec![(indent_len, marker.to_string())];
        if folded {
            let insert_at = match content.rfind('{') {
                Some(pos) => pos + 1,
                None => content.len(),
            };
            insertions.push((insert_at, " ... }".to_string()));
        }
        self.spans_with_insertions(content, segments, insertions)
    }

    /// Turns `content`'s `segments` (byte ranges tagged with an optional
    /// `SyntaxRole`, covering all of `content`) into styled `Span`s,
    /// splicing in `insertions` — `(byte position in content, literal
    /// text)` pairs, each rendered as its own unstyled `Span` at that
    /// point (fold-marker/collapse-summary text is never part of the
    /// highlighted source, so it never carries a role).
    pub(super) fn spans_with_insertions(
        &self,
        content: &str,
        segments: Vec<(Range<usize>, Option<SyntaxRole>)>,
        mut insertions: Vec<(usize, String)>,
    ) -> Vec<Span<'static>> {
        insertions.sort_by_key(|(pos, _)| *pos);
        let mut segments: std::collections::VecDeque<_> = segments.into();
        let mut result = Vec::new();
        for (ins_pos, ins_text) in insertions {
            while let Some((range, role)) = segments.pop_front() {
                if range.end <= ins_pos {
                    result.push(self.make_span(content[range].to_string(), role));
                } else if range.start < ins_pos {
                    result.push(self.make_span(content[range.start..ins_pos].to_string(), role));
                    segments.push_front((ins_pos..range.end, role));
                    break;
                } else {
                    segments.push_front((range, role));
                    break;
                }
            }
            result.push(Span::raw(ins_text));
        }
        for (range, role) in segments {
            result.push(self.make_span(content[range].to_string(), role));
        }
        result
    }

    pub(super) fn make_span(&self, text: String, role: Option<SyntaxRole>) -> Span<'static> {
        match role {
            Some(role) => Span::styled(text, theme::style_for(role, self.theme)),
            None => Span::raw(text),
        }
    }

    /// Spec 0113 D33: the node `line_idx` is one of the *own*
    /// header/footer lines of (`line_to_node`'s opening-line mapping, or
    /// `footer_line_to_node`'s closing-line mapping) — never a node one
    /// of whose descendants owns the line, which is what keeps the
    /// active-override weight from cascading down a whole overridden
    /// subtree.
    ///
    /// Spec 0192 S2: this used to be `line_has_active_override`, which
    /// folded the `resolve_active_override` call in. `render`'s hoisted
    /// override pass needs the node itself, so it can skip re-resolving
    /// when consecutive rows belong to one node.
    pub(super) fn node_at_own_line(&self, line_idx: usize) -> Option<usize> {
        self.node_at_header_line(line_idx)
            .or_else(|| self.node_at_footer_line(line_idx))
    }

    /// Spec 0192 S2: which of `window`'s rows draw in bold because the
    /// node they belong to carries an active override, plus how many
    /// `resolve_active_override` calls that took.
    ///
    /// Hoisted out of `render`'s `text_lines` closure into a pass of its
    /// own — same shape and position as the heat-cue pass (spec 0154 G6)
    /// and the highlighting pass (spec 0187 S2), and for the same borrow
    /// reason.
    ///
    /// Consecutive rows resolving to one *record* are resolved once. In
    /// practice that means a packed run: its N element rows are N
    /// distinct nodes but one addressable record (spec 0184 S2), so they
    /// share one positional path and therefore one answer. Rows of one
    /// node are not otherwise adjacent — a message's header and footer
    /// rows have its whole subtree between them — so a one-entry memo is
    /// all the collapsing there is to do here.
    ///
    /// The returned count is what makes that claim testable rather than
    /// merely asserted.
    pub(super) fn override_bold_flags(&self, window: &[DisplayRow]) -> (Vec<bool>, usize) {
        let mut resolutions = 0;
        let mut last: Option<(usize, bool)> = None;
        let flags = window
            .iter()
            .map(|&row| {
                let DisplayRow::Committed(line_idx) = row else {
                    return false;
                };
                let Some(idx) = self.node_at_own_line(line_idx) else {
                    return false;
                };
                let reusable = last.filter(|&(seen, _)| {
                    seen == idx
                        || decode::same_packed_record(&self.tree[seen].span, &self.tree[idx].span)
                });
                match reusable {
                    Some((_, answer)) => answer,
                    None => {
                        resolutions += 1;
                        let answer = self.resolve_active_override(idx).is_some();
                        last = Some((idx, answer));
                        answer
                    }
                }
            })
            .collect();
        (flags, resolutions)
    }

    /// Auto-dismiss `self.message` after `MESSAGE_TIMEOUT` of it staying
    /// unchanged — otherwise a passive status/error notice (e.g. "pattern
    /// not found") stays on screen indefinitely once set, even while the
    /// user is just navigating a side pane with nothing left to say about
    /// it. `self.message` has no dedicated setter (assigned directly all
    /// over this file), so a freshly-set message is detected here by
    /// comparing against `last_message_seen` rather than at each
    /// assignment site. Never dismissed while `command_buffer`/
    /// `manage_rename` is `Some` (the global command/message row renders
    /// those instead of `self.message` while either is active — see
    /// `render`'s `cmd_text`) or while `quit_confirm` is armed (both are
    /// actively awaiting a keypress, unlike a plain notice). Called once
    /// per `render()`.
    pub(super) fn track_message_timeout(&mut self) {
        if self.message != self.last_message_seen {
            self.last_message_seen = self.message.clone();
            self.message_deadline = if self.message.is_empty() {
                None
            } else {
                Some(Instant::now() + MESSAGE_TIMEOUT)
            };
            return;
        }
        if self.command_buffer.is_some() || self.manage_rename.is_some() || self.quit_confirm {
            return;
        }
        if let Some(deadline) = self.message_deadline {
            if Instant::now() >= deadline {
                self.message.clear();
                self.last_message_seen.clear();
                self.message_deadline = None;
            }
        }
    }

    /// Auto-dismiss the startup splash after `SPLASH_TIMEOUT` (item 13 of
    /// 2026-07-17 feedback), in addition to its existing keypress/mouse
    /// dismissal. Called once per `render()`, mirroring
    /// `track_message_timeout`'s deadline-based approach.
    fn track_splash_timeout(&mut self) {
        if self.splash && Instant::now() >= self.splash_deadline {
            self.splash = false;
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        self.track_message_timeout();
        self.track_splash_timeout();
        let area = frame.area();
        self.term_width = area.width;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),    // main pane + side pane, each with its own local statusline
                Constraint::Length(1), // global command/message row (spec 0147 G4)
            ])
            .split(area);

        // Ephemeral right-hand split (spec 0114 §2, extended by spec 0117
        // §3 to the management pane) when either the override selection
        // pane or the management pane is open — 50/50 (minus the
        // separator column), giving the candidate/entry list enough room
        // to be legible. The two panes are mutually exclusive (spec 0117
        // §3), so at most one of these is ever true. A single `'│'`-filled
        // `Length(1)` separator column stands in for the border that used
        // to divide the two panes (spec 0147 G3).
        let (main_outer, separator_outer, right_outer) =
            if self.override_target.is_some() || self.manage_open {
                let split = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(50),
                        Constraint::Length(1),
                        Constraint::Percentage(50),
                    ])
                    .split(chunks[0]);
                (split[0], Some(split[1]), Some(split[2]))
            } else {
                (chunks[0], None, None)
            };

        if let Some(separator_area) = separator_outer {
            // Spec 0147 G3: one fixed, neutral style — not focus-colored,
            // unlike the two panes it divides.
            let separator_style = Style::default().fg(Color::DarkGray);
            let separator_lines: Vec<Line> = (0..separator_area.height)
                .map(|_| Line::styled("│", separator_style))
                .collect();
            frame.render_widget(Paragraph::new(separator_lines), separator_area);
        }

        // `pane_focus_style` marks whichever pane currently holds keyboard
        // focus, shared with the override/management panes' own
        // `render_override_pane`/`render_manage_pane` — the main pane has
        // focus exactly when neither side pane does (2026-07-14 feedback:
        // no prior visible sign of which pane focus was in).
        let main_focused = !self.override_focus && !self.manage_focus;
        let main_style = pane_focus_style(main_focused, self.theme);

        // Spec 0147 G1/G2: no border — the main pane's own content splits
        // into a `Min(0)` text area above its own `Length(1)` local
        // statusline row.
        let main_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(main_outer);
        let inner = main_split[0];
        self.main_area = inner;

        let pane_height = inner.height as usize;
        let cursor_row = if self.tree.is_empty() {
            0
        } else {
            self.cursor_display_row()
        };
        // 2026-07-19 feedback item 3: auto-pan into view only on genuine
        // cursor movement (`cursor_row` differs from the last render that
        // clamped), not on every render regardless of cause — otherwise
        // a manual vertical pan (item 1's now-unclamped `pan_vertical_*`)
        // would immediately get fought back into following the cursor.
        if !self.tree.is_empty() && self.last_cursor_row != Some(cursor_row) {
            clamp_scroll_to_visible(&mut self.scroll_offset, cursor_row, pane_height);
            self.last_cursor_row = Some(cursor_row);
        }
        // Spec 0185 S2: the row the cursor is *drawn* on, in composed
        // coordinates — distinct from `cursor_row` above, which stays in
        // `visible_rows` coordinates because that is what
        // `last_cursor_row` (and `clamp_pan_offset`'s matching guard) is
        // compared against. With no overlay the two are equal.
        let cursor_draw_row = if self.tree.is_empty() {
            0
        } else {
            self.cursor_composed_row()
        };
        let total_rows = self.composed_row_count();
        let end = (self.scroll_offset + pane_height).min(total_rows);
        // Collected (not a borrowed slice) so the heat-cue pass below can
        // mutate `self.heat_range_cache`/`self.heat_current_score_cache`
        // — a plain slice would keep `self` borrowed immutably for the
        // rest of this block.
        let t_window = std::time::Instant::now();
        let window: Vec<DisplayRow> = (self.scroll_offset.min(total_rows)..end)
            .filter_map(|d| self.display_row(d))
            .collect();
        let d_window = t_window.elapsed();

        // Spec 0187 S3: highlight exactly the rows about to be drawn,
        // and nothing else. Its own `&mut self` pass, ahead of the
        // immutable-`self` `text_lines` closure below — the same shape
        // and the same reason as the heat-cue pass that follows.
        let t_styles = std::time::Instant::now();
        self.refresh_window_styles(&window);
        let d_styles = t_styles.elapsed();

        // Spec 0129 §G1: the drag-selected `line_idx` range (if any) gets
        // the same `REVERSED` treatment as the single cursor row below —
        // the two can coexist harmlessly since `REVERSED` on an already-
        // `REVERSED` span is a no-op.
        let selection_range = match (self.select_anchor, self.select_end) {
            (Some(a), Some(b)) => Some(a.min(b)..=a.max(b)),
            _ => None,
        };

        // Spec 0138 (item 12, 2026-07-17 feedback), restructured by
        // spec 0154 G6: computed in its own pass, ahead of the
        // (immutable-`self`) `text_lines` closure below, since
        // populating the heat-cue caches needs `&mut self`.
        //
        // Spec 0185 S4: an overlay row has no committed node, so it has
        // no cue. That is correct rather than merely convenient — a cue
        // reports how well a *committed* node's bytes score as its
        // current type, and an overlay row has no committed node.
        let t_heat = std::time::Instant::now();
        let heat_displays: Vec<heat_cue::HeatDisplay> = window
            .iter()
            .map(|&row| match row {
                DisplayRow::Committed(line_idx) => self.heat_cue_for(line_idx),
                DisplayRow::Overlay(_) => heat_cue::HeatDisplay::None,
            })
            .collect();
        let d_heat = t_heat.elapsed();
        let t_ovr = std::time::Instant::now();
        let (row_overridden, _) = self.override_bold_flags(&window);
        let d_ovr = t_ovr.elapsed();
        let t_lines = std::time::Instant::now();

        let text_lines: Vec<Line> = window
            .iter()
            .zip(heat_displays.iter())
            .enumerate()
            .map(|(row, (&display_row, display))| {
                // `None` for an overlay row (spec 0185 S4), which gates
                // the active-override hint and the drag selection below
                // exactly as it already gates fold markers inside
                // `row_spans`.
                let line_idx = match display_row {
                    DisplayRow::Committed(l) => Some(l),
                    DisplayRow::Overlay(_) => None,
                };
                let mut spans = pan_spans(self.row_spans(display_row, row), self.pan_offset);
                if row_overridden[row] {
                    for span in &mut spans {
                        span.style = span.style.add_modifier(Modifier::BOLD);
                    }
                }
                // Leading gutter glyph + trailing suffix (spec 0138 N1,
                // G9, spec 0154 G6) — the glyph column is always
                // reserved (a blank space when absent), so node
                // indentation never shifts; both are appended/prepended
                // after panning, so neither is affected by horizontal
                // scroll. The glyph is shown only for a complete
                // `Cue` — never during a partial/pending state, even
                // when `best` alone is known — and `heat_style` itself
                // returns `None` on the ANSI-16 fallback for a
                // low-confidence `best_score` (G7/G12's narrowing of
                // the gate), in which case no cue shows at all, glyph
                // or suffix.
                let pending_style = || theme::style_for(SyntaxRole::Comment, self.theme);
                match display {
                    heat_cue::HeatDisplay::Cue(c) => {
                        let hue = match c.kind {
                            heat_cue::HeatCueKind::Mismatch { .. } => theme::HeatHue::Red,
                            heat_cue::HeatCueKind::Tie { .. } => theme::HeatHue::Blue,
                        };
                        match theme::heat_style(c.level, hue, self.theme) {
                            Some(style) => {
                                let suffix = match c.kind {
                                    heat_cue::HeatCueKind::Mismatch { current, best } => {
                                        let current = current
                                            .map(|c| c.to_string())
                                            .unwrap_or_else(|| "-".to_string());
                                        Span::styled(
                                            format!(" [{current}/{best}]"),
                                            theme::heat_suffix_style(self.theme),
                                        )
                                    }
                                    heat_cue::HeatCueKind::Tie { tie_count, score } => {
                                        Span::styled(
                                            format!(" [{tie_count}@{score}]"),
                                            theme::style_for(SyntaxRole::Boolean, self.theme),
                                        )
                                    }
                                };
                                spans.push(suffix);
                                spans.insert(0, Span::styled(heat_cue::HEAT_GLYPH, style));
                            }
                            None => spans.insert(0, Span::raw(" ")),
                        }
                    }
                    heat_cue::HeatDisplay::PendingCurrent { best } => {
                        spans.insert(0, Span::raw(" "));
                        spans.push(Span::styled(format!(" [?/{best}]"), pending_style()));
                    }
                    heat_cue::HeatDisplay::Unknown => {
                        spans.insert(0, Span::raw(" "));
                        spans.push(Span::styled(" [?]", pending_style()));
                    }
                    heat_cue::HeatDisplay::None => {
                        spans.insert(0, Span::raw(" "));
                    }
                }
                let selected = line_idx
                    .is_some_and(|l| selection_range.as_ref().is_some_and(|r| r.contains(&l)));
                if self.scroll_offset + row == cursor_draw_row || selected {
                    for span in &mut spans {
                        span.style = span.style.add_modifier(Modifier::REVERSED);
                    }
                }
                Line::from(spans)
            })
            .collect();
        let d_lines = t_lines.elapsed();
        frame.render_widget(Paragraph::new(text_lines), inner);
        trace::trace!(
            "render window_us={} styles_us={} heat_us={} ovr_us={} lines_us={} rows={}",
            d_window.as_micros(),
            d_styles.as_micros(),
            d_heat.as_micros(),
            d_ovr.as_micros(),
            d_lines.as_micros(),
            window.len(),
        );

        // Local statusline (spec 0147 G2): the main pane's own position/
        // selection info, plain-styled (`main_style`, the same accent
        // `pane_focus_style` already chose above) across the whole row —
        // no per-span coloring. The byte-range part of the right-flushed
        // ruler is dropped (not truncated) when the side pane is open,
        // since the main pane is only half-width then and there's rarely
        // enough room for it too — the line number stays either way.
        let path_label = self.blob_path.display();
        let (main_left, main_right) = if self.tree.is_empty() {
            (
                format!("{path_label} (empty — decoded to zero fields)"),
                None,
            )
        } else {
            let node_path = self.positional_path(self.cursor);
            // `status_type_label` returns a bare keyword for a primitive
            // scalar type, or an fqdn for a message/group/enum type — both
            // get a colon separator (2026-07-19/2026-07-20 feedback); the
            // `[tag]` suffix is shown only for the latter, and only in
            // full-width mode, since half-width rarely has room for it.
            let type_label = self.status_type_label(self.cursor);
            let left = match type_label {
                Some((t, Some(tag))) if right_outer.is_none() => {
                    format!("{path_label} {node_path}: {t} [{tag}]")
                }
                Some((t, _)) => format!("{path_label} {node_path}: {t}"),
                None => format!("{path_label} {node_path}"),
            };
            // Spec 0185 S7/Q1: the focus lock is announced in words
            // rather than shown — the cursor is deliberately left
            // unrestyled, so that the main pane looks exactly as it
            // would after a real splice (G3). The notice is tied to the
            // pane being open, not to an overlay existing, so it is
            // shown for a candidate that failed to render too, where
            // the lock still holds but no overlay does.
            let left = if self.override_target.is_some() {
                match self.preview_overlay {
                    Some(_) => format!("{left} - preview (main pane locked)"),
                    None => format!("{left} - main pane locked"),
                }
            } else {
                left
            };
            // The byte-range ruler is dropped (not truncated) once a side
            // pane is open and the main pane is only half-width, since
            // there's rarely enough room for both halves — but the line
            // number is short enough to always fit, so it stays.
            let line_ruler = format!("L{}/{}", self.cursor_line() + 1, self.lines.len());
            let right = if right_outer.is_some() {
                line_ruler
            } else {
                let range = self.display_range(self.cursor);
                format!("[{}..{})  {line_ruler}", range.start, range.end)
            };
            (left, Some(right))
        };
        let main_statusline = main_split[1];
        let main_text = statusline_text(
            &main_left,
            main_right.as_deref(),
            main_statusline.width as usize,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(main_text, main_style)),
            main_statusline,
        );

        // Global command/message row (spec 0147 G4): a single borderless
        // `Length(1)` row, always reserved, shared across every pane —
        // never duplicated per-pane, per the spec's "locality" principle.
        // The management pane's rename buffer (spec 0119 §G4's `f` key)
        // shares this same row rather than being appended inside the side
        // pane's own line list (2026-07-14 interactive feedback): unlike
        // `:command`/`/`-search, that side-pane-local spot never got a
        // real terminal cursor, making it unclear where typing lands —
        // this row already solves that for the main pane's own command/
        // search input, so reusing it fixes both at once.
        const RENAME_PREFIX: &str = "field name: ";
        let cmd_text = match &self.command_buffer {
            Some(buf) => {
                let prefix = match self.command_kind {
                    CommandLineKind::Command => ':',
                    CommandLineKind::Search(SearchDir::Forward) => '/',
                    CommandLineKind::Search(SearchDir::Backward) => '?',
                };
                format!("{prefix}{buf}")
            }
            None => match &self.manage_rename {
                Some(buf) => format!("{RENAME_PREFIX}{buf}"),
                None => self.message.clone(),
            },
        };
        // Spec 0190 S5: column 0 of the global row is reserved for the
        // activity dot, unconditionally — so the command row's geometry
        // never shifts underneath the user mid-edit. Everything
        // downstream (`cmd_area` for mouse hit-testing, `width` for pan
        // clamping, `set_cursor_position`) already derives from
        // `cmd_row`, so re-binding it here is the whole change.
        let global_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(chunks[1]);
        self.render_activity_dot(frame, global_row[0]);
        let cmd_row = global_row[1];
        if cmd_text.is_empty() {
            self.cmd_area = None;
            frame.render_widget(Paragraph::new(""), cmd_row);
        } else {
            self.cmd_area = Some(cmd_row);

            // Spec 0127 §G1: cursor char position (including the leading
            // "prefix"/"field name: " char(s)) within `cmd_text`, `None` while
            // just displaying a plain message (no active edit, so no
            // cursor to keep visible).
            let cursor_pos = if self.command_buffer.is_some() {
                Some(1 + self.command_cursor)
            } else {
                self.manage_rename
                    .as_ref()
                    .map(|buf| RENAME_PREFIX.chars().count() + buf.chars().count())
            };
            let width = cmd_row.width as usize;
            if let Some(pos) = cursor_pos {
                // Auto-follow the cursor while typing (mirrors the main
                // pane's cursor-follow vertical scroll) — coexists with,
                // rather than replaces, manual Shift+wheel/native
                // horizontal-scroll pan on this same field.
                if pos < self.command_pan_offset {
                    self.command_pan_offset = pos;
                } else if width > 0 && pos >= self.command_pan_offset + width {
                    self.command_pan_offset = pos + 1 - width;
                }
            }
            let spans = pan_spans(vec![Span::raw(cmd_text)], self.command_pan_offset);
            frame.render_widget(Paragraph::new(Line::from(spans)), cmd_row);
            if let Some(pos) = cursor_pos {
                let x = cmd_row.x + (pos - self.command_pan_offset) as u16;
                frame.set_cursor_position((x, cmd_row.y));
            }
        }

        if let Some(right_area) = right_outer {
            if self.override_target.is_some() {
                self.render_override_pane(frame, right_area);
            } else if self.manage_open {
                self.render_manage_pane(frame, right_area);
            }
        }

        if self.splash {
            self.render_splash(frame, area);
        } else if self.help_open {
            self.render_help(frame, area);
        }
    }

    /// Spec 0190 S5/S6: one cell reporting the highest-priority tier the
    /// heat-cue subsystem is currently working for — queued or in flight
    /// — or a blank when it has nothing to do.
    ///
    /// Spec 0191 S2: reads `self.activity_shown`, the value `run_loop`
    /// decided from its high-water sample, rather than probing
    /// `heat_activity()` here. A probe taken at draw time cannot see the
    /// requests this very draw is about to queue (`heat_cue_for` runs
    /// per visible row below), which is what made the dot emit one dark
    /// frame per completed request.
    ///
    /// Colors reuse `theme::heat_style`'s existing light/dark-aware
    /// ramps rather than inventing any, and every level is deliberately
    /// at least 4: `heat_style` returns `None` for `level <= 3` on the
    /// ANSI-16 fallback, and a diagnostic that silently vanishes on a 16-color
    /// terminal would be worse than no diagnostic. On that fallback
    /// these three collapse to `LightRed`/`Red`/`Blue` — still three
    /// distinguishable states, still darkening along the priority
    /// order.
    ///
    fn render_activity_dot(&mut self, frame: &mut Frame, area: Rect) {
        let style = self.activity_shown.and_then(|tier| {
            let (hue, level) = match tier {
                tiered::Tier::User => (theme::HeatHue::Red, 12),
                tiered::Tier::Visible => (theme::HeatHue::Red, 5),
                tiered::Tier::Prefetch => (theme::HeatHue::Blue, 5),
            };
            theme::heat_style(level, hue, self.theme)
        });
        let span = match style {
            Some(style) => Span::styled(ACTIVITY_GLYPH, style),
            None => Span::raw(" "),
        };
        frame.render_widget(Paragraph::new(Line::from(span)), area);
    }

    /// Ephemeral right-hand override pane (spec 0114 §2): local statusline
    /// (spec 0147 G2) showing the target's own node path and sort mode,
    /// and the ranked/lexicographic candidate list (§3.2) with the
    /// highlighted row reverse-styled, scrolled to keep it visible. In
    /// alphabetic mode row 0 is always the `None` raw-type candidate
    /// (spec 0137 §G1/§G4) — no more separate pinned row. Each row is
    /// styled by kind (spec 0137 §G8): `None` and a primitive keyword
    /// (default style), an enum FQDN (`Attribute`, with a ` [enum]`
    /// suffix), else a message/group FQDN (unstyled), with G6's
    /// leading-dot collision-avoidance applied to non-sentinel FQDNs. The
    /// `/`/`?` search buffer (§4) renders in the global command/message
    /// row instead of a row here (spec 0147 G4). Apply-on-`Enter` (§5)
    /// lands in a later implementation
    /// step.
    pub(super) fn render_override_pane(&mut self, frame: &mut Frame, area: Rect) {
        let Some(idx) = self.override_target else {
            return;
        };
        let style = pane_focus_style(self.override_focus, self.theme);

        // Spec 0147 G1/G2: no border — content splits into a `Min(0)`
        // area above its own `Length(1)` local statusline row.
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        let inner = split[0];
        self.side_area = inner;

        let node_path = self.positional_path(idx);
        let mode_label = match self.override_sort {
            SortMode::Lexicographic => "all types",
            SortMode::Inferred => "inferred types",
        };
        let left = format!("{node_path} - {mode_label}");
        let right = format!(
            "L{}/{}",
            self.override_highlight + 1,
            self.override_candidates.len(),
        );
        let text = statusline_text(&left, Some(&right), split[1].width as usize);
        frame.render_widget(Paragraph::new(Line::styled(text, style)), split[1]);

        let list_height = inner.height as usize;
        self.override_list_height = list_height;

        let total_rows = self.override_candidates.len();
        // 2026-07-19 feedback item 3: auto-pan into view only on genuine
        // highlight movement, mirroring the main pane's own
        // `last_cursor_row` gate above.
        if self.last_override_highlight != Some(self.override_highlight) {
            clamp_scroll_to_visible(
                &mut self.override_scroll,
                self.override_highlight,
                list_height,
            );
            self.last_override_highlight = Some(self.override_highlight);
        }
        let end = (self.override_scroll + list_height).min(total_rows);
        let start = self.override_scroll.min(total_rows);
        // 2026-07-20 feedback: warm the wrapper-descriptor registration
        // for the whole currently-visible window ahead of time, so
        // arrowing through already-visible rows never re-pays the
        // registration cost per keystroke.
        self.warm_visible_override_wrappers(start, end);

        let mut lines: Vec<Line> = Vec::new();
        for row in start..end {
            // Spec 0114/0137 amendment (2026-07-17 feedback): simplify
            // the lexicographic-mode color scheme — primitive types
            // (including the `None` sentinel) get the default style,
            // no longer a distinct comment/punctuation color; enums keep
            // their blue `Attribute` color but gain an explicit
            // ` [enum]` suffix instead. Factored into `override_row_
            // display` (2026-07-19 feedback item 4) so `override_max_
            // visible_line_len` computes the same text.
            let (text, base_style) = self.override_row_display(row);
            let style = if row == self.override_highlight {
                base_style.add_modifier(Modifier::REVERSED)
            } else {
                base_style
            };
            // Spec 0127 §G1: pan the override pane's own rows
            // independently of the main pane's `pan_offset`.
            lines.push(Line::from(pan_spans(
                vec![Span::styled(text, style)],
                self.override_pan_offset,
            )));
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Centered modal listing `HELP_TEXT`, scrollable via `help_scroll`.
    pub(super) fn render_help(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(70, 70, area);
        frame.render_widget(Clear, popup);
        let block = Block::bordered()
            .title(" Help (j/k scroll, q/Esc/F1 close) ")
            .border_type(BorderType::Rounded);
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        self.help_area = inner;

        let visible_height = (inner.height as usize).max(1);
        let max_scroll = HELP_TEXT.len().saturating_sub(visible_height);
        self.help_scroll = self.help_scroll.min(max_scroll);
        let end = (self.help_scroll + visible_height).min(HELP_TEXT.len());
        let lines: Vec<Line> = HELP_TEXT[self.help_scroll..end]
            .iter()
            .map(|&l| Line::from(l))
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Startup splash — dismissed by any key/mouse event or after
    /// `SPLASH_TIMEOUT` elapses (item 13 of 2026-07-17 feedback) —
    /// telling the user how to reach the `F1` help overlay (spec 0113
    /// D22).
    pub(super) fn render_splash(&self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(60, 30, area);
        frame.render_widget(Clear, popup);
        let block = Block::bordered()
            .title(" protolens ")
            .border_type(BorderType::Rounded);
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let text = vec![
            Line::from(self.header.as_str()),
            Line::from(""),
            Line::from("Press F1 for help."),
        ];
        frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), inner);
    }
}
