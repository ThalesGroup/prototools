// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

use prototext_core::serialize::encode_text::annotation_start;
use std::borrow::Cow;

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

/// Spec 0193 S1: the fold marker's glyphs, open (children shown) and
/// closed (children collapsed into `{ ... }`).
const FOLD_GLYPH_OPEN: char = '▾';
const FOLD_GLYPH_CLOSED: char = '▸';

/// Spec 0193 S1: how many columns the fold field reserves left of the
/// row's own text. Two, because that is the marker plus the space that
/// keeps it from reading as part of the identifier beside it (`▾options`
/// is one token to the eye; `▾ options` is not).
///
/// The field is reserved unconditionally, exactly as spec 0138 N1's
/// heat-cue column is: a field that appeared and vanished with the
/// window's contents would move the text origin as the user scrolls,
/// which is worse than spending the two columns.
pub(super) const FOLD_FIELD_WIDTH: usize = 2;

/// Spec 0193 S1: the two-column fold field followed by the line's own
/// indentation, with `marker` — when there is one — placed in the two
/// display columns *immediately left of* the line's first non-blank
/// character.
///
/// When the indentation is at least `FOLD_FIELD_WIDTH` wide the marker
/// overwrites its last two columns and the field stays blank; when it is
/// narrower (a root-level row, or any row under `--indent 0`/`1`) those
/// columns would fall left of the pane's text origin, so the marker goes
/// in the reserved field instead. Either way the result is exactly
/// `FOLD_FIELD_WIDTH + indent_len` columns wide, which is what keeps a
/// foldable row's text aligned with a non-foldable one's (G1).
fn fold_margin(indent_len: usize, marker: Option<char>) -> String {
    match marker {
        Some(m) if indent_len < FOLD_FIELD_WIDTH => format!("{m} {}", " ".repeat(indent_len)),
        Some(m) => format!("{}{m} ", " ".repeat(indent_len)),
        None => " ".repeat(FOLD_FIELD_WIDTH + indent_len),
    }
}

/// Spec 0193 S1/G6: the column a foldable line's marker occupies within
/// the rendered row — `mouse.rs`'s fold-marker hit test, which adds back
/// the heat-cue column `render` prepends downstream.
///
/// This is the same rule `fold_margin` draws with, and must stay so:
/// under the default `--indent 2` it happens to be `indent_len` for
/// every level below the root, which is what the pre-0193 implementation
/// returned unconditionally — and is why that implementation was correct
/// by coincidence and wrong at `--indent 1`.
pub(super) fn marker_column(line: &str) -> u16 {
    let indent_len = line.len() - line.trim_start().len();
    if indent_len < FOLD_FIELD_WIDTH {
        0
    } else {
        indent_len as u16
    }
}

/// Spec 0193 S1/S2: drop `cut`'s bytes from `segments`, which stay in
/// their original `content` coordinates — the bytes are simply never
/// emitted by `spans_with_insertions`.
///
/// Two callers: the leading indentation, which `fold_margin` replaces
/// wholesale, and a brace that is re-emitted as a styled insertion at
/// the very same position. A segment straddling both ends of `cut`
/// survives as two.
fn cut_segments(segments: &mut Vec<(Range<usize>, Option<SyntaxRole>)>, cut: Range<usize>) {
    if cut.is_empty() {
        return;
    }
    let mut kept = Vec::with_capacity(segments.len() + 1);
    for (range, role) in segments.drain(..) {
        if range.end <= cut.start || range.start >= cut.end {
            kept.push((range, role));
            continue;
        }
        if range.start < cut.start {
            kept.push((range.start..cut.start, role));
        }
        if range.end > cut.end {
            kept.push((cut.end..range.end, role));
        }
    }
    *segments = kept;
}

/// Spec 0194 S1/A5: a byte offset into a row's text expressed as a
/// caret-track column, which counts `char`s. The two coordinates differ
/// the moment a string value holds a multi-byte character, and a column
/// must never be used to slice a `str` — this is the only conversion.
fn char_column(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

/// Spec 0194 S2: apply `restyle` to exactly the `index`-th character of
/// `spans`, splitting whichever span contains it into up to three.
///
/// Walking the *final* span list by character index — rather than
/// cutting a byte range out of the row's syntax segments the way spec
/// 0193's brace did — is deliberate. `row_text` splices a folded node's
/// `" ... }"` summary into the text while `row_spans` adds it as an
/// insertion into an unmodified `content`, so the two disagree on byte
/// offsets on exactly the rows the brace pair cares about. Characters of
/// the drawn row are the one coordinate system every zone shares.
///
/// An `index` past the row's end pads with spaces and draws on the last
/// one, which is how a blank row still gets a cell to carry the caret.
fn restyle_char(spans: &mut Vec<Span<'static>>, index: usize, restyle: impl Fn(Style) -> Style) {
    let drawn: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if index < drawn {
        restyle_range(spans, index..index + 1, restyle);
        return;
    }
    let base = spans.last().map(|s| s.style).unwrap_or_default();
    if index > drawn {
        spans.push(Span::styled(" ".repeat(index - drawn), base));
    }
    spans.push(Span::styled(" ".to_string(), restyle(base)));
}

/// Spec 0235 S16: `restyle_char` over a character *range* — the same
/// walk over the same coordinate system, which is characters of the
/// *drawn* row for the reason `restyle_char` gives above.
///
/// Unlike `restyle_char` this does not pad: a search match is text that
/// exists, so a range past the row's end is a range with nothing in it.
fn restyle_range(
    spans: &mut Vec<Span<'static>>,
    range: Range<usize>,
    restyle: impl Fn(Style) -> Style,
) {
    if range.is_empty() {
        return;
    }
    let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len() + 2);
    let mut seen = 0;
    for span in spans.drain(..) {
        let count = span.content.chars().count();
        let (start, end) = (seen, seen + count);
        seen = end;
        if end <= range.start || start >= range.end {
            out.push(span);
            continue;
        }
        let style = span.style;
        let text = span.content.into_owned();
        let lo = byte_of_char(&text, range.start.saturating_sub(start));
        let hi = byte_of_char(&text, range.end - start);
        if lo > 0 {
            out.push(Span::styled(text[..lo].to_string(), style));
        }
        out.push(Span::styled(text[lo..hi].to_string(), restyle(style)));
        if hi < text.len() {
            out.push(Span::styled(text[hi..].to_string(), style));
        }
    }
    *spans = out;
}

/// The byte offset of `text`'s `n`-th character, or its length when it
/// has fewer than `n`.
fn byte_of_char(text: &str, n: usize) -> usize {
    text.char_indices()
        .nth(n)
        .map_or(text.len(), |(byte, _)| byte)
}

/// Spec 0194 S2: put the caret on the character at `index`.
///
/// `Style::patch` composes `caret_style` over whatever syntax color the
/// character already had, so the caret keeps the color of what it rests
/// on.
///
/// A plain patch is enough because `REVERSED` is the caret's alone (spec
/// 0233 S2): every other cue in the pane — the cursor's row, the
/// selection, a matched brace, a search hit — is a background tint, and
/// a tint under an inversion still shows, as the character's color. The
/// selection used to reverse too, and two reversals cancel, so the cell
/// at the moving end of a selection came out looking plain; that is why
/// `selection_style` is a background now.
///
/// Takes no style. Spec 0233 S2: the caret is drawn the same wherever it
/// rests, and a parameter here is the door that rule exists to close.
fn apply_caret(spans: &mut Vec<Span<'static>>, index: usize) {
    restyle_char(spans, index, move |style| style.patch(theme::caret_style()));
}

