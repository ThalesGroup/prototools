// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

use prototext_core::serialize::encode_text::annotation_start;
use std::borrow::Cow;

/// Stand-in for a row that has no styles at all.
static NO_STYLES: LineStyles = Vec::new();

/// The activity dot's glyph (spec 0190 S5/S6) — deliberately its own
/// constant rather than a reuse of `heat_cue::HEAT_GLYPH`, which carries
/// its own meaning (this row's inferred type disagrees with the
/// assigned one). The two say unrelated things and must be free to
/// diverge, and **since spec 0327 they have**: the cue went to `■`
/// because it carries a twelve-step brightness ramp that needs the ink,
/// and this dot carries no scale at all — it is present or absent, in
/// one of a few tier colors, which is what a dot is for.
///
/// `●` is still not an aesthetic choice: it and its plausible
/// alternatives are East-Asian Ambiguous width, so in a CJK-configured
/// terminal they render double-width and would overflow the dot's
/// one-cell slot. That is the same class `heat_cue::HEAT_GLYPH` is in,
/// so the dot cannot break in a configuration the heat cues survive.
pub(super) const ACTIVITY_GLYPH: &str = "●";

/// How many columns the activity dot's field reserves at the left of
/// the global command/message row: the dot, then the blank that keeps
/// it off the leading `:` of a command being typed. The same separation
/// `HEAT_FIELD_WIDTH` buys in the document pane, and its own constant
/// because the two gutters are unrelated and must be free to differ.
pub(super) const ACTIVITY_FIELD_WIDTH: u16 = 2;

/// Spec 0193 S1: the fold marker's glyphs, open (children shown) and
/// closed (children collapsed into `{ ... }`).
///
/// **Enlarged 2026-08-10.** These were `▾`/`▸`, U+25BE and U+25B8 —
/// whose Unicode names say `SMALL TRIANGLE`, and they are. The marker
/// carries a *color* (spec 0260's five-state margin), and a glyph with
/// that little ink is where two of those colors stop being tellable
/// apart, so the mark that most has to be read was the one drawn
/// smallest.
///
/// **Enlarged again 2026-08-18**, the verdict on the medium pair
/// (U+23F7 and U+23F5) having been "almost OK". `▼` U+25BC `BLACK
/// DOWN-POINTING TRIANGLE` and `▶` U+25B6 `BLACK RIGHT-POINTING
/// TRIANGLE` are the largest pair available, and — the requirement that
/// decides between the candidates — a genuine *pair*: the same glyph
/// rotated a quarter turn, same block, same weight, same optical size.
/// `►` U+25BA `BLACK RIGHT-POINTING POINTER` was tried first because it
/// carries no emoji property, and rejected on sight: it is a different
/// shape from `▼`, and the eye reads the mismatch before it reads the
/// direction.
///
/// Two risks are accepted knowingly, both observable in one glance:
///
/// - **U+25B6 carries the Emoji property** — it is the ▶️ play button
///   in `emoji-data.txt` — though its `Emoji_Presentation` is *No*, so
///   a conforming terminal draws it as text. One that reaches for an
///   emoji font anyway draws it colored and double-width, and a glyph
///   supplying its own color destroys the only thing this marker is
///   for: spec 0260's five-state margin. `▼` U+25BC has no emoji
///   property at all, so the failure shows up as an *asymmetry* — the
///   closed marker goes colored and wide, the open one does not.
/// - **Both are East Asian Ambiguous**, where U+23F7/U+23F5 were
///   Neutral, so under a CJK locale a foldable row's text shifts one
///   column right of a non-foldable one. Spec 0322 already admitted an
///   Ambiguous glyph to this very column (`ANOMALY_GLYPH`, `◆` U+25C6)
///   and the heat cue's `●` U+25CF has always been one; this is the
///   same bet, not a new one. (Spec 0327 moved the cue's glyph to `■`
///   U+25A0, which is the same width class again.)
///
/// Should either risk materialize, the ladder back down is
/// U+23F7/U+23F5 (medium, Neutral, non-emoji — also a rotated pair) and
/// then `▾`/`▸` U+25BE/U+25B8 (`SMALL`, the original), each a one-line
/// revert.
pub(super) const FOLD_GLYPH_OPEN: char = '▼';
pub(super) const FOLD_GLYPH_CLOSED: char = '▶';

/// Spec 0193 S1: how many columns the fold field reserves left of the
/// row's own text. Two, because that is the marker plus the space that
/// keeps it from reading as part of the identifier beside it (`▼options`
/// is one token to the eye; `▼ options` is not).
///
/// The field is reserved unconditionally, exactly as spec 0138 N1's
/// heat-cue column is: a field that appeared and vanished with the
/// window's contents would move the text origin as the user scrolls,
/// which is worse than spending the two columns.
pub(super) const FOLD_FIELD_WIDTH: usize = 2;

/// How many columns the heat cue reserves left of the fold field.
///
/// Two, for the same reason `FOLD_FIELD_WIDTH` is two: the glyph plus
/// the blank that keeps it from reading as part of what follows. Spec
/// 0138 N1 spent one column here and the cue ended up flush against the
/// fold marker, so `■ ▼ options` had the cue and the triangle touching
/// and the eye read the pair as one compound mark. The blank costs a
/// column of text and buys the separation.
///
/// Like the fold field, reserved unconditionally — a gutter that came
/// and went with the window's contents would move the text origin as
/// the reader scrolls.
pub(super) const HEAT_FIELD_WIDTH: usize = 2;

/// Spec 0322 S1: the mark a leaf wears in the fold column when its own
/// status is an anomaly — the two tiers `annotation::Tier` names, i.e.
/// `Status::NonCanonical` and `Status::Invalid`.
///
/// A leaf has no fold toggle, so before this the column was blank and
/// the only thing carrying the leaf's status was its annotation, which
/// `a` removes outright and a pan moves off the right edge (0322
/// Background). The diamond is what stays.
///
/// `◆` U+25C6, over the `■` the change was proposed with, for three
/// reasons and one constraint:
///
/// - A filled square is the *stop* mark; a diamond is the caution-sign
///   silhouette, which is the thing being said.
/// - A fully inked cell is the heaviest mark available, and this is the
///   *narrowest*-scoped signal in this column — an ancestor's toggle
///   speaks for a whole subtree. Weighting them the other way round
///   inverts the column.
/// - Two adjacent hues are hardest to tell apart on a solid block, and
///   being able to read the hue is the whole point.
///
/// The constraint is the one `FOLD_GLYPH_OPEN` documents: no Emoji
/// property, or a terminal draws it in an emoji font's own color and
/// destroys the color that is the message. That rules out `▪` U+25AA,
/// `◼` U+25FC, `⬛` U+2B1B, `⏹` U+23F9 and — the real loss — `⚠`
/// U+26A0. U+25C6 carries no emoji property.
///
/// Unlike the triangles this one *is* East Asian Ambiguous, so a
/// CJK-configured terminal may draw it double-width and push a leaf
/// row's text one column right of its siblings'. Accepted: it is the
/// same width class as `heat_cue::HEAT_GLYPH`, which the app already
/// draws in the gutter of every row, and the narrow alternatives
/// (`◇` U+25C7 hollow, `⋄` U+22C4 tiny) give up the legibility the
/// mark exists for.
pub(super) const ANOMALY_GLYPH: char = '◆';

/// Spec 0349 S5: hollow counterpart of [`ANOMALY_GLYPH`], worn by a leaf
/// whose status is exactly `Shadowed` — overridden by a later occurrence
/// but free of any other annotation.  `◇` U+25C7 `WHITE DIAMOND`, same
/// block and width class as U+25C6.
pub(super) const HOLLOW_ANOMALY_GLYPH: char = '◇';

/// Spec 0349 S5: hollow open-fold toggle for a node whose subtree is
/// entirely `Shadowed` (no genuine non-canonical annotation anywhere).
/// `▽` U+25BD `WHITE DOWN-POINTING TRIANGLE`.
pub(super) const FOLD_GLYPH_OPEN_HOLLOW: char = '▽';

/// Spec 0349 S5: hollow closed-fold toggle, counterpart of
/// [`FOLD_GLYPH_CLOSED`].  `▷` U+25B7 `WHITE RIGHT-POINTING TRIANGLE`.
pub(super) const FOLD_GLYPH_CLOSED_HOLLOW: char = '▷';

/// Spec 0318 S7: the bar an override preview draws down the fold column
/// of its own rows. A box-drawing light vertical (U+2502), not one of
/// the block elements: the column is one cell wide and shared with a
/// triangle on the rows above and below, so the mark has to read as a
/// rule rather than as a filled cell.
pub(super) const TIER_BAR_GLYPH: char = '│';

/// Alias for [`TIER_BAR_GLYPH`]: all bars use the same thin glyph.
/// Kept so callers that conceptually refer to the near bar remain
/// self-documenting.
pub(super) const NEAR_BAR_GLYPH: char = '│';

/// Spec 0328 S1/S2 and 0334 S1: where the caret node's bar and each of
/// its ancestors' run, and under which document shape and caret they
/// were worked out.
///
/// Held in a `RefCell` on `App` for the same reason `WireRowCache` is:
/// `absolute_start` is a climb and every row of the viewport asks. With
/// the ancestors in it the fill is a climb per ancestor, which is what
/// makes the memo load-bearing rather than merely nice.
pub(super) struct CursorBarCache {
    version: u64,
    cursor: usize,
    /// Nearest node first: the caret node, then its parent, up to the
    /// root. Spec 0334 S4 reads the order to break a column tie.
    bars: Vec<CursorBar>,
}

