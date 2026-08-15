// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0285: what one token of one document row means, in words.
//!
//! `popup_wire.rs` is the same job one row down. The difference is the
//! source: a wire hit is *computed* from the bytes, whereas a document
//! row is already a string in a grammar we own
//! (`docs/prototext/annotation-format.md`), so this lexes the row and
//! nothing else. No blob is re-read and no tree is consulted — S3, and
//! the reason the highlighter's spans were not reused (0280 S10).
//!
//! The explanations themselves are not here. They live in
//! `crate::annotation` beside the tiers, so that a keyword reads the
//! same in this box and in the wire box (spec 0285 G2).

use super::*;
use crate::annotation::{clause, tier_of, wire_type_clause, Tier};
use prototext_core::serialize::encode_text::annotation_start;

const PACKED: &str = "[packed=true]";

/// One kind of thing a drawn document row can be pointing at
/// (spec 0285 S4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DocElement {
    /// The field key, left of the `:` or the `{`.
    Key,
    /// The value, right of the `: `.
    Value,
    /// The `#@` marker itself.
    Marker,
    /// The annotation's leading wire-type token.
    WireType,
    /// `repeated` or `required`.
    Label,
    /// The field declaration's type name. **Not one of S4's eight**:
    /// spec 0280 S10 owns this span and [`App::doc_element_at_point`]
    /// refuses it (S5). It is lexed anyway because 0280's own hit test
    /// wants exactly this span and it is the same parse.
    Type,
    /// The `[packed=true]` flag.
    Packed,
    /// The `= 85` that ends a field declaration.
    Number,
    /// Any other annotation token — a modifier, with or without a
    /// value.
    Modifier,

    // Spec 0287 S1: four members that are *chrome* rather than grammar.
    // They join the nine above so that there is one hit type, one
    // dwell, one "still on the same thing" comparison and one box
    // builder; a parallel `ChromeHit` would duplicate `DocHit`'s
    // `line`/`at` identity, which is the whole of what makes two hovers
    // the same hover. Each carries what its wording depends on, read
    // off the mark as drawn (G2) rather than re-derived when the box is
    // built.
    /// The `●` in the reserved gutter (spec 0138 N1). `tie` is which of
    /// the two cues drew it — blue for a tie, red for a mismatch.
    HeatGlyph { tie: bool },
    /// The ` [3/47]`, ` [2@85]`, ` [?/47]` or ` [?]` at the end of the
    /// row.
    HeatSuffix(SuffixShape),
    /// The `⏷`/`⏵` in the fold margin. `colored` is spec 0247 S10's
    /// status hue, which the box only mentions when the glyph is
    /// actually wearing one.
    FoldMarker { folded: bool, colored: bool },
    /// The `{ ... }` a folded node collapses to. `unread` is spec 0260
    /// S2's violet: nobody has looked inside this region, as against a
    /// fold the reader made.
    FoldSummary { unread: bool },
}

/// Which of `HeatDisplay`'s shapes the drawn suffix is (spec 0154 G6),
/// to the precision the box's words need.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SuffixShape {
    /// ` [3/47]` — this node's own score, then the best any candidate
    /// reached.
    Mismatch,
    /// ` [-/47]` — the current type does not fit these bytes at all.
    NoFit,
    /// ` [2@85]` — `n` types share the top score.
    Tie,
    /// ` [?/47]` — the best is known and this node's own is not.
    Pending,
    /// ` [?]` — nothing is known yet.
    Unknown,
}

impl SuffixShape {
    /// The shape of `display`, or `None` where no suffix is drawn.
    ///
    /// Spec 0287 G2: taken from the value `heat_chrome` formatted, so
    /// the box and the suffix cannot disagree about which of the five
    /// the reader is looking at.
    fn of(display: &heat_cue::HeatDisplay) -> Option<Self> {
        Some(match display {
            heat_cue::HeatDisplay::Cue(cue) => match cue.kind {
                heat_cue::HeatCueKind::Mismatch { current: None, .. } => Self::NoFit,
                heat_cue::HeatCueKind::Mismatch { .. } => Self::Mismatch,
                heat_cue::HeatCueKind::Tie { .. } => Self::Tie,
            },
            heat_cue::HeatDisplay::PendingCurrent { .. } => Self::Pending,
            heat_cue::HeatDisplay::Unknown => Self::Unknown,
            heat_cue::HeatDisplay::None => return None,
        })
    }
}

