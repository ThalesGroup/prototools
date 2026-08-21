// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0225: the bytes a drawn line is made of, as one hex row.
//!
//! Two halves. [`App::wire_slice`] answers *which* bytes belong to a
//! line — S2's head/tail partition, S3's wrapper suppression, S4's
//! packed-run walk. [`wire_spans`] answers what they look like: the
//! punctuation of S11, the four-tier palette shared with the `#@`
//! annotation, and the flags of G4.
//!
//! Everything here reads the blob and nothing writes it, so wire mode
//! adds no state to the document model (G5). The parsing is genuinely
//! re-derived rather than read back out of the annotation text, because
//! the annotation carries no byte positions and byte positions are the
//! point.
//!
//! One trap governs the shape of this module: `parse_varint` and
//! `parse_wiretag` report a truncation as "the rest of the buffer",
//! whatever buffer they were handed. Every parse below is therefore run
//! against a slice that stops at the record's own end
//! ([`WireSlice::record_end`]), never against the whole blob — otherwise
//! a perfectly well-formed field would be reported as swallowing the
//! file.

use super::*;
use crate::annotation::{self, Tier};
use crate::node_status::row_status;
use prototext_core::helpers::{
    parse_varint, parse_wiretag, WT_END_GROUP, WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT,
};
use prototext_core::serialize::encode_text::annotation_start;
use prototext_core::serialize::render_text::NodeSpan;

/// How many bytes a wire row draws before eliding the rest as `…×N`.
///
/// A legibility choice, not a measurement: 64 bytes is 191 columns of
/// hex, already twice a comfortable terminal, and a LEN payload can be
/// megabytes. The row has to stay pannable (N4), so *some* cap is
/// needed; this one is large enough that no scalar and no tag ever
/// reaches it, and only a long string or a long packed element does.
pub(super) const WIRE_ROW_MAX_BYTES: usize = 64;

/// The bytes one drawn line owns, and how to read them.
pub(super) struct WireSlice {
    /// Absolute range in the wrapped blob — exactly what this row draws.
    pub(super) bytes: Range<usize>,
    /// Where the enclosing wire *record* ends, which is `bytes.end`
    /// except on a packed run's first row, where the record continues
    /// onto the rows below.
    ///
    /// The truncation tests ask about the record, not about the row.
    /// Without this a packed record's element-0 row would report its own
    /// declared length as unsatisfied, since only one element's worth of
    /// it is on that row.
    pub(super) record_end: usize,
    pub(super) framing: Framing,
    /// The node's own field number — needed only by [`Framing::Closing`],
    /// which is the one row whose bytes have to *agree* with something.
    pub(super) field_number: u32,
    /// Spec 0307 S1: the absolute offset before which this row's bytes
    /// are protolens' own rather than the user's.
    ///
    /// `0` on every slice but the wrapper root's, where it is
    /// `wrapper_offset` — the width of the field-1 tag and length
    /// `Blob` prepends. Those bytes are drawn like any others and then
    /// marked, rather than withheld: withholding them left the one row
    /// that is the whole document as the only LEN node on screen with
    /// no framing.
    pub(super) synthetic_end: usize,
}

/// What the row's first byte is.
#[derive(Clone, Copy)]
pub(super) enum Framing {
    /// Begins with the node's own tag: a header row, or a whole scalar.
    Tagged,
    /// A packed record's tag and length prefix, then its first element
    /// (S4). `close` is set when that element is also the last.
    PackedHead { varint: bool, close: bool },
    /// A later element of a packed run.
    Element { varint: bool, close: bool },
    /// A group's END tag, which must name `field_number`. Empty means
    /// the group was never closed.
    Closing,
    /// Bytes no framing claims — trailing content inside a message that
    /// no child took. Drawn plain, which is all that can honestly be
    /// said about them.
    Raw,
}

/// The four hues a row's bytes borrow from the document row above them
/// (spec 0225 S11), resolved once per row.
///
/// Finished `Style`s rather than `SyntaxRole`s: the borrowing — and the
/// dimming — happens once in `theme::wire_style`, and the painter only
/// ever picks one of four.
pub(super) struct WirePalette {
    pub(super) tag: Style,
    pub(super) ty: Style,
    pub(super) len: Style,
    pub(super) payload: Style,
    /// The one thing this row borrows from the document row that is not
    /// a color (spec 0232). See [`SchemaFlaw`].
    pub(super) flaw: Option<SchemaFlaw>,
}

/// An accusation the document row made that the wire row can point at,
/// but could never have found on its own.
///
/// Every case is a schema question — whether these bytes are a `string`
/// or a `bytes`, whether this eight-byte payload is a `double`, whether
/// the schema declares this field at all — and this module is
/// deliberately schema-free (spec 0225 S11, "one classifier, two rows").
/// The document row's classifier has already answered; what is left is
/// *where*, which is the one question the hex is in a position to
/// answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SchemaFlaw {
    /// `INVALID_STRING`: the payload is not valid UTF-8.
    Utf8,
    /// `INVALID_PACKED_RECORDS`: the payload does not divide into whole
    /// elements.
    PackedElements,
    /// `ENUM_UNKNOWN`: the schema has no name for this enum value.
    EnumUnknown,
    /// `nan_bits`: a NaN whose payload bits are not the ones a re-encode
    /// would write.
    NanBits,
    /// `neg`: a negative packed element, sign-extended to ten bytes.
    Neg,
    /// `TYPE_MISMATCH`: the tag's wire type contradicts the declared
    /// proto type.
    TypeMismatch,
    /// No schema declares this field, so the row above shows a field
    /// *number*. The one flaw with no keyword behind it — see
    /// [`schema_flaw`].
    Undeclared,
}

/// What a byte's band says.
///
/// Almost always a severity, decided by `annotation::tier_of` off the
/// keyword this module named for the byte. [`Band::Unknown`] is the one
/// that is not a severity at all: a field the schema does not declare is
/// not an anomaly — the bytes are well formed and mean exactly what the
/// tag says — so it wears the fold margin's own blue rather than a tier
/// color, and the two columns agree because they read one classifier.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Band {
    Tier(Tier),
    Unknown,
}

impl From<Tier> for Band {
    fn from(tier: Tier) -> Self {
        Band::Tier(tier)
    }
}

#[cfg(test)]
impl WirePalette {
    /// Four hues that are distinct from each other and from every tier
    /// color, so a test can tell which region a byte was painted from —
    /// and tell a borrowed hue from an accented one.
    pub(super) fn for_test() -> Self {
        // In the background, like every hue `theme::wire_style` hands
        // back: the run-joining in `separate` keys on that, and so does
        // the handover from a borrowed hue to a tier's, so a fixture
        // coloring the foreground would exercise a row shape the app
        // never draws. No foreground at all, because the app's is one
        // color for the whole row and so distinguishes nothing.
        let hue = |r, g, b| Style::default().bg(Color::Rgb(r, g, b));
        WirePalette {
            tag: hue(1, 0, 0),
            ty: hue(1, 1, 0),
            len: hue(0, 1, 0),
            payload: hue(0, 0, 1),
            flaw: None,
        }
    }

    /// The same four hues, with the document row's accusation attached.
    pub(super) fn accusing(flaw: SchemaFlaw) -> Self {
        WirePalette {
            flaw: Some(flaw),
            ..WirePalette::for_test()
        }
    }
}

/// Which part of the row the pen is in, so that a byte carrying no tier
/// takes that part's borrowed hue (spec 0225 S11).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Region {
    Tag,
    Type,
    Len,
    Payload,
    /// Bytes no framing claimed. They belong to none of the four and
    /// take the subdued default: nothing honest can be said about them.
    Unclaimed,
}

/// What the pointer can be resting on, one wire row's worth
/// (spec 0282 S3).
///
/// [`Region`] is wrapped rather than extended. Its five variants are
/// exactly the parts of a *record*, and the painter uses it to decide
/// banding; the two marks below are not parts of a record — they stand
/// where bytes were never drawn — so giving them `Region` variants
/// would put two non-regions into a banding enum and oblige every
/// `match` on it to answer for them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum WirePart {
    Region(Region),
    /// The `??` a [`Painter::cut`] draws: the region whose bytes ran
    /// out, and how many the record still owes when it declared a
    /// length to subtract from.
    Truncated {
        region: Region,
        missing: Option<usize>,
    },
    /// The `…×N` a [`Painter::finish`] draws, carrying its own N —
    /// `finish` has already counted it, and a second count in the popup
    /// is a second count that can disagree.
    Elided {
        hidden: usize,
    },
}

/// One run of columns of a wire row, and what the painter drew there.
///
/// `cols` are within the row's own hex, before [`margin`] prepends the
/// indent and the connector; `bytes` are absolute offsets in the blob,
/// since the painter works on a slice of it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct PartSpan {
    pub(super) cols: Range<usize>,
    pub(super) part: WirePart,
    pub(super) bytes: Range<usize>,
}

/// What one wire row drew where, and what it accused while drawing it.
///
/// Built only when someone asks for it (spec 0282 G4). The hot path
/// wants neither list: the parts are wanted on hover and only on hover,
/// and the keywords have always been a cross-check rather than
/// something the row draws.
#[derive(Default)]
pub(super) struct WireRecord {
    pub(super) parts: Vec<PartSpan>,
    /// Each accusation with the part it is *about* — which is not
    /// always the part the pen was in when it was found (spec 0282 S4).
    pub(super) flags: Vec<(Region, &'static str)>,
    /// Spec 0283 S1: the same columns at byte resolution.
    ///
    /// `parts` coalesces a run into one entry, which is right for the
    /// question *what part is this* and throws away the one this
    /// answers: *which byte*. One entry per byte drawn, in the order
    /// they were drawn.
    pub(super) cells: Vec<ByteCell>,
}

/// The columns one drawn byte occupies, and where it came from
/// (spec 0283 S1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct ByteCell {
    pub(super) cols: Range<usize>,
    /// An absolute offset in the blob.
    pub(super) at: usize,
}

/// What every part of a wire row with no hue of its own wears:
/// punctuation, the elision (spec 0225 S11) — and the whole row when the
/// document row above has no colors either (S7).
fn subdued() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// The band `style` paints, `None` if it is ordinary text — the test
/// `separate` needs to know whether the gap it is about to draw falls
/// inside one band or between two.
///
/// The background alone, not the whole style: a space has no glyph, so
/// the foreground the hex is drawn in says nothing about it, and two
/// bytes on the same band are one run whether or not the row is also
/// coloring their digits.
fn block(style: Style) -> Option<Style> {
    style.bg.map(|bg| Style::default().bg(bg))
}

/// One rendered wire row.
pub(super) struct WireRow {
    pub(super) spans: Vec<Span<'static>>,
    /// What was drawn where, and the annotation keywords this row's
    /// bytes justify — the wire-level subset of what prototext-core
    /// would print as `#@ ...` on the line above.
    ///
    /// `None` on every row a frame draws: nothing draws a keyword, the
    /// styling already went through `tier_of`, and keeping either list
    /// at runtime would allocate on every row of every frame to say
    /// what the row already shows. It is filled when a hover asks which
    /// byte it is on (spec 0282 S5), and by the tests, which use it to
    /// cross-check the two independent derivations (0225 S11).
    pub(super) record: Option<WireRecord>,
}

impl WireRow {
    /// The keywords alone, in the order they were found — the shape the
    /// cross-check of 0225 S11 wants, where the hover box wants them
    /// filtered by the part they are about and reads `flags` directly.
    #[cfg(test)]
    pub(super) fn flags(&self) -> Vec<&'static str> {
        match &self.record {
            Some(record) => record.flags.iter().map(|(_, kw)| *kw).collect(),
            None => Vec::new(),
        }
    }
}

/// The one-entry memo of S4: where the *next* element of a packed run
/// begins.
///
/// A window's rows for a single packed node are consecutive, so this
/// always hits and the payload is walked once per frame rather than once
/// per row — the difference between linear and quadratic in the run
/// length.
///
/// Only the committed path needs it. A preview's spans are one per
/// packed element already, so the overlay reads the boundaries off them
/// instead of re-deriving them (S9).
#[derive(Default)]
pub(super) struct PackedCursor {
    at: Option<(usize, u32, usize)>,
}

impl PackedCursor {
    fn hit(&self, node: usize, line: u32) -> Option<usize> {
        match self.at {
            Some((n, l, offset)) if n == node && l == line => Some(offset),
            _ => None,
        }
    }

    fn record(&mut self, node: usize, line: u32, offset: usize) {
        self.at = Some((node, line, offset));
    }
}