/// The rows one node's bar occupies, and the column it occupies.
#[derive(Clone, Copy)]
struct CursorBar {
    /// The node the bar belongs to — the caret node, or one of its
    /// ancestors. Spec 0334 S2 takes the color from *this* node.
    owner: usize,
    /// The node's header line — the row its fold triangle is on, and so
    /// the row directly above the bar's first document row.
    header: usize,
    /// One past the node's last line, i.e. past its closing brace.
    end: usize,
    /// `marker_column` of the header, which is the column that
    /// triangle sits in (S1). Also a byte offset into any row's fold
    /// margin, which is spaces up to and including it.
    column: usize,
}

impl CursorBar {
    /// The document rows the bar runs down — the node's subtree minus
    /// its header, which keeps its triangle (spec 0328 S2), and minus
    /// its closing brace.
    ///
    /// The brace is the node's own extent made visible; a bar beside it
    /// says the same thing twice, and the pair reads as one mark that
    /// overshot. Stopping a row short also leaves the bar pointing *at*
    /// the brace rather than running past it, which is where a reader
    /// following the bar down is going.
    fn covers(&self, line: usize) -> bool {
        line > self.header && line + 1 < self.end
    }

    /// Spec 0328 S5: a wire row is a continuation of the row above, so
    /// the header's own wire row — directly under the triangle — takes
    /// the bar as well. The closing brace's wire row is excluded with
    /// the brace itself, for the reason [`CursorBar::covers`] gives.
    fn covers_wire(&self, line: usize) -> bool {
        line >= self.header && line + 1 < self.end
    }

    fn glyph(&self) -> char {
        TIER_BAR_GLYPH
    }
}

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

/// Spec 0362: one contiguous piece of a rendered row.
///
/// `Raw` pieces borrow from the caller's `&str` — zero allocation.
/// `Lit` pieces are `&'static str` — also zero allocation.
pub(super) enum DisplayPiece<'a> {
    /// A contiguous slice of the raw line text, with its byte range in
    /// that raw text and the syntax role the highlighter assigned to it
    /// (`None` for unstyled gaps between hints).
    Raw {
        text: &'a str,
        range: Range<usize>,
        role: Option<SyntaxRole>,
    },
    /// A literal insertion with no backing in the raw text.
    /// `style` is `Some` when the caller has pre-computed the color
    /// (e.g. the bake-unread fold color); `None` when `row_spans`
    /// derives it from `role`.
    Lit {
        text: &'static str,
        role: Option<SyntaxRole>,
        style: Option<Style>,
    },
}

/// Spec 0362 S2/S3: forward-only iterator of display pieces for one
/// rendered row.  All state is passed explicitly so that `row_spans`
/// can hold a `&LineStyles` borrow at the call site without conflict.
///
/// Phases (each fires at most once, in order):
///   0. Root prefix: `"1"` → `"/"` at byte 0 when `owner ==
///      Some(first_node)`.
///   1. Raw body: raw bytes `[cursor .. end)` split at hint boundaries.
///      `end` = `brace_pos` when folded, else annotation-hide boundary.
///   2. Fold summary: emit `"{ ... }"` when folded.
///   3. Shadowed suffix: emit `";"`  + `" shadowed_scalar"` when
///      `shadowed && annotations && !fold_closed`.
pub(super) fn display_pieces<'a>(
    raw: &'a str,
    owner: Option<usize>,
    hints: &'a LineStyles,
    first_node: usize,
    fold_closed: bool,
    fold_style: Option<Style>,
    shadowed: bool,
    annotations: bool,
) -> impl Iterator<Item = DisplayPiece<'a>> {
    // Pre-compute everything the phases need so the iterator closure is
    // a pure forward walk with no searching inside it.
    let is_root = owner == Some(first_node) && raw.starts_with("1 ");
    // When folded: cut Phase 1 just after the last `{`, then Phase 2
    // inserts `" ... }"` at that point.  Bytes after the `{` (i.e. the
    // content inside the braces) are skipped by jumping `cursor` to
    // `raw.len()` after Phase 2, while the annotation clause that
    // follows on the same header line is re-emitted from Phase 1b.
    //
    // Concretely, `"  a {  #@ Mid = 1"` folds to
    // `"  a { ... }  #@ Mid = 1"`: the annotation is NOT suppressed,
    // only the message body between `{` and the closing `}` is.
    let brace_pos: Option<usize> = if fold_closed { raw.rfind('{') } else { None };
    // Phase 1 runs in two sub-windows:
    //   window_a: [raw_start .. brace_pos)   — up to but not including `{`
    //   (Phase 2 emits the full "{ ... }" at this point)
    //   window_b: [brace_pos+1 .. raw_end)   — the annotation clause
    //     after the `{`; suppressed when !annotations.
    // When not folded, window_a covers [raw_start .. raw_end) and there
    // is no window_b.
    let annot_end = if !annotations {
        annotation_start(raw).unwrap_or(raw.len())
    } else {
        raw.len()
    };
    let (window_a_end, window_b_start, window_b_end) = match brace_pos {
        // When folded: Phase 1a ends before `{`, Phase 1b re-emits the
        // annotation clause after `{`, but only up to `annot_end` when
        // `!annotations` (suppressing the `#@` clause).
        Some(pos) => (pos, pos + 1, annot_end),
        None => (annot_end, annot_end, annot_end),
    };
    // Cursor into `raw`; starts at 1 when the root prefix fires
    // (byte 0 = `"1"` is replaced by the `"/"` Lit).
    let raw_start = usize::from(is_root);

    // The iterator is a small state machine collected into a Vec up
    // front.  Total items is O(grammar tokens per line) — at most ~9.
    let mut pieces: Vec<DisplayPiece<'a>> = Vec::with_capacity(10);

    // Phase 0: root prefix.
    if is_root {
        let role = hints.iter().find(|(r, _)| r.contains(&0)).map(|(_, r)| *r);
        pieces.push(DisplayPiece::Lit {
            text: "/",
            role,
            style: None,
        });
    }

    // Helper: emit raw bytes [from..to) split at hint boundaries.
    let emit_raw = |pieces: &mut Vec<DisplayPiece<'a>>, from: usize, to: usize| {
        let mut cursor = from;
        for (hint_range, hint_role) in hints.iter() {
            let h_start = hint_range.start.max(cursor);
            let h_end = hint_range.end.min(to);
            if h_start >= h_end {
                if hint_range.start >= to {
                    break;
                }
                cursor = cursor.max(hint_range.end);
                continue;
            }
            if h_start > cursor {
                pieces.push(DisplayPiece::Raw {
                    text: &raw[cursor..h_start],
                    range: cursor..h_start,
                    role: None,
                });
            }
            pieces.push(DisplayPiece::Raw {
                text: &raw[h_start..h_end],
                range: h_start..h_end,
                role: Some(*hint_role),
            });
            cursor = h_end;
        }
        if cursor < to {
            pieces.push(DisplayPiece::Raw {
                text: &raw[cursor..to],
                range: cursor..to,
                role: None,
            });
        }
    };

    // Phase 1a: raw body up to (and including) the `{`, or to raw_end.
    emit_raw(&mut pieces, raw_start, window_a_end);

    // Phase 2: fold summary replaces the `{` and its content.
    if fold_closed {
        pieces.push(DisplayPiece::Lit {
            text: "{ ... }",
            role: None,
            style: fold_style,
        });
    }

    // Phase 1b: text after the `{` (the annotation clause).
    // When not folded, window_b_start == window_a_end so this is a
    // no-op — Phase 1a already covered the whole line.
    if window_b_start < window_b_end {
        emit_raw(&mut pieces, window_b_start, window_b_end);
    }

    // Phase 3: shadowed-scalar suffix — appended regardless of whether
    // the node is folded; the suffix follows the annotation clause.
    if shadowed && annotations {
        pieces.push(DisplayPiece::Lit {
            text: ";",
            role: Some(SyntaxRole::Comment),
            style: None,
        });
        pieces.push(DisplayPiece::Lit {
            text: " shadowed_scalar",
            role: None,
            style: None,
        });
    }

    pieces.into_iter()
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

/// Spec 0274 S13: `restyle_range` over a range of a *row's text*, which
/// the fold margin offsets and the pan and the pane's right edge then
/// cut down to what is drawn.
///
/// Clamped to the pan rather than dropped past it, unlike the
/// single-row loop's `checked_sub`: every row of a multi-row match but
/// the first is tinted from its column 0, which is off to the left of
/// any pan at all.
fn tint_columns(
    spans: &mut Vec<Span<'static>>,
    columns: Range<usize>,
    pan: usize,
    width: usize,
    style: Style,
) {
    let lo = FOLD_FIELD_WIDTH + columns.start.max(pan) - pan + HEAT_FIELD_WIDTH;
    let hi = (FOLD_FIELD_WIDTH + columns.end.max(pan) - pan + HEAT_FIELD_WIDTH).min(width);
    if lo < hi {
        restyle_range(spans, lo..hi, |s| s.patch(style));
    }
}

/// Spec 0339 S1: where a row's text sits among the cells drawn for it.
///
/// `pan` columns of the text are scrolled off to the left; `lead` cells
/// of gutter precede what is left of it and `trail` more follow that
/// gutter; the row is cut at `width`. The main pane's two gutters are
/// the fold margin and the heat field, in that order and on either side
/// of the pan; a side pane has neither.
#[derive(Clone, Copy)]
pub(super) struct RowCells {
    pub pan: usize,
    pub lead: usize,
    pub trail: usize,
    pub width: usize,
}

