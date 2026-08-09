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
    /// a color (spec 0232). See [`PayloadFlaw`].
    pub(super) flaw: Option<PayloadFlaw>,
}

/// An accusation the document row made about a payload that the wire
/// row can point at, but could never have found on its own.
///
/// Both cases are schema questions — whether these bytes are a `string`
/// or a `bytes`, whether this packed record's elements are varints or
/// eight bytes wide — and this module is deliberately schema-free (spec
/// 0225 S11, "one classifier, two rows"). The document row's classifier
/// has already answered; what is left is *where*, which is the one
/// question the hex is in a position to answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PayloadFlaw {
    /// `INVALID_STRING`: the payload is not valid UTF-8.
    Utf8,
    /// `INVALID_PACKED_RECORDS`: the payload does not divide into whole
    /// elements.
    PackedElements,
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
    pub(super) fn accusing(flaw: PayloadFlaw) -> Self {
        WirePalette {
            flaw: Some(flaw),
            ..WirePalette::for_test()
        }
    }
}

/// Which part of the row the pen is in, so that a byte carrying no tier
/// takes that part's borrowed hue (spec 0225 S11).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Tag,
    Type,
    Len,
    Payload,
    /// Bytes no framing claimed. They belong to none of the four and
    /// take the subdued default: nothing honest can be said about them.
    Unclaimed,
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
    /// The annotation keywords this row's bytes justify, in the order
    /// they were found — the wire-level subset of what prototext-core
    /// would print as `#@ ...` on the line above.
    ///
    /// Test-only, and deliberately so: nothing draws a keyword, the
    /// styling already went through `tier_of`, and keeping the list at
    /// runtime would allocate on every row of every frame to say what
    /// the row already shows. It exists so a test can cross-check the
    /// two independent derivations (S11, "where the facts come from").
    #[cfg(test)]
    pub(super) flags: Vec<&'static str>,
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
        };
    }
    let tail_start = children.map_or(bytes.end, |c| c.end);
    WireSlice {
        bytes: tail_start..bytes.end,
        record_end: bytes.end,
        framing: if u32::from(span.wire_type) == WT_START_GROUP {
            Framing::Closing
        } else {
            Framing::Raw
        },
        field_number,
    }
}

impl App {
    /// The bytes the line `pos` names owns, or `None` when it owns none
    /// it may show — the wrapper root (S3) and an unrendered slot.
    pub(super) fn wire_slice(&self, pos: LinePos, memo: &mut PackedCursor) -> Option<WireSlice> {
        let idx = pos.node;
        let node = self.tree.get(idx)?;
        if !node.is_rendered() {
            return None;
        }
        // S3: slot 0's head is `Blob`'s synthetic `write_tag(1, WT_LEN)`
        // and length prefix. Showing bytes that are not in the user's
        // file would be a fabrication in the one view whose entire
        // purpose is fidelity to them.
        if self.wrapper_offset > 0 && self.parent(idx).is_none() {
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
        Some(head_or_tail(&node.span, start..end, children, footer))
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
        let elem_wt = u32::from(node.span.wire_type);
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
        })
    }

    /// The whole wire row for `pos`, margin included — `None` when the
    /// line has no bytes it may show, or none at all.
    ///
    /// `indent` is the document row's own indentation, so the hex lines
    /// up under the text it describes (S5). Column 0 is not here: the
    /// heat-cue gutter is prepended by `render`, blank, for the same
    /// reason it is blank — a cue reports how a *node* scores.
    pub(super) fn wire_row(
        &self,
        pos: LinePos,
        indent: usize,
        memo: &mut PackedCursor,
        palette: Option<&WirePalette>,
    ) -> Option<Vec<Span<'static>>> {
        let slice = self.wire_slice(pos, memo)?;
        margin(indent, wire_spans(&self.blob, &slice, self.theme, palette))
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
            flaw: payload_flaw(text),
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
        indent: usize,
        palette: Option<&WirePalette>,
    ) -> Option<Vec<Span<'static>>> {
        let overlay = self.preview_overlay.as_ref()?;
        let slice = preview_slice(&overlay.spans, line)?;
        margin(
            indent,
            wire_spans(&overlay.bytes, &slice, self.theme, palette),
        )
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