/// S2's head/tail rule, stated once for both the committed document and
/// a preview overlay.
///
/// A node's *head* runs from its own tag to its first rendered child's,
/// its *tail* from its last rendered child's end to its own. Head ∪
/// children ∪ tail is the node's whole range, recursively, which is G2
/// by construction rather than by three separately correct cases.
///
/// `bytes` is the node's whole extent and `children` its rendered
/// children's, `None` when it draws none; only the two ends of the
/// latter matter, since children are contiguous between them. `footer`
/// picks the tail. The two callers differ only in where they read those
/// three from — the arena, or the preview's own flat span list.
fn head_or_tail(
    span: &NodeSpan,
    bytes: Range<usize>,
    children: Option<Range<usize>>,
    footer: bool,
) -> WireSlice {
    let field_number = span.field_number;
    if !footer {
        let head_end = children.map_or(bytes.end, |c| c.start);
        return WireSlice {
            bytes: bytes.start..head_end,
            record_end: bytes.end,
            framing: Framing::Tagged,
            field_number,
            synthetic_end: 0,
        };
    }
    let tail_start = children.map_or(bytes.end, |c| c.end);
    WireSlice {
        bytes: tail_start..bytes.end,
        record_end: bytes.end,
        framing: if u32::from(span.wire_type()) == WT_START_GROUP {
            Framing::Closing
        } else {
            Framing::Raw
        },
        field_number,
        synthetic_end: 0,
    }
}

impl App {
    /// The bytes the line `pos` names owns, or `None` for an unrendered
    /// slot.
    pub(super) fn wire_slice(&self, pos: LinePos, memo: &mut PackedCursor) -> Option<WireSlice> {
        let idx = pos.node;
        let node = self.tree.get(idx)?;
        if !node.is_rendered() {
            return None;
        }
        let start = self.arena.raw_start()[idx] as usize;
        let end = self.arena.raw_end()[idx] as usize;

        if node.span.packed_record_start != NO_PACKED_RECORD {
            return self.packed_slice(pos, start, end, memo);
        }

        let children = self.first_child(idx).map(|first| {
            let last = self.last_child(idx).unwrap_or(first);
            self.arena.raw_start()[first] as usize..self.arena.raw_end()[last] as usize
        });
        // A bracketed node's own lines are its first and its last; a
        // flat one has only the head.
        let footer = node.is_bracketed() && pos.line_in_node > 0;
        let mut slice = head_or_tail(&node.span, start..end, children, footer);
        // Spec 0307 S1. `Blob`'s field-1 tag and length are protolens'
        // own bytes, and spec 0225 S3 refused the whole node rather than
        // show them — which, on an untyped blob rendering as one flat
        // line, hid the entire file. Spec 0306 narrowed them away
        // instead, and had to call what was left `Framing::Raw`: the one
        // row that is the whole document became the only LEN node on
        // screen with no framing at all.
        //
        // So they are drawn, framed like anything else, and marked. Set
        // on both of the root's rows: a bracketed root's tail begins at
        // or past `wrapper_offset`, so the flag costs that row nothing
        // and needs no second predicate to say so.
        if self.wrapper_offset > 0 && self.parent(idx).is_none() {
            slice.synthetic_end = self.wrapper_offset;
        }
        Some(slice)
    }

    /// S4: element `pos.line_in_node` of a packed run, with the record's
    /// tag and length prefixed onto element 0's row so that G2 still
    /// holds across the run.
    fn packed_slice(
        &self,
        pos: LinePos,
        start: usize,
        end: usize,
        memo: &mut PackedCursor,
    ) -> Option<WireSlice> {
        let idx = pos.node;
        let node = &self.tree[idx];
        // The element's own wire type, not the record's outer `WT_LEN` —
        // which is exactly what `NodeSpan::wire_type` holds for a packed
        // element, so nothing has to be looked up in the schema.
        let elem_wt = u32::from(node.span.wire_type());
        let varint = !matches!(elem_wt, WT_I64 | WT_I32);

        let tag = parse_wiretag(&self.blob, start);
        let len = parse_varint(&self.blob, tag.next_pos);
        let payload_start = len.next_pos;
        let payload_end = payload_start
            .saturating_add(len.varint.unwrap_or(0) as usize)
            .min(end);

        let mut at = match memo.hit(idx, pos.line_in_node) {
            Some(offset) => offset,
            None => {
                let mut offset = payload_start;
                for _ in 0..pos.line_in_node {
                    offset = element_end(&self.blob, offset, payload_end, elem_wt);
                }
                offset
            }
        };
        if at > payload_end {
            at = payload_end;
        }
        let stop = element_end(&self.blob, at, payload_end, elem_wt);
        memo.record(idx, pos.line_in_node + 1, stop);

        let close = pos.line_in_node + 1 == node.lines_total;
        let framing = if pos.line_in_node == 0 {
            Framing::PackedHead { varint, close }
        } else {
            Framing::Element { varint, close }
        };
        Some(WireSlice {
            bytes: if pos.line_in_node == 0 { start } else { at }..stop,
            record_end: payload_end,
            framing,
            field_number: node.span.field_number,
            // A packed run is never the root, so none of its bytes are
            // protolens' own.
            synthetic_end: 0,
        })
    }

    /// The whole wire row for `pos`, margin included — `None` when the
    /// line has no bytes it may show, or none at all.
    ///
    /// `left` is the document row's own margin — as wide as its
    /// indentation, so the hex lines up under the text it describes
    /// (S5), and carrying spec 0328's bars so they do not break on
    /// every other terminal row. Column 0 is not here: the heat-cue
    /// gutter is prepended by `render`, blank, for the same reason it
    /// is blank — a cue reports how a *node* scores.
    pub(super) fn wire_row(
        &self,
        pos: LinePos,
        left: Vec<Span<'static>>,
        memo: &mut PackedCursor,
        palette: Option<&WirePalette>,
    ) -> Option<Vec<Span<'static>>> {
        let slice = self.wire_slice(pos, memo)?;
        margin(left, wire_spans(&self.blob, &slice, self.theme, palette))
    }

    /// Spec 0225 S11: the four hues this row's bytes borrow, resolved
    /// from the document row's own syntax hints.
    ///
    /// `None` is S7's monochrome state, and is reached with no branch of
    /// its own: `render` *clears* `window_styles` while input is
    /// pending, so a row with no hints is exactly a row whose document
    /// half is uncolored, and the two go grey together.
    pub(super) fn wire_palette(&self, window_index: usize, text: &str) -> Option<WirePalette> {
        let hints = self.window_styles.get(window_index)?;
        if hints.is_empty() {
            return None;
        }
        let role_at = |offset: usize| {
            hints
                .iter()
                .find(|(range, _)| range.contains(&offset))
                .map(|(_, role)| *role)
        };
        let indent = text.len() - text.trim_start().len();
        let name = role_at(indent);
        Some(WirePalette {
            tag: theme::wire_style(theme::WireRole::Tag, name, self.theme),
            // Falling back to the field name's role, not to nothing: a
            // row with the annotations hidden has no type token to
            // borrow from, and the honest rendering there is a tag that
            // is one color throughout rather than one with a grey wire
            // type in front of it.
            //
            // The same fallback catches the row whose first annotation
            // token is an accusation rather than a type — `INVALID_LEN`,
            // `INVALID_STRING`. The wire type there is well formed and
            // is not what the accusation is about, so borrowing the
            // tier's red would put the alarm on the one part of the row
            // that is fine. This row's own `accuse` calls are what put a
            // tier on the tag, and they name the bits they mean.
            ty: theme::wire_style(
                theme::WireRole::Type,
                type_offset(text)
                    .and_then(role_at)
                    .filter(is_a_type)
                    .or(name),
                self.theme,
            ),
            len: theme::wire_style(theme::WireRole::Length, None, self.theme),
            payload: theme::wire_style(
                theme::WireRole::Payload,
                value_offset(text).and_then(role_at),
                self.theme,
            ),
            flaw: schema_flaw(text),
        })
    }

    /// The same, for line `line` of the preview overlay (S9).
    ///
    /// A preview is a proposal to read the same bytes as a different
    /// type, so the wire row is what shows the bytes really are the
    /// same. It is drawn from the overlay's own spans and its own copy
    /// of the rendered bytes — nothing here touches the arena, which the
    /// preview deliberately never enters.
    pub(super) fn preview_wire_row(
        &self,
        line: usize,
        left: Vec<Span<'static>>,
        palette: Option<&WirePalette>,
    ) -> Option<Vec<Span<'static>>> {
        let overlay = self.preview_overlay.as_ref()?;
        let slice = preview_slice(&overlay.spans, line)?;
        margin(
            left,
            wire_spans(&overlay.bytes, &slice, self.theme, palette),
        )
    }

    /// What the pointer is resting on, when it is resting on a wire row
    /// (spec 0282 S5).
    ///
    /// The row is drawn again, once, with the recorder attached, and
    /// the column looked up in what it noted. Drawing it again rather
    /// than remembering it is the whole of G4: this happens at most
    /// once per pointer movement, where remembering would cost a `Vec`
    /// per wire row per frame for a feature that is idle almost always.
    ///
    /// `None` for the document row of a pair, for a column past the
    /// row's drawn end, and for the punctuation between two parts.
    pub(super) fn wire_part_at(&self, column: u16, row: u16) -> Option<WireHit> {
        let (line_idx, part) = self.main_pane_line_part(column, row)?;
        if part == 0 {
            return None;
        }
        let pos = self.line_pos(line_idx)?;
        // `display_row_text`, and not `row_content`: the margin `margin`
        // draws is `FOLD_FIELD_WIDTH + indent` wide, so the indent it
        // wants is the line's own — and `row_content` has already put
        // the fold field in front of it. Measuring there subtracts the
        // fold field twice. This is the same text `render` measures for
        // the same purpose, and the same text `wire_palette` reads.
        let text = self.display_row_text(self.committed_row_at(line_idx, pos));

        // The same mapping `set_caret_from_click` computes — pane
        // column, less the reserved glyph gutter, plus the pan already
        // taken off the content — and then the margin `margin` puts in
        // front of the hex, which is the document row's own indent.
        let index = (column.saturating_sub(self.main_area.x) as usize)
            .checked_sub(render::HEAT_FIELD_WIDTH)?
            + self.pan_offset;
        let indent = text.len() - text.trim_start().len();
        let hex = index
            .checked_sub(render::FOLD_FIELD_WIDTH + indent + WIRE_CONNECTOR.chars().count())?;

        let mut memo = PackedCursor::default();
        let slice = self.wire_slice(pos, &mut memo)?;
        let palette = recording_palette(&text, self.theme);
        let record = wire_spans_recorded(&self.blob, &slice, self.theme, Some(&palette)).record?;
        let span = record.parts.iter().find(|p| p.cols.contains(&hex))?;

        // Filed under the part they are about (S4), so a tag's
        // `tag_ohb` does not appear in the box for the type digit
        // beside it. The `??` inherits the flaws of the region it
        // interrupted: the keyword accuses a byte, and the mark is that
        // same accusation drawn where the byte is missing.
        let region = match span.part {
            WirePart::Region(region) | WirePart::Truncated { region, .. } => Some(region),
            WirePart::Elided { .. } => None,
        };
        let flaws = record
            .flags
            .iter()
            .filter(|(at, _)| Some(*at) == region)
            .map(|(_, keyword)| *keyword)
            .collect();
        Some(WireHit {
            pos,
            part: span.part,
            bytes: span.bytes.clone(),
            // Spec 0283 S2: which of the part's bytes, for the parts
            // that are more than one. `None` at the two marks, which
            // stand where bytes are not.
            byte: record
                .cells
                .iter()
                .find(|cell| cell.cols.contains(&hex))
                .map(|cell| cell.at),
            flaws,
        })
    }
}

/// One part of one wire row, as the pointer found it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct WireHit {
    pub(super) pos: LinePos,
    pub(super) part: WirePart,
    /// Absolute offsets in the blob — empty for the two marks, which
    /// stand where bytes are not.
    pub(super) bytes: Range<usize>,
    /// The one byte of `bytes` the pointer is actually on
    /// (spec 0283 S2).
    pub(super) byte: Option<usize>,
    /// The annotation keywords filed under this part.
    pub(super) flaws: Vec<&'static str>,
}

impl WireHit {
    /// Whether two hits are the same part — everything except which of
    /// its bytes the pointer is on (spec 0283 S11).
    ///
    /// Destructured rather than compared field by field, so a field
    /// added later has to be put on one side of this line deliberately.
    pub(super) fn same_part(&self, other: &Self) -> bool {
        let Self {
            pos,
            part,
            bytes,
            byte: _,
            flaws,
        } = self;
        (pos, part, bytes, flaws) == (&other.pos, &other.part, &other.bytes, &other.flaws)
    }
}