/// The pointer resting on one element of one row.
///
/// `line` and `at` are what make two hovers the same hover: without
/// them the identical modifier on two rows would compare equal and the
/// box would refuse to follow the pointer (0280 S6's "still on the
/// same thing").
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct DocHit {
    line: usize,
    at: usize,
    element: DocElement,
    /// The token as drawn, `tag_ohb: 3` and all — spec 0285 S6.
    token: String,
    /// For a value on an enum row, the number the annotation's type
    /// token carries: `Color(1)`'s `1`. `None` everywhere else,
    /// including a packed `Color([1, 2])`, where no single number is
    /// this line's.
    enum_number: Option<String>,
}

impl DocHit {
    /// Which of [`DocElement`]'s kinds this hit landed on.
    #[cfg(test)]
    pub(super) fn element(&self) -> DocElement {
        self.element
    }
}

/// Every element of a drawn row, in ascending order.
///
/// One pass, one small `Vec`, and both queries over it — "which
/// element is at this byte" and 0280's "where is the type name" — are
/// `find`s. Two parsers over one grammar is how they come to disagree.
fn doc_elements(row: &str) -> Vec<(DocElement, Range<usize>)> {
    let mut out = Vec::new();
    let value_end = annotation_start(row);
    push_body(&row[..value_end.unwrap_or(row.len())], &mut out);

    // `annotation_start` stops before the two separator spaces, so the
    // marker is still ahead of us.
    let Some(mark) = value_end.and_then(|at| row[at..].find("#@").map(|m| at + m)) else {
        return out;
    };
    out.push((DocElement::Marker, mark..mark + 2));
    push_annotation(row, mark + 2, &mut out);
    out
}

/// The field line proper: the key, and the value if there is one.
fn push_body(body: &str, out: &mut Vec<(DocElement, Range<usize>)>) {
    // The fold margin and the indentation are chrome, and so is the
    // `⏷`/`⏵` in them. Rather than naming the glyphs, skip to the first
    // character that can *begin* a key — which also leaves a closing
    // `}` line with no key at all, correctly.
    let Some(start) = body.find(|c: char| c.is_alphanumeric() || c == '_' || c == '[') else {
        return;
    };
    let rest = &body[start..];
    let key_end = start + rest.find([':', ' ', '{']).unwrap_or(rest.len());
    out.push((DocElement::Key, start..key_end));

    let Some(value) = body[key_end..].strip_prefix(": ") else {
        return;
    };
    let lo = key_end + ": ".len();
    let hi = lo + value.trim_end().len();
    if hi > lo {
        out.push((DocElement::Value, lo..hi));
    }
}

/// The annotation's `"; "`-separated tokens, from just past the `#@`.
fn push_annotation(row: &str, from: usize, out: &mut Vec<(DocElement, Range<usize>)>) {
    let mut start = from;
    for (i, raw) in row[from..].split(';').enumerate() {
        let lo = start + (raw.len() - raw.trim_start().len());
        start += raw.len() + 1;
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        let hi = lo + token.len();
        // Only the first token can be a wire type, which is what keeps
        // `bytes` there from being read as the scalar type name it also
        // is (spec 0285's rejected word-keyed table).
        if i == 0 && wire_type_clause(token).is_some() {
            out.push((DocElement::WireType, lo..hi));
        } else if token.contains(" = ") {
            push_decl(token, lo, out);
        } else {
            out.push((DocElement::Modifier, lo..hi));
        }
    }
}

/// `[label ]type[ [packed=true]] = number`, at `base`.
///
/// `" = "` is what tells a declaration from a modifier: `push_field_decl`
/// is the only thing that writes it, since every modifier spells itself
/// `key: value`.
fn push_decl(token: &str, base: usize, out: &mut Vec<(DocElement, Range<usize>)>) {
    let Some(eq) = token.find(" = ") else {
        return;
    };
    let decl = &token[..eq];
    let mut lo = 0;
    for label in ["repeated", "required"] {
        if decl.starts_with(label) {
            out.push((DocElement::Label, base..base + label.len()));
            lo = label.len() + 1;
            break;
        }
    }
    // An enum's type carries its value (`Color(5)`); the name alone is
    // the span, because the value is a datum.
    let hi = lo + decl[lo..].find(['(', ' ']).unwrap_or(decl.len() - lo);
    if hi > lo {
        out.push((DocElement::Type, base + lo..base + hi));
    }
    if let Some(at) = decl.find(PACKED) {
        out.push((DocElement::Packed, base + at..base + at + PACKED.len()));
    }
    // From the `=` to the end of the token, so the reader who points at
    // the number and the one who points at the sign get one answer.
    out.push((DocElement::Number, base + eq + 1..base + token.len()));
}