/// Spec 0339 S1: tint every occurrence of `pattern` in `text`, each in
/// the style `style_of` gives for its start column.
///
/// `text` is the row as **drawn**, not the haystack the sweep matched
/// against (S3): the two differ in both side panes, and the reader's
/// eye is on the former. That is also why this re-scans rather than
/// mapping a hit's byte offsets — the other occurrences have no hit to
/// map, and while a pattern is still being typed neither has the
/// current one.
///
/// `find_range_from` rather than a slice: spec 0273 S6 makes `^` and
/// `\b` depend on what precedes the offset, which a slice would hide.
/// The loop is bounded by the text as well as by the matches because a
/// regex may match nothing at all, and an empty match at the end would
/// otherwise be re-found forever. Stepping past a match's *start*
/// rather than its end is what keeps overlapping occurrences honest.
pub(super) fn tint_matches(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    pattern: &SearchPattern,
    cells: RowCells,
    style_of: impl Fn(usize) -> Style,
) {
    let mut at = 0;
    while at <= text.len() {
        let Some(found) = pattern.find_range_from(text, at) else {
            break;
        };
        let start = char_column(text, found.start);
        let chars = text[found.clone()].chars().count();
        at = found.start + text[found.start..].chars().next().map_or(1, char::len_utf8);
        let style = style_of(start);
        let Some(index) = (cells.lead + start).checked_sub(cells.pan) else {
            continue;
        };
        let lo = index + cells.trail;
        let hi = (lo + chars).min(cells.width);
        if lo < hi {
            restyle_range(spans, lo..hi, |s| s.patch(style));
        }
    }
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

    /// `committed_row_of`'s inverse, for a whole run: the display rows
    /// that draw committed rows `rows`.
    ///
    /// The overlay's rows are **atomic** — a run reaching any part of
    /// the block they stand in for reaches all of them. That is the
    /// only rule available. An overlay row has no node (spec 0185 N6),
    /// so it cannot be named by line; and its spans index the preview's
    /// own byte buffer, which a truncated interior (spec 0174) makes
    /// not a sub-slice of the blob, so it cannot be named by byte
    /// either. What is left is the substitution itself: those rows
    /// stand in for that block, so they answer whatever the block
    /// answered.
    pub(super) fn display_rows_of(&self, rows: std::ops::Range<usize>) -> std::ops::Range<usize> {
        let Some(o) = &self.preview_overlay else {
            return rows;
        };
        let (first, covered) = (o.first_row, o.covered_rows);
        // Past the block, a row is displaced by however many rows the
        // overlay added or removed. `rows.start >= first + covered`
        // there, so the subtraction cannot underflow.
        let past = |c: usize| (c - covered) + o.lines.len();
        let start = if rows.start < first {
            rows.start
        } else if rows.start < first + covered {
            first
        } else {
            past(rows.start)
        };
        let end = if rows.end <= first {
            rows.end
        } else if rows.end <= first + covered {
            first + o.lines.len()
        } else {
            past(rows.end)
        };
        start..end
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
    /// Spec 0318 S8: every row is prototext. Spec 0174 S4's `...` marker
    /// was the one row that was not, and had to be blanked here — `...`
    /// is not in the grammar, highlighting happens at draw time, and a
    /// syntax error would silently strip the color off every row
    /// *beneath* it through tree-sitter's error recovery. It is gone: a
    /// truncated preview now says so with a bar in its fold column,
    /// which is margin rather than document and is never parsed.
    fn window_text(&self, window: &[DisplayRow]) -> Vec<String> {
        window
            .iter()
            .map(|&row| self.display_row_text(row).into_owned())
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
    /// and its `▼`/`▶` are gutter furniture, and pasting them back into
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
    /// hence the `{ ... }` collapse summary (and spec 0343 B10's
    /// `; shadowed_scalar` word), never the underlying text.
    pub(super) fn row_text_of(&self, row: DisplayRow, owner: Option<usize>) -> String {
        let source = self.display_row_text(row);
        let content = if !self.annotations {
            code_part(&source)
        } else {
            &source
        };
        self.line_display_text(content, owner)
    }

    /// Spec 0362 S5: the single source of truth for display-transformed
    /// line text, built by concatenating `display_pieces`.
    ///
    /// - `"1"` → `"/"` on the root node header (spec 0361).
    /// - `"{ ... }"` fold summary on a closed node's header (spec 0194).
    /// - `"; shadowed_scalar"` suffix on a shadowed slot (spec 0343 B10).
    ///
    /// `content` is the raw line text (before annotation hiding).  The
    /// caller is responsible for stripping the annotation suffix first
    /// when `self.annotations` is false — `row_text_of` does this via
    /// `code_part`; the search path passes the raw text and the
    /// `annotations` flag gates the shadowed-scalar branch inside
    /// `display_pieces`.
    pub(super) fn line_display_text(&self, content: &str, owner: Option<usize>) -> String {
        let fold_closed = self.fold_marker_of(owner) == Some(FOLD_GLYPH_CLOSED);
        // `content` is already annotation-hidden by the caller (via
        // `code_part`) when `!self.annotations`, so pass
        // `annotations=true` to prevent `display_pieces` applying the
        // annotation-hiding truncation a second time. Phase 3 (the
        // shadowed suffix) must still be suppressed when
        // `!self.annotations`, so `shadowed` is masked by that flag.
        let shadowed = self.annotations && owner.is_some_and(|i| self.is_shadowed(i));
        display_pieces(
            content,
            owner,
            &NO_STYLES,
            self.first_node,
            fold_closed,
            None,  // fold_style: irrelevant, text-only path
            shadowed,
            true, // annotation hiding already done by caller
        )
        .map(|p| match p {
            DisplayPiece::Raw { text, .. } => text,
            DisplayPiece::Lit { text, .. } => text,
        })
        .collect()
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
        (fold_margin(indent_len, self.margin_glyph(row)), indent_len)
    }

    /// The glyph this row's node currently warrants in the fold column,
    /// or `None` when the row has no node of its own (footer and
    /// overlay rows) or its node warrants neither mark.
    fn margin_glyph(&self, row: DisplayRow) -> Option<char> {
        self.margin_glyph_of(self.display_row_source(row).1)
    }

    /// Spec 0322 S2: the one decision about what occupies the fold
    /// column — the node's fold toggle when it has one, else spec
    /// 0322's anomaly mark when its own status is one.
    ///
    /// The two cases cannot both apply: a node with children has a
    /// toggle, and the toggle already carries the subtree roll-up that
    /// includes the node's own status.
    ///
    /// Spec 0215 S6: takes the owner alone. Once it is known the row
    /// itself is not consulted, so passing it would be an unused
    /// argument rather than documentation.
    fn margin_glyph_of(&self, owner: Option<usize>) -> Option<char> {
        let idx = owner?;
        match self.fold_marker_of(owner) {
            Some(glyph) => Some(glyph),
            // N1: `Unknown` and `Unbaked` are absence of information,
            // not defects, and both are near-universal where they occur
            // — an untyped document is `Unknown` throughout (spec 0247
            // S12), so marking it would mark every leaf on screen.
            // Spec 0349 S3: Shadowed gets a hollow diamond; NonCanonical
            // and above get the filled one.
            None => match self.status_of(idx) {
                s if s >= Status::NonCanonical => Some(ANOMALY_GLYPH),
                Status::Shadowed => Some(HOLLOW_ANOMALY_GLYPH),
                _ => None,
            },
        }
    }

    /// Spec 0193 S1: the *fold toggle* alone, `None` for a node with
    /// nothing to fold.
    ///
    /// Narrower than `margin_glyph_of` on purpose: `row_text_of` asks
    /// this to decide whether to splice in `{ ... }`, and that is a
    /// property of being folded, not of occupying the column.
    fn fold_marker_of(&self, owner: Option<usize>) -> Option<char> {
        let idx = owner?;
        if !self.has_children(idx) {
            return None;
        }
        // Spec 0349 S4: hollow glyphs when the subtree's worst signal is
        // exactly Shadowed — no genuine non-canonical annotation anywhere.
        Some(if self.is_folded(idx) {
            if self.status_of(idx) == Status::Shadowed {
                FOLD_GLYPH_CLOSED_HOLLOW
            } else {
                FOLD_GLYPH_CLOSED
            }
        } else if self.status_of(idx) == Status::Shadowed {
            FOLD_GLYPH_OPEN_HOLLOW
        } else {
            FOLD_GLYPH_OPEN
        })
    }

    /// Spec 0247 S10: the color this row's margin glyph wears — the
    /// worst status anywhere in its node's subtree, `None` when that is
    /// `Ok`.
    ///
    /// For a leaf that is the node's own status, because
    /// `rolled = own.max(children…)` and there are no children. Spec
    /// 0322 S3: a leaf never reaches here in the `Ok` color, since a
    /// clean leaf is given no glyph to color.
    ///
    /// Applied to the glyph alone by `margin_spans`, which is where the
    /// reason it cannot be the whole margin is written down.
    pub(super) fn margin_glyph_color(&self, owner: Option<usize>) -> Option<Color> {
        let idx = owner?;
        self.margin_glyph_of(owner)?;
        theme::status_color(self.status_of(idx), self.theme)
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
        if self.is_folded(self.cursor) && self.has_children(self.cursor) {
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
    /// heat glyph's reserved column and index 1 the blank beside it.
    /// `None` when a horizontal pan has
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
            Some((FOLD_FIELD_WIDTH + column).checked_sub(self.pan_offset)? + HEAT_FIELD_WIDTH)
        } else {
            Some(HEAT_FIELD_WIDTH + panned_chars + (column - text_chars))
        }
    }

    /// Styled counterpart of `row_content` (spec 0116 §7/§9): applies the
    /// row's syntax-highlighting spans via `theme::style_for`, prepends
    /// the same `fold_margin` `row_content` does, and splices in the same
    /// `" ... }"` collapse-summary text — the two **must** agree
    /// byte for byte, and a test asserts they do.
    ///
    /// Spec 0362: uses `display_pieces` as the single source of truth
    /// for all display transforms (root label, fold summary,
    /// shadowed-scalar suffix, annotation hiding).
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
    /// lands on the three places that say what an override *is* — the
    /// row's key, its fold marker, and the type name in its `#@ Type =
    /// N` annotation — and nowhere else. Which of the three are on
    /// screen varies (a leaf has no marker, `a` hides the annotation),
    /// and the key is always there, which is what keeps the cue from
    /// disappearing.
    pub(super) fn row_spans(
        &self,
        row: DisplayRow,
        window_index: usize,
        emphasis: Modifier,
    ) -> Vec<Span<'static>> {
        let (source, node) = self.display_row_source(row);
        let hints = self.window_styles.get(window_index).unwrap_or(&NO_STYLES);

        let fold_closed = node.is_some_and(|i| self.is_folded(i) && self.has_children(i));
        let fold_style = node.and_then(|i| self.unread_fold_style(i));
        let shadowed = node.is_some_and(|i| self.is_shadowed(i));

        // Spec 0307 S6 / 0361: the root row's `"/"` key is synthetic —
        // protolens invented the field number, not the file.
        let synthetic_key =
            self.wrapper_offset > 0 && node.is_some_and(|idx| self.parent(idx).is_none());

        // Spec 0362 S6: pre-compute the amber kw_style for the
        // shadowed-scalar suffix once, outside the piece loop.
        let kw_style = if shadowed && self.annotations {
            theme::status_color(Status::NonCanonical, self.theme)
                .map(|c| Style::default().fg(c))
        } else {
            None
        };

        // Spec 0193 S1: the fold margin replaces the row's leading
        // indentation.  Compute `body_start` (the byte where the margin
        // ends) from the raw source before display_pieces sees it, so
        // the margin spans come first and the pieces start after.
        let (margin, body_start) = self.fold_margin_of(row, &source);

        // Spec 0362 S3: iterate display pieces.  Each piece is either a
        // raw slice of `source` with a syntax role from `window_styles`,
        // or a static-literal insertion.  Pieces whose `range.end <=
        // body_start` belong to the margin and are skipped here (the
        // margin spans already cover them).
        let mut spans = Vec::with_capacity(hints.len() + 8);
        if !margin.is_empty() {
            spans.extend(self.margin_spans(margin, row, node, emphasis));
        }

        // Spec 0192 S2: weight the *first* Attribute piece as the key.
        let mut key_seen = false;
        for piece in display_pieces(
            &source,
            node,
            hints,
            self.first_node,
            fold_closed,
            fold_style,
            shadowed,
            self.annotations,
        ) {
            match piece {
                DisplayPiece::Raw { text, range, role } => {
                    // Skip bytes that belong to the fold margin.
                    if range.end <= body_start {
                        continue;
                    }
                    let text = if range.start < body_start {
                        &text[body_start - range.start..]
                    } else {
                        text
                    };
                    if text.is_empty() {
                        continue;
                    }
                    let key = matches!(role, Some(SyntaxRole::Attribute))
                        && !std::mem::replace(&mut key_seen, true);
                    let mut weight = if key || matches!(role, Some(SyntaxRole::Type)) {
                        emphasis
                    } else {
                        Modifier::empty()
                    };
                    if key && synthetic_key {
                        weight |= theme::SYNTHETIC;
                    }
                    spans.push(self.make_span(text.to_string(), role, weight));
                }
                DisplayPiece::Lit { text, role, style } => {
                    // Spec 0362 S6: Lit pieces participate in key/type
                    // weighting just like Raw pieces. The "/" root label
                    // is the first Attribute and must get emphasis +
                    // SYNTHETIC. The shadowed suffix and fold summary
                    // carry explicit styles and bypass weighting.
                    let span = if text == " shadowed_scalar" {
                        match kw_style {
                            Some(s) => Span::styled(text, s),
                            None => Span::raw(text),
                        }
                    } else if let Some(s) = style {
                        // Pre-styled piece (e.g. bake-unread fold color).
                        Span::styled(text, s)
                    } else {
                        let key = matches!(role, Some(SyntaxRole::Attribute))
                            && !std::mem::replace(&mut key_seen, true);
                        let mut weight = if key || matches!(role, Some(SyntaxRole::Type)) {
                            emphasis
                        } else {
                            Modifier::empty()
                        };
                        if key && synthetic_key {
                            weight |= theme::SYNTHETIC;
                        }
                        self.make_span(text.to_string(), role, weight)
                    };
                    spans.push(span);
                }
            }
        }

        // Spec 0328 S6/S7: overlay-ellipsis is a preview-overlay
        // concern, not a node_text transform — kept as a post-loop
        // append rather than in display_pieces (spec 0362 S6).
        if let (DisplayRow::Overlay(i), Some(o)) = (row, self.preview_overlay.as_ref()) {
            if o.ellipsis_row == Some(i) {
                let style = self.preview_bar_color(o).map(|c| Style::default().fg(c));
                let span = match style {
                    Some(s) => Span::styled("...", s),
                    None => Span::raw("..."),
                };
                spans.push(span);
            }
        }

        spans
    }

    /// The fold margin: one span when it has nothing to say, three when
    /// it does.
    ///
    /// The glyph alone is styled, not the whole margin. A foreground
    /// color would show on the surrounding spaces either way (spec 0247
    /// S10 settled for one span on that argument), but an underline
    /// does not — it would draw a rule across the indentation, which
    /// says nothing about the node and everything about how deep it is.
    /// The two extra spans are paid only on a row that has a marker
    /// *and* something to say about it.
    ///
    /// And the glyph never wears the underline either, whatever spec
    /// 0192's weight says: the marker is a triangle a single rule runs
    /// straight through, and the two together read as neither. The
    /// bold half of a manual override's weight still lands here, and
    /// the underline still lands on the row's key and type name, which
    /// is where it can be read.
    /// Spec 0318 S7: an overlay row takes its margin from
    /// `overlay_margin_spans` instead. The column is free on every row of
    /// a preview whatever `--indent` is set to, because an overlay row
    /// has no owner and so `margin_glyph_of` gives it no glyph.
    fn margin_spans(
        &self,
        margin: String,
        row: DisplayRow,
        owner: Option<usize>,
        emphasis: Modifier,
    ) -> Vec<Span<'static>> {
        if let DisplayRow::Overlay(index) = row {
            return self.overlay_margin_spans(margin, index);
        }
        // Marked cells, as `(byte range in `margin`, what to draw
        // there, how)`: the row's own glyph, and one bar per node on the
        // path from the root to the caret (spec 0334 S1).
        let mut marks: Vec<(Range<usize>, String, Style)> = Vec::with_capacity(2);

        let emphasis = emphasis - Modifier::UNDERLINED;
        let color = self.margin_glyph_color(owner);
        if !(color.is_none() && emphasis.is_empty()) {
            if let Some(at) = self.margin_glyph_of(owner).and_then(|g| margin.find(g)) {
                let end = at + margin[at..].chars().next().map_or(0, char::len_utf8);
                let mut style = Style::default().add_modifier(emphasis);
                if let Some(color) = color {
                    style = style.fg(color);
                }
                marks.push((at..end, margin[at..end].to_string(), style));
            }
        }

        if let Some(line) = row.committed_line() {
            for (column, glyph, style) in self.bars_on_row(&margin, line, false) {
                marks.push((column..column + 1, glyph.to_string(), style));
            }
        }

        if marks.is_empty() {
            return vec![Span::raw(margin)];
        }
        marks.sort_by_key(|(range, _, _)| range.start);
        let mut spans = Vec::with_capacity(2 * marks.len() + 1);
        let mut cut = 0;
        for (range, text, style) in marks {
            if range.start > cut {
                spans.push(Span::raw(margin[cut..range.start].to_string()));
            }
            spans.push(Span::styled(text, style));
            cut = range.end;
        }
        if cut < margin.len() {
            spans.push(Span::raw(margin[cut..].to_string()));
        }
        spans
    }

    /// Spec 0334 S1/S4: the bars to draw into `margin` on committed line
    /// `line`, as `(byte column, style)` — at most one per column.
    ///
    /// `wire` selects `covers_wire` over `covers`, because a wire row is
    /// a continuation of the row above it and so takes that row's
    /// header's bar too (spec 0328 S5).
    ///
    /// Two filters, in this order:
    ///
    /// - Spec 0328 S4: **the row's own mark wins the cell.** A bar goes
    ///   in only where this row's margin is blank at the column, which
    ///   under `--indent 2` is always — a child's marker is at least two
    ///   columns deeper — and under `--indent 0`/`1` is the test that
    ///   lets a child's triangle, a control and the row's own, outrank
    ///   an ancestor's readout. One byte compare, and it covers every
    ///   `--indent`. It also keeps a bar out of the *middle* of a
    ///   multi-byte glyph, whose continuation bytes are not `b' '`.
    /// - Spec 0334 S4: **the nearer node's bar wins a shared column.**
    ///   The cache is ordered nearest-first, so keeping the first
    ///   claimant is that rule. It is also what keeps the returned
    ///   columns distinct, which the splice in `margin_spans` needs.
    fn bars_on_row(&self, margin: &str, line: usize, wire: bool) -> Vec<(usize, char, Style)> {
        // Collected under the borrow, styled outside it: the color comes
        // from `status_of`, which tracks a node's heat and so must not
        // be frozen into the cache.
        let claimants: Vec<CursorBar> = {
            self.fill_cursor_bars();
            let cache = self.cursor_bar.borrow();
            let Some(cache) = cache.as_ref() else {
                return Vec::new();
            };
            cache
                .bars
                .iter()
                .copied()
                .filter(|b| {
                    if wire {
                        b.covers_wire(line)
                    } else {
                        b.covers(line)
                    }
                })
                .filter(|b| margin.as_bytes().get(b.column) == Some(&b' '))
                .collect()
        };
        let mut out: Vec<(usize, char, Style)> = Vec::with_capacity(claimants.len());
        for bar in claimants {
            if out.iter().any(|&(column, _, _)| column == bar.column) {
                continue;
            }
            out.push((bar.column, bar.glyph(), self.bar_style(&bar)));
        }
        out
    }

    /// Spec 0349 S9: a bar wears a dimmed variant of its owner's status
    /// color (same hue, ~60% luminance), so it recedes behind the fold
    /// toggle it descends from. An `Ok` node has no status color and
    /// draws in the terminal's default foreground.
    ///
    /// Near and far bars are not told apart. All bars use the same thin
    /// glyph ([`TIER_BAR_GLYPH`]) and same dimmed color formula.
    fn bar_style(&self, bar: &CursorBar) -> Style {
        match theme::bar_status_color(self.status_of(bar.owner), self.theme) {
            Some(color) => Style::default().fg(color),
            None => Style::default(),
        }
    }

    /// Brings [`App::cursor_bar`]'s memo up to date with the current
    /// document shape and caret.
    fn fill_cursor_bars(&self) {
        if let Some(cache) = self.cursor_bar.borrow().as_ref() {
            if cache.version == self.structural_version && cache.cursor == self.cursor {
                return;
            }
        }
        let bars = self.compute_cursor_bars();
        *self.cursor_bar.borrow_mut() = Some(CursorBarCache {
            version: self.structural_version,
            cursor: self.cursor,
            bars,
        });
    }

    /// The uncached half of [`App::fill_cursor_bars`]: one bar per node
    /// on the path from the caret to the root, nearest first.
    ///
    /// A node the walk passes may contribute none — see
    /// [`App::compute_bar`] — and the walk carries on regardless, which
    /// is spec 0334 S1's "a caret on a leaf still draws its ancestors'
    /// bars".
    ///
    /// All bars use the same glyph and the same style formula, so
    /// near/far has no visual representation beyond relative column
    /// position. The one case with no bars at all: nothing on the path
    /// from the caret to the root draws one, which takes a caret at the
    /// top of a document whose root is folded or unbracketed.
    fn compute_cursor_bars(&self) -> Vec<CursorBar> {
        let mut bars = Vec::new();
        let mut idx = self.cursor;
        loop {
            if let Some(bar) = self.compute_bar(idx) {
                bars.push(bar);
            }
            match self.parent(idx) {
                Some(parent) => idx = parent,
                None => return bars,
            }
        }
    }

    /// Spec 0328 S1/S2: one node's bar.
    ///
    /// `None` for a node with nothing to draw a bar beside: a **leaf**,
    /// whose `lines_total` is 1, and a **folded** node, which draws its
    /// body as the one-row `{ ... }` collapse — right in both cases,
    /// since a collapsed node's extent is the row you are on.
    ///
    /// Also `None` at two lines — a header and its closing brace with
    /// no interior, which `covers` excludes both of. **`Some` means
    /// *draws at least one cell*, and [`App::compute_cursor_bars`]
    /// relies on that**: a bar covering nothing would still take the
    /// undimmed slot and leave the screen with no visible undimmed bar.
    /// That is also why the fold is now tested for outright rather than
    /// left to fall out of the line count — a folded node's rows are
    /// not on screen at all.
    ///
    /// The range is in **absolute line numbers**, and all three terms
    /// have to agree on that: `absolute_start` sums `lines_total`, and
    /// `covers` is asked about `CommittedRow::line`, which is absolute
    /// too. So the extent is `lines_total`, never `lines_visible` —
    /// mixing the two makes the bar stop short by however many lines a
    /// folded descendant hides. Those hidden lines fall inside the
    /// range and no drawn row carries their numbers, so counting them
    /// costs nothing.
    ///
    /// O(1) — no per-row walk up each row's ancestors, and no second
    /// definition of "in this node".
    fn compute_bar(&self, idx: usize) -> Option<CursorBar> {
        if idx >= self.tree.len() || !self.has_children(idx) || self.is_folded(idx) {
            return None;
        }
        let total = self.tree[idx].lines_total as usize;
        if total < 3 {
            return None;
        }
        let header = self.absolute_start(idx);
        let text = self.node_text[idx].as_deref()?;
        Some(CursorBar {
            owner: idx,
            header,
            end: header + total,
            column: marker_column(text.split('\n').next()?) as usize,
        })
    }

    /// Spec 0318 S7: what an overlay row draws in its fold column, given
    /// the row's index within the preview.
    ///
    /// Row 0 is the previewed node's own header, and it keeps whatever
    /// that node draws in the column — the reader is deciding about
    /// *that* node, and covering its mark would take away the one
    /// control, or the one warning, on the row they are looking at.
    /// The overlay hides the committed row, so the
    /// glyph has to be drawn here or not at all; `override_target` is the
    /// node it belongs to.
    ///
    /// The bar runs from row 1 to the closing brace, starting directly
    /// below the triangle. It says two things at once: *these rows are
    /// the preview*, which the reader would otherwise have to infer, and
    /// whether they are all of it.
    ///
    /// It draws [`NEAR_BAR_GLYPH`] (same as [`TIER_BAR_GLYPH`]). A
    /// preview is the node the reader is deciding about, and the overlay
    /// hides that node's committed rows, so no bar is doubled.
    fn overlay_margin_spans(&self, margin: String, index: usize) -> Vec<Span<'static>> {
        let Some(o) = self.preview_overlay.as_ref() else {
            return vec![Span::raw(margin)];
        };
        // The margin is `FOLD_FIELD_WIDTH + indent` spaces and the column
        // is the one the *first* line's marker would occupy, which is the
        // shallowest — so it always lands inside. A blank row has no
        // margin at all and never reaches here.
        let at = o.tier_column;
        if at >= margin.len() {
            return vec![Span::raw(margin)];
        }
        let (glyph, color) = if index == 0 {
            let owner = self.override_target;
            match self.margin_glyph_of(owner) {
                // Spec 0322 S6: a previewed leaf keeps its anomaly mark
                // here too — it is the row the reader is deciding
                // about, and covering the one thing on it that says the
                // node is wrong would be the worst place to do it.
                Some(glyph) => (glyph, self.margin_glyph_color(owner)),
                // A previewed clean leaf has nothing to fold and
                // nothing to warn about, so the bar starts at the top.
                None => (NEAR_BAR_GLYPH, self.preview_bar_color(o)),
            }
        } else {
            (NEAR_BAR_GLYPH, self.preview_bar_color(o))
        };
        let mut spans = Vec::with_capacity(3);
        if at > 0 {
            spans.push(Span::raw(margin[..at].to_string()));
        }
        let style = match color {
            Some(color) => Style::default().fg(color),
            None => Style::default(),
        };
        spans.push(Span::styled(glyph.to_string(), style));
        if at + 1 < margin.len() {
            spans.push(Span::raw(margin[at + 1..].to_string()));
        }
        spans
    }

    /// Spec 0318 S5: the bar's color — `None`, i.e. the default
    /// foreground, when the preview is the whole node.
    fn preview_bar_color(&self, overlay: &PreviewOverlay) -> Option<Color> {
        theme::preview_bar_color(overlay.tier.is_whole(), self.theme)
    }

    /// Spec 0328 S5: a wire row's left margin, taken from the same
    /// function its document row's comes from.
    ///
    /// The blank `wire.rs` used to build for itself is exactly
    /// `fold_margin(indent, None)`, so this is a substitution and not a
    /// new layout — which is what keeps `wire_part_at`'s column
    /// arithmetic and the pan untouched.
    ///
    /// The bar is drawn here, the triangle is not: a wire row is a
    /// continuation of the row above it, and a second triangle in the
    /// column would read as a second foldable node.
    pub(super) fn wire_margin_spans(&self, row: DisplayRow, indent: usize) -> Vec<Span<'static>> {
        let margin = fold_margin(indent, None);
        let mut marks = match row {
            DisplayRow::Overlay(_) => match self.preview_overlay.as_ref() {
                Some(o) if o.tier_column < margin.len() => vec![(
                    o.tier_column,
                    NEAR_BAR_GLYPH,
                    match self.preview_bar_color(o) {
                        Some(color) => Style::default().fg(color),
                        None => Style::default(),
                    },
                )],
                _ => return vec![Span::raw(margin)],
            },
            DisplayRow::Committed(c) => self.bars_on_row(&margin, c.line, true),
        };
        if marks.is_empty() {
            return vec![Span::raw(margin)];
        }
        marks.sort_by_key(|&(column, _, _)| column);
        let mut spans = Vec::with_capacity(2 * marks.len() + 1);
        let mut cut = 0;
        for (column, glyph, style) in marks {
            if column > cut {
                spans.push(Span::raw(margin[cut..column].to_string()));
            }
            spans.push(Span::styled(glyph.to_string(), style));
            cut = column + 1;
        }
        if cut < margin.len() {
            spans.push(Span::raw(margin[cut..].to_string()));
        }
        spans
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
    /// `match`. Spec 0284 S1 adds a third caller: `heat_cue_at_point`,
    /// which measures the suffix to hit-test it.
    pub(super) fn heat_chrome(
        &self,
        display: &heat_cue::HeatDisplay,
    ) -> (Span<'static>, Option<Span<'static>>) {
        let pending_style = || theme::style_for(SyntaxRole::Comment, self.theme);
        let blank = || Span::raw(" ");
        match display {
            heat_cue::HeatDisplay::Cue(c) => {
                // Spec 0336 S4 / 0351 S1: green mismatch when the best
                // candidate scores well (best >= 0, a call to action);
                // amber mismatch when the best candidate is itself a poor
                // fit (best < 0); blue for a tie (optimal but not urgent).
                let hue = match c.kind {
                    heat_cue::HeatCueKind::Mismatch { best, .. } => {
                        if best >= 0 {
                            theme::HeatHue::Green
                        } else {
                            theme::HeatHue::Amber
                        }
                    }
                    heat_cue::HeatCueKind::Tie { .. } => theme::HeatHue::Blue,
                };
                // Spec 0336 S5: the word is always the hue's flat top
                // (`heat_label_style`), never the graded ramp.
                let label_style = theme::heat_label_style(hue, self.theme);
                let suffix = match c.kind {
                    heat_cue::HeatCueKind::Mismatch { current, best } => {
                        let current = current
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "-".to_string());
                        Span::styled(format!(" [{current}/{best}]"), label_style)
                    }
                    heat_cue::HeatCueKind::Tie { tie_count, score } => {
                        Span::styled(format!(" [{tie_count}@{score}]"), label_style)
                    }
                };
                // When t < 0.25 on a non-RGB terminal, the square is too
                // cold to colour — show a blank glyph but keep the suffix
                // so the scores are still readable in any environment.
                let glyph = match theme::heat_style(c.t, hue, self.theme) {
                    Some(style) => Span::styled(heat_cue::HEAT_GLYPH, style),
                    None => blank(),
                };
                (glyph, Some(suffix))
            }
            heat_cue::HeatDisplay::PendingCurrent { best } => (
                blank(),
                Some(Span::styled(format!(" [?/{best}]"), pending_style())),
            ),
            heat_cue::HeatDisplay::Unknown => {
                (blank(), Some(Span::styled(" [?]", pending_style())))
            }
            // Spec 0331 S4 / N6 / 0336 S4: being right is not a finding,
            // so no square. The word is flat blue — Settled{Some} is a
            // tie of one: no other type shares the top. The tie suffix
            // and the agree suffix say the same thing with different
            // cardinality, and giving them different hues would assert
            // a difference in kind that is not there.
            heat_cue::HeatDisplay::Settled { score: Some(n) } => (
                blank(),
                Some(Span::styled(
                    format!(" [{n}]"),
                    theme::heat_label_style(theme::HeatHue::Blue, self.theme),
                )),
            ),
            // Spec 0335 S4: the one square whose color is a *sentinel
            // and not a ramp position*. It sits at the top of the range
            // because it must be seen, not because it is large — do not
            // read a maximum score off it.
            //
            // The deliberate exception to 0138 G9 / 0331 N6. N6's
            // argument was that an unmatched range is common enough
            // that a glyph would ink the whole column; spec 0335 S1's
            // gate is what makes that false, by taking away the rows
            // where the verdict was never meaningful. What is left is
            // rare, and is the finding that most deserves the column.
            heat_cue::HeatDisplay::Settled { score: None } => {
                // Spec 0336 S4/S5: amber, flat, not on the ramp.
                let style = theme::heat_label_style(theme::HeatHue::Amber, self.theme);
                (
                    Span::styled(heat_cue::HEAT_GLYPH, style),
                    Some(Span::styled(" [unmatched]", style)),
                )
            }
            heat_cue::HeatDisplay::None => (blank(), None),
        }
    }

    /// One segment's span: its role's color, plus whatever weight the
    /// caller decided this segment carries (spec 0192 S2).
    pub(super) fn make_span(
        &self,
        text: String,
        role: Option<SyntaxRole>,
        emphasis: Modifier,
    ) -> Span<'static> {
        match role {
            Some(role) => Span::styled(
                text,
                theme::style_for(role, self.theme).add_modifier(emphasis),
            ),
            // `Style::default()` is what `Span::raw` sets, so an
            // unweighted roleless segment is unchanged.
            None => Span::styled(text, Style::default().add_modifier(emphasis)),
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
        self.track_hover_dwell();
        let area = frame.area();
        self.term_width = area.width;
        // Spec 0271 S4: the script pane and its separator sit above
        // everything, terminal width, and are both `Length(0)` when no
        // script is loaded — which is what keeps an ordinary session's
        // geometry byte-for-byte what it was.
        //
        // The two heights are asked for separately. While navigation is
        // off the commentary is zero rows and the separator is still
        // one, so the rule is the only thing left saying a script is
        // attached.
        let script_rows = self.script_rows(area.height);
        let separator_rows = self.script_separator_rows();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(script_rows), // script commentary (spec 0271 S4)
                Constraint::Length(separator_rows), // its separator (S5)
                Constraint::Min(0), // main pane + side pane, each with its own local statusline
                Constraint::Length(1), // global command/message row (spec 0147 G4)
            ])
            .split(area);
        if script_rows > 0 {
            self.render_script_pane(frame, chunks[0]);
        }
        if separator_rows > 0 {
            self.render_script_separator(frame, chunks[1]);
        }
        let chunks = [chunks[2], chunks[3]];

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

        // Last, and outside the splash/help alternative: a context menu
        // is the innermost modal there is, so it draws over the help
        // rather than instead of it — the help can legitimately be open
        // underneath, and `handle_key` answers the menu first to match.
        self.render_menu(frame, area);
        // Spec 0280 S14/S17: after the menu, not before it — the box is
        // refused while a menu is open, so the two never overlap, and
        // drawing it here keeps the menu the last thing on screen if
        // that guarantee were ever weakened.
        self.render_popup(frame, area);
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
        let heights = self.row_heights();
        let (blank_rows, rows) = self
            .scroll
            .window(inner.height as usize, &heights, total_rows);
        // Collected (not a borrowed slice) so the heat-cue pass below can
        // mutate `self.heat_range_cache`/`self.heat_current_score_cache`
        // — a plain slice would keep `self` borrowed immutably for the
        // rest of this block.
        let t_window = std::time::Instant::now();
        let first_row = rows.start;
        let window: Vec<DisplayRow> = self.build_window(first_row, rows.len());
        let d_window = t_window.elapsed();
        self.note_visible_stops(&window);
        // Spec 0259 S1: and remember which node owns the pane's top row,
        // so the next splice can put that row back where it was drawn.
        self.capture_scroll_anchor(&window);

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
        //
        // Spec 0252 S1: ahead of the pass, so the requests it pushes
        // carry the new window rather than the one being left. The key
        // is what decides *which rows are drawn* and nothing else:
        // `first_row` and the row count for a scroll, a pan or a resize,
        // and `structural_version` for a fold or a splice, which change
        // the rows without moving the viewport. The cursor is not in it
        // — an `Alt` pan moves the window and leaves the cursor where it
        // is.
        let window_key = (first_row, window.len(), self.structural_version);
        if self.heat_window_key != Some(window_key) {
            self.heat_window_key = Some(window_key);
            if let Some(worker) = &self.heat_worker {
                worker.new_window();
            }
        }
        let t_heat = std::time::Instant::now();
        let heat_displays: Vec<heat_cue::HeatDisplay> = window
            .iter()
            .map(|&row| match row {
                DisplayRow::Committed(c) => self.heat_cue_at(c.pos),
                DisplayRow::Overlay(_) => heat_cue::HeatDisplay::None,
            })
            .collect();
        // Spec 0262 S7: the cursor row asks last, so that among this
        // frame's `Visible` pushes it is the one at the head of the band.
        self.refresh_cursor_heat_cue();
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
            let index =
                (FOLD_FIELD_WIDTH + column).checked_sub(self.pan_offset)? + HEAT_FIELD_WIDTH;
            (index < inner.width as usize).then_some((row, index))
        });

        // Spec 0235 S14: resolved per frame for the window only, the
        // same rule and the same reason as spec 0187 S3's syntax pass —
        // a match index kept across frames would need invalidating on
        // every fold, splice and scroll, and a pane's worth of `find`
        // costs less than the bookkeeping would.
        let search_pattern = self.search_highlight_pattern();
        let search_current = self.search_current_cell();
        // Spec 0274 S13: hoisted out of the per-row closure because
        // resolving a hit's rows to absolute lines is a descent apiece.
        let search_hit = self.search_hit_span();
        // Spec 0274 S13: and the other occurrences, which a cross-row
        // pattern cannot find one row at a time. Resolved for the whole
        // window at once, since the runs it matches over span rows.
        let search_occurrences = search_pattern
            .as_ref()
            .filter(|pattern| matches!(***pattern, SearchPattern::Multi(_)))
            .map(|pattern| self.multi_row_occurrences(pattern, first_row, &window));
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
                // The cue's glyph, then the blank that keeps it off the
                // fold marker — `HEAT_FIELD_WIDTH` columns, in one shift
                // of the span list rather than two.
                spans.splice(0..0, [glyph, Span::raw(" ")]);

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
                    let lo =
                        (FOLD_FIELD_WIDTH + range.start).saturating_sub(pan) + HEAT_FIELD_WIDTH;
                    let hi = ((FOLD_FIELD_WIDTH + range.end).saturating_sub(pan)
                        + HEAT_FIELD_WIDTH)
                        .min(width);
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
                    // Spec 0274 S13: a pattern that may cross a row
                    // takes its ranges from the window-wide pass above
                    // rather than from the single-row loop below, which
                    // cannot describe a match two rows tall. Every
                    // occurrence is tinted, as it is for a single-row
                    // pattern; the current hit goes on last, so it wins
                    // the cells the two share.
                    if matches!(**pattern, SearchPattern::Multi(_)) {
                        let pan = self.pan_offset;
                        if let Some(occurrences) = &search_occurrences {
                            for columns in &occurrences[row] {
                                tint_columns(&mut spans, columns.clone(), pan, width, other);
                            }
                        }
                        if let (Some(span), Some(line)) = (search_hit, line_idx) {
                            let chars = text.chars().count();
                            if let Some(columns) = self.selected_columns(span, line, chars) {
                                tint_columns(&mut spans, columns, pan, width, current);
                            }
                        }
                    } else {
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
                                let cell = index + HEAT_FIELD_WIDTH;
                                if cell < width {
                                    restyle_range(&mut spans, cell..cell + 1, |style| {
                                        style.patch(current)
                                    });
                                }
                            }
                        }
                        let cells = RowCells {
                            pan: self.pan_offset,
                            lead: FOLD_FIELD_WIDTH,
                            trail: HEAT_FIELD_WIDTH,
                            width,
                        };
                        tint_matches(&mut spans, &text, pattern, cells, |start| {
                            if search_current.is_some_and(|(line, column, _, on_path)| {
                                !on_path && Some(line) == line_idx && column == start
                            }) {
                                current
                            } else {
                                other
                            }
                        });
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
                //
                // Spec 0268 S1: only the rows in the shown run get one,
                // which is the same question `heights` already answered
                // for the viewport.
                let shows_bytes = heights.height(self.scroll.index + row) > 1;
                let wire_line = shows_bytes.then(|| {
                    let source = self.display_row_text(display_row);
                    let indent = source.len() - source.trim_start().len();
                    let palette = self.wire_palette(row, &source);
                    // Spec 0328 S5: the same margin the document row
                    // draws, so both bars run through the hex rows
                    // instead of breaking on every other terminal row.
                    let left = self.wire_margin_spans(display_row, indent);
                    let wire_spans = match display_row {
                        DisplayRow::Committed(c) => {
                            self.wire_row(c.pos, left, &mut packed_memo, palette.as_ref())
                        }
                        // S9: an overlay's bytes are drawn from the
                        // preview's own spans, since a preview is a
                        // proposal to read the same bytes as a different
                        // type and the wire row is what shows they are
                        // indeed the same.
                        DisplayRow::Overlay(i) => self.preview_wire_row(i, left, palette.as_ref()),
                    };
                    // A row of the run with no bytes to show still gets
                    // its margin. `wire_row` answers `None` for a
                    // closing brace whose children ended exactly where
                    // the message did, and suppressing the elbow there
                    // is right — it would point at an empty row. But
                    // spec 0328 S5's bars are in the margin, so dropping
                    // the line with the elbow broke every bar covering
                    // it for one terminal row.
                    //
                    // Built again rather than cloned: the margin is a
                    // handful of spans, this is the rare row, and the
                    // common one pays nothing.
                    let mut spans = pan_spans(
                        wire_spans.unwrap_or_else(|| self.wire_margin_spans(display_row, indent)),
                        self.pan_offset,
                    );
                    spans.insert(0, Span::raw(" ".repeat(HEAT_FIELD_WIDTH)));
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
        // The blob's file *name*, not the path it was named by
        // (2026-08-20). The row's job is to say which node the caret is
        // on; the directories leading to the blob cannot change during
        // a session, and on a path of any depth they were spending most
        // of the left half saying so — and being the head, they are
        // also the first thing spec 0193 S3 drops when the row is
        // narrow, so they were paid for exactly when there was least
        // room. `file_name` is `None` only for a path ending in `..` or
        // in a root, neither of which names a blob, and the whole path
        // is a better answer there than nothing.
        let path_label = self.blob_path.file_name().map_or_else(
            || self.blob_path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        // Spec 0286 S6 wants the viewport label picked out of the
        // composed row, so it is carried out of the match beside the two
        // halves rather than left inside the `format!` that built it.
        let (main_left, main_right, viewport) = if self.tree.is_empty() {
            (
                format!("{path_label} (empty — decoded to zero fields)"),
                None,
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
            // Spec 0244 S10: in terminal rows, the unit the signed top is
            // counted in — `total_rows` is document lines, and the ones
            // showing their bytes are two rows thick.
            let viewport = viewport_label(
                self.scroll_top(),
                inner.height as usize,
                heights.offset(total_rows),
            );
            let line_ruler = format!(
                "L{}/{}  {viewport}",
                self.cursor_line() + 1,
                self.total_lines()
            );
            let right = if half_width {
                line_ruler
            } else {
                let range = self.display_range(self.cursor);
                format!("[{}..{})  {line_ruler}", range.start, range.end)
            };
            (left, Some(right), Some(viewport))
        };
        let main_statusline = main_split[1];
        let main_text = statusline_text(
            &main_left,
            main_right.as_deref(),
            main_statusline.width as usize,
        );
        frame.render_widget(
            Paragraph::new(statusline_line(
                main_text,
                viewport.as_deref(),
                main_style,
                self.scroll_resistance.pushing(),
            )),
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
        // Spec 0278 S2: a search prompt's buffer and the echo a
        // committed search leaves behind are the same text on the same
        // row, and `search_row_text` is the one place that decides
        // which of them — if either — the row is showing. A find's own
        // prefix (spec 0276 S3) is chosen there too.
        let cmd_text = match (&self.command_buffer, self.search_row_text()) {
            (_, Some(text)) => text,
            (Some(buf), None) => format!(":{buf}"),
            (None, None) => self.message.clone(),
        };
        // Spec 0190 S5: the global row's leading `ACTIVITY_FIELD_WIDTH`
        // columns are reserved for the activity dot, unconditionally —
        // so the command row's geometry never shifts underneath the
        // user mid-edit. Everything downstream (`cmd_area` for mouse
        // hit-testing, `width` for pan clamping, `set_cursor_position`)
        // already derives from `cmd_row`, so re-binding it here is the
        // whole change.
        let global_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(ACTIVITY_FIELD_WIDTH), Constraint::Min(0)])
            .split(area);
        self.render_activity_dot(frame, global_row[0]);
        // Spec 0277 S8: `27 of 42`, right-aligned, in a field the
        // command text is not given. Its own slice of the row rather
        // than a span appended to `cmd_text`, because the two belong to
        // different things: the text pans under the reader's cursor and
        // may be empty, while the count is a fact about the search that
        // holds whether or not a prompt is open.
        let tally = self.search_tally_text();
        let (cmd_row, tally_row) = match &tally {
            Some(text) if global_row[1].width as usize > text.len() + 1 => {
                let field = text.len() as u16 + 1;
                (
                    Rect {
                        width: global_row[1].width - field,
                        ..global_row[1]
                    },
                    Some(Rect {
                        x: global_row[1].x + global_row[1].width - field,
                        width: field,
                        ..global_row[1]
                    }),
                )
            }
            _ => (global_row[1], None),
        };
        if let (Some(area), Some(text)) = (tally_row, &tally) {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    text.clone(),
                    Style::default().add_modifier(Modifier::DIM),
                ))
                .alignment(Alignment::Right),
                area,
            );
        }
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
            //
            // A miss is drawn in two colors, on the same rule
            // `App::not_found` qualifies its message by: while the bake
            // still owes subtrees the sweep never saw their text, so
            // "not there" is not yet a claim the search is entitled to
            // make. `Unbaked`'s gray until it is, and the fold margin
            // beside it is already drawing that same color against the
            // very subtrees the answer is missing.
            let searching = matches!(self.command_kind, CommandLineKind::Search { .. })
                && self.command_buffer.as_ref().is_some_and(|b| !b.is_empty());
            let miss = || match self.search_miss_is_conclusive(self.search_scope()) {
                true => theme::search_unmatched_style(self.theme),
                false => theme::search_unbaked_style(self.theme),
            };
            let pattern_style = searching
                .then(|| match &self.search_sweep {
                    // No sweep at all: an empty pane, nothing left to
                    // look in — a finished miss by another name.
                    None => Some(miss()),
                    Some(s) if s.found.is_some() => None,
                    Some(s) if s.is_finished() => Some(miss()),
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
    /// Colors reuse `theme::heat_style`'s light/dark-aware ramps, and
    /// every `t` is deliberately above the ANSI floor (0.25): a
    /// diagnostic that silently vanishes on a 16-color terminal would be
    /// worse than no diagnostic. On that fallback these three collapse to
    /// `LightGreen`/`Green`/`Blue` — still three distinguishable states,
    /// still darkening along the priority order. Green replaces the
    /// former red: the dot is about sweep activity, not a fault.
    ///
    /// Priority order, highest first:
    ///   1. Heat-cue `User` tier — bright green.
    ///   2. Heat-cue `Visible` tier — dim green.
    ///   3. Bake in progress (spec 0249/0255) — light gray.
    ///   4. Shadow-sweep trie walk (spec 0343 B6) — violet.
    ///   5. Heat-cue `Prefetch` tier — dim blue.
    ///   6. Idle — blank.
    ///
    fn render_activity_dot(&mut self, frame: &mut Frame, area: Rect) {
        // Priority: User/Visible heat > bake > shadow sweep > Prefetch heat.
        let upper_heat = self.activity_shown.and_then(|tier| {
            let (hue, t) = match tier {
                tiered::Tier::User => (theme::HeatHue::Green, 1.0_f32),
                tiered::Tier::Visible => (theme::HeatHue::Green, 4.0 / 11.0),
                tiered::Tier::Prefetch => return None, // below bake
            };
            theme::heat_style(t, hue, self.theme)
        });
        let prefetch_heat = self.activity_shown.and_then(|tier| {
            if tier == tiered::Tier::Prefetch {
                theme::heat_style(4.0 / 11.0, theme::HeatHue::Blue, self.theme)
            } else {
                None
            }
        });
        let style = upper_heat
            .or_else(|| self.bake_dot_style())
            .or_else(|| self.shadow_dot_style())
            .or_else(|| prefetch_heat);
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

        // Spec 0309 S5: the mode label names the list `i` would switch
        // *to*, not the one on screen — the reader can already see which
        // list they are looking at, and a bare "all types" beside it
        // reads as a claim about the rows above rather than as the one
        // thing they might want next.
        let mode_label = match self.override_sort {
            SortMode::Lexicographic => "i → inferred types",
            SortMode::Inferred => "i → all types",
        };
        // Spec 0309 S4: the sentence `Enter` would carry out — the
        // origin `projected_override_origin` will actually build, and
        // the type the highlighted row names. The node's own positional
        // path is only the fallback for a target with no projectable
        // origin at all; `override <origin>` is otherwise strictly more
        // informative, since a `path` projection *is* that path.
        //
        // The type is its last `.`-segment: this row is a reminder of
        // what is highlighted a few lines above, not a second place to
        // read a FQDN, and the full name would routinely crowd out the
        // mode label on a pane this narrow.
        let origin = match self.projected_override_origin() {
            Ok(origin) => origin.label(),
            Err(_) => self.positional_path(idx),
        };
        let left = match self
            .highlighted_override_type()
            .and_then(|f| f.rsplit('.').next())
        {
            // Spec 0321 S3: parentheses, not a dash. The left half is a
            // value — the origin `Enter` will create — and the right is
            // an affordance; a dash reads as joining two peers, and on a
            // pane narrow enough for `statusline_text` to cut the left
            // half a bare dash inside truncated text is ambiguous in a
            // way `(` is not.
            Some(short) => format!("override {origin} as {short} ({mode_label})"),
            None => format!("override {origin} ({mode_label})"),
        };

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
        let (blank_rows, rows) = self
            .override_scroll
            .window(list_height, &FLAT_ROWS, total_rows);
        let (start, end) = (rows.start, rows.end);

        // Spec 0193 S4: drawn only now, since the viewport label needs
        // the scroll offset this pane's own clamp above just settled.
        let viewport = viewport_label(
            self.override_scroll.top(&FLAT_ROWS),
            list_height,
            total_rows,
        );
        let right = format!(
            "L{}/{}  {}",
            self.override_highlight + 1,
            total_rows,
            viewport
        );
        let text = statusline_text(&left, Some(&right), split[1].width as usize);
        frame.render_widget(
            // Spec 0286 S6: the label wears the accent while this pane's
            // own end of its own list is being pushed against.
            Paragraph::new(statusline_line(
                text,
                Some(&viewport),
                style,
                self.override_resistance.pushing(),
            )),
            split[1],
        );

        // Warm the wrapper-descriptor registration for the whole
        // currently-visible window ahead of time, so arrowing through
        // already-visible rows never re-pays the registration cost per
        // keystroke.
        self.warm_visible_override_wrappers(start, end);

        // Spec 0339 S1/S5: hoisted out of the row loop, as the main
        // pane's own are — one compile per frame, not one per row — and
        // gated on this pane owning the search, since
        // `search_highlight_pattern` answers with the live prompt
        // buffer whichever pane opened it.
        let search = (self.active_search_scope() == SearchScope::Override)
            .then(|| self.search_highlight_pattern())
            .flatten()
            .map(|pattern| {
                (
                    pattern,
                    theme::search_current_style(self.theme),
                    theme::search_match_style(self.theme),
                    self.search_current_index(),
                )
            });

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
            let mut spans = pan_spans(
                vec![Span::styled(text.clone(), style)],
                self.override_pan_offset,
            );
            // Spec 0339 S1/S4: the tint goes on after the pan, over the
            // drawn text, and the whole row is the current match or
            // none of it is — a side pane's stop is its whole entry
            // (spec 0246 N4), so there is no column to tell two matches
            // inside one apart.
            if let Some((pattern, current, other, current_index)) = &search {
                let hit = *current_index == Some(row);
                let cells = RowCells {
                    pan: self.override_pan_offset,
                    lead: 0,
                    trail: 0,
                    width: inner.width as usize,
                };
                let style = if hit { *current } else { *other };
                tint_matches(&mut spans, &text, pattern, cells, |_| style);
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Centered modal listing `HELP_TEXT`.
    ///
    /// Spec 0340 S1/S3/S6/S7: a list pane like the other two, drawn the
    /// same way — a `PaneScroll` window over `FLAT_ROWS`, a reversed
    /// cursor row, `help_pan_offset` through `pan_spans`, and the shared
    /// search tint on top.
    pub(super) fn render_help(&mut self, frame: &mut Frame, area: Rect) {
        let inner = popup_frame(
            frame,
            area,
            70,
            70,
            " Help (j/k move, / search, Esc/F1 close) ",
        );
        self.help_area = inner;

        let list_height = inner.height as usize;
        self.help_list_height = list_height;

        // Auto-pan into view only on genuine cursor movement, so that a
        // Ctrl-Up pan is not undone by the next frame.
        if self.last_help_highlight != Some(self.help_highlight) {
            clamp_scroll_to_visible(&mut self.help_scroll, self.help_highlight, list_height);
            self.last_help_highlight = Some(self.help_highlight);
        }
        let (blank_rows, rows) = self
            .help_scroll
            .window(list_height, &FLAT_ROWS, HELP_TEXT.len());

        let search = (self.active_search_scope() == SearchScope::Help)
            .then(|| self.search_highlight_pattern())
            .flatten()
            .map(|pattern| {
                (
                    pattern,
                    theme::search_current_style(self.theme),
                    theme::search_match_style(self.theme),
                    self.search_current_index(),
                )
            });

        let mut lines: Vec<Line> = vec![Line::default(); blank_rows];
        for row in rows {
            let text = HELP_TEXT[row];
            let style = if row == self.help_highlight {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let mut spans = pan_spans(
                vec![Span::styled(text.to_string(), style)],
                self.help_pan_offset,
            );
            if let Some((pattern, current, other, current_index)) = &search {
                let cells = RowCells {
                    pan: self.help_pan_offset,
                    lead: 0,
                    trail: 0,
                    width: inner.width as usize,
                };
                let hit = *current_index == Some(row);
                let style = if hit { *current } else { *other };
                tint_matches(&mut spans, text, pattern, cells, |_| style);
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// The context menu, drawn at its anchor rather than centered.
    ///
    /// Records the box's outer `Rect` on the `Menu` for the hit tests in
    /// `handle_mouse`, exactly as `render_help` records `help_area`.
    pub(super) fn render_menu(&mut self, frame: &mut Frame, area: Rect) {
        let Some(menu) = &self.menu else { return };

        // Sized to its content, then clamped to the screen: a menu is
        // the one popup whose width is a fact about the strings in it.
        let inner_width = menu.content_width().max(1);
        let width = (inner_width + 2).min(area.width.max(1));
        let height = (menu.items.len() as u16 + 2).min(area.height.max(1));

        let rect = anchored_rect(menu.anchor, width, height, area);

        let lines: Vec<Line> = menu
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let hint = key_label(&item.key);
                // The label and its key hint are justified to the two
                // edges of the box, which is what makes the column of
                // hints readable as a column.
                let pad = (inner_width as usize)
                    .saturating_sub(item.label.chars().count() + hint.chars().count())
                    .max(1);
                let text = format!("{}{}{}", item.label, " ".repeat(pad), hint);
                if i == menu.selected {
                    Line::styled(text, theme::focus_style(self.theme))
                } else {
                    Line::from(text)
                }
            })
            .collect();

        frame.render_widget(Clear, rect);
        let block = Block::bordered().border_type(BorderType::Rounded);
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        frame.render_widget(Paragraph::new(lines), inner);

        if let Some(menu) = &mut self.menu {
            menu.area = rect;
        }
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

/// A `width`×`height` box put where `anchor` asked for it, without
/// leaving `area` (spec 0280 S14).
///
/// Anchored below-right of the anchor, flipped when that would cross an
/// edge — the standard behavior, and the reason an anchor is a request
/// rather than a position. `saturating_sub` handles the degenerate case
/// of a box taller than the screen by pinning it to the top-left; it is
/// clamped to `area` either way.
///
/// Shared by the context menu and the score box because there is
/// exactly one right answer to "put a box at a point without leaving
/// the screen", and two copies of it would be two chances to drift.
pub(super) fn anchored_rect(anchor: (u16, u16), width: u16, height: u16, area: Rect) -> Rect {
    let (ax, ay) = anchor;
    let x = if ax + width <= area.right() {
        ax
    } else {
        area.right().saturating_sub(width)
    };
    let y = if ay + 1 + height <= area.bottom() {
        ay + 1
    } else {
        ay.saturating_sub(height).max(area.y)
    };
    Rect {
        x: x.max(area.x),
        y: y.max(area.y),
        width,
        height,
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