/// The palette a recording run is given.
///
/// The four hues are what `wire_palette` computes with no syntax hints
/// at all, because no answer a hover box gives depends on a color. What
/// it does depend on is the document row's accusation, and that is read
/// from the row's own text exactly as `wire_palette` reads it — so the
/// keywords this run files are the ones the drawn row banded.
fn recording_palette(text: &str, theme: ThemeKind) -> WirePalette {
    WirePalette {
        tag: theme::wire_style(theme::WireRole::Tag, None, theme),
        ty: theme::wire_style(theme::WireRole::Type, None, theme),
        len: theme::wire_style(theme::WireRole::Length, None, theme),
        payload: theme::wire_style(theme::WireRole::Payload, None, theme),
        flaw: schema_flaw(text),
    }
}

/// Where the row's annotation names its type: the first token after
/// `#@ ` (spec 0225 S11). That is the declared proto type on a known
/// field (`string`, `int64`) and the wire type on an unknown one
/// (`varint`, `bytes`) — the same fact the tag's low three bits carry.
///
/// `None` when the row has no annotation, which includes every row
/// while `--no-annotations` is in force.
fn type_offset(text: &str) -> Option<usize> {
    // `annotation_start` reports where the *value* ends, before the two
    // separator spaces, so the marker still has to be stepped over.
    let value_end = annotation_start(text)?;
    let rest = text.get(value_end..)?;
    let after_marker = rest.find("#@")? + "#@".len();
    let token = rest.get(after_marker..)?;
    let at = value_end + after_marker + (token.len() - token.trim_start().len());
    (at < text.len()).then_some(at)
}

/// The first accusation in the row's annotation that this module can
/// localize (spec 0232, widened by spec 0279 S2).
///
/// Every token, not only the first: two of these keywords *open* an
/// annotation (`INVALID_STRING`, `INVALID_PACKED_RECORDS`) and the rest
/// follow a type token (`#@ double = 6; nan_bits: …`), so reading the
/// first word alone finds half of them.
///
/// Read from the text rather than from the styles: the keyword is the
/// fact, and two keywords out of the fifteen in `annotation::INVALID`
/// share the one tier color.
///
/// [`SchemaFlaw::Undeclared`] is the one that has no keyword — an
/// undeclared field is not an anomaly and prototext-core emits nothing
/// for it, so what says so is the *absence* of a name. `row_status` is
/// the one classifier for that (spec 0247 S3), and it already ranks
/// every anomaly keyword above `Unknown`, so this arm answers only for a
/// row that accused nothing.
fn schema_flaw(text: &str) -> Option<SchemaFlaw> {
    let keyword = type_offset(text).and_then(|at| {
        text[at..]
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .find_map(|word| match word {
                "INVALID_STRING" => Some(SchemaFlaw::Utf8),
                "INVALID_PACKED_RECORDS" => Some(SchemaFlaw::PackedElements),
                "ENUM_UNKNOWN" => Some(SchemaFlaw::EnumUnknown),
                "nan_bits" => Some(SchemaFlaw::NanBits),
                "neg" => Some(SchemaFlaw::Neg),
                "TYPE_MISMATCH" => Some(SchemaFlaw::TypeMismatch),
                _ => None,
            })
    });
    keyword.or_else(|| (row_status(text) == Status::Unknown).then_some(SchemaFlaw::Undeclared))
}

/// Whether the role `type_offset` landed on really names a type.
///
/// It does not on a row whose annotation opens with an accusation
/// instead — `INVALID_STRING`, `TRUNCATED_BYTES` — and a tier color is
/// how that is told apart, since the two are drawn from the same token
/// position.
fn is_a_type(role: &SyntaxRole) -> bool {
    !matches!(
        role,
        SyntaxRole::AnnotationNonCanonical | SyntaxRole::AnnotationInvalid
    )
}

/// Where the row's value begins: after the first `:` outside the
/// annotation, past the spaces (spec 0225 S11). The first `:` is the
/// separator even when the value is a string containing one, since the
/// field name comes first.
///
/// `None` on a row with no value — a message header, a footer — which is
/// also a row with no payload bytes to color.
fn value_offset(text: &str) -> Option<usize> {
    let code = render::code_part(text);
    let colon = code.find(':')?;
    let rest = &code[colon + 1..];
    let at = colon + 1 + (rest.len() - rest.trim_start().len());
    (at < code.len()).then_some(at)
}

/// The connector `tree(1)` draws, at the document row's own indent
/// (spec 0225 S5).
pub(super) const WIRE_CONNECTOR: &str = "└── ";