/// Spec 0280 S10's query, over the one parse (spec 0285 S3).
pub(super) fn annotation_type_span(row: &str) -> Option<Range<usize>> {
    doc_elements(row)
        .into_iter()
        .find(|(element, _)| *element == DocElement::Type)
        .map(|(_, span)| span)
}

/// A modifier's keyword, with any `: value` taken off.
fn keyword_of(token: &str) -> &str {
    token.split(':').next().unwrap_or(token).trim()
}

/// The number in an annotation type token of the form `Name(7)`.
fn enum_number(row: &str, elements: &[(DocElement, Range<usize>)]) -> Option<String> {
    let span = &elements
        .iter()
        .find(|(element, _)| *element == DocElement::Type)?
        .1;
    let digits = row.get(span.end..)?.strip_prefix('(')?.split(')').next()?;
    let numeric = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit() || b == b'-');
    numeric.then(|| digits.to_string())
}

/// Which of the three kinds a field key is — readable from the key
/// itself, which is the format's own rule.
fn key_clause(key: &str) -> &'static str {
    if key.starts_with('[') {
        "an extension field, named by its full path"
    } else if key.bytes().all(|b| b.is_ascii_digit()) {
        "a field number: no schema declared this field"
    } else {
        "the field's name, from the schema"
    }
}

impl App {
    /// Spec 0287 S3: the two heat-cue targets, both in columns no token
    /// of the row can reach — column 0 exactly, which spec 0285's
    /// mapping gives up on with a `checked_sub(1)`, and the suffix past
    /// the row's last character, where its `nth(index)` already yields
    /// `None`.
    ///
    /// The cue is asked for once and the glyph and the suffix are both
    /// read off that one answer, which is G2: `heat_cue_at` and
    /// `heat_chrome` are what drew the mark, so a box built from them
    /// cannot describe a cue the reader is not looking at. A cue hidden
    /// by `i` is `HeatDisplay::None` and so is not a target at all.
    fn heat_chrome_hit(
        &mut self,
        column: u16,
        row: u16,
        line: usize,
        pos: LinePos,
    ) -> Option<DocHit> {
        let on_glyph = column == self.main_area.x;
        // Spec 0287 S3: the suffix's own geometry, unchanged — it adds
        // no `pan_offset`, because `render` pushes the suffix after
        // `pan_spans` and so the suffix does not pan.
        if !on_glyph && self.heat_cue_at_point(column, row) != Some(line) {
            return None;
        }
        let display = self.heat_cue_at(pos);
        let (glyph, suffix) = self.heat_chrome(&display);

        let (element, token) = if on_glyph {
            // N3: the reserved blank is not a target. `heat_chrome`
            // draws one for a settled node, for a pending one, and for
            // a cue the ANSI-16 palette cannot color — so what decides
            // is the glyph that came back, never the display's shape.
            if glyph.content != heat_cue::HEAT_GLYPH {
                return None;
            }
            let heat_cue::HeatDisplay::Cue(cue) = display else {
                return None;
            };
            let tie = matches!(cue.kind, heat_cue::HeatCueKind::Tie { .. });
            (DocElement::HeatGlyph { tie }, glyph.content.into_owned())
        } else {
            let suffix = suffix?;
            (
                DocElement::HeatSuffix(SuffixShape::of(&display)?),
                suffix.content.trim().to_string(),
            )
        };
        Some(DocHit {
            line,
            at: 0,
            element,
            token,
            enum_number: None,
        })
    }

    /// Spec 0287 S3: the fold marker, located by the one locator
    /// `handle_click` toggles from — so the box's *click to unfold it*
    /// is true of every column it appears over, by construction.
    fn fold_marker_hit(&self, column: u16, line: usize, pos: LinePos) -> Option<DocHit> {
        if !self.in_fold_field(column, pos) {
            return None;
        }
        let folded = self.is_folded(pos.node);
        Some(DocHit {
            line,
            at: 0,
            element: DocElement::FoldMarker {
                folded,
                colored: self.fold_marker_color(Some(pos.node)).is_some(),
            },
            token: if folded {
                render::FOLD_GLYPH_CLOSED
            } else {
                render::FOLD_GLYPH_OPEN
            }
            .to_string(),
            enum_number: None,
        })
    }