/// A rendered line with its trailing `#@ ...` annotation removed (spec
/// 0187 S1) — the part of the line the *format* considers a value.
/// Used both for the display-time annotation hiding of spec 0133 G4 and,
/// in `window_styles_for`, for depth arithmetic that must not be
/// confused by a brace inside an annotation.
pub(super) fn code_part(line: &str) -> &str {
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
    /// committed visible rows, adjusted by however many rows the
    /// preview overlay adds or removes.
    pub(super) fn composed_row_count(&self) -> usize {
        match &self.preview_overlay {
            Some(o) => (self.visible_row_count() + o.lines.len()) - o.covered_rows,
            None => self.visible_row_count(),
        }
    }

    /// Spec 0185 S2: the committed visible row a display row draws, or
    /// `None` if the row belongs to the overlay instead. Exactly one
    /// contiguous span of rows is ever substituted, so this is
    /// arithmetic rather than a rebuilt vector.
    fn committed_row_of(&self, d: usize) -> Option<usize> {
        let Some(o) = &self.preview_overlay else {
            return Some(d);
        };
        if d < o.first_row {
            Some(d)
        } else if d < o.first_row + o.lines.len() {
            None
        } else {
            Some((d - o.lines.len()) + o.covered_rows)
        }
    }

    /// Spec 0185 S2: resolve a display row to the line it draws. `None`
    /// past the last row.
    ///
    /// Spec 0210 S3: one descent per call, which is why nothing outside
    /// the tests calls it any more — every production caller wanted a
    /// whole window and now goes through `build_window`, which resolves
    /// once and then walks. Kept because it is the direct statement of
    /// what the overlay's row mapping means, and the tests assert
    /// against it row by row.
    #[cfg(test)]
    pub(super) fn display_row(&self, d: usize) -> Option<DisplayRow> {
        match self.committed_row_of(d) {
            Some(row) => self
                .visible_row_pos(row)
                .map(|(pos, line)| self.committed_row_at(line, pos)),
            None => Some(DisplayRow::Overlay(
                d - self
                    .preview_overlay
                    .as_ref()
                    .expect("committed_row_of only declines a row while an overlay is held")
                    .first_row,
            )),
        }
    }

    /// Spec 0210 S3: the `count` display rows starting at `from`, each
    /// committed one carrying its own owner for the passes that follow.
    ///
    /// The whole reason absolute line numbers can stop being stored:
    /// a viewport is one descent plus a walk, so nothing per-frame
    /// depends on an O(document) index existing. `display_row` in a
    /// loop would instead be one descent per row, and each of those
    /// crosses the root's 7 771 children on the reference corpus.
    ///
    /// Spec 0222 S3: the walk's own answer is kept rather than thrown
    /// away and recovered later — every downstream pass wanted it, and
    /// the window-sized cache that used to serve them is gone.
    ///
    /// The overlay splits the committed rows into at most two runs
    /// (before it and after it), each of which is resolved and walked
    /// on its own.
    pub(super) fn build_window(&self, from: usize, count: usize) -> Vec<DisplayRow> {
        let mut rows = Vec::with_capacity(count);
        let mut cursor: Option<(LinePos, usize)> = None;
        let mut d = from;
        while rows.len() < count {
            let Some(first) = self.committed_row_of(d) else {
                let o = self
                    .preview_overlay
                    .as_ref()
                    .expect("committed_row_of only declines a row while an overlay is held");
                rows.push(DisplayRow::Overlay(d - o.first_row));
                d += 1;
                continue;
            };
            // How many display rows this committed run has left before
            // the overlay (if any) interrupts it.
            let run = match &self.preview_overlay {
                Some(o) if d < o.first_row => o.first_row - d,
                _ => count - rows.len(),
            };
            let run = run.min(count - rows.len());
            let walked = self.visible_window(first, run);
            if walked.is_empty() {
                break;
            }
            d += walked.len();
            for (line, pos) in walked {
                let offset = self.advance_offset(&mut cursor, pos);
                rows.push(DisplayRow::Committed(CommittedRow { line, pos, offset }));
            }
        }
        rows
    }

    /// Spec 0222 S4's byte cursor: where `pos` starts inside its owner's
    /// text, stepped on from the previous row when the two are
    /// consecutive lines of one node and resolved from scratch
    /// otherwise.
    ///
    /// Without this a frame drawn inside a long packed run would be
    /// quadratic — the run's N lines are one string, so its k-th line
    /// is k newlines in. The walk is already in document order, so the
    /// whole frame becomes one scan of the run.
    pub(super) fn advance_offset(
        &self,
        cursor: &mut Option<(LinePos, usize)>,
        pos: LinePos,
    ) -> usize {
        // A bracketed node stores only its header, and its footer is
        // derived rather than sliced, so neither has an offset to step.
        if self.tree[pos.node].is_bracketed() {
            *cursor = Some((pos, 0));
            return 0;
        }
        let offset = match *cursor {
            Some((prev, at))
                if prev.node == pos.node && prev.line_in_node + 1 == pos.line_in_node =>
            {
                let text = self.node_text[pos.node].as_deref().unwrap_or("");
                match text[at..].find('\n') {
                    Some(nl) => at + nl + 1,
                    None => text.len(),
                }
            }
            _ => self.line_offset(pos),
        };
        *cursor = Some((pos, offset));
        offset
    }

    /// A committed display row for a line whose owner is already known.
    ///
    /// Spec 0222 S3: a row carries its owner, so the handful of rows
    /// built outside `build_window` — the cursor's own line, a click
    /// target, a brace pair — have to supply one. All of them are per
    /// keystroke, not per row.
    pub(super) fn committed_row_at(&self, line: usize, pos: LinePos) -> DisplayRow {
        DisplayRow::Committed(CommittedRow {
            line,
            pos,
            offset: self.line_offset(pos),
        })
    }

    /// `committed_row_at` for a caller holding only the line number,
    /// which costs the `line_pos` descent. `None` past the end of the
    /// document.
    #[cfg(test)]
    pub(super) fn committed_row(&self, line: usize) -> Option<DisplayRow> {
        Some(self.committed_row_at(line, self.line_pos(line)?))
    }

    /// Spec 0185 S2: `cursor_display_row()` carried into composed row
    /// space, which is what the render loop's `REVERSED` comparison
    /// needs — it compares against the rows it is drawing, not against
    /// the committed ones. A cursor inside the block the overlay stands
    /// in for (the usual case: the preview's target *is* the node the
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
    fn display_row_source(&self, row: DisplayRow) -> (Cow<'_, str>, Option<usize>) {
        let owner = match row {
            DisplayRow::Committed(c) => (!self.is_footer(c.pos)).then_some(c.pos.node),
            DisplayRow::Overlay(_) => None,
        };
        (self.display_row_text(row), owner)
    }

    /// Spec 0215 S4: the text half of `display_row_source`, on its own.
    ///
    /// Spec 0222 S1/S2: a committed row's text is a slice of its owner's
    /// own text, borrowed — except a bracketed node's closing brace,
    /// which is derived from the header's indentation rather than
    /// stored, and so is the one row that owns its string.
    pub(super) fn display_row_text(&self, row: DisplayRow) -> Cow<'_, str> {
        match row {
            DisplayRow::Committed(c) => self.line_text_at(c.pos, c.offset),
            DisplayRow::Overlay(i) => {
                let o = self
                    .preview_overlay
                    .as_ref()
                    .expect("an Overlay row is only ever produced while an overlay is held");
                Cow::Borrowed(&o.lines[i])
            }
        }
    }

    /// Spec 0187 S2: the rows currently on screen, as the highlighter
    /// sees them — the committed line or overlay line each `DisplayRow`
    /// draws, untruncated and unfolded (fold markers and the annotation
    /// transform are display insertions applied downstream, and must not
    /// reach the parser).
    ///
    /// Rows that are not prototext at all are emitted as `""`. The
    /// document is not purely prototext: spec 0174 §S4's `...` marker
    /// is a literal row in a truncated preview's lines, and `...` is not
    /// in the grammar. Highlighting happens at draw time, so a syntax
    /// error here would silently strip the color off every row *beneath*
    /// the marker via tree-sitter's error recovery. Substituting a blank
    /// line rather than dropping the row keeps the result
    /// index-parallel with `window`, so the row's own bucket comes out
    /// empty by construction and no index surgery is needed.
    fn window_text(&self, window: &[DisplayRow]) -> Vec<String> {
        window
            .iter()
            .map(|&row| {
                let line = self.display_row_text(row);
                if line.trim() == TRUNCATION_MARKER {
                    String::new()
                } else {
                    line.into_owned()
                }
            })
            .collect()
    }

    /// Spec 0187 S3: bring `window_styles` up to date for the rows this
    /// frame is about to draw. Called once per `render()`, from
    /// `render()` only — the result is never document-sized.
    ///
    /// Spec 0245 S3: kept, not recomputed and not dropped, when the
    /// window is the one it was already computed for. `window_styles_for`
    /// is a pure function of the window text and the indent size, so
    /// while those hold the hints are current.
    ///
    /// Spec 0223 S3: only a *changed* window under pending input goes
    /// monochrome, and it is *cleared* rather than merely left alone —
    /// `row_spans` falls back to `NO_STYLES` for a missing entry, so an
    /// empty vector is precisely the monochrome path, whereas keeping
    /// the previous window's hints would paint one viewport's colors
    /// onto another viewport's rows.
    pub(super) fn refresh_window_styles(&mut self, window: &[DisplayRow]) {
        let text = self.window_text(window);
        let current = self
            .window_styles_key
            .as_ref()
            .is_some_and(|(prev, indent)| *indent == self.indent_size && *prev == text);
        if current {
            return;
        }
        if self.input_pending {
            self.window_styles.clear();
            self.window_styles_key = None;
            return;
        }
        self.window_styles = window_styles_for(&text, self.indent_size);
        self.window_styles_key = Some((text, self.indent_size));
    }

    /// A row as it is drawn, fold margin included — the row's own text
    /// with `fold_margin` in front of it, so a foldable line's first
    /// token lands in the same column as a non-foldable one's at the
    /// same depth (spec 0193 G1).
    ///
    /// Spec 0185 S2: takes a display row, so overlay rows come through
    /// here too — **there must not be a second line-rendering path.**
    pub(super) fn row_content(&self, row: DisplayRow) -> String {
        let text = self.row_text(row);
        let (margin, body_start) = self.fold_margin_of(row, &text);
        format!("{margin}{}", &text[body_start..])
    }

    /// Spec 0193 S1: the row's text *without* the fold margin — the
    /// annotation transform (spec 0133 G4) and a folded node's `{ ... }`
    /// collapse summary, both of which are properties of the content,
    /// and neither of which is chrome.
    ///
    /// This is what a clipboard copy wants (`selected_text`): the margin
    /// and its `▾`/`▸` are gutter furniture, and pasting them back into
    /// a `.textproto` would not parse.
    pub(super) fn row_text(&self, row: DisplayRow) -> String {
        self.row_text_of(row, self.display_row_source(row).1)
    }

    /// Spec 0215 S6: `row_text` for a caller that already knows which
    /// node owns the row, saving the `line_pos` descent that finding out
    /// would cost.
    ///
    /// `owner` must be what `display_row_source` would have returned —
    /// `None` for an overlay row or a footer line, the owning node for a
    /// header line. Passing a wrong node changes only the fold glyph and
    /// hence the `{ ... }` collapse summary, never the underlying text.
    pub(super) fn row_text_of(&self, row: DisplayRow, owner: Option<usize>) -> String {
        let source = self.display_row_text(row);
        let content = if !self.annotations {
            code_part(&source)
        } else {
            &source
        };
        let mut text = content.to_string();
        if self.fold_marker_of(owner) == Some(FOLD_GLYPH_CLOSED) {
            match text.rfind('{') {
                Some(pos) => text.insert_str(pos + 1, " ... }"),
                None => text.push_str(" ... }"),
            }
        }
        text
    }

    /// Spec 0193 S1: `(the row's left margin, the byte offset in
    /// `content` at which the margin's coverage ends)`. The margin
    /// subsumes the line's own leading indentation, so the caller emits
    /// `content` from `body_start` onwards and never re-emits the
    /// indent.
    ///
    /// An all-blank row gets no margin at all: there is nothing on it to
    /// align, and padding it would only lengthen the row that
    /// `max_visible_line_len` measures for panning.
    fn fold_margin_of(&self, row: DisplayRow, content: &str) -> (String, usize) {
        let trimmed = content.trim_start();
        if trimmed.is_empty() {
            return (String::new(), content.len());
        }
        let indent_len = content.len() - trimmed.len();
        (fold_margin(indent_len, self.fold_marker(row)), indent_len)
    }

    /// The fold glyph this row's node currently warrants, or `None` when
    /// the row has no node of its own (footer and overlay rows) or its
    /// node has nothing to fold.
    fn fold_marker(&self, row: DisplayRow) -> Option<char> {
        self.fold_marker_of(self.display_row_source(row).1)
    }

    /// Spec 0215 S6: `fold_marker` for a caller that already holds the
    /// row's owner. Takes the owner alone — once it is known the row
    /// itself is not consulted, so passing it would be an unused
    /// argument rather than documentation.
    fn fold_marker_of(&self, owner: Option<usize>) -> Option<char> {
        let idx = owner?;
        if !self.has_children(idx) {
            return None;
        }
        Some(if self.folded.contains(&idx) {
            FOLD_GLYPH_CLOSED
        } else {
            FOLD_GLYPH_OPEN
        })
    }

    /// Spec 0194 S4: the cursor node's brace pair, each member given as
    /// `(line index, caret-track column)` — the `{` on the node's own
    /// header line and the `}` that closes it.
    ///
    /// `None` for a node with no *distinct* footer line, which is
    /// exactly the `has_children` test for a node being bracketed at
    /// all: an unbracketed scalar's `{` would otherwise be a brace
    /// inside a string literal.
    ///
    /// A *folded* node draws its whole body as the one-row `{ ... }`
    /// collapse summary (spec 0193 S1), so both members are on the
    /// header line and the footer line is not on screen at all.
    pub(super) fn cursor_brace_pair(&self) -> Option<((usize, usize), (usize, usize))> {
        if self.cursor >= self.tree.len() || !self.has_children(self.cursor) {
            return None;
        }
        let header = self.absolute_start(self.cursor);
        let footer = header + self.tree[self.cursor].lines_total as usize - 1;
        let header_row = self.committed_row_at(header, LinePos::header(self.cursor));
        let header_text = self.row_text(header_row);
        let open = header_text.rfind('{')?;
        let open_pos = (header, char_column(&header_text, open));
        if self.folded.contains(&self.cursor) && self.has_children(self.cursor) {
            // `row_text` splices the six bytes `" ... }"` in immediately
            // after the `{`, so the synthetic closing brace is the sixth
            // of them.
            let close = char_column(&header_text, open + 6);
            return Some((open_pos, (header, close)));
        }
        let footer_row = self.committed_row_at(
            footer,
            LinePos {
                node: self.cursor,
                line_in_node: self.tree[self.cursor].lines_total - 1,
            },
        );
        let footer_text = self.row_text(footer_row);
        let close = char_column(&footer_text, footer_text.rfind('}')?);
        Some((open_pos, (footer, close)))
    }

    /// Spec 0194 S1/S2: where caret-track column `column` lands in a
    /// drawn row's final span list, chrome included — so index 0 is the
    /// heat glyph's reserved column. `None` when a horizontal pan has
    /// taken the column off the left edge.
    ///
    /// The track's two zones map differently: the text zone sits behind
    /// the `FOLD_FIELD_WIDTH`-wide fold field and moves with the pan,
    /// while the heat suffix is appended *after* panning and so does not
    /// move at all. `panned_chars` is the row's own text as it survived
    /// the pan, which is where the suffix starts.
    fn caret_draw_index(
        &self,
        column: usize,
        text_chars: usize,
        panned_chars: usize,
    ) -> Option<usize> {
        if column < text_chars {
            Some((FOLD_FIELD_WIDTH + column).checked_sub(self.pan_offset)? + 1)
        } else {
            Some(1 + panned_chars + (column - text_chars))
        }
    }

    /// Styled counterpart of `row_content` (spec 0116 §7/§9): applies the
    /// row's syntax-highlighting spans via `theme::style_for`, prepends
    /// the same `fold_margin` `row_content` does, and splices in the same
    /// `" ... }"` collapse-summary text — the two **must** agree
    /// byte for byte, and a test asserts they do.
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
    ///
    /// Spec 0192 S2: `emphasis` is the row's active-override weight. It
    /// lands on the type name alone (see `make_span`) whenever the row
    /// has one, and on the whole row when it has not — a row with the
    /// annotations hidden (`a`), or a bare footer, has nowhere else to
    /// carry the cue, and losing it there would say the node is not
    /// overridden.
    pub(super) fn row_spans(
        &self,
        row: DisplayRow,
        window_index: usize,
        emphasis: Modifier,
    ) -> Vec<Span<'static>> {
        let (source, node) = self.display_row_source(row);
        let full_content: &str = &source;
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
        let mut segments = segment_line(content, &hints);
        let (type_emphasis, row_emphasis) =
            match segments.iter().any(|&(_, r)| r == Some(SyntaxRole::Type)) {
                true => (emphasis, Modifier::empty()),
                false => (Modifier::empty(), emphasis),
            };

        // Spec 0193 S1: the margin *replaces* the line's indentation
        // rather than displacing it, so those bytes are cut from the
        // segments and re-emitted as one raw span in front.
        let (margin, body_start) = self.fold_margin_of(row, content);
        cut_segments(&mut segments, 0..body_start);
        let mut spans = Vec::with_capacity(segments.len() + 4);
        if !margin.is_empty() {
            spans.push(Span::raw(margin));
        }

        // Spec 0194 S2: one insertion. The synthetic closing brace needs
        // no span of its own to carry `brace_match_style` — the caret
        // restyles it by character index over the finished span list —
        // so the summary stays one piece of text.
        let mut insertions: Vec<(usize, String, Option<Style>)> = Vec::new();
        if node.is_some_and(|idx| self.folded.contains(&idx) && self.has_children(idx)) {
            let insert_at = match content.rfind('{') {
                Some(pos) => pos + 1,
                None => content.len(),
            };
            insertions.push((insert_at, " ... }".to_string(), None));
        }
        spans.extend(self.spans_with_insertions(content, segments, insertions, type_emphasis));
        if !row_emphasis.is_empty() {
            for span in &mut spans {
                span.style = span.style.add_modifier(row_emphasis);
            }
        }
        spans
    }

    /// Turns `content`'s `segments` (byte ranges tagged with an optional
    /// `SyntaxRole`) into styled `Span`s, splicing in `insertions` —
    /// `(byte position in content, literal text, optional style)` triples,
    /// each rendered as its own `Span` at that point.
    ///
    /// `segments` need not cover all of `content`: `cut_segments` removes
    /// the bytes the fold margin replaces, and those an insertion stands
    /// in for (spec 0193). Anything cut is simply never emitted.
    pub(super) fn spans_with_insertions(
        &self,
        content: &str,
        segments: Vec<(Range<usize>, Option<SyntaxRole>)>,
        mut insertions: Vec<(usize, String, Option<Style>)>,
        emphasis: Modifier,
    ) -> Vec<Span<'static>> {
        insertions.sort_by_key(|(pos, _, _)| *pos);
        let mut segments: std::collections::VecDeque<_> = segments.into();
        let mut result = Vec::new();
        for (ins_pos, ins_text, ins_style) in insertions {
            while let Some((range, role)) = segments.pop_front() {
                if range.end <= ins_pos {
                    result.push(self.make_span(content[range].to_string(), role, emphasis));
                } else if range.start < ins_pos {
                    result.push(self.make_span(
                        content[range.start..ins_pos].to_string(),
                        role,
                        emphasis,
                    ));
                    segments.push_front((ins_pos..range.end, role));
                    break;
                } else {
                    segments.push_front((range, role));
                    break;
                }
            }
            result.push(match ins_style {
                Some(style) => Span::styled(ins_text, style),
                None => Span::raw(ins_text),
            });
        }
        for (range, role) in segments {
            result.push(self.make_span(content[range].to_string(), role, emphasis));
        }
        result
    }

    /// Spec 0138 N1/G9: a row's heat chrome — the leading glyph, whose
    /// column is reserved unconditionally (a blank space when there is
    /// no cue) so that node indentation never shifts as the user
    /// scrolls, and the optional trailing ` [current/best]` suffix.
    ///
    /// The glyph is shown only for a complete `Cue` — never during a
    /// partial/pending state, even when `best` alone is known — and
    /// `heat_style` itself returns `None` on the ANSI-16 fallback for a
    /// low-confidence `best_score` (G7/G12's narrowing of the gate), in
    /// which case no cue shows at all, glyph or suffix.
    ///
    /// Spec 0194 S1 makes the suffix's *length* the second zone of the
    /// caret track, which is why this is a function `render` can call
    /// twice — once to draw, once to measure — rather than an inline
    /// `match`.
    fn heat_chrome(
        &self,
        display: &heat_cue::HeatDisplay,
    ) -> (Span<'static>, Option<Span<'static>>) {
        let pending_style = || theme::style_for(SyntaxRole::Comment, self.theme);
        let blank = || Span::raw(" ");
        match display {
            heat_cue::HeatDisplay::Cue(c) => {
                let hue = match c.kind {
                    heat_cue::HeatCueKind::Mismatch { .. } => theme::HeatHue::Red,
                    heat_cue::HeatCueKind::Tie { .. } => theme::HeatHue::Blue,
                };
                let Some(style) = theme::heat_style(c.level, hue, self.theme) else {
                    return (blank(), None);
                };
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
                    heat_cue::HeatCueKind::Tie { tie_count, score } => Span::styled(
                        format!(" [{tie_count}@{score}]"),
                        theme::accent_style(self.theme),
                    ),
                };
                (Span::styled(heat_cue::HEAT_GLYPH, style), Some(suffix))
            }
            heat_cue::HeatDisplay::PendingCurrent { best } => (
                blank(),
                Some(Span::styled(format!(" [?/{best}]"), pending_style())),
            ),
            heat_cue::HeatDisplay::Unknown => {
                (blank(), Some(Span::styled(" [?]", pending_style())))
            }
            heat_cue::HeatDisplay::None => (blank(), None),
        }
    }

    /// `emphasis` (spec 0192 S2) rides only on a `Type` segment, which
    /// on a rendered row is the type name in its `#@ Type = N`
    /// annotation — the one part of the row an override actually
    /// changes.
    pub(super) fn make_span(
        &self,
        text: String,
        role: Option<SyntaxRole>,
        emphasis: Modifier,
    ) -> Span<'static> {
        match role {
            Some(SyntaxRole::Type) => Span::styled(
                text,
                theme::style_for(SyntaxRole::Type, self.theme).add_modifier(emphasis),
            ),
            Some(role) => Span::styled(text, theme::style_for(role, self.theme)),
            None => Span::raw(text),
        }
    }

    /// Spec 0113 D33: the node `line_idx` is one of the *own* header or
    /// footer lines of — never a node one of whose descendants owns the
    /// line, which is what keeps the active-override weight from
    /// cascading down a whole overridden subtree.
    ///
    /// Spec 0222 S3: nothing on the draw path calls this any more — a
    /// row carries its owner, and the union of "header or footer" is
    /// just that owner. Kept because it is the direct statement of what
    /// D33's restriction means, and the tests assert against it.
    #[cfg(test)]
    pub(super) fn node_at_own_line(&self, line_idx: usize) -> Option<usize> {
        self.node_at_header_line(line_idx)
            .or_else(|| self.node_at_footer_line(line_idx))
    }

    /// Spec 0192 S2: the extra weight each of `window`'s rows draws its
    /// type name with because the node it belongs to carries an active
    /// override, plus how many resolutions that took.
    ///
    /// `Modifier::empty()` is no override at all. An auto-derived one
    /// (the Any/MessageSet expansion protolens seeds itself, spec 0120)
    /// is bold; one the user asked for is bold *and* underlined, since
    /// bold alone does not read as reliably as a deliberate override
    /// deserves.
    ///
    /// Hoisted out of `render`'s `text_lines` closure into a pass of its
    /// own — same shape and position as the heat-cue pass (spec 0154 G6)
    /// and the highlighting pass (spec 0187 S2), and for the same borrow
    /// reason.
    ///
    /// Consecutive rows resolving to one node are resolved once. In
    /// practice that means a packed run: spec 0216 S22 makes its N
    /// element rows one node, so they share one positional path and
    /// therefore one answer. Rows of one node are not otherwise
    /// adjacent — a message's header and footer rows have its whole
    /// subtree between them — so a one-entry memo is all the collapsing
    /// there is to do here.
    ///
    /// The returned count is what makes that claim testable rather than
    /// merely asserted.
    pub(super) fn override_emphasis(&self, window: &[DisplayRow]) -> (Vec<Modifier>, usize) {
        let mut resolutions = 0;
        let mut last: Option<(usize, Modifier)> = None;
        let flags = window
            .iter()
            .map(|&row| {
                let DisplayRow::Committed(c) = row else {
                    return Modifier::empty();
                };
                let idx = c.pos.node;
                let reusable = last.filter(|&(seen, _)| seen == idx);
                match reusable {
                    Some((_, answer)) => answer,
                    None => {
                        resolutions += 1;
                        let answer = match self.resolve_active_override_entry(idx) {
                            None => Modifier::empty(),
                            Some(e) if e.auto => Modifier::BOLD,
                            Some(_) => Modifier::BOLD | Modifier::UNDERLINED,
                        };
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
    /// assignment site. Never dismissed while `command_buffer` is `Some`
    /// — the global command/message row renders that instead of
    /// `self.message` while it is active (see `render`'s `cmd_text`),
    /// and it is actively awaiting a keypress, unlike a plain notice.
    /// Called once per `render()`.
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
        if self.command_buffer.is_some() {
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

    /// Auto-dismiss the startup splash after `SPLASH_TIMEOUT`, in
    /// addition to its keypress/mouse dismissal. Called once per
    /// `render()`, mirroring `track_message_timeout`'s deadline-based
    /// approach.
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

        self.render_main_pane(frame, main_outer, right_outer.is_some());

        self.render_command_row(frame, chunks[1]);

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

    /// The text area and its local statusline — everything `render`
    /// delegates once it has decided how wide the main pane is.
    ///
    /// `half_width` says a side pane is open. It is the only thing this
    /// needs to know about the rest of the frame, and it does not mean
    /// "narrow": it selects what the statusline *drops*, since the
    /// byte-range ruler and the `[tag]` suffix rarely fit beside a side
    /// pane and are dropped rather than truncated.
    ///
    /// Kept as one function rather than split further because the order
    /// here is load-bearing: the window is built from `scroll_offset`
    /// after `clamp_scroll_to_visible` has moved it, the caret is clamped
    /// against the suffix length only this frame's heat pass knows, and
    /// the statusline reports all of it.
    fn render_main_pane(&mut self, frame: &mut Frame, main_outer: Rect, half_width: bool) {
        // `pane_focus_style` marks whichever pane currently holds keyboard
        // focus, shared with the override/management panes' own
        // `render_override_pane`/`render_manage_pane` — the main pane has
        // focus exactly when neither side pane does. Without it there is
        // no visible sign of which pane focus is in.
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

        // Spec 0225 S8: document *lines*, which in wire mode is half the
        // terminal rows — everything from here down (the scroll clamp,
        // the window, the per-row vectors) is keyed by window index, and
        // the doubling happens once, in the `flat_map` that draws.
        //
        // Spec 0230: rounded up rather than down, and net of
        // `scroll_skip`, because a half-line scroll leaves a partial
        // line at each end and both are drawn.
        let cursor_row = if self.tree.is_empty() {
            0
        } else {
            self.cursor_display_row()
        };
        // Auto-pan into view only on genuine cursor movement
        // (`cursor_row` differs from the last render that clamped), not
        // on every render regardless of cause — otherwise a manual
        // vertical pan (`pan_vertical_*`, deliberately unclamped) would
        // immediately get fought back into following the cursor.
        if !self.tree.is_empty() && self.last_cursor_row != Some(cursor_row) {
            self.clamp_scroll_to_cursor(cursor_row);
            self.last_cursor_row = Some(cursor_row);
        }
        // Spec 0235 S13: after the clamp above rather than before, so
        // that a search's centering is the last word on the viewport.
        // The two do not in fact contend — the clamp is gated on the
        // cursor having moved, and a sweep (S8) never moves it.
        self.center_search_match(inner);
        // Spec 0185 S2: the row the cursor is *drawn* on, in composed
        // coordinates — distinct from `cursor_row` above, which stays in
        // committed visible-row coordinates because that is what
        // `last_cursor_row` (and `clamp_pan_offset`'s matching guard) is
        // compared against. With no overlay the two are equal.
        let cursor_draw_row = if self.tree.is_empty() {
            0
        } else {
            self.cursor_composed_row()
        };
        let total_rows = self.composed_row_count();
        let (blank_rows, rows) =
            self.scroll
                .window(inner.height as usize, self.row_height(), total_rows);
        // Collected (not a borrowed slice) so the heat-cue pass below can
        // mutate `self.heat_range_cache`/`self.heat_current_score_cache`
        // — a plain slice would keep `self` borrowed immutably for the
        // rest of this block.
        let t_window = std::time::Instant::now();
        let first_row = rows.start;
        let window: Vec<DisplayRow> = self.build_window(first_row, rows.len());
        let d_window = t_window.elapsed();

        // Spec 0187 S3: highlight exactly the rows about to be drawn,
        // and nothing else. Its own `&mut self` pass, ahead of the
        // immutable-`self` `text_lines` closure below — the same shape
        // and the same reason as the heat-cue pass that follows.
        //
        // Spec 0223 S3 / spec 0245 S3: whether that means parsing,
        // keeping what is already there, or going monochrome is
        // `refresh_window_styles`'s own decision.
        let t_styles = std::time::Instant::now();
        self.refresh_window_styles(&window);
        let d_styles = t_styles.elapsed();

        // Spec 0242 S11: the selection (if any) is a background tint,
        // laid over the cursor row's own weaker one. Resolved once for
        // the frame; each row then asks it for its own columns.
        let selection_span = self.selection_span();

        // Spec 0154 G6: computed in its own pass, ahead of the
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
                DisplayRow::Committed(c) => self.heat_cue_at(c.pos),
                DisplayRow::Overlay(_) => heat_cue::HeatDisplay::None,
            })
            .collect();
        let d_heat = t_heat.elapsed();
        let t_ovr = std::time::Instant::now();
        let (row_emphasis, _) = self.override_emphasis(&window);
        let d_ovr = t_ovr.elapsed();

        // Spec 0194 S1: the caret track's suffix zone is exactly as long
        // as the heat suffix this frame drew, and `heat_cue_for` — the
        // only thing that knows — needs `&mut self`, so a keypress
        // cannot ask. It is carried on the app instead, refreshed here
        // and read by `caret_bounds`. The column is then clamped into
        // the track it now describes, which is also what absorbs a row
        // that shrank under the caret (S11).
        let suffix_len = if self.tree.is_empty() {
            0
        } else {
            let cursor_row_line = self.cursor_line();
            window
                .iter()
                .zip(heat_displays.iter())
                .find(|&(&r, _)| r.committed_line() == Some(cursor_row_line))
                .and_then(|(_, display)| self.heat_chrome(display).1)
                .map_or(0, |s| s.content.chars().count())
        };
        self.caret_suffix_len = suffix_len;
        self.clamp_caret_column();

        // Spec 0194 S4, as revised by spec 0233: when the caret rests
        // *on* one of the cursor node's braces and the other one is on
        // screen, the other one is tinted. Only it — the caret marks the
        // member it stands on, and it marks it the same way it marks
        // anything else. Resolved here, once, because the partner is
        // routinely on a different row from the caret — and because
        // visibility is a property of this frame: a pan or a scroll
        // decides it with no key pressed.
        //
        // A brace counts as visible when its row is inside the window
        // *and* its column survives the pan and the pane's right edge.
        //
        // Spec 0234 S1: and a third case, because the caret's cell being
        // a brace is not where the caret spends its time. A caret
        // *voluntarily* at the Home anchor of the row that opens the
        // pair speaks for that row's `{`. Voluntarily, on spec 0199 S1's
        // rule: a caret a vertical move's clamp pushed to this column
        // was passing over the row, not reading it.
        let caret_cell = (!self.tree.is_empty()).then(|| (self.cursor_line(), self.cursor_column));
        let home_column = (self.caret_anchor == CaretAnchor::Home).then(|| self.caret_bounds().0);
        let partner =
            self.cursor_brace_pair()
                .zip(caret_cell)
                .and_then(|((open, close), caret)| {
                    if caret == open {
                        Some(close)
                    } else if caret == close {
                        Some(open)
                    } else if caret.0 == open.0 && Some(caret.1) == home_column {
                        Some(close)
                    } else {
                        None
                    }
                });
        let partner_cell = partner.and_then(|(line, column)| {
            let row = window
                .iter()
                .position(|&r| r.committed_line() == Some(line))?;
            let index = (FOLD_FIELD_WIDTH + column).checked_sub(self.pan_offset)? + 1;
            (index < inner.width as usize).then_some((row, index))
        });

        // Spec 0235 S14: resolved per frame for the window only, the
        // same rule and the same reason as spec 0187 S3's syntax pass —
        // a match index kept across frames would need invalidating on
        // every fold, splice and scroll, and a pane's worth of `find`
        // costs less than the bookkeeping would.
        let search_pattern = self.search_highlight_pattern();
        let search_current = self.search_current_cell();
        let search_styles = search_pattern.as_ref().map(|_| {
            (
                theme::search_current_style(self.theme),
                theme::search_match_style(self.theme),
            )
        });

        let t_lines = std::time::Instant::now();

        // Spec 0225 S4: carried across the whole window so a packed run
        // drawn over many rows is walked once, not once per row.
        let mut packed_memo = wire::PackedCursor::default();

        let mut text_lines: Vec<Line> = window
            .iter()
            .zip(heat_displays.iter())
            .enumerate()
            .flat_map(|(row, (&display_row, display))| {
                // `None` for an overlay row (spec 0185 S4), which gates
                // the active-override hint and the drag selection below
                // exactly as it already gates fold markers inside
                // `row_spans`.
                let line_idx = display_row.committed_line();
                let mut spans = pan_spans(
                    self.row_spans(display_row, row, row_emphasis[row]),
                    self.pan_offset,
                );
                // Spec 0194 S1: where the row's own text ends, which is
                // where the caret track's suffix zone begins. Measured
                // before the chrome goes on and only on the row that
                // needs it.
                let on_cursor_row = self.scroll.index + row == cursor_draw_row;
                let panned_chars = on_cursor_row.then(|| {
                    spans
                        .iter()
                        .map(|s| s.content.chars().count())
                        .sum::<usize>()
                });
                let (glyph, suffix) = self.heat_chrome(display);
                if let Some(suffix) = suffix {
                    spans.push(suffix);
                }
                spans.insert(0, glyph);

                // Spec 0194 S2: three distinct highlights, not one
                // row-wide `REVERSED`. The caret's *row* gets the weaker
                // `cursor_row_style` (G7), patched so it colors the
                // background and leaves every syntax foreground alone;
                // the selection gets a stronger background over it; and
                // the caret itself is a single cell, applied last so it
                // wins.
                if on_cursor_row {
                    let row_style = theme::cursor_row_style(self.theme);
                    for span in &mut spans {
                        span.style = span.style.patch(row_style);
                    }
                }
                // Spec 0242 S11: only the selected *characters*, not the
                // whole row — a selection is a span of characters (G1),
                // and the fold margin and the heat suffix are gutter
                // furniture that is in no selection.
                //
                // `saturating_sub` rather than the search highlight's
                // `checked_sub`: a selection routinely starts left of
                // the pan (its first row is often scrolled past
                // horizontally), and the visible tail of such a range
                // still has to be tinted. Saturating to 0 clamps it to
                // the leftmost drawn cell, which is exactly right.
                if let Some(range) = line_idx.zip(selection_span).and_then(|(line, span)| {
                    let text_chars = self.row_text(display_row).chars().count();
                    self.selected_columns(span, line, text_chars)
                }) {
                    let width = inner.width as usize;
                    let pan = self.pan_offset;
                    let lo = (FOLD_FIELD_WIDTH + range.start).saturating_sub(pan) + 1;
                    let hi = ((FOLD_FIELD_WIDTH + range.end).saturating_sub(pan) + 1).min(width);
                    if lo < hi {
                        let style = theme::selection_style(self.theme);
                        restyle_range(&mut spans, lo..hi, |s| s.patch(style));
                    }
                }
                // Spec 0233 S3: a background patch, not a second
                // inversion — and applied before the caret, since on a
                // folded node's `{ ... }` row the two land three
                // characters apart.
                if let Some((partner_row, partner_index)) = partner_cell {
                    if partner_row == row {
                        let match_style = theme::brace_match_style(self.theme);
                        restyle_char(&mut spans, partner_index, move |style| {
                            style.patch(match_style)
                        });
                    }
                }
                // Spec 0235 S14: after the brace partner and before the
                // caret, which is the order the four cues are listed in
                // — the caret applied last so it still wins its cell.
                if let (Some(pattern), Some((current, other))) = (&search_pattern, search_styles) {
                    let text = self.row_text(display_row);
                    let width = inner.width as usize;
                    // Spec 0235 S22: a path match has nothing visible to
                    // mark, so it gets one cell rather than a range —
                    // exactly the cell the caret will land on at
                    // `Enter`.
                    let path_cell = search_current
                        .filter(|&(line, _, _, on_path)| on_path && Some(line) == line_idx);
                    if let Some((_, column, _, _)) = path_cell {
                        if let Some(index) =
                            (FOLD_FIELD_WIDTH + column).checked_sub(self.pan_offset)
                        {
                            if index + 1 < width {
                                restyle_range(&mut spans, index + 1..index + 2, |style| {
                                    style.patch(current)
                                });
                            }
                        }
                    }
                    let mut at = 0;
                    while let Some(found) = pattern.find_range(&text[at..]) {
                        let start = char_column(&text, at + found.start);
                        let chars = text[at + found.start..at + found.end].chars().count();
                        // A zero-width match cannot happen — an empty
                        // pattern never reaches here — but stepping past
                        // the match's *start* rather than its end is
                        // what keeps overlapping occurrences honest.
                        at += found.start
                            + text[at + found.start..]
                                .chars()
                                .next()
                                .map_or(1, char::len_utf8);
                        let style = if search_current.is_some_and(|(line, column, _, on_path)| {
                            !on_path && Some(line) == line_idx && column == start
                        }) {
                            current
                        } else {
                            other
                        };
                        let Some(index) = (FOLD_FIELD_WIDTH + start).checked_sub(self.pan_offset)
                        else {
                            continue;
                        };
                        let lo = index + 1;
                        let hi = (lo + chars).min(width);
                        if lo < hi {
                            restyle_range(&mut spans, lo..hi, |s| s.patch(style));
                        }
                    }
                }
                if let (true, Some(panned_chars)) = (on_cursor_row, panned_chars) {
                    let text_chars = self.row_text(display_row).chars().count();
                    if let Some(index) =
                        self.caret_draw_index(self.cursor_column, text_chars, panned_chars)
                    {
                        apply_caret(&mut spans, index);
                    }
                }

                // Spec 0225 S8: the wire row is built from the raw
                // spans, after every one of the highlights above — the
                // cursor row style, the selection, the caret and the
                // brace partner all belong to the document row alone.
                // It carries the heat gutter's blank column so its hex
                // lines up under the text, and is panned with it.
                //
                // Spec 0225 S7: the classification still runs
                // under `input_pending` — it is a per-byte pass over
                // at most `WIRE_ROW_MAX_BYTES` bytes with no parser
                // involved — but its *colors* come from
                // `window_styles`, which spec 0223 clears then. So the
                // row goes monochrome with the document row it echoes,
                // rather than the two disagreeing.
                let wire_line = self.wire.then(|| {
                    let source = self.display_row_text(display_row);
                    let indent = source.len() - source.trim_start().len();
                    let palette = self.wire_palette(row, &source);
                    let wire_spans = match display_row {
                        DisplayRow::Committed(c) => {
                            self.wire_row(c.pos, indent, &mut packed_memo, palette.as_ref())
                        }
                        // S9: an overlay's bytes are drawn from the
                        // preview's own spans, since a preview is a
                        // proposal to read the same bytes as a different
                        // type and the wire row is what shows they are
                        // indeed the same.
                        DisplayRow::Overlay(i) => {
                            self.preview_wire_row(i, indent, palette.as_ref())
                        }
                    };
                    let mut spans = pan_spans(wire_spans.unwrap_or_default(), self.pan_offset);
                    spans.insert(0, Span::raw(" "));
                    Line::from(spans)
                });
                std::iter::once(Line::from(spans)).chain(wire_line)
            })
            .collect();
        // Spec 0230: the half-line scroll, applied once at the end. The
        // rows above are all built the same way whether or not the first
        // of them is fully on screen, so this is where the difference
        // belongs — a `skip`/`repeat` pair rather than anything the row
        // builders have to know about.
        let text_lines = if self.scroll.skip > 0 {
            let cut = (self.scroll.skip as usize).min(text_lines.len());
            text_lines.split_off(cut)
        } else if blank_rows > 0 {
            std::iter::repeat_n(Line::default(), blank_rows)
                .chain(text_lines)
                .collect()
        } else {
            text_lines
        };
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
            // get a colon separator; the `[tag]` suffix is shown only
            // for the latter, and only in full-width mode, since
            // half-width rarely has room for it.
            let type_label = self.status_type_label(self.cursor);
            let left = match type_label {
                Some((t, Some(tag))) if !half_width => {
                    format!("{path_label} {node_path}: {t} [{tag}]")
                }
                Some((t, _)) => format!("{path_label} {node_path}: {t}"),
                None => format!("{path_label} {node_path}"),
            };
            // Spec 0185 S7/Q1 announced the focus lock in words here —
            // the cursor is deliberately left unrestyled, so the main
            // pane looks exactly as it would after a real splice (G3),
            // and something had to say why it was inert.
            //
            // Spec 0193 S3 drops that half: the lock is redundant with
            // `OVERRIDE_FOCUS_LOCK_MESSAGE`, which already fires in the
            // command/message row on both ways of trying to leave and
            // says strictly more (including the way out). What is left
            // means exactly one thing — *what you are looking at is
            // hypothetical* — and its absence correctly means committed
            // content, which is what a candidate that failed to render
            // leaves on screen anyway.
            let left = match self.preview_overlay {
                Some(_) => format!("{left} (preview)"),
                None => left,
            };
            // The byte-range ruler is dropped (not truncated) once a side
            // pane is open and the main pane is only half-width, since
            // there's rarely enough room for both halves — but the line
            // number is short enough to always fit, so it stays.
            // Spec 0193 S4: the cursor ruler answers "where am I", the
            // viewport label answers "where is the window" — panning
            // (`z`/`x`, the wheel) moves the second without moving the
            // first, which is what made a lone cursor ruler look stuck.
            let line_ruler = format!(
                "L{}/{}  {}",
                self.cursor_line() + 1,
                self.total_lines(),
                // Spec 0244 S10: in terminal rows, the unit the signed
                // top is counted in — `total_rows` is document lines,
                // which in wire mode are two rows thick.
                viewport_label(
                    self.scroll_top(),
                    inner.height as usize,
                    total_rows * self.row_height(),
                ),
            );
            let right = if half_width {
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
    }

    /// The global command/message row (spec 0147 G4): a single borderless
    /// `Length(1)` row, always reserved, shared across every pane — never
    /// duplicated per-pane, per the spec's "locality" principle.
    ///
    /// Split out of `render` because it is the one part of it that reads
    /// none of `render`'s locals: it takes its area and is otherwise a
    /// function of the command/message state alone.
    fn render_command_row(&mut self, frame: &mut Frame, area: Rect) {
        let cmd_text = match &self.command_buffer {
            Some(buf) => {
                let prefix = match self.command_kind {
                    CommandLineKind::Command => ':',
                    CommandLineKind::Search(SearchDir::Forward) => '/',
                    CommandLineKind::Search(SearchDir::Backward) => '?',
                };
                format!("{prefix}{buf}")
            }
            None => self.message.clone(),
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
            .split(area);
        self.render_activity_dot(frame, global_row[0]);
        let cmd_row = global_row[1];
        if cmd_text.is_empty() {
            self.cmd_area = None;
            frame.render_widget(Paragraph::new(""), cmd_row);
        } else {
            self.cmd_area = Some(cmd_row);

            // Spec 0127 §G1: cursor char position (including the leading
            // prefix char) within `cmd_text`, `None` while just
            // displaying a plain message (no active edit, so no cursor
            // to keep visible).
            let cursor_pos = self
                .command_buffer
                .is_some()
                .then_some(1 + self.command_cursor);
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
            // Spec 0235 S10: while a search prompt is open, the
            // *pattern* reports the sweep — this row and the `pattern
            // not found` message are the same row, and writing the
            // message per keystroke would flicker the prompt away under
            // the user's hands.
            //
            // Spec 0237 S11 splits that report in three. 0235 tinted a
            // running sweep the same red as a finished one, arguing the
            // two were one fact from the user's seat; on a document
            // large enough for the sweep to be visible at all they are
            // not, and a red that is about to turn out fine teaches the
            // user to ignore red.
            let searching = matches!(self.command_kind, CommandLineKind::Search(_))
                && self.command_buffer.as_ref().is_some_and(|b| !b.is_empty());
            let pattern_style = searching
                .then(|| match &self.search_sweep {
                    // No sweep at all: an empty pane, nothing left to
                    // look in — a finished miss by another name.
                    None => Some(theme::search_unmatched_style(self.theme)),
                    Some(s) if s.found.is_some() => None,
                    Some(s) if s.is_finished() => Some(theme::search_unmatched_style(self.theme)),
                    Some(_) => Some(theme::search_running_style(self.theme)),
                })
                .flatten();
            let raw = match pattern_style {
                Some(style) => {
                    let (prefix, pattern) = cmd_text.split_at(1);
                    vec![
                        Span::raw(prefix.to_string()),
                        Span::styled(pattern.to_string(), style),
                    ]
                }
                None => vec![Span::raw(cmd_text)],
            };
            let spans = pan_spans(raw, self.command_pan_offset);
            frame.render_widget(Paragraph::new(Line::from(spans)), cmd_row);
            if let Some(pos) = cursor_pos {
                let x = cmd_row.x + (pos - self.command_pan_offset) as u16;
                frame.set_cursor_position((x, cmd_row.y));
            }
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

        let list_height = inner.height as usize;
        self.override_list_height = list_height;

        let total_rows = self.override_candidates.len();
        // Auto-pan into view only on genuine highlight movement,
        // mirroring the main pane's own `last_cursor_row` gate above.
        if self.last_override_highlight != Some(self.override_highlight) {
            clamp_scroll_to_visible(
                &mut self.override_scroll,
                self.override_highlight,
                list_height,
            );
            self.last_override_highlight = Some(self.override_highlight);
        }
        let (blank_rows, rows) = self.override_scroll.window(list_height, 1, total_rows);
        let (start, end) = (rows.start, rows.end);

        // Spec 0193 S4: drawn only now, since the viewport label needs
        // the scroll offset this pane's own clamp above just settled.
        let right = format!(
            "L{}/{}  {}",
            self.override_highlight + 1,
            total_rows,
            viewport_label(self.override_scroll.top(1), list_height, total_rows),
        );
        let text = statusline_text(&left, Some(&right), split[1].width as usize);
        frame.render_widget(Paragraph::new(Line::styled(text, style)), split[1]);

        // Warm the wrapper-descriptor registration for the whole
        // currently-visible window ahead of time, so arrowing through
        // already-visible rows never re-pays the registration cost per
        // keystroke.
        self.warm_visible_override_wrappers(start, end);

        // Spec 0244 S9: an over-panned viewport draws blank rows above
        // the first candidate, exactly as the main pane does.
        let mut lines: Vec<Line> = vec![Line::default(); blank_rows];
        for row in start..end {
            // Spec 0114/0137: the lexicographic-mode color scheme is
            // deliberately plain — primitive types (including the `None`
            // sentinel) get the default style rather than a distinct
            // color; enums keep their blue `Attribute` color and carry
            // an explicit ` [enum]` suffix. Factored into
            // `override_row_display` so `override_max_visible_line_len`
            // computes the same text.
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
        let inner = popup_frame(frame, area, 70, 70, " Help (j/k scroll, q/Esc/F1 close) ");
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
    /// `SPLASH_TIMEOUT` elapses — telling the user how to reach the `F1`
    /// help overlay (spec 0113 D22).
    pub(super) fn render_splash(&self, frame: &mut Frame, area: Rect) {
        let inner = popup_frame(frame, area, 60, 30, " protolens ");
        let mut text = vec![
            Line::from(self.header.as_str()),
            Line::from(""),
            Line::from("Press F1 for help."),
        ];
        // Spec 0197 §S3: the descriptor set had to be decoded whole. Said
        // here as well as on stderr, because a TUI launch is exactly the
        // case where nobody was watching stderr.
        if let Some(fallback) = &self.ctx.fallback {
            text.push(Line::from(""));
            text.push(Line::styled(
                format!("warning: {}", fallback.message),
                Style::default().fg(Color::Yellow),
            ));
        }
        frame.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            inner,
        );
    }
}

/// Draws a centered, `Clear`-ed, rounded-bordered modal `percent_x`%
/// wide and `percent_y`% tall over `area`, and hands back the area
/// inside its border for the caller to fill.
///
/// The `Clear` is what makes it a modal rather than an overlay: the
/// popup stands over whatever the pane below already drew, and without
/// it the border encloses stale cells.
fn popup_frame(frame: &mut Frame, area: Rect, percent_x: u16, percent_y: u16, title: &str) -> Rect {
    let popup = centered_rect(percent_x, percent_y, area);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(title.to_string())
        .border_type(BorderType::Rounded);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    inner
}