/// The row's indentation and connector (spec 0225 S5).
///
/// The document row's own indent — no deeper — and then
/// [`WIRE_CONNECTOR`], which is what says *which* document row this one
/// belongs to. Indentation alone cannot: a nested message's own first
/// field carries the same indent one row further down, so on a screen of
/// alternating rows the eye has to count to pair them up. The elbow
/// points at exactly one row, the one above it.
///
/// It is drawn in [`subdued`] because it is structure, not content, and
/// the row's hex is loud. Column 0 is not here — the heat-cue gutter is
/// prepended by `render`, blank, for the same reason it is blank: a cue
/// reports how a *node* scores.
///
/// `None` when the row drew nothing. A line can own a byte range and
/// still have nothing to show for it — a footer whose children ended
/// exactly where the message did, most often — and an elbow pointing at
/// an empty row says only that the elbow is there. Suppressing it makes
/// the connector mean what it looks like it means: below this line are
/// its bytes.
/// Spec 0328 S5: `left` is the document row's own margin spans, from
/// `render`'s `wire_margin_spans` — the same `FOLD_FIELD_WIDTH + indent`
/// columns this used to build for itself, with the bars drawn in.
fn margin(left: Vec<Span<'static>>, row: WireRow) -> Option<Vec<Span<'static>>> {
    if row.spans.is_empty() {
        return None;
    }
    let mut spans = Vec::with_capacity(row.spans.len() + left.len() + 1);
    spans.extend(left);
    spans.push(Span::styled(WIRE_CONNECTOR, subdued()));
    spans.extend(row.spans);
    Some(spans)
}

/// The bytes overlay line `line` owns, within the preview's own rendered
/// byte buffer.
///
/// The spans are in post-order, so a descendant always precedes its
/// ancestor and the *first* span containing a line is the innermost one
/// — the one that draws it. That is the whole of the structure this
/// needs; the committed path's arena has no counterpart here and needs
/// none.
fn preview_slice(spans: &[NodeSpan], line: usize) -> Option<WireSlice> {
    let line = u32::try_from(line).ok()?;
    let (i, span) = spans
        .iter()
        .enumerate()
        .find(|(_, s)| s.text_range.contains(&line))?;

    if span.packed_record_start != NO_PACKED_RECORD {
        return Some(preview_packed_slice(spans, i));
    }

    let bytes = span.raw_range.start as usize..span.raw_range.end as usize;
    if span.is_message {
        if line == span.text_range.start {
            // The children are the spans this one encloses at one more
            // level of indentation. They are contiguous, so only the
            // extent matters, and post-order puts them all before `i`.
            let children = child_extent(&spans[..i], span);
            return Some(head_or_tail(span, bytes, children, false));
        }
        if line + 1 == span.text_range.end {
            let children = child_extent(&spans[..i], span);
            return Some(head_or_tail(span, bytes, children, true));
        }
        // Spec 0174's `...` marker: a line inside a bracketed span that
        // is neither its header nor its footer belongs to no span at
        // all, because the span whose line it replaced was dropped. It
        // is protolens' own note that bytes were withheld, so there are
        // none to show under it.
        return None;
    }
    Some(head_or_tail(span, bytes, None, false))
}

/// The byte extent of `parent`'s direct children among the spans that
/// precede it.
fn child_extent(before: &[NodeSpan], parent: &NodeSpan) -> Option<Range<usize>> {
    let mut extent: Option<Range<usize>> = None;
    for s in before {
        if s.level != parent.level + 1 || !parent.text_range.contains(&s.text_range.start) {
            continue;
        }
        // A packed element's span covers its element only, but the
        // record's tag and length ride on the run's first row (S4). They
        // are therefore the children's, not the parent's head's.
        let start = if s.packed_record_start == NO_PACKED_RECORD {
            s.raw_range.start as usize
        } else {
            s.packed_record_start as usize
        };
        let end = s.raw_range.end as usize;
        extent = Some(match extent {
            Some(e) => e.start.min(start)..e.end.max(end),
            None => start..end,
        });
    }
    extent
}

/// S4 for a preview: `spans[i]` is one element of a packed run, and the
/// run's elements are consecutive spans sharing a `packed_record_start`.
///
/// No walk and no memo — prototext-core already emitted one span per
/// element, so the boundaries the committed path has to re-derive from
/// the bytes are simply read off here.
fn preview_packed_slice(spans: &[NodeSpan], i: usize) -> WireSlice {
    let span = &spans[i];
    let run = span.packed_record_start;
    let same_run = |s: &NodeSpan| s.packed_record_start == run;
    let first = i == 0 || !same_run(&spans[i - 1]);
    let close = i + 1 == spans.len() || !same_run(&spans[i + 1]);

    let elem_wt = u32::from(span.wire_type());
    let varint = !matches!(elem_wt, WT_I64 | WT_I32);
    let start = if first {
        run as usize
    } else {
        span.raw_range.start as usize
    };
    let end = span.raw_range.end as usize;
    WireSlice {
        bytes: start..end,
        record_end: end,
        framing: if first {
            Framing::PackedHead { varint, close }
        } else {
            Framing::Element { varint, close }
        },
        field_number: span.field_number,
        // A preview's bytes are its own re-encoded buffer, whose
        // offsets have nothing to do with `Blob`'s prefix (0307 N4).
        synthetic_end: 0,
    }
}

/// Where the packed element starting at `at` ends.
///
/// The boundaries are not in the arena — the maximal walk does not
/// descend into a packed payload (spec 0216 S2) — so they are re-derived
/// from the bytes: a varint parse for the varint kinds, a fixed stride
/// otherwise.
fn element_end(blob: &[u8], at: usize, payload_end: usize, elem_wt: u32) -> usize {
    if at >= payload_end {
        return payload_end;
    }
    match elem_wt {
        WT_I64 => (at + 8).min(payload_end),
        WT_I32 => (at + 4).min(payload_end),
        _ => parse_varint(&blob[..payload_end], at).next_pos,
    }
}

/// `slice`'s bytes as a styled hex row.
pub(super) fn wire_spans(
    blob: &[u8],
    slice: &WireSlice,
    theme: ThemeKind,
    palette: Option<&WirePalette>,
) -> WireRow {
    paint(blob, slice, theme, palette, false)
}

/// The same row, with a note of what was drawn where (spec 0282 S3).
///
/// One row per pointer movement, never per frame: this is the whole of
/// G4, and the reason the recorder is a parameter rather than always
/// on.
pub(super) fn wire_spans_recorded(
    blob: &[u8],
    slice: &WireSlice,
    theme: ThemeKind,
    palette: Option<&WirePalette>,
) -> WireRow {
    paint(blob, slice, theme, palette, true)
}

fn paint(
    blob: &[u8],
    slice: &WireSlice,
    theme: ThemeKind,
    palette: Option<&WirePalette>,
    record: bool,
) -> WireRow {
    let record_end = slice.record_end.max(slice.bytes.end).min(blob.len());
    let start = slice.bytes.start.min(record_end);
    let mut painter = Painter {
        rec: &blob[start..record_end],
        base: start,
        limit: slice.bytes.end.saturating_sub(start),
        theme,
        palette,
        region: Region::Unclaimed,
        synthetic: slice.synthetic_end.saturating_sub(start),
        last_block: None,
        spans: Vec::new(),
        record: record.then(WireRecord::default),
        cols: 0,
        drawn: 0,
        dropped: 0,
        need_space: false,
    };
    let end = match slice.framing {
        Framing::Raw => 0,
        Framing::Tagged => draw_tagged(&mut painter),
        Framing::PackedHead { varint, close } => draw_packed_head(&mut painter, varint, close),
        Framing::Element { varint, close } => {
            let end = draw_element(&mut painter, 0, varint);
            if close {
                painter.punct("]");
            }
            end
        }
        Framing::Closing => draw_closing(&mut painter, slice.field_number),
    };
    // Whatever the framing did not account for is still this row's, and
    // G2 says every byte appears. Drawn plain: nothing here can say more
    // about them than that they are there.
    painter.region = Region::Unclaimed;
    painter.bytes(end..painter.limit, None);
    painter.finish()
}

/// A tagged row: the node's tag, then whatever its wire type implies.
fn draw_tagged(painter: &mut Painter) -> usize {
    if let Some(end) = draw_stray_group_end(painter) {
        return end;
    }
    let (mut at, wtype) = draw_tag(painter, false);
    // A tag that did not parse says nothing about what follows it, so
    // nothing follows it: the rest of the row falls through to the
    // caller's plain fill.
    let Some(wtype) = wtype else {
        return at;
    };
    match wtype {
        WT_LEN => {
            painter.punct(":");
            painter.region = Region::Len;
            let (declared, next) = draw_varint(painter, at, "len_ohb", "INVALID_LEN", None);
            at = next;
            let Some(declared) = declared else {
                return at;
            };
            let declared = declared as usize;
            painter.punct("[");
            painter.region = Region::Payload;
            let available = painter.rec.len().saturating_sub(at);
            let shown = declared.min(available);
            // A payload that ran out is an invalid payload, and the
            // bytes that did arrive are the ones the reader has to look
            // at: they wear the band, not just the `!` at their end.
            let short = declared > available;
            let tier = short
                .then(|| painter.accuse(Region::Payload, "TRUNCATED_BYTES"))
                .flatten();
            if short {
                painter.bytes(at..at + shown, tier);
                painter.cut();
                painter.missing(declared - available);
            } else {
                draw_payload(painter, at..at + shown);
            }
            if !short && at + shown <= painter.limit {
                // The payload is entirely on this row, so this row is
                // where it closes. A message header's payload is on its
                // children's rows and gets no `]` anywhere, which is
                // right: nothing has ended.
                painter.punct("]");
            }
            at + shown
        }
        WT_I64 | WT_I32 => {
            let width = if wtype == WT_I64 { 8 } else { 4 };
            painter.punct("[");
            painter.region = Region::Payload;
            let available = painter.rec.len().saturating_sub(at);
            let shown = width.min(available);
            let short = shown < width;
            let tier = short
                .then(|| {
                    painter.accuse(
                        Region::Payload,
                        if wtype == WT_I64 {
                            "INVALID_FIXED64"
                        } else {
                            "INVALID_FIXED32"
                        },
                    )
                })
                .flatten();
            if short {
                painter.bytes(at..at + shown, tier);
                painter.cut();
            } else {
                let nan = painter.told(Region::Payload, SchemaFlaw::NanBits, "nan_bits");
                draw_fixed(painter, at..at + shown, nan);
                painter.punct("]");
            }
            at + shown
        }
        WT_VARINT => {
            painter.punct("[");
            painter.region = Region::Payload;
            // Spec 0279 S3: an enum value the schema has no name for is
            // accused as a whole. The number *is* the accusation, and
            // every byte of the varint carries part of it.
            let unknown = painter.told(Region::Payload, SchemaFlaw::EnumUnknown, "ENUM_UNKNOWN");
            let (value, next) = draw_varint(painter, at, "val_ohb", "INVALID_VARINT", unknown);
            if value.is_some() {
                painter.punct("]");
            }
            next
        }
        // `WT_START_GROUP`: a group's opening tag is the whole of its
        // row, with no length and no payload beside it. Its closing tag
        // arrives as `Framing::Closing` instead, and a closing tag that
        // arrives here is `draw_stray_group_end`'s, already returned.
        _ => at,
    }
}

/// An END_GROUP tag on a row that is not a group's footer, which is
/// prototext-core's `INVALID_GROUP_END`: a group end closing a group
/// nobody opened. `None` when the row is not one, which is almost
/// always.
///
/// The core renders it as a bytes-valued pseudo-field whose "value" is
/// however much of the buffer followed the tag, so the accusation runs
/// to the end of the row: those bytes are on it only because the stray
/// tag was accepted, and calling them unclaimed would say the row found
/// nothing to complain about after the first two.
///
/// The wire type is spared, as it is under `TAG_OOR` and
/// `END_MISMATCH`. It reports a group end and there is one; what is
/// wrong is that the tag is here at all, which is a fact about the row
/// and not about those three bits.
///
/// A tag too malformed to parse is left to `draw_tag`, which has a
/// better answer for it — `INVALID_GROUP_END` would be claiming to
/// know a wire type that was never read.
fn draw_stray_group_end(painter: &mut Painter) -> Option<usize> {
    // The wire type is the first byte's low three bits whatever the
    // tag's length, so this costs one byte to rule out.
    if u32::from(painter.rec.first()? & 0x07) != WT_END_GROUP {
        return None;
    }
    let tag = parse_wiretag(painter.rec, 0);
    if tag.wtag_gar.is_some() {
        return None;
    }
    let tier = painter.accuse(Region::Tag, "INVALID_GROUP_END");
    painter.region = Region::Tag;
    let end = tag.next_pos.min(painter.rec.len());
    painter.tag_head(0, tier, None);
    for i in 1..end {
        painter.byte(i, tier);
    }
    painter.region = Region::Unclaimed;
    painter.bytes(end..painter.limit, tier);
    Some(painter.limit)
}

/// S4's element-0 row: the tag and length of the wire record the run's
/// elements belong to, drawn exactly as any other record's are
/// (spec 0267 S5). `pack_size` accuses nothing, so there is no accent
/// here to set the length prefix apart — the row still names the
/// keyword, and `tier_of` answers `None`, which is the severity.
fn draw_packed_head(painter: &mut Painter, varint: bool, close: bool) -> usize {
    painter.accuse(Region::Len, "pack_size");
    let (mut at, wtype) = draw_tag(painter, false);
    if wtype != Some(WT_LEN) {
        return at;
    }
    painter.punct(":");
    painter.region = Region::Len;
    let (_, next) = draw_varint(painter, at, "len_ohb", "INVALID_LEN", None);
    at = next;
    painter.punct("[");
    painter.region = Region::Payload;
    let end = draw_element(painter, at, varint);
    if close {
        painter.punct("]");
    }
    end
}

/// One packed element, from `at` to the end of the row.
///
/// The palette is the *element's* own document row's, so the two
/// accusations a single element can carry (spec 0279 S3) land on that
/// element and not on its neighbors in the run.
fn draw_element(painter: &mut Painter, at: usize, varint: bool) -> usize {
    painter.region = Region::Payload;
    if varint {
        // A negative element is sign-extended to ten bytes and an
        // unnamed enum value is however many its number needs: in both
        // cases the whole varint is what the row above is about.
        let whole = painter
            .told(Region::Payload, SchemaFlaw::Neg, "neg")
            .or_else(|| painter.told(Region::Payload, SchemaFlaw::EnumUnknown, "ENUM_UNKNOWN"));
        draw_varint(painter, at, "val_ohb", "INVALID_VARINT", whole).1
    } else {
        let nan = painter.told(Region::Payload, SchemaFlaw::NanBits, "nan_bits");
        draw_fixed(painter, at..painter.limit, nan);
        painter.limit
    }
}

/// A group's footer row: the END tag, which has to name the group's own
/// field number.
fn draw_closing(painter: &mut Painter, field_number: u32) -> usize {
    painter.region = Region::Tag;
    if painter.limit == 0 {
        // Spec 0225 S11: `!` already means *the byte that should be here
        // is not*, and an unterminated group is exactly that one level
        // up — the END tag is the missing byte. The word said no more.
        painter.accuse(Region::Tag, "OPEN_GROUP");
        painter.cut();
        return 0;
    }
    let tag = parse_wiretag(painter.rec, 0);
    if let Some(actual) = tag.wfield {
        if actual != u64::from(field_number) {
            // Spec 0225 S11: the same shape `TAG_OOR` uses below. The
            // wire-type digit is END_GROUP and is correct; the field
            // number beside it is not. The number is then legible in
            // the hex, so no marker repeats it.
            let tier = painter.accuse(Region::Tag, "END_MISMATCH");
            painter.tag_head(0, tier, None);
            for i in 1..tag.next_pos {
                painter.byte(i, tier);
            }
            return tag.next_pos;
        }
    }
    draw_tag(painter, true).0
}

/// The tag varint at offset 0, styled per S11: the first byte split by
/// `Painter::tag_head` into its wire type and its field number, because
/// the two fail separately.
///
/// Returns where the tag ended and the wire type it declared — `None`
/// when the tag did not parse, which is the caller's signal that nothing
/// can be said about the bytes after it.
fn draw_tag(painter: &mut Painter, closing: bool) -> (usize, Option<u32>) {
    painter.region = Region::Tag;
    if painter.rec.is_empty() {
        painter.cut();
        return (0, None);
    }
    if painter.rec[0] & 0x07 > 5 {
        // `parse_wiretag` reports this by swallowing the rest of the
        // buffer, which says nothing about *where* the fault is. Only
        // the three type bits are wrong, so only they are accused.
        let tier = painter.accuse(Region::Type, "INVALID_TAG_TYPE");
        painter.tag_head(0, None, tier);
        return (1, None);
    }
    let tag = parse_wiretag(painter.rec, 0);
    if tag.wtag_gar.is_some() {
        // Spec 0279 S4: the last byte is the one whose continuation bit
        // is not honored; the ones before it are a prefix that decoded.
        let tier = painter.accuse(
            Region::Tag,
            if closing {
                "INVALID_GROUP_END"
            } else {
                "INVALID_VARINT"
            },
        );
        let end = painter.rec.len();
        painter.bytes(0..end.saturating_sub(1), None);
        painter.bytes(end.saturating_sub(1)..end, tier);
        painter.cut();
        return (end, None);
    }
    let end = tag.next_pos;
    let out_of_range = tag.wfield_oor.is_some();
    let overhang = tag.wfield_ohb.unwrap_or(0) as usize;
    let oor = out_of_range
        .then(|| painter.accuse(Region::Tag, if closing { "ETAG_OOR" } else { "TAG_OOR" }))
        .flatten();
    let ohb = (!out_of_range && overhang > 0)
        .then(|| painter.accuse(Region::Tag, if closing { "etag_ohb" } else { "tag_ohb" }))
        .flatten();
    // Spec 0279 S3: the three type bits are the whole of a
    // `TYPE_MISMATCH`. The field number found its declaration and the
    // payload is what the tag says it is; what contradicts the schema
    // is the type the tag declares, and nothing else on the row.
    let mismatch = painter.told(Region::Type, SchemaFlaw::TypeMismatch, "TYPE_MISMATCH");
    // Spec 0279's 2026-08-12 amendment: when no schema declares the
    // field, its number is what the reader has instead of a name, so the
    // whole field-number half wears the fold margin's blue. `TAG_OOR`
    // outranks it — a number out of range is a defect, not a gap in the
    // schema — and the padding of an overlong tag keeps its own band the
    // same way a padded value's does.
    let unknown = painter.undeclared();
    // The field portion goes red, the wire type stays whatever it was:
    // an out-of-range field number says nothing about the type bits.
    painter.tag_head(0, oor.or(unknown), mismatch);
    for i in 1..end {
        let band = if out_of_range {
            oor
        } else if i + overhang >= end {
            // A varint is little-endian, so the padding of an overlong
            // one is its trailing `0x80`…`0x00` run.
            ohb
        } else {
            unknown
        };
        painter.byte(i, band);
    }
    (end, tag.wtype)
}

/// A whole, present LEN payload, with the `Invalid` band on whichever
/// of its bytes the document row's accusation is about (spec 0232).
///
/// Plain when the row made no accusation this module can localize,
/// which is every ordinary row.
fn draw_payload(painter: &mut Painter, range: Range<usize>) {
    let Some(flaw) = painter.palette.and_then(|p| p.flaw) else {
        painter.bytes(range, None);
        return;
    };
    // Only as far as the row will draw. A payload can be megabytes and
    // this runs per frame; bytes the elision swallows cannot be pointed
    // at anyway.
    let end = range
        .end
        .min(painter.limit)
        .min(range.start + WIRE_ROW_MAX_BYTES)
        .max(range.start);
    let payload = &painter.rec[range.start..end];
    let (bad, open) = match flaw {
        SchemaFlaw::Utf8 => (utf8_flaws(payload), false),
        SchemaFlaw::PackedElements => {
            let (span, open) = packed_flaw(payload);
            (vec![span], open)
        }
        // The other flaws are not about a LEN payload's contents; they
        // reach the row through `draw_varint`, `draw_fixed` and
        // `draw_tag` instead (spec 0279 S3).
        _ => {
            painter.bytes(range, None);
            return;
        }
    };
    let tier = painter.accuse(
        Region::Payload,
        match flaw {
            SchemaFlaw::Utf8 => "INVALID_STRING",
            _ => "INVALID_PACKED_RECORDS",
        },
    );
    let mut at = 0;
    for span in bad {
        painter.bytes(range.start + at..range.start + span.start, None);
        painter.bytes(range.start + span.start..range.start + span.end, tier);
        at = span.end;
    }
    painter.bytes(range.start + at..range.end, None);
    if open {
        // Spec 0279 S4: the same failure as a bare `INVALID_VARINT`, so
        // the same rendering — the byte whose continuation bit was not
        // honored, then the slot that byte promised.
        painter.cut();
    }
}

/// A whole, present fixed-width payload (spec 0279 S3).
///
/// `nan` bands the bytes that differ from the NaN a re-encode would
/// write — `f64::NAN` for eight, `f32::NAN` for four — which is exactly
/// "what would change", said in the one place it can be seen. Plain
/// everywhere else, which is every payload the row above said nothing
/// about.
///
/// Neither constant is schema knowledge (spec 0279 N1): *that* these
/// bytes are a float is what the `nan_bits` keyword carries, and which
/// float is the payload's own width.
fn draw_fixed(painter: &mut Painter, range: Range<usize>, nan: Option<Band>) {
    let Some(band) = nan else {
        painter.bytes(range, None);
        return;
    };
    let eight = f64::NAN.to_bits().to_le_bytes();
    let four = f32::NAN.to_bits().to_le_bytes();
    let canonical: &[u8] = match range.len() {
        8 => &eight,
        4 => &four,
        _ => &[],
    };
    let end = range.end.min(painter.rec.len());
    for i in range.start..end {
        let same = canonical
            .get(i - range.start)
            .is_some_and(|&b| b == painter.rec[i]);
        painter.byte(i, (!same).then_some(band));
    }
}

/// The sub-ranges of `payload` that make it invalid UTF-8 — every
/// ill-formed sequence, not only the first, since a reader looking for
/// the bad byte wants all of them.
fn utf8_flaws(payload: &[u8]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < payload.len() {
        let Err(e) = std::str::from_utf8(&payload[at..]) else {
            break;
        };
        let start = at + e.valid_up_to();
        // `error_len` is `None` for a sequence that is merely
        // *unfinished* — the payload ended in the middle of one — and
        // there is nothing after it to draw.
        let end = e.error_len().map_or(payload.len(), |n| start + n);
        out.push(start..end);
        at = end;
    }
    out
}

/// The bytes of `payload` that do not complete a packed element, and
/// whether the payload ends inside a varint.
///
/// The row has no schema, so it reads the payload as the one packed
/// encoding that delimits itself: a stream of varints. A varint left
/// open at the end is the case the document reports, and spec 0279 S4
/// names its **last** byte: the ones before it decoded, and what is
/// wrong is a continuation bit promising a byte the payload does not
/// have. The caller draws the `??` that stands where that byte would.
///
/// When every varint does close, the record is a fixed-width one whose
/// length is not a multiple of its element size — and the size is
/// precisely what this row cannot know, four and eight being equally
/// consistent with the bytes. The whole payload is named, which is the
/// most that is true, and nothing is missing from it.
fn packed_flaw(payload: &[u8]) -> (Range<usize>, bool) {
    let mut at = 0;
    while at < payload.len() {
        let parsed = parse_varint(payload, at);
        if parsed.varint_gar.is_some() {
            return (payload.len() - 1..payload.len(), true);
        }
        at = parsed.next_pos;
    }
    (0..payload.len(), false)
}

/// A length or value varint at `at`, with its overlong padding in
/// yellow and its truncation closed by `??`.
///
/// `whole` bands every byte the varint really has, for the accusations
/// that are about the value rather than about one of its bytes (spec
/// 0279 S3). Its padding still takes `overhang_flag`'s tier instead: a
/// second fact about a specific run of bytes outranks a fact about all
/// of them.
fn draw_varint(
    painter: &mut Painter,
    at: usize,
    overhang_flag: &'static str,
    invalid_flag: &'static str,
    whole: Option<Band>,
) -> (Option<u64>, usize) {
    // Both accusations are about the varint being drawn, which is the
    // length prefix or the value depending on who called — so the part
    // is the pen's own here, unlike `draw_tag`'s.
    let region = painter.region;
    if at >= painter.rec.len() {
        painter.accuse(region, invalid_flag);
        painter.cut();
        return (None, painter.rec.len());
    }
    let parsed = parse_varint(painter.rec, at);
    if parsed.varint_gar.is_some() {
        // Spec 0279 S4: the bytes before the last one decoded. What is
        // wrong is the continuation bit of the last, which promises a
        // byte the record does not have — so that byte is banded and
        // the `??` stands where the byte it promised would be.
        let tier = painter.accuse(region, invalid_flag);
        let end = painter.rec.len();
        let last = end.saturating_sub(1).max(at);
        painter.bytes(at..last, None);
        painter.bytes(last..end, tier);
        painter.cut();
        return (None, end);
    }
    let end = parsed.next_pos;
    let overhang = parsed.varint_ohb.unwrap_or(0) as usize;
    painter.bytes(at..end - overhang, whole);
    if overhang > 0 {
        let tier = painter.accuse(region, overhang_flag);
        painter.bytes(end - overhang..end, tier);
    }
    (parsed.varint, end)
}

/// Accumulates the row. Offsets are relative to the row's first byte;
/// `rec` runs to the record's end, `limit` to the row's, and the two
/// differ only on a packed run's element-0 row.
struct Painter<'a> {
    rec: &'a [u8],
    /// Where `rec` begins in the blob, so a recorded byte range is one
    /// the caller can index the blob with (spec 0282 S3).
    base: usize,
    limit: usize,
    theme: ThemeKind,
    /// The hues borrowed from the document row (spec 0225 S11), or `None`
    /// when it has none of its own — in which case the whole row is
    /// subdued, tiers included (S7).
    palette: Option<&'a WirePalette>,
    /// Which of the four the pen is currently in.
    region: Region,
    /// Spec 0307 S2: the row's [`WireSlice::synthetic_end`], as an
    /// offset into `rec` rather than into the blob — `0` on every row
    /// but the wrapper root's, where `tint` is then a comparison
    /// against zero that never fires.
    synthetic: usize,
    /// The background block the byte just drawn wore. What tells
    /// `separate` whether the gap it is about to draw is inside one
    /// block or between two.
    ///
    /// One slot, because a tag's first byte — the only one drawn in two
    /// halves — ends on its field-number half, which is the half a
    /// multi-byte tag's remaining bytes continue.
    last_block: Option<Style>,
    spans: Vec<Span<'static>>,
    /// `Some` only when someone asked what is drawn where.
    record: Option<WireRecord>,
    /// How many columns the row has emitted, kept as the spans are
    /// pushed rather than measured afterwards — the recorder needs a
    /// column range per drawing call, and summing the spans at each one
    /// would be quadratic in the row's length.
    cols: usize,
    drawn: usize,
    dropped: usize,
    need_space: bool,
}

