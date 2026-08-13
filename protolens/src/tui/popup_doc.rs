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
        let content = self.row_content(self.committed_row_at(line, pos));

        // The same mapping `type_annotation_at` computes: pane column,
        // less the reserved glyph gutter (spec 0138 N1), plus the pan
        // already taken off the content.
        let index =
            (column.saturating_sub(self.main_area.x) as usize).checked_sub(1)? + self.pan_offset;
        let byte = content.char_indices().nth(index)?.0;

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