/// The accusation the row's annotation opens with, when it is one this
/// module can localize (spec 0232).
///
/// Read from the text rather than from the styles: the keyword is the
/// fact, and two keywords out of the fifteen in `annotation::INVALID`
/// share the one tier color.
fn payload_flaw(text: &str) -> Option<PayloadFlaw> {
    let at = type_offset(text)?;
    let mut words = text[at..].split(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    match words.next()? {
        "INVALID_STRING" => Some(PayloadFlaw::Utf8),
        "INVALID_PACKED_RECORDS" => Some(PayloadFlaw::PackedElements),
        _ => None,
    }
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
fn margin(indent: usize, row: WireRow) -> Option<Vec<Span<'static>>> {
    if row.spans.is_empty() {
        return None;
    }
    let mut spans = Vec::with_capacity(row.spans.len() + 2);
    spans.push(Span::raw(" ".repeat(render::FOLD_FIELD_WIDTH + indent)));
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

    let elem_wt = u32::from(span.wire_type);
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
    let record_end = slice.record_end.max(slice.bytes.end).min(blob.len());
    let start = slice.bytes.start.min(record_end);
    let mut painter = Painter {
        rec: &blob[start..record_end],
        limit: slice.bytes.end.saturating_sub(start),
        theme,
        palette,
        region: Region::Unclaimed,
        last_block: None,
        spans: Vec::new(),
        #[cfg(test)]
        flags: Vec::new(),
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
            let (declared, next) = draw_varint(painter, at, "len_ohb", "INVALID_LEN");
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
            let tier = short.then(|| painter.accuse("TRUNCATED_BYTES")).flatten();
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
                    painter.accuse(if wtype == WT_I64 {
                        "INVALID_FIXED64"
                    } else {
                        "INVALID_FIXED32"
                    })
                })
                .flatten();
            painter.bytes(at..at + shown, tier);
            if short {
                painter.cut();
            } else {
                painter.punct("]");
            }
            at + shown
        }
        WT_VARINT => {
            painter.punct("[");
            painter.region = Region::Payload;
            let (value, next) = draw_varint(painter, at, "val_ohb", "INVALID_VARINT");
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
    let tier = painter.accuse("INVALID_GROUP_END");
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
    painter.accuse("pack_size");
    let (mut at, wtype) = draw_tag(painter, false);
    if wtype != Some(WT_LEN) {
        return at;
    }
    painter.punct(":");
    painter.region = Region::Len;
    let (_, next) = draw_varint(painter, at, "len_ohb", "INVALID_LEN");
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
fn draw_element(painter: &mut Painter, at: usize, varint: bool) -> usize {
    painter.region = Region::Payload;
    if varint {
        draw_varint(painter, at, "val_ohb", "INVALID_VARINT").1
    } else {
        painter.bytes(at..painter.limit, None);
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
        painter.accuse("OPEN_GROUP");
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
            let tier = painter.accuse("END_MISMATCH");
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
        let tier = painter.accuse("INVALID_TAG_TYPE");
        painter.tag_head(0, None, tier);
        return (1, None);
    }
    let tag = parse_wiretag(painter.rec, 0);
    if tag.wtag_gar.is_some() {
        let tier = painter.accuse(if closing {
            "INVALID_GROUP_END"
        } else {
            "INVALID_VARINT"
        });
        painter.bytes(0..painter.rec.len(), tier);
        painter.cut();
        return (painter.rec.len(), None);
    }
    let end = tag.next_pos;
    let out_of_range = tag.wfield_oor.is_some();
    let overhang = tag.wfield_ohb.unwrap_or(0) as usize;
    let oor = out_of_range
        .then(|| painter.accuse(if closing { "ETAG_OOR" } else { "TAG_OOR" }))
        .flatten();
    let ohb = (!out_of_range && overhang > 0)
        .then(|| painter.accuse(if closing { "etag_ohb" } else { "tag_ohb" }))
        .flatten();
    // The field portion goes red, the wire type stays whatever it was:
    // an out-of-range field number says nothing about the type bits.
    painter.tag_head(0, oor, None);
    for i in 1..end {
        let tier = if out_of_range {
            oor
        } else if i + overhang >= end {
            // A varint is little-endian, so the padding of an overlong
            // one is its trailing `0x80`…`0x00` run.
            ohb
        } else {
            None
        };
        painter.byte(i, tier);
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
    let bad = match flaw {
        PayloadFlaw::Utf8 => utf8_flaws(payload),
        PayloadFlaw::PackedElements => vec![packed_flaw(payload)],
    };
    let tier = painter.accuse(match flaw {
        PayloadFlaw::Utf8 => "INVALID_STRING",
        PayloadFlaw::PackedElements => "INVALID_PACKED_RECORDS",
    });
    let mut at = 0;
    for span in bad {
        painter.bytes(range.start + at..range.start + span.start, None);
        painter.bytes(range.start + span.start..range.start + span.end, tier);
        at = span.end;
    }
    painter.bytes(range.start + at..range.end, None);
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

/// The bytes of `payload` that do not complete a packed element.
///
/// The row has no schema, so it reads the payload as the one packed
/// encoding that delimits itself: a stream of varints. A varint left
/// open at the end is the case the document reports, and the run from
/// its first byte to the payload's end is exactly what does not decode.
///
/// When every varint does close, the record is a fixed-width one whose
/// length is not a multiple of its element size — and the size is
/// precisely what this row cannot know, four and eight being equally
/// consistent with the bytes. The whole payload is named, which is the
/// most that is true.
fn packed_flaw(payload: &[u8]) -> Range<usize> {
    let mut at = 0;
    while at < payload.len() {
        let parsed = parse_varint(payload, at);
        if parsed.varint_gar.is_some() {
            return at..payload.len();
        }
        at = parsed.next_pos;
    }
    0..payload.len()
}

/// A length or value varint at `at`, with its overlong padding in
/// yellow and its truncation closed by `!`.
fn draw_varint(
    painter: &mut Painter,
    at: usize,
    overhang_flag: &'static str,
    invalid_flag: &'static str,
) -> (Option<u64>, usize) {
    if at >= painter.rec.len() {
        painter.accuse(invalid_flag);
        painter.cut();
        return (None, painter.rec.len());
    }
    let parsed = parse_varint(painter.rec, at);
    if parsed.varint_gar.is_some() {
        let tier = painter.accuse(invalid_flag);
        painter.bytes(at..painter.rec.len(), tier);
        painter.cut();
        return (None, painter.rec.len());
    }
    let end = parsed.next_pos;
    let overhang = parsed.varint_ohb.unwrap_or(0) as usize;
    painter.bytes(at..end - overhang, None);
    if overhang > 0 {
        let tier = painter.accuse(overhang_flag);
        painter.bytes(end - overhang..end, tier);
    }
    (parsed.varint, end)
}

/// Accumulates the row. Offsets are relative to the row's first byte;
/// `rec` runs to the record's end, `limit` to the row's, and the two
/// differ only on a packed run's element-0 row.
struct Painter<'a> {
    rec: &'a [u8],
    limit: usize,
    theme: ThemeKind,
    /// The hues borrowed from the document row (spec 0225 S11), or `None`
    /// when it has none of its own — in which case the whole row is
    /// subdued, tiers included (S7).
    palette: Option<&'a WirePalette>,
    /// Which of the four the pen is currently in.
    region: Region,
    /// The background block the byte just drawn wore. What tells
    /// `separate` whether the gap it is about to draw is inside one
    /// block or between two.
    ///
    /// One slot, because a tag's first byte — the only one drawn in two
    /// halves — ends on its field-number half, which is the half a
    /// multi-byte tag's remaining bytes continue.
    last_block: Option<Style>,
    spans: Vec<Span<'static>>,
    #[cfg(test)]
    flags: Vec<&'static str>,
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
    /// Absent a tier it says what the byte is *for*, in the hue its
    /// region borrowed from the document row above. A tier says the
    /// byte is wrong, and takes the band over at full strength: what a
    /// byte is for stops mattering once it is malformed, and a band
    /// swap is the loudest thing the row can do without changing shape.
    ///
    /// With no palette the whole row is subdued, tiers included (S7):
    /// the `#@` tokens the tiers echo have gone gray too, and one row
    /// cannot claim a severity the other has stopped showing. Bytes no
    /// framing claimed keep the same subdued text, since a band would
    /// be a claim about them.
    fn style(&self, tier: Option<Tier>) -> Style {
        self.style_in(self.region, tier)
    }

    fn style_in(&self, region: Region, tier: Option<Tier>) -> Style {
        let Some(palette) = self.palette else {
            return subdued();
        };
        match tier {
            Some(tier) => theme::tier_band(tier, self.theme),
            None => match region {
                Region::Tag => palette.tag,
                Region::Type => palette.ty,
                Region::Len => palette.len,
                Region::Payload => palette.payload,
                Region::Unclaimed => subdued(),
            },
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
            self.spans.push(Span::styled(" ", fill));
        }
        self.need_space = true;
    }

    fn byte(&mut self, i: usize, tier: Option<Tier>) {
        if i >= self.limit || i >= self.rec.len() {
            return;
        }
        if self.drawn >= WIRE_ROW_MAX_BYTES {
            self.dropped += 1;
            return;
        }
        let style = self.style(tier);
        self.separate(style);
        self.spans
            .push(Span::styled(format!("{:02x}", self.rec[i]), style));
        self.last_block = block(style);
        self.drawn += 1;
    }

    fn bytes(&mut self, range: Range<usize>, tier: Option<Tier>) {
        let end = range.end.min(self.limit).min(self.rec.len());
        let mut i = range.start;
        while i < end {
            if self.drawn >= WIRE_ROW_MAX_BYTES {
                // Counted in one step rather than one byte at a time: a
                // LEN payload can be megabytes and this runs per frame.
                self.dropped += end - i;
                return;
            }
            self.byte(i, tier);
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
    /// two different tiers. This is the only byte on the row that is
    /// ever split, and it is always a tag's first.
    fn tag_head(&mut self, i: usize, number: Option<Tier>, wtype: Option<Tier>) {
        if i >= self.limit || i >= self.rec.len() || self.drawn >= WIRE_ROW_MAX_BYTES {
            return;
        }
        let byte = self.rec[i];
        let number_style = self.style_in(Region::Tag, number);
        let wtype_style = self.style_in(Region::Type, wtype);
        self.separate(wtype_style);
        self.spans
            .push(Span::styled(format!("{}", byte & 0x07), wtype_style));
        // The bar joins the two halves when they wear the same band and
        // parts them when they do not — the same rule `separate` applies
        // to the gap between two hex pairs, for the same reason.
        let bar = match (block(wtype_style), block(number_style)) {
            (Some(band), Some(other)) if band == other => band,
            _ => Style::default(),
        };
        self.spans.push(Span::styled("|", bar));
        self.spans
            .push(Span::styled(format!("{:02x}", byte & 0xF8), number_style));
        self.last_block = block(number_style);
        self.drawn += 1;
    }

    fn punct(&mut self, text: &'static str) {
        if self.drawn >= WIRE_ROW_MAX_BYTES {
            return;
        }
        self.spans.push(Span::styled(text, subdued()));
        self.need_space = false;
    }

    /// The single glyph that says the bytes ran out. It replaces
    /// whatever would have come next, so *where* it sits is what
    /// distinguishes a truncated payload from a truncated varint and a
    /// second glyph would be redundant.
    ///
    /// Colored like the bytes it stands in for: this is a defect of the
    /// message, not of the display, and the row flags every one of those
    /// the same way.
    fn cut(&mut self) {
        self.spans.push(Span::styled("!".to_string(), self.alarm()));
        self.need_space = false;
    }

    /// How many bytes the record promised and did not deliver, glued to
    /// the `!` that says it ran out: `!×4` (spec 0225 S11). The count is
    /// the one fact the row cannot show, since the bytes it counts are
    /// precisely the ones absent from it — and `×N` reads as "N of
    /// these", the same as it does after the elision.
    ///
    /// Alarmed with the `!`, and contiguous with it: the glyph and its
    /// count are one accusation.
    fn missing(&mut self, n: usize) {
        self.spans.push(Span::styled(format!("×{n}"), self.alarm()));
        self.need_space = false;
    }

    /// The style the row flags a defect of the *message* with: exactly
    /// what an invalid byte would wear here, since that is what the
    /// glyph stands in for — the `Invalid` band, and subdued with the
    /// rest of the row when there is no palette to carry one (S7).
    fn alarm(&self) -> Style {
        self.style(Some(Tier::Invalid))
    }

    /// Name the annotation keyword prototext-core would have emitted for
    /// what was just found, and get back the tier it carries.
    ///
    /// Spec 0225 S11 "one classifier, two rows": this module never
    /// decides a severity of its own. It reports the keyword, and
    /// `annotation::tier_of` — the same table `highlights.scm` mirrors
    /// for the `#@` row — decides. The keyword is kept so a test can
    /// cross-check this row's findings against the annotation above it.
    fn accuse(&mut self, keyword: &'static str) -> Option<Tier> {
        #[cfg(test)]
        self.flags.push(keyword);
        annotation::tier_of(keyword)
    }

    /// Closes the row, adding `…×N` if the byte budget cut it short.
    ///
    /// Subdued, never alarmed: the row ran out of columns, the message
    /// did not run out of bytes. The tier colors are reserved for
    /// defects of the message, so that a screen full of `…` never reads
    /// as a screen full of alarms.
    fn finish(mut self) -> WireRow {
        if self.dropped > 0 {
            self.spans
                .push(Span::styled(format!("…×{}", self.dropped), subdued()));
        }
        WireRow {
            spans: self.spans,
            #[cfg(test)]
            flags: self.flags,
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
        wire_spans(
            blob,
            &WireSlice {
                bytes,
                record_end,
                framing,
                field_number,
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
        theme::tier_band(tier, ThemeKind::Dark)
            .bg
            .expect("a tier is a band")
    }

    fn text(row: &WireRow) -> String {
        row.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// A `^` under every column `tier` claimed, aligned with `text`.
    ///
    /// Reads the *background*, which is where a tier goes and where
    /// `separate`'s run-joining is decided.
    fn accused(row: &WireRow, tier: Tier) -> String {
        let hue = tier_hue(tier);
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
        // count of what is absent, glued to the `!` that says so.
        let row = draw(&[0x0A, 0x05, 0x48], Framing::Tagged, 1);
        assert_eq!(text(&row), "2|08:05[48!×4");
        assert_eq!(row.flags, ["TRUNCATED_BYTES"]);
        // Both glyphs wear the `Invalid` tier, and they are one
        // accusation: the bytes are missing from the *message*, not from
        // the row.
        for span in row.spans.iter().filter(|s| {
            let c = s.content.as_ref();
            c == "!" || c == "×4"
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
        assert_eq!(text(&row), "82 80 80!");
        assert_eq!(row.flags, ["INVALID_VARINT"]);
    }

    #[test]
    fn the_wire_type_is_styled_apart_from_the_field_number() {
        // field 1, wire type 7.
        let row = draw(&[0x0F], Framing::Tagged, 1);
        assert_eq!(text(&row), "7|08");
        assert_eq!(row.flags, ["INVALID_TAG_TYPE"]);
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
        assert_eq!(row.flags, ["INVALID_GROUP_END"]);
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

    /// Spec 0225 S11: the missing END tag is a missing byte, and `!` is
    /// already what this row says about one.
    #[test]
    fn an_open_group_footer_row_is_a_bare_bang() {
        let row = draw_of(&[], 0..0, 0, Framing::Closing, 3);
        assert_eq!(text(&row), "!");
        assert_eq!(row.flags, ["OPEN_GROUP"]);
    }

    #[test]
    fn a_group_footer_names_the_group_it_closes() {
        // field 3, END_GROUP.
        let row = draw(&[0x1C], Framing::Closing, 3);
        assert_eq!(text(&row), "4|18");
        assert!(row.flags.is_empty());
        // The same bytes closing field 2 instead. Spec 0225 S11: the
        // wrong number is *shown* wrong rather than restated in words,
        // so the row is unchanged and the field number reddens.
        let row = draw(&[0x1C], Framing::Closing, 2);
        assert_eq!(text(&row), "4|18");
        assert_eq!(row.flags, ["END_MISMATCH"]);
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
        assert_eq!(row.flags, ["val_ohb"]);
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
        assert_eq!(row.flags, ["TAG_OOR"]);
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
        assert_eq!(head.flags, ["pack_size"]);
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
        assert!(row.flags.is_empty());
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
        let row = wire_spans(
            &[0x00, 0x01],
            &WireSlice {
                bytes: 0..2,
                record_end: 2,
                framing: Framing::Tagged,
                field_number: 0,
            },
            ThemeKind::Dark,
            None,
        );
        assert_eq!(text(&row), "0|00[01]");
        assert_eq!(row.flags, ["TAG_OOR"]);
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
        assert_eq!(text(&row), "1|28[01 02 03!");
        //                                       1|28[01 02 03!
        assert_eq!(accused(&row, Tier::Invalid), "     ^^^^^^^^^");
        // field 5, fixed32, one of the four.
        let row = draw(&[0x2D, 0x01], Framing::Tagged, 5);
        assert_eq!(text(&row), "5|28[01!");
        assert_eq!(accused(&row, Tier::Invalid), "     ^^^");
        // field 1, LEN, five bytes promised and two delivered.
        let row = draw(&[0x0A, 0x05, 0x48, 0x45], Framing::Tagged, 1);
        assert_eq!(text(&row), "2|08:05[48 45!×3");
        assert_eq!(accused(&row, Tier::Invalid), "        ^^^^^^^^");
    }

    /// Spec 0232: the two accusations the document row makes about a
    /// payload that this row can point at, having been told.
    #[test]
    fn an_accusation_about_a_payload_is_pointed_at_its_bytes() {
        let accusing = |blob: &[u8], flaw: PayloadFlaw| {
            let palette = WirePalette::accusing(flaw);
            wire_spans(
                blob,
                &WireSlice {
                    bytes: 0..blob.len(),
                    record_end: blob.len(),
                    framing: Framing::Tagged,
                    field_number: 1,
                },
                ThemeKind::Dark,
                Some(&palette),
            )
        };
        // field 1, LEN: "A" then the two bytes of a UTF-16 BOM, which is
        // not UTF-8 at all, then "B". Only the middle two are named.
        let row = accusing(&[0x0A, 0x04, 0x41, 0xFF, 0xFE, 0x42], PayloadFlaw::Utf8);
        assert_eq!(text(&row), "2|08:04[41 ff fe 42]");
        //                                        2|08:04[41 ff fe 42]
        assert_eq!(accused(&row, Tier::Invalid), "           ^^^^^    ");
        assert_eq!(row.flags, ["INVALID_STRING"]);
        // field 1, LEN: two whole varints and a third left open by its
        // continuation bit. The run from that byte to the end is what
        // does not decode; the two before it are whole and stay plain.
        let row = accusing(&[0x0A, 0x03, 0x01, 0x02, 0x80], PayloadFlaw::PackedElements);
        assert_eq!(text(&row), "2|08:03[01 02 80]");
        //                                        2|08:03[01 02 80]
        assert_eq!(accused(&row, Tier::Invalid), "              ^^ ");
        assert_eq!(row.flags, ["INVALID_PACKED_RECORDS"]);
    }
}