    /// Spec 0287 S3: the `{ ... }` a folded node collapses to, which
    /// `row_text_of` has already spliced into the content the mapping
    /// indexes.
    ///
    /// The node's own fold state is what says there is a summary here —
    /// searching the row for `"{ ... }"` would find those characters
    /// inside a string value too, and `doc_elements` cannot tell a
    /// value from a splice. Where the splice went is then read back the
    /// way `row_text_of` put it there: after the row's last `{`, or at
    /// the end when the row has none.
    ///
    /// The span runs from the brace to the closing one, so pointing at
    /// either it or the ellipsis gives one answer.
    fn fold_summary_hit(
        &self,
        line: usize,
        pos: LinePos,
        content: &str,
        byte: usize,
    ) -> Option<DocHit> {
        if self.is_footer(pos) || !self.has_children(pos.node) || !self.is_folded(pos.node) {
            return None;
        }
        let span = match content.rfind('{') {
            Some(at) => at..at + "{ ... }".len(),
            None => content.len().checked_sub(" ... }".len())?..content.len(),
        };
        if !span.contains(&byte) {
            return None;
        }
        Some(DocHit {
            line,
            at: span.start,
            element: DocElement::FoldSummary {
                unread: self.unread_fold_style(pos.node).is_some(),
            },
            token: content.get(span)?.to_string(),
            enum_number: None,
        })
    }

    /// The element drawn at this point, if it is one the box has
    /// anything to say about (spec 0285 S4).
    ///
    /// Two refusals. The type name goes to 0280's score box, so that a
    /// point has exactly one target and one dwell (S5). A modifier with
    /// no clause is refused because a box whose only line is the word
    /// the reader is already looking at is worse than no box — which is
    /// also what keeps the `#@ prototext: protoc` header from being a
    /// target (N4). `annotation::every_keyword_has_a_clause` is what
    /// stops that arm from swallowing the vocabulary.
    pub(super) fn doc_element_at_point(&mut self, column: u16, row: u16) -> Option<DocHit> {
        let line = self.main_pane_line_idx(column, row)?;
        let pos = self.line_pos(line)?;

        // Spec 0287 S6: the chrome is tested before the row is lexed.
        // The tests are integer range checks and are disjoint from
        // every token, so pointing at text — the common case — pays for
        // them and nothing else.
        if let Some(hit) = self.heat_chrome_hit(column, row, line, pos) {
            return Some(hit);
        }
        if let Some(hit) = self.fold_marker_hit(column, line, pos) {
            return Some(hit);
        }

        let content = self.row_content(self.committed_row_at(line, pos));

        // The same mapping `type_annotation_at` computes: pane column,
        // less the reserved glyph gutter (spec 0138 N1), plus the pan
        // already taken off the content.
        let index =
            (column.saturating_sub(self.main_area.x) as usize).checked_sub(1)? + self.pan_offset;
        let byte = content.char_indices().nth(index)?.0;

        if let Some(hit) = self.fold_summary_hit(line, pos, &content, byte) {
            return Some(hit);
        }

        let elements = doc_elements(&content);
        let (element, span) = elements.iter().find(|(_, s)| s.contains(&byte))?.clone();
        let token = content[span.clone()].to_string();
        match element {
            DocElement::Type => return None,
            DocElement::Modifier if clause(keyword_of(&token)).is_none() => return None,
            _ => {}
        }
        let enum_number = match element {
            DocElement::Value => enum_number(&content, &elements),
            _ => None,
        };
        Some(DocHit {
            line,
            at: span.start,
            element,
            token,
            enum_number,
        })
    }

    /// Opens the document box for `hit` at `anchor`.
    ///
    /// The same guard `open_score_popup` carries: the menu is the
    /// innermost modal and a box cannot be opened underneath one.
    pub(super) fn open_doc_popup(&mut self, hit: &DocHit, anchor: (u16, u16)) {
        if self.menu.is_some() {
            return;
        }
        self.popup = Some(Popup {
            body: PopupBody::Doc(doc_lines(hit)),
            anchor,
        });
    }
}