impl Painter<'_> {
    /// The whole row's hex is one muted color and every hue it carries
    /// is a background (spec 0225 S11): hex is dense and uniform, and a
    /// color one glyph-pair wide is a much weaker signal than the same
    /// color as a filled band.
    ///
    /// The background says two things, in that order of precedence.
    /// Absent a [`Band`] it says what the byte is *for*, in the hue its
    /// region borrowed from the document row above. A band says the
    /// byte is not ordinary, and takes the background over at full
    /// strength: what a byte is for stops mattering once it is
    /// malformed — or once nothing knows what it is for — and a band
    /// swap is the loudest thing the row can do without changing shape.
    ///
    /// With no palette the whole row is subdued, bands included (S7):
    /// the `#@` tokens the tiers echo have gone gray too, and one row
    /// cannot claim a severity the other has stopped showing. Bytes no
    /// framing claimed keep the same subdued text, since a band would
    /// be a claim about them.
    fn style(&self, band: Option<Band>) -> Style {
        self.style_in(self.region, band)
    }

    fn style_in(&self, region: Region, band: Option<Band>) -> Style {
        let Some(palette) = self.palette else {
            return subdued();
        };
        match band {
            Some(Band::Tier(tier)) => theme::tier_band(tier, self.theme),
            Some(Band::Unknown) => theme::unknown_band(self.theme),
            None => match region {
                Region::Tag => palette.tag,
                Region::Type => palette.ty,
                Region::Len => palette.len,
                Region::Payload => palette.payload,
                Region::Unclaimed => subdued(),
            },
        }
    }

    /// Spec 0307 S2: `style`, with byte `i`'s provenance on it.
    ///
    /// Neither color is touched — a fabricated tag is still a tag and
    /// still wears `Region::Tag`'s hue, which is what makes it readable
    /// as one, and the hex keeps the one foreground that stays legible
    /// over every band.
    fn tint(&self, style: Style, i: usize) -> Style {
        if i < self.synthetic {
            return style.add_modifier(theme::SYNTHETIC);
        }
        style
    }

    /// Every span the row emits goes through here, so `cols` is always
    /// the width of what has been drawn so far.
    fn emit(&mut self, span: Span<'static>) {
        self.cols += span.content.chars().count();
        self.spans.push(span);
    }

    /// Note that the columns from `from` to here drew `part`
    /// (spec 0282 S3).
    ///
    /// Consecutive, adjacent spans of the same part coalesce, so a
    /// payload run is one entry and a multi-byte tag's continuation
    /// bytes join the field number they continue. Punctuation records
    /// nothing and so breaks the adjacency, which is what keeps a `[`
    /// from being swallowed into the payload behind it (N1).
    fn mark(&mut self, from: usize, part: WirePart, bytes: Range<usize>) {
        let cols = from..self.cols;
        let Some(record) = &mut self.record else {
            return;
        };
        if let Some(last) = record.parts.last_mut() {
            if last.part == part && last.cols.end == cols.start {
                last.cols.end = cols.end;
                last.bytes.end = last.bytes.end.max(bytes.end);
                return;
            }
        }
        record.parts.push(PartSpan { cols, part, bytes });
    }

    /// Note that the columns from `from` to here drew the byte at `at`
    /// (spec 0283 S1). Never coalesced: that is the whole difference
    /// between this list and `mark`'s.
    fn cell(&mut self, from: usize, at: usize) {
        let cols = from..self.cols;
        if let Some(record) = &mut self.record {
            record.cells.push(ByteCell { cols, at });
        }
    }

    /// A space between two hex pairs.
    ///
    /// Filled if it lies *inside* one band — the byte before it wore
    /// the same background and so does the byte about to be drawn — and
    /// plain otherwise. Bytes saying one thing are one fact, and a
    /// region broken every third column reads as several.
    fn separate(&mut self, style: Style) {
        if self.need_space {
            let here = block(style);
            let fill = match here {
                Some(band) if self.last_block == here => band,
                _ => Style::default(),
            };
            self.emit(Span::styled(" ", fill));
        }
        self.need_space = true;
    }

    fn byte(&mut self, i: usize, band: Option<Band>) {
        if i >= self.limit || i >= self.rec.len() {
            return;
        }
        if self.drawn >= WIRE_ROW_MAX_BYTES {
            self.dropped += 1;
            return;
        }
        // From before the separator, not after it: a space inside a run
        // is part of the run, so including it keeps a recorded part's
        // columns contiguous and lets the coalescing in `mark` do its
        // job.
        let from = self.cols;
        let style = self.tint(self.style(band), i);
        self.separate(style);
        self.emit(Span::styled(format!("{:02x}", self.rec[i]), style));
        self.last_block = block(style);
        self.drawn += 1;
        self.mark(
            from,
            WirePart::Region(self.region),
            self.base + i..self.base + i + 1,
        );
        self.cell(from, self.base + i);
    }

    fn bytes(&mut self, range: Range<usize>, band: Option<Band>) {
        let end = range.end.min(self.limit).min(self.rec.len());
        let mut i = range.start;
        while i < end {
            if self.drawn >= WIRE_ROW_MAX_BYTES {
                // Counted in one step rather than one byte at a time: a
                // LEN payload can be megabytes and this runs per frame.
                self.dropped += end - i;
                return;
            }
            self.byte(i, band);
            i += 1;
        }
    }

    /// The first tag byte, drawn as `t|nn` — the wire type as the one
    /// decimal digit it is, then the same byte with those three bits
    /// cleared.
    ///
    /// Not a nibble split, which is what this used to be: the wire type
    /// is the low *three* bits, so a low nibble is one bit of field
    /// number too many and a colored wire type accused a bit it should
    /// not. Spelling the type out separately costs two columns on one
    /// byte of the row and is exact. The remainder is still shown as a
    /// full hex pair, and still reads as part of the run beside it, so
    /// nothing about the byte is hidden: `2|08` and `0a` say the same
    /// thing and the reader can add the digit back.
    ///
    /// The two halves are two different facts — the field number and the
    /// wire type — so they take two different borrowed hues, not only
    /// two different bands. This is the only byte on the row that is
    /// ever split, and it is always a tag's first.
    fn tag_head(&mut self, i: usize, number: Option<Band>, wtype: Option<Band>) {
        if i >= self.limit || i >= self.rec.len() || self.drawn >= WIRE_ROW_MAX_BYTES {
            return;
        }
        let byte = self.rec[i];
        let number_style = self.tint(self.style_in(Region::Tag, number), i);
        let wtype_style = self.tint(self.style_in(Region::Type, wtype), i);
        // One byte, so one cell (spec 0283 S1) — the two halves and the
        // bar between them are all of it.
        let whole = self.cols;
        let from = self.cols;
        self.separate(wtype_style);
        self.emit(Span::styled(format!("{}", byte & 0x07), wtype_style));
        // The two halves are the row's two smallest targets and the
        // only place one byte is two parts (spec 0282 S3). The bar
        // between them is recorded as neither: it is punctuation, and
        // pointing at it points at nothing (N1).
        self.mark(
            from,
            WirePart::Region(Region::Type),
            self.base + i..self.base + i + 1,
        );
        // The bar joins the two halves when they wear the same band and
        // parts them when they do not — the same rule `separate` applies
        // to the gap between two hex pairs, for the same reason.
        let bar = match (block(wtype_style), block(number_style)) {
            (Some(band), Some(other)) if band == other => band,
            _ => Style::default(),
        };
        // Spec 0307 S3: the bar is part of the byte, so it says what
        // the byte's two halves say about where the byte came from.
        self.emit(Span::styled("|", self.tint(bar, i)));
        let from = self.cols;
        self.emit(Span::styled(format!("{:02x}", byte & 0xF8), number_style));
        self.last_block = block(number_style);
        self.drawn += 1;
        self.mark(
            from,
            WirePart::Region(Region::Tag),
            self.base + i..self.base + i + 1,
        );
        self.cell(whole, self.base + i);
    }

    fn punct(&mut self, text: &'static str) {
        if self.drawn >= WIRE_ROW_MAX_BYTES {
            return;
        }
        self.emit(Span::styled(text, subdued()));
        self.need_space = false;
    }

    /// The empty byte slot that says the bytes ran out. It replaces
    /// whatever would have come next, so *where* it sits is what
    /// distinguishes a truncated payload from a truncated varint and a
    /// second glyph would be redundant.
    ///
    /// Two columns wide and spaced like a hex pair, because that is what
    /// it stands in for: the row is a hex dump, `??` is what a hex dump
    /// writes for a byte that is not there, and a mark occupying the
    /// slot reads as an absence where a one-column `!` glued to the
    /// previous pair read as an accusation against *it*.
    ///
    /// Colored like the bytes it stands in for: this is a defect of the
    /// message, not of the display, and the row flags every one of those
    /// the same way.
    fn cut(&mut self) {
        let from = self.cols;
        let style = self.alarm();
        self.separate(style);
        self.emit(Span::styled("??".to_string(), style));
        self.need_space = false;
        // An empty byte range, because that is exactly what it stands
        // for: the mark is where bytes are not (spec 0282 S11).
        let at = self.base + self.rec.len();
        self.mark(
            from,
            WirePart::Truncated {
                region: self.region,
                missing: None,
            },
            at..at,
        );
    }

    /// How many bytes the record promised and did not deliver, glued to
    /// the `??` that says it ran out: `??×4` (spec 0225 S11). The count is
    /// the one fact the row cannot show, since the bytes it counts are
    /// precisely the ones absent from it — and `×N` reads as "N of
    /// these", the same as it does after the elision.
    ///
    /// Alarmed with the `!`, and contiguous with it: the glyph and its
    /// count are one accusation.
    fn missing(&mut self, n: usize) {
        self.emit(Span::styled(format!("×{n}"), self.alarm()));
        self.need_space = false;
        // Glued to the `??` on screen and glued to it here: one
        // accusation, so one target, and the count is the fact the box
        // wants (spec 0282 S11).
        let cols = self.cols;
        if let Some(record) = &mut self.record {
            if let Some(last) = record.parts.last_mut() {
                if let WirePart::Truncated { missing, .. } = &mut last.part {
                    *missing = Some(n);
                    last.cols.end = cols;
                }
            }
        }
    }

    /// The style the row flags a defect of the *message* with: exactly
    /// what an invalid byte would wear here, since that is what the
    /// glyph stands in for — the `Invalid` band, and subdued with the
    /// rest of the row when there is no palette to carry one (S7).
    fn alarm(&self) -> Style {
        self.style(Some(Band::Tier(Tier::Invalid)))
    }

    /// Name the annotation keyword prototext-core would have emitted for
    /// what was just found, and get back the tier it carries.
    ///
    /// Spec 0225 S11 "one classifier, two rows": this module never
    /// decides a severity of its own. It reports the keyword, and
    /// `annotation::tier_of` — the same table `highlights.scm` mirrors
    /// for the `#@` row — decides. The keyword is kept so a test can
    /// cross-check this row's findings against the annotation above it,
    /// and so the hover box can read it aloud (spec 0282 S13).
    ///
    /// `region` is the part the accusation is *about*, and is stated
    /// rather than read off `self.region`: a `TYPE_MISMATCH` is found
    /// while the pen is in the tag but is about the type digit beside
    /// it. Spec 0279 already pairs a flaw with its bytes at the call
    /// site, for the same reason — the site is what knows.
    fn accuse(&mut self, region: Region, keyword: &'static str) -> Option<Band> {
        if let Some(record) = &mut self.record {
            record.flags.push((region, keyword));
        }
        annotation::tier_of(keyword).map(Band::from)
    }

    /// The tier for `keyword` when the document row above accused
    /// exactly `flaw`, and `None` on every other row (spec 0279 S3).
    ///
    /// The pairing of flaw and keyword is spelled at each call site
    /// rather than in a table, because the site is where the *bytes*
    /// the keyword is about are known — which is the whole of what this
    /// module contributes.
    fn told(&mut self, region: Region, flaw: SchemaFlaw, keyword: &'static str) -> Option<Band> {
        if self.palette.and_then(|p| p.flaw) != Some(flaw) {
            return None;
        }
        self.accuse(region, keyword)
    }

    /// [`Band::Unknown`] on a row the document half said no schema
    /// declares, and `None` on every other row.
    ///
    /// The one band that does not go through [`Painter::accuse`],
    /// because there is no keyword to accuse with: prototext-core emits
    /// none for an undeclared field — a numeric key is the whole of what
    /// it says — and `tier_of` would have nothing to answer.
    fn undeclared(&self) -> Option<Band> {
        (self.palette.and_then(|p| p.flaw) == Some(SchemaFlaw::Undeclared)).then_some(Band::Unknown)
    }

    /// Closes the row, adding `…×N` if the byte budget cut it short.
    ///
    /// Subdued, never alarmed: the row ran out of columns, the message
    /// did not run out of bytes. The tier colors are reserved for
    /// defects of the message, so that a screen full of `…` never reads
    /// as a screen full of alarms.
    fn finish(mut self) -> WireRow {
        if self.dropped > 0 {
            let from = self.cols;
            self.emit(Span::styled(format!("…×{}", self.dropped), subdued()));
            let end = self.base + self.rec.len();
            self.mark(
                from,
                WirePart::Elided {
                    hidden: self.dropped,
                },
                end..end,
            );
        }
        WireRow {
            spans: self.spans,
            record: self.record,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(blob: &[u8], framing: Framing, field_number: u32) -> WireRow {
        draw_of(blob, 0..blob.len(), blob.len(), framing, field_number)
    }

    fn draw_of(
        blob: &[u8],
        bytes: Range<usize>,
        record_end: usize,
        framing: Framing,
        field_number: u32,
    ) -> WireRow {
        // Recorded, because that is what carries `flags` — and because a
        // test that asserts on the parts is asserting on the same run
        // the styling came out of.
        wire_spans_recorded(
            blob,
            &WireSlice {
                bytes,
                record_end,
                framing,
                field_number,
                synthetic_end: 0,
            },
            ThemeKind::Dark,
            Some(&WirePalette::for_test()),
        )
    }

    /// The hue `WirePalette::for_test` lends each region, so a test can
    /// say *which* of the four a byte was painted from.
    const TAG_HUE: Color = Color::Rgb(1, 0, 0);
    const TYPE_HUE: Color = Color::Rgb(1, 1, 0);
    const LEN_HUE: Color = Color::Rgb(0, 1, 0);
    const PAYLOAD_HUE: Color = Color::Rgb(0, 0, 1);

    /// The band a tier takes over with — what an accused byte's
    /// background is, instead of the hue its region borrowed.
    fn tier_hue(tier: Tier) -> Color {
        band_hue(Band::Tier(tier))
    }

    fn band_hue(band: Band) -> Color {
        match band {
            Band::Tier(tier) => theme::tier_band(tier, ThemeKind::Dark),
            Band::Unknown => theme::unknown_band(ThemeKind::Dark),
        }
        .bg
        .expect("a band is a background")
    }

    fn text(row: &WireRow) -> String {
        row.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// A `^` under every column `band` claimed, aligned with `text`.
    ///
    /// Reads the *background*, which is where a band goes and where
    /// `separate`'s run-joining is decided.
    fn accused(row: &WireRow, band: impl Into<Band>) -> String {
        let hue = band_hue(band.into());
        row.spans
            .iter()
            .flat_map(|s| {
                let mark = if s.style.bg == Some(hue) { '^' } else { ' ' };
                std::iter::repeat_n(mark, s.content.chars().count())
            })
            .collect()
    }

    #[test]
    fn the_punctuation_reads_without_color() {
        // field 1, LEN, five bytes — "HELLO".
        let blob = [0x0A, 0x05, 0x48, 0x45, 0x4C, 0x4C, 0x4F];
        let row = draw(&blob, Framing::Tagged, 1);
        assert_eq!(text(&row), "2|08:05[48 45 4c 4c 4f]");
        // Spec 0225 S11: the framing carries no hue of its own. Only the
        // bytes it frames do.
        for span in &row.spans {
            if matches!(span.content.as_ref(), ":" | "[" | "]") {
                assert_eq!(span.style.fg, None, "{}", span.content);
                assert_eq!(span.style.bg, None, "{}", span.content);
            }
        }
        // field 2, varint, 42.
        assert_eq!(text(&draw(&[0x10, 0x2A], Framing::Tagged, 2)), "0|10[2a]");
        // field 1, fixed32.
        assert_eq!(
            text(&draw(&[0x0D, 0x00, 0x00, 0x80, 0x3F], Framing::Tagged, 1)),
            "5|08[00 00 80 3f]"
        );
        // field 3, group start — all a header row has.
        assert_eq!(text(&draw(&[0x1B], Framing::Tagged, 3)), "3|18");
    }

    /// Spec 0225 S11: the hex is worn as a background, not as text, and
    /// a region is one ribbon rather than a row of tiles.
    ///
    /// The punctuation is what breaks it: the framing has no hue, so a
    /// payload reads as one block from `[` to `]` while the tag and the
    /// length before it read as their own.
    #[test]
    fn a_region_is_one_unbroken_ribbon() {
        // field 1, LEN, five bytes — "HELLO".
        let row = draw(
            &[0x0A, 0x05, 0x48, 0x45, 0x4C, 0x4C, 0x4F],
            Framing::Tagged,
            1,
        );
        assert_eq!(text(&row), "2|08:05[48 45 4c 4c 4f]");
        let filled: String = row
            .spans
            .iter()
            .flat_map(|s| {
                let mark = if s.style.bg.is_some() { '#' } else { ' ' };
                std::iter::repeat_n(mark, s.content.chars().count())
            })
            .collect();
        // The `|` between the wire type and the field number is one of
        // the two blocks' own only when they agree, and here they do not.
        //                  2|08: 05[48 45 4c 4c 4f]
        assert_eq!(filled, "# ## ## ############## ");
        // Every payload byte and every gap between two of them is the
        // one hue, so the ribbon is unbroken rather than merely dense.
        for span in &row.spans {
            let inside_payload = matches!(span.content.as_ref(), "48" | "45" | "4c" | "4f" | " ");
            if inside_payload {
                assert_eq!(span.style.bg, Some(PAYLOAD_HUE), "{:?}", span.content);
            }
        }
    }

    /// Spec 0225 S11: each of the four parts wears the hue it borrowed,
    /// and they are four, not one. The wire type is one of the four: it
    /// names a different fact than the field number beside it.
    #[test]
    fn the_tag_the_type_the_length_and_the_payload_wear_four_hues() {
        // field 1, LEN, two bytes.
        let row = draw(&[0x0A, 0x02, 0x48, 0x49], Framing::Tagged, 1);
        let hue = |c: &str| {
            row.spans
                .iter()
                .find(|s| s.content.as_ref() == c)
                .unwrap_or_else(|| panic!("{c} is on the row: {}", text(&row)))
                .style
                .bg
        };
        assert_eq!(hue("08"), Some(TAG_HUE));
        assert_eq!(hue("2"), Some(TYPE_HUE));
        assert_eq!(hue("02"), Some(LEN_HUE));
        assert_eq!(hue("48"), Some(PAYLOAD_HUE));
        assert_eq!(hue("49"), Some(PAYLOAD_HUE));
    }

    #[test]
    fn a_truncation_counts_the_bytes_it_cannot_show() {
        // Declared five payload bytes, one present. Spec 0225 S11: the
        // count of what is absent, glued to the `??` that says so — and
        // the `??` itself spaced off `48`, which is a byte that arrived.
        let row = draw(&[0x0A, 0x05, 0x48], Framing::Tagged, 1);
        assert_eq!(text(&row), "2|08:05[48 ??×4");
        assert_eq!(row.flags(), ["TRUNCATED_BYTES"]);
        // Both glyphs wear the `Invalid` tier, and they are one
        // accusation: the bytes are missing from the *message*, not from
        // the row.
        for span in row.spans.iter().filter(|s| {
            let c = s.content.as_ref();
            c == "??" || c == "×4"
        }) {
            assert_eq!(
                span.style.bg,
                Some(tier_hue(Tier::Invalid)),
                "{}",
                span.content
            );
        }
        // A tag varint that never terminates. Not `FF FF FF`, which is
        // wire type 7 and so fails earlier, on the type bits.
        let row = draw(&[0x82, 0x80, 0x80], Framing::Tagged, 1);
        assert_eq!(text(&row), "82 80 80 ??");
        assert_eq!(row.flags(), ["INVALID_VARINT"]);
    }

    #[test]
    fn the_wire_type_is_styled_apart_from_the_field_number() {
        // field 1, wire type 7.
        let row = draw(&[0x0F], Framing::Tagged, 1);
        assert_eq!(text(&row), "7|08");
        assert_eq!(row.flags(), ["INVALID_TAG_TYPE"]);
        // The field number is fine and keeps its borrowed hue; the wire
        // type is not, and the accusation takes its band over — saying
        // what a byte is for stops mattering once it is malformed
        // (spec 0225 S11).
        assert_eq!(row.spans[0].content.as_ref(), "7");
        assert_eq!(row.spans[0].style.bg, Some(tier_hue(Tier::Invalid)));
        assert_eq!(row.spans[2].content.as_ref(), "08");
        assert_eq!(row.spans[2].style.bg, Some(TAG_HUE));
    }

    /// A group end where no group was opened. The document row above
    /// says `INVALID_GROUP_END` over a bytes-valued pseudo-field, and
    /// the wire row has to accuse the same thing — including the bytes
    /// that pseudo-field swallowed, which are on the row only because
    /// the stray tag was taken.
    #[test]
    fn a_group_end_closing_nothing_accuses_its_whole_row() {
        // Field 106, END_GROUP — a two-byte tag — then the two bytes
        // prototext-core renders as the pseudo-field's value.
        let row = draw(&[0xD4, 0x06, 0x08, 0x01], Framing::Tagged, 106);
        assert_eq!(text(&row), "4|d0 06 08 01");
        assert_eq!(row.flags(), ["INVALID_GROUP_END"]);
        // Everything but the wire type: it reports a group end and
        // there is one. What is wrong is that the tag is here.
        let invalid = tier_hue(Tier::Invalid);
        let marked: String = row
            .spans
            .iter()
            .flat_map(|s| {
                let mark = if s.style.bg == Some(invalid) {
                    '^'
                } else {
                    ' '
                };
                std::iter::repeat_n(mark, s.content.chars().count())
            })
            .collect();
        //                  4|d0 06 08 01
        assert_eq!(marked, "  ^^^^^^^^^^^");
        assert_eq!(row.spans[0].content.as_ref(), "4");
        assert_eq!(row.spans[0].style.bg, Some(TYPE_HUE));
    }

    /// Spec 0225 S11: the missing END tag is a missing byte, and `??` is
    /// already what this row says about one. Nothing precedes it here,
    /// so the slot opens the row rather than being spaced off anything.
    #[test]
    fn an_open_group_footer_row_is_a_bare_empty_slot() {
        let row = draw_of(&[], 0..0, 0, Framing::Closing, 3);
        assert_eq!(text(&row), "??");
        assert_eq!(row.flags(), ["OPEN_GROUP"]);
    }

    #[test]
    fn a_group_footer_names_the_group_it_closes() {
        // field 3, END_GROUP.
        let row = draw(&[0x1C], Framing::Closing, 3);
        assert_eq!(text(&row), "4|18");
        assert!(row.flags().is_empty());
        // The same bytes closing field 2 instead. Spec 0225 S11: the
        // wrong number is *shown* wrong rather than restated in words,
        // so the row is unchanged and the field number reddens.
        let row = draw(&[0x1C], Framing::Closing, 2);
        assert_eq!(text(&row), "4|18");
        assert_eq!(row.flags(), ["END_MISMATCH"]);
        assert_eq!(row.spans[2].content.as_ref(), "18");
        assert_eq!(row.spans[2].style.bg, Some(tier_hue(Tier::Invalid)));
        // The wire type is not what is wrong — it says END_GROUP, and
        // it does.
        assert_eq!(row.spans[0].content.as_ref(), "4");
        assert_eq!(row.spans[0].style.bg, Some(TYPE_HUE));
    }

    #[test]
    fn an_overlong_varint_shows_its_trailing_padding() {
        // field 2, varint, value 42 written in three bytes.
        let row = draw(&[0x10, 0xAA, 0x80, 0x00], Framing::Tagged, 2);
        assert_eq!(text(&row), "0|10[aa 80 00]");
        assert_eq!(row.flags(), ["val_ohb"]);
        let yellow = tier_hue(Tier::NonCanonical);
        // The padding bytes are accused, and only they: `aa` is a
        // perfectly good first byte. The gap between the two joins
        // them, so the accusation is one band and not two.
        let colored: Vec<&str> = row
            .spans
            .iter()
            .filter(|s| s.style.bg == Some(yellow))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(colored, ["80", " ", "00"]);
        // The gap before the run is not part of it, and not part of the
        // payload's ribbon either: it straddles two bands, and a gap
        // between two bands belongs to neither.
        assert_eq!(
            row.spans
                .iter()
                .filter(|s| s.content.as_ref() == " ")
                .map(|s| s.style.bg)
                .collect::<Vec<_>>(),
            [None, Some(yellow)],
        );
    }

    #[test]
    fn an_out_of_range_field_number_spares_the_wire_type() {
        // Field number 0, wire type 0 — `TAG_OOR`, and the type bits are
        // perfectly good.
        let row = draw(&[0x00, 0x01], Framing::Tagged, 0);
        assert_eq!(text(&row), "0|00[01]");
        assert_eq!(row.flags(), ["TAG_OOR"]);
        assert_eq!(row.spans[2].content.as_ref(), "00");
        assert_eq!(row.spans[2].style.bg, Some(tier_hue(Tier::Invalid)));
        assert_eq!(row.spans[0].content.as_ref(), "0");
        assert_eq!(row.spans[0].style.bg, Some(TYPE_HUE));
    }

    /// Spec 0267 S5. `pack_size` accuses nothing, so a packed record's
    /// head row is drawn from the four region hues like every other
    /// record's — no foreground accent anywhere on it.
    #[test]
    fn a_packed_record_is_drawn_like_any_other() {
        // field 1, LEN, two varint elements: 01 02.
        let blob = [0x0A, 0x02, 0x01, 0x02];
        let head = draw_of(
            &blob,
            0..3,
            4,
            Framing::PackedHead {
                varint: true,
                close: false,
            },
            1,
        );
        assert_eq!(text(&head), "2|08:02[01");
        // The row still names what it found; `tier_of` answers `None`.
        assert_eq!(head.flags(), ["pack_size"]);
        assert_eq!(head.spans[0].style.bg, Some(TYPE_HUE), "the wire type");
        assert_eq!(head.spans[2].style.bg, Some(TAG_HUE), "the field number");
        assert_eq!(head.spans[4].content.as_ref(), "02");
        assert_eq!(head.spans[4].style.bg, Some(LEN_HUE), "the length prefix");
        // The hex is one color throughout: every hue this row carries is
        // a band, which is what leaves a tier free to mean "wrong".
        let plain = subdued().fg;
        for span in &head.spans {
            assert_eq!(span.style.fg, plain, "{}", span.content);
        }
        // The element bytes are payload, and payload is all they are.
        let last = draw_of(
            &blob,
            3..4,
            4,
            Framing::Element {
                varint: true,
                close: true,
            },
            1,
        );
        assert_eq!(text(&last), "02]");
        assert_eq!(
            last.spans
                .iter()
                .find(|s| s.content.as_ref() == "02")
                .map(|s| (s.style.fg, s.style.bg)),
            Some((None, Some(PAYLOAD_HUE))),
            "an element is payload, and payload is all it is",
        );
    }

    #[test]
    fn a_header_rows_payload_is_left_to_its_children() {
        // field 1, LEN, three payload bytes the row does not own —
        // the message-header case. No `]`: nothing has ended.
        let blob = [0x0A, 0x03, 0x08, 0x01, 0x00];
        let row = draw_of(&blob, 0..2, 5, Framing::Tagged, 1);
        assert_eq!(text(&row), "2|08:03[");
        assert!(row.flags().is_empty());
    }

    #[test]
    fn unclaimed_trailing_bytes_are_drawn_plain() {
        let row = draw_of(&[0xAA, 0xBB], 0..2, 2, Framing::Raw, 1);
        assert_eq!(text(&row), "aa bb");
        // No framing claimed them, so they belong to none of the three
        // regions and borrow nothing (spec 0225 S11).
        assert!(row.spans.iter().all(|s| s.style.bg.is_none()));
    }

    /// Spec 0225 S7: with no palette the row is subdued end to end —
    /// the tiers included, because the `#@` tokens they echo have gone
    /// gray too.
    #[test]
    fn a_row_with_nothing_to_borrow_is_gray_throughout() {
        // The `TAG_OOR` case above, which is as colorful as a row gets.
        let row = wire_spans_recorded(
            &[0x00, 0x01],
            &WireSlice {
                bytes: 0..2,
                record_end: 2,
                framing: Framing::Tagged,
                field_number: 0,
                synthetic_end: 0,
            },
            ThemeKind::Dark,
            None,
        );
        assert_eq!(text(&row), "0|00[01]");
        assert_eq!(row.flags(), ["TAG_OOR"]);
        assert!(row
            .spans
            .iter()
            .all(|s| s.style.fg.is_none() && s.style.bg.is_none()));
    }

    #[test]
    fn a_long_payload_is_elided_rather_than_wrapped() {
        let mut blob = vec![0x0A, 0x80, 0x01];
        blob.extend_from_slice(&[0x41; 128]);
        let row = draw_of(&blob, 0..blob.len(), blob.len(), Framing::Tagged, 1);
        let drawn = text(&row);
        // The tag byte and the two length bytes count against the cap
        // too, so 61 of the 128 payload bytes are drawn.
        assert!(drawn.ends_with("…×67"), "{drawn}");
        assert_eq!(drawn.matches("41").count(), WIRE_ROW_MAX_BYTES - 3);
        // The row ran out of columns; the message did not run out of
        // bytes. Nothing here is alarmed (spec 0225 S11).
        let marker = row.spans.last().expect("the row ends with the elision");
        assert_eq!(marker.style.fg, None);
        assert_eq!(marker.style.bg, None);
    }

    /// The two cases spec 0225 S11 draws.
    ///
    /// An accusation is one unbroken block: the bytes it names and the
    /// gaps between them, so that a run of bad bytes reads as one fact
    /// rather than as several.
    #[test]
    fn an_accusation_marks_the_bytes_it_names() {
        // field 2, varint, a value written in four bytes: the last two
        // add nothing, and the run starts in the middle of the row.
        let row = draw(&[0x10, 0xAC, 0x86, 0x80, 0x00], Framing::Tagged, 2);
        assert_eq!(text(&row), "0|10[ac 86 80 00]");
        //                                            0|10[ac 86 80 00]
        assert_eq!(accused(&row, Tier::NonCanonical), "           ^^^^^ ");
        // Field number 0 over a five-byte tag: the accusation opens on
        // the field-number half of the first byte and runs unbroken to
        // the end of the tag. The wire type is spelled ahead of it and
        // is spared, and so is the bar between them — the two halves
        // wear different bands, and a bar that joined them would say
        // they did not.
        let row = draw(&[0x80, 0x80, 0x80, 0x80, 0x10, 0x01], Framing::Tagged, 0);
        assert_eq!(text(&row), "0|80 80 80 80 10[01]");
        //                                            0|80 80 80 80 10[01]
        assert_eq!(accused(&row, Tier::Invalid), "  ^^^^^^^^^^^^^^    ");
    }

    /// A payload that ran out is an invalid payload, and the bytes that
    /// did arrive are the ones the reader has to look at: they wear the
    /// band, not only the `!` at their end.
    #[test]
    fn a_payload_that_ran_out_wears_the_band_it_earned() {
        // field 5, fixed64, three of the eight bytes present.
        let row = draw(&[0x29, 0x01, 0x02, 0x03], Framing::Tagged, 5);
        assert_eq!(text(&row), "1|28[01 02 03 ??");
        // The space before the empty slot is inside the band too: the
        // bytes that came and the slot that did not are one accusation.
        //                                       1|28[01 02 03 ??
        assert_eq!(accused(&row, Tier::Invalid), "     ^^^^^^^^^^^");
        // field 5, fixed32, one of the four.
        let row = draw(&[0x2D, 0x01], Framing::Tagged, 5);
        assert_eq!(text(&row), "5|28[01 ??");
        assert_eq!(accused(&row, Tier::Invalid), "     ^^^^^");
        // field 1, LEN, five bytes promised and two delivered.
        let row = draw(&[0x0A, 0x05, 0x48, 0x45], Framing::Tagged, 1);
        assert_eq!(text(&row), "2|08:05[48 45 ??×3");
        assert_eq!(accused(&row, Tier::Invalid), "        ^^^^^^^^^^");
    }

    /// A blob drawn with the document row's accusation attached, so the
    /// row is told what it could not have found on its own.
    fn told(blob: &[u8], flaw: SchemaFlaw, framing: Framing, field_number: u32) -> WireRow {
        told_of(blob, 0..blob.len(), blob.len(), flaw, framing, field_number)
    }

    fn told_of(
        blob: &[u8],
        bytes: Range<usize>,
        record_end: usize,
        flaw: SchemaFlaw,
        framing: Framing,
        field_number: u32,
    ) -> WireRow {
        // Recorded, for the same reason `draw_of` is.
        wire_spans_recorded(
            blob,
            &WireSlice {
                bytes,
                record_end,
                framing,
                field_number,
                synthetic_end: 0,
            },
            ThemeKind::Dark,
            Some(&WirePalette::accusing(flaw)),
        )
    }

    /// Specs 0232 and 0279 S3: every accusation the document row makes
    /// about a named byte, put on that byte and on no other.
    #[test]
    fn an_accusation_about_a_payload_is_pointed_at_its_bytes() {
        // field 1, LEN: "A" then the two bytes of a UTF-16 BOM, which is
        // not UTF-8 at all, then "B". Only the middle two are named.
        let row = told(
            &[0x0A, 0x04, 0x41, 0xFF, 0xFE, 0x42],
            SchemaFlaw::Utf8,
            Framing::Tagged,
            1,
        );
        assert_eq!(text(&row), "2|08:04[41 ff fe 42]");
        //                                        2|08:04[41 ff fe 42]
        assert_eq!(accused(&row, Tier::Invalid), "           ^^^^^    ");
        assert_eq!(row.flags(), ["INVALID_STRING"]);
        // field 4, varint: an enum value the schema has no name for. The
        // number is the accusation, so every byte of it is banded — and
        // the tag before it is not: the field is declared, and declared
        // an enum.
        let row = told(&[0x20, 0x63], SchemaFlaw::EnumUnknown, Framing::Tagged, 4);
        assert_eq!(text(&row), "0|20[63]");
        //                                             0|20[63]
        assert_eq!(accused(&row, Tier::NonCanonical), "     ^^ ");
        // field 6, fixed64: a NaN whose payload bits are not the
        // canonical NaN's. What a re-encode would change is one byte,
        // and one byte is what the row marks.
        let row = told(
            &[0x31, 0x01, 0, 0, 0, 0, 0, 0xF8, 0x7F],
            SchemaFlaw::NanBits,
            Framing::Tagged,
            6,
        );
        assert_eq!(text(&row), "1|30[01 00 00 00 00 00 f8 7f]");
        //                                             1|30[01 00 00 00 00 00 f8 7f]
        assert_eq!(
            accused(&row, Tier::NonCanonical),
            "     ^^                      "
        );
        // A packed element holding -1 sign-extended to ten bytes. The
        // ten bytes are the anomaly, which is what makes the run's shape
        // legible beside its one-byte neighbors.
        let blob = [
            0x0A, 0x0A, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01,
        ];
        let row = told_of(
            &blob,
            2..12,
            12,
            SchemaFlaw::Neg,
            Framing::Element {
                varint: true,
                close: true,
            },
            1,
        );
        assert_eq!(text(&row), "ff ff ff ff ff ff ff ff ff 01]");
        //                                             ff ff ff ff ff ff ff ff ff 01]
        assert_eq!(
            accused(&row, Tier::NonCanonical),
            "^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ "
        );
        // field 1, varint, where the schema declares a string. The tag
        // wins; the three bits that contradict the schema are the whole
        // of it, and the field number beside them is right.
        let row = told(&[0x08, 0x07], SchemaFlaw::TypeMismatch, Framing::Tagged, 1);
        assert_eq!(text(&row), "0|08[07]");
        //                                        0|08[07]
        assert_eq!(accused(&row, Tier::Invalid), "^       ");
        assert_eq!(row.spans[2].content.as_ref(), "08");
        assert_eq!(row.spans[2].style.bg, Some(TAG_HUE));
    }

    /// Spec 0279's 2026-08-12 amendment: when no schema declares the
    /// field, its number is what the reader has instead of a name, and
    /// the bytes carrying it say so in the fold margin's own blue.
    #[test]
    fn an_undeclared_fields_number_wears_the_fold_margins_blue() {
        // field 200, varint, 42. The tag is two bytes; both are field
        // number, and the wire-type digit in front of them is not.
        let row = told(
            &[0xC0, 0x0C, 0x2A],
            SchemaFlaw::Undeclared,
            Framing::Tagged,
            200,
        );
        assert_eq!(text(&row), "0|c0 0c[2a]");
        //                                     0|c0 0c[2a]
        assert_eq!(accused(&row, Band::Unknown), "  ^^^^^    ");
        // Not a tier: the bytes are well formed and the row accuses
        // nothing, so neither anomaly color appears anywhere on it.
        assert_eq!(accused(&row, Tier::NonCanonical), " ".repeat(11));
        assert_eq!(accused(&row, Tier::Invalid), " ".repeat(11));
        assert!(row.flags().is_empty(), "no keyword says this");
        // The wire type keeps its own borrowed hue, and so does the
        // payload: what is unknown is which field these bytes are for,
        // not what they are.
        assert_eq!(row.spans[0].content.as_ref(), "0");
        assert_eq!(row.spans[0].style.bg, Some(TYPE_HUE));
        assert_eq!(row.spans[6].content.as_ref(), "2a");
        assert_eq!(row.spans[6].style.bg, Some(PAYLOAD_HUE));
    }

    /// The same row read end to end by [`schema_flaw`], which is where
    /// the absence of a keyword becomes [`SchemaFlaw::Undeclared`].
    #[test]
    fn a_row_with_a_numeric_key_and_no_keyword_is_undeclared() {
        assert_eq!(
            schema_flaw("  200: 42  #@ varint"),
            Some(SchemaFlaw::Undeclared)
        );
        assert_eq!(
            schema_flaw("  203 {  #@ group"),
            Some(SchemaFlaw::Undeclared)
        );
        // A *declared* field whose tag disagrees with it also renders a
        // numeric key, and it is not this: `row_status` ranks the
        // keyword above `Unknown`, so the keyword still wins.
        assert_eq!(
            schema_flaw("  1: 7  #@ varint; TYPE_MISMATCH"),
            Some(SchemaFlaw::TypeMismatch)
        );
        // So does a malformed record, whose field *is* declared.
        assert_eq!(
            schema_flaw(r#"  2: "\001\002"  #@ TRUNCATED_BYTES; MISSING: 3"#),
            None
        );
        // And a declared field of the same proto type is untouched: the
        // `= N` is what says the schema had something to say.
        assert_eq!(schema_flaw(r#"  name: "x"  #@ string = 1"#), None);
    }

    /// Spec 0279 S4. One failure, one rendering: the bytes before the
    /// last one decoded, so only the byte that promised another is
    /// accused, and the `??` is what says the fault is an absence.
    #[test]
    fn an_open_varint_accuses_its_last_byte() {
        // A tag varint that never terminates.
        let row = draw(&[0x82, 0x80, 0x80], Framing::Tagged, 1);
        assert_eq!(text(&row), "82 80 80 ??");
        assert_eq!(row.flags(), ["INVALID_VARINT"]);
        //                                        82 80 80 ??
        assert_eq!(accused(&row, Tier::Invalid), "      ^^^^^");
        // The same failure in a value varint.
        let row = draw(&[0x10, 0x80, 0x80], Framing::Tagged, 2);
        assert_eq!(text(&row), "0|10[80 80 ??");
        assert_eq!(row.flags(), ["INVALID_VARINT"]);
        //                                        0|10[80 80 ??
        assert_eq!(accused(&row, Tier::Invalid), "        ^^^^^");
        // And in a packed payload, which spec 0232 drew a third way:
        // the open run banded and no `??` at all.
        let row = told(
            &[0x0A, 0x03, 0x01, 0x02, 0x80],
            SchemaFlaw::PackedElements,
            Framing::Tagged,
            1,
        );
        assert_eq!(text(&row), "2|08:03[01 02 80 ??]");
        //                                        2|08:03[01 02 80 ??]
        assert_eq!(accused(&row, Tier::Invalid), "              ^^^^^ ");
        assert_eq!(row.flags(), ["INVALID_PACKED_RECORDS"]);
    }

    /// Spec 0279 S2. Two of the six keywords open an annotation and four
    /// follow a type token, so reading only the first word finds none of
    /// the latter.
    #[test]
    fn a_flaw_is_read_from_any_token_of_the_annotation() {
        // The two that open an annotation.
        assert_eq!(
            schema_flaw(r#"  10: "\377\376"  #@ INVALID_STRING"#),
            Some(SchemaFlaw::Utf8)
        );
        // And the four that follow a type token.
        assert_eq!(
            schema_flaw("  double_value: nan  #@ double = 6; nan_bits: 0x7ff8000000000001"),
            Some(SchemaFlaw::NanBits)
        );
        assert_eq!(
            schema_flaw("  label: 99  #@ Label(99) = 4; ENUM_UNKNOWN"),
            Some(SchemaFlaw::EnumUnknown)
        );
        assert_eq!(
            schema_flaw("  path: -1  #@ repeated int32 [packed=true] = 1; neg"),
            Some(SchemaFlaw::Neg)
        );
        assert_eq!(
            schema_flaw("  1: 7  #@ varint; TYPE_MISMATCH"),
            Some(SchemaFlaw::TypeMismatch)
        );
        // `truncated_neg` is a different keyword about a different fact
        // — five bytes that all decode — and splitting on `_` would have
        // read it as `neg`.
        assert_eq!(
            schema_flaw("  number: -1  #@ int32 = 3; truncated_neg"),
            None
        );
        // Only the annotation is read. A value that spells a keyword is
        // a value.
        assert_eq!(
            schema_flaw(r#"  reserved_name: "neg"  #@ repeated string = 10"#),
            None
        );
    }
}