/// The box: the token as drawn, then what it means (spec 0285 S4, S6).
///
/// The token leads every one of them, so that a reader who pointed at
/// `tag_ohb: 3` sees the 3 without any clause having to be a format
/// string with one caller.
pub(super) fn doc_lines(hit: &DocHit) -> Vec<BoxLine> {
    let mut lines = vec![hit.token.clone()];
    match hit.element {
        DocElement::Key => lines.push(key_clause(&hit.token).to_string()),
        DocElement::Value => {
            lines.push("the field's value, as protoc --decode prints it".to_string());
            if hit.token.starts_with("0x") {
                lines.push("raw bits: no schema said how to read them".to_string());
            } else if let Some(number) = &hit.enum_number {
                lines.push(format!("the schema's name for the {number} on the wire"));
            }
        }
        // The marker is the only element whose box is about the whole
        // trailing part, so it is the one place the term can be coined
        // without repeating it under every member of the annotation.
        // The last line is the one that does the work: it says the left
        // of the row is a format the reader may already know and the
        // right is the addition.
        DocElement::Marker => {
            lines.push("opens a prototext annotation: the rest of this line".to_string());
            lines.push("is how the bytes were encoded, not part of the message".to_string());
            lines.push("prototext is textproto, plus these annotations".to_string());
        }
        DocElement::WireType => {
            lines.extend(wire_type_clause(&hit.token).map(str::to_string));
        }
        DocElement::Label => {
            lines.push(
                match hit.token.as_str() {
                    "required" => "proto2: this field must be present",
                    _ => "this field may occur more than once",
                }
                .to_string(),
            );
            lines.push("optional is the default and is never printed".to_string());
        }
        DocElement::Packed => {
            lines.push("these elements share one length-prefixed record".to_string());
        }
        DocElement::Number => {
            lines.push("the field's number in its .proto message".to_string());
            lines.push("the number is on the wire; the name is not".to_string());
        }
        DocElement::Modifier => {
            let keyword = keyword_of(&hit.token);
            lines.extend(tier_of(keyword).map(|tier| tier.clause().to_string()));
            lines.extend(clause(keyword).map(str::to_string));
        }
        // Spec 0287 S4. Every one of these is an *orientation* box in
        // N5's sense — it says what a mark is, not what a value is —
        // and none of them carries a number the reader does not
        // already have on screen: where one is wanted, the box says
        // where it is.
        DocElement::HeatGlyph { tie } => {
            lines.push(
                match tie {
                    true => "another type scores exactly as well as this one",
                    false => "another type scores higher on these bytes",
                }
                .to_string(),
            );
            lines.push(
                match tie {
                    true => "brighter means a higher score",
                    false => "brighter means a bigger difference",
                }
                .to_string(),
            );
            lines.push("the [...] at the end of the row has the numbers".to_string());
        }
        DocElement::HeatSuffix(shape) => {
            match shape {
                SuffixShape::Mismatch => {
                    lines.push("left: what this node's type scores here".to_string());
                    lines.push("right: the best score any candidate reached".to_string());
                }
                SuffixShape::NoFit => {
                    lines.push("the - is: this node's type does not fit at all".to_string());
                    lines.push("right: the best score any candidate reached".to_string());
                }
                SuffixShape::Tie => {
                    lines.push("n types score s here - the best, but not the".to_string());
                    lines.push("only best".to_string());
                }
                SuffixShape::Pending => {
                    lines.push("the best is known; this node's own score is".to_string());
                    lines.push("still being computed".to_string());
                }
                SuffixShape::Unknown => lines.push("still scoring these bytes".to_string()),
            }
            // G3: the one thing about this mark a reader cannot
            // discover by looking at it (spec 0284 S2).
            lines.push("double-click to choose a type for this node".to_string());
        }
        DocElement::FoldMarker { folded, colored } => {
            lines.push(
                match folded {
                    true => "this node is folded",
                    false => "this node is unfolded",
                }
                .to_string(),
            );
            lines.push(
                match folded {
                    true => "click to unfold it",
                    false => "click to fold it",
                }
                .to_string(),
            );
            if colored {
                lines.push("the color is the worst thing found anywhere inside".to_string());
            }
        }
        DocElement::FoldSummary { unread } => {
            if unread {
                lines.push("nobody has looked inside this region yet".to_string());
            } else {
                lines.push("this node is folded: its fields are not shown".to_string());
                lines.push("click the marker in the left margin to unfold it".to_string());
            }
        }
        // Refused by `doc_element_at_point`, so unreachable — and left
        // as a box with only its own token rather than a panic, since a
        // wrong box is a smaller failure than a dead app.
        DocElement::Type => {}
    }
    lines.into_iter().map(BoxLine::plain).collect()
}

impl Tier {
    /// What the tier means, for a box that has just printed a keyword
    /// carrying it.
    pub(super) fn clause(self) -> &'static str {
        match self {
            Tier::NonCanonical => "non-canonical: legal, but no writer should emit it",
            Tier::Invalid => "invalid: the blob is malformed, or this is not the schema",
        }
    }
}
