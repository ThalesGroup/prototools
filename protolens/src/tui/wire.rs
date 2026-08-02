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
    parse_varint, parse_wiretag, WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT,
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
}

#[cfg(test)]
impl WirePalette {
    /// Four hues that are distinct from each other and from every tier
    /// color, so a test can tell which region a byte was painted from —
    /// and tell a borrowed hue from an accented one.
    pub(super) fn for_test() -> Self {
        WirePalette {
            tag: Style::default().fg(Color::Rgb(1, 0, 0)),
            ty: Style::default().fg(Color::Rgb(1, 1, 0)),
            len: Style::default().fg(Color::Rgb(0, 1, 0)),
            payload: Style::default().fg(Color::Rgb(0, 0, 1)),
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
/// punctuation, the `\x` label, the elision (spec 0225 S11) — and the
/// whole row when the document row above has no colors either (S7).
fn subdued() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// `style` if it is one of the reversed anomaly accents, `None` if it is
/// an ordinary borrowed hue — the test `separate` needs to know whether
/// two neighbours belong to the same accusation.
fn accent(style: Style) -> Option<Style> {
    style
        .add_modifier
        .contains(Modifier::REVERSED)
        .then_some(style)
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
    /// line has no bytes it may show.
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
        Some(margin(
            indent,
            wire_spans(&self.blob, &slice, self.theme, palette),
        ))
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
            // is one color throughout rather than one with a grey
            // nibble in the middle of it.
            ty: theme::wire_style(
                theme::WireRole::Type,
                type_offset(text).and_then(role_at).or(name),
                self.theme,
            ),
            len: theme::wire_style(theme::WireRole::Length, None, self.theme),
            payload: theme::wire_style(
                theme::WireRole::Payload,
                value_offset(text).and_then(role_at),
                self.theme,
            ),
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
        Some(margin(
            indent,
            wire_spans(&overlay.bytes, &slice, self.theme, palette),
        ))
    }
}

/// Where the row's annotation names its type: the first token after
/// `#@ ` (spec 0225 S11). That is the declared proto type on a known
/// field (`string`, `int64`) and the wire type on an unknown one
/// (`varint`, `bytes`) — the same fact the tag's low nibble carries.
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

/// The row's indentation and its label (spec 0225 S5).
///
/// The document row's own indent — no deeper. The brightness step of S11
/// is what separates the two rows, and it does it everywhere on the row
/// rather than only at its left edge; a second indent level on top of it
/// only pushes long rows further past the pan bound (N3).
///
/// `\x ` rather than `0x: `: the row already spells `tag:length`, and a
/// second colon meaning something else is exactly the ambiguity a label
/// is supposed to remove. Column 0 is not here — the heat-cue gutter is
/// prepended by `render`, blank, for the same reason it is blank: a cue
/// reports how a *node* scores.
fn margin(indent: usize, row: WireRow) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(row.spans.len() + 2);
    spans.push(Span::raw(" ".repeat(render::FOLD_FIELD_WIDTH + indent)));
    spans.push(Span::styled("\\x ", subdued()));
    spans.extend(row.spans);
    spans
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
        last_accent: None,
        plain: None,
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
            painter.bytes(at..at + shown, None);
            if declared > available {
                painter.accuse("TRUNCATED_BYTES");
                painter.cut();
                painter.missing(declared - available);
            } else if at + shown <= painter.limit {
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
            painter.bytes(at..at + shown, None);
            if shown < width {
                painter.accuse(if wtype == WT_I64 {
                    "INVALID_FIXED64"
                } else {
                    "INVALID_FIXED32"
                });
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
        // `WT_START_GROUP`/`WT_END_GROUP`: a group's tags are the whole
        // of their rows, with no length and no payload beside them.
        _ => at,
    }
}

/// S4's element-0 row: the record's own tag and length wearing the
/// landmark accent — the same accent `pack_size` wears in the annotation
/// above, so the record boundary appears at the same place in both rows.
fn draw_packed_head(painter: &mut Painter, varint: bool, close: bool) -> usize {
    painter.plain = painter.accuse("pack_size");
    let (mut at, wtype) = draw_tag(painter, false);
    if wtype != Some(WT_LEN) {
        painter.plain = None;
        return at;
    }
    painter.punct(":");
    let (_, next) = draw_varint(painter, at, "len_ohb", "INVALID_LEN");
    at = next;
    painter.plain = None;
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
            // Spec 0225 S11: the same shape `TAG_OOR` uses below. The low
            // nibble of the first byte is the wire type, which is
            // END_GROUP and is correct; everything above it is the field
            // number, which is not. The number is then legible in the
            // hex, so no marker repeats it.
            let tier = painter.accuse("END_MISMATCH");
            painter.nibbles(0, tier, None);
            for i in 1..tag.next_pos {
                painter.byte(i, tier);
            }
            return tag.next_pos;
        }
    }
    draw_tag(painter, true).0
}

/// The tag varint at offset 0, styled per S11: the first byte split into
/// its wire-type nibble and its field-number nibble, because the two
/// fail separately.
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
        painter.nibbles(0, None, tier);
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
    painter.nibbles(0, oor, None);
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
    /// The reversed style the byte just drawn carried, if it carried one
    /// on either of its nibbles — what tells `separate` whether the gap
    /// it is about to draw is inside an accented run or beside it.
    last_accent: Option<Style>,
    /// The tier an unremarkable byte takes — `None` everywhere except
    /// inside a packed record's tag and length prefix, which are a
    /// landmark rather than an anomaly.
    plain: Option<Tier>,
    spans: Vec<Span<'static>>,
    #[cfg(test)]
    flags: Vec<&'static str>,
    drawn: usize,
    dropped: usize,
    need_space: bool,
}

impl Painter<'_> {
    /// Hue from the tier, reverse video from the wire row: hex is dense
    /// and uniform, so two yellow pairs among forty are easy to miss.
    /// Reverse is a locator, never a severity, so the landmark — which
    /// is not an anomaly — does not get it.
    ///
    /// An unremarkable byte takes the hue its region borrowed from the
    /// document row above (spec 0225 S11). With no palette the whole row
    /// is subdued, tiers included (S7): the `#@` tokens the tiers echo
    /// have gone gray too, and one row cannot claim a severity the other
    /// has stopped showing.
    fn style(&self, tier: Option<Tier>) -> Style {
        self.style_in(self.region, tier)
    }

    fn style_in(&self, region: Region, tier: Option<Tier>) -> Style {
        let Some(palette) = self.palette else {
            return subdued();
        };
        match tier {
            None => match region {
                Region::Tag => palette.tag,
                Region::Type => palette.ty,
                Region::Len => palette.len,
                Region::Payload => palette.payload,
                Region::Unclaimed => subdued(),
            },
            Some(Tier::Landmark) => theme::tier_style(Tier::Landmark, self.theme),
            Some(tier) => theme::tier_style(tier, self.theme).add_modifier(Modifier::REVERSED),
        }
    }

    /// A space between two hex pairs.
    ///
    /// Reversed if it lies *inside* an accented run — the byte before it
    /// carried the same reversal and so does the byte about to be drawn
    /// — and plain otherwise. An accusation over several bytes is one
    /// fact, and a run of blocks broken every third column reads as
    /// several.
    fn separate(&mut self, style: Style) {
        if self.need_space {
            let inside = accent(style).is_some() && self.last_accent == accent(style);
            self.spans.push(Span::styled(
                " ",
                if inside { style } else { Style::default() },
            ));
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
        let style = self.style(tier.or(self.plain));
        self.separate(style);
        self.spans
            .push(Span::styled(format!("{:02x}", self.rec[i]), style));
        self.last_accent = accent(style);
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

    /// The first tag byte, split into its two nibbles. Strictly, the top
    /// bit of the low nibble belongs to the field number, so a colored
    /// wire type accuses one bit it should not; the nibble is the finest
    /// unit a hex dump exposes, and legibility at the position the eye
    /// already goes is worth that.
    fn nibbles(&mut self, i: usize, high: Option<Tier>, low: Option<Tier>) {
        if i >= self.limit || i >= self.rec.len() || self.drawn >= WIRE_ROW_MAX_BYTES {
            return;
        }
        let byte = self.rec[i];
        // The two nibbles are two different facts — the field number and
        // the wire type — so they take two different borrowed hues, not
        // only two different tiers. This is the only byte on the row
        // that is ever split, and it is always a tag's first.
        let high_style = self.style_in(Region::Tag, high.or(self.plain));
        let low_style = self.style_in(Region::Type, low.or(self.plain));
        self.separate(high_style);
        self.spans
            .push(Span::styled(format!("{:x}", byte >> 4), high_style));
        self.spans
            .push(Span::styled(format!("{:x}", byte & 0x0F), low_style));
        // Either nibble opens a run: a field number accused across a
        // multi-byte tag starts on the high nibble and continues past a
        // wire type that is perfectly good.
        self.last_accent = accent(low_style).or(accent(high_style));
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
    /// Reversed like the bytes it stands in for: this is a defect of the
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
    /// Reversed with the `!`, and contiguous with it: the glyph and its
    /// count are one accusation.
    fn missing(&mut self, n: usize) {
        self.spans.push(Span::styled(format!("×{n}"), self.alarm()));
        self.need_space = false;
    }

    /// The style the row flags a defect of the *message* with.
    fn alarm(&self) -> Style {
        match self.palette {
            Some(_) => {
                theme::tier_style(Tier::Invalid, self.theme).add_modifier(Modifier::REVERSED)
            }
            None => subdued().add_modifier(Modifier::REVERSED),
        }
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
    /// Subdued, never reversed: the row ran out of columns, the message
    /// did not run out of bytes. Reversal is reserved for defects of the
    /// message, so that a screen full of `…` never reads as a screen
    /// full of alarms.
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

    fn text(row: &WireRow) -> String {
        row.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_punctuation_reads_without_color() {
        // field 1, LEN, five bytes — "HELLO".
        let blob = [0x0A, 0x05, 0x48, 0x45, 0x4C, 0x4C, 0x4F];
        let row = draw(&blob, Framing::Tagged, 1);
        assert_eq!(text(&row), "0a:05[48 45 4c 4c 4f]");
        // Spec 0225 S11: the framing carries no hue of its own. Only the
        // bytes it frames do.
        for span in &row.spans {
            if matches!(span.content.as_ref(), ":" | "[" | "]") {
                assert_eq!(span.style.fg, None, "{}", span.content);
            }
        }
        // field 2, varint, 42.
        assert_eq!(text(&draw(&[0x10, 0x2A], Framing::Tagged, 2)), "10[2a]");
        // field 1, fixed32.
        assert_eq!(
            text(&draw(&[0x0D, 0x00, 0x00, 0x80, 0x3F], Framing::Tagged, 1)),
            "0d[00 00 80 3f]"
        );
        // field 3, group start — all a header row has.
        assert_eq!(text(&draw(&[0x1B], Framing::Tagged, 3)), "1b");
    }

    /// Spec 0225 S11: each of the four parts wears the hue it borrowed,
    /// and they are four, not one. The wire-type nibble is one of the
    /// four: it names a different fact than the field number beside it.
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
                .fg
        };
        assert_eq!(hue("0"), Some(TAG_HUE));
        assert_eq!(hue("a"), Some(TYPE_HUE));
        assert_eq!(hue("02"), Some(LEN_HUE));
        assert_eq!(hue("48"), Some(PAYLOAD_HUE));
        assert_eq!(hue("49"), Some(PAYLOAD_HUE));
    }

    #[test]
    fn a_truncation_counts_the_bytes_it_cannot_show() {
        // Declared five payload bytes, one present. Spec 0225 S11: the
        // count of what is absent, glued to the `!` that says so.
        let row = draw(&[0x0A, 0x05, 0x48], Framing::Tagged, 1);
        assert_eq!(text(&row), "0a:05[48!×4");
        assert_eq!(row.flags, ["TRUNCATED_BYTES"]);
        // Both glyphs are reversed, and they are one accusation: the
        // bytes are missing from the *message*, not from the row.
        for span in row.spans.iter().filter(|s| {
            let c = s.content.as_ref();
            c == "!" || c == "×4"
        }) {
            assert!(
                span.style.add_modifier.contains(Modifier::REVERSED),
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
    fn the_type_nibble_is_styled_apart_from_the_field_number() {
        // field 1, wire type 7.
        let row = draw(&[0x0F], Framing::Tagged, 1);
        assert_eq!(text(&row), "0f");
        assert_eq!(row.flags, ["INVALID_TAG_TYPE"]);
        let invalid = theme::tier_style(Tier::Invalid, ThemeKind::Dark)
            .add_modifier(Modifier::REVERSED)
            .fg
            .unwrap();
        assert_eq!(row.spans[0].content.as_ref(), "0");
        assert_eq!(row.spans[0].style.fg, Some(TAG_HUE));
        assert_eq!(row.spans[1].content.as_ref(), "f");
        assert_eq!(row.spans[1].style.fg, Some(invalid));
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
        assert_eq!(text(&row), "1c");
        assert!(row.flags.is_empty());
        // The same bytes closing field 2 instead. Spec 0225 S11: the
        // wrong number is *shown* wrong rather than restated in words,
        // so the row is unchanged and the field-number nibbles redden.
        let row = draw(&[0x1C], Framing::Closing, 2);
        assert_eq!(text(&row), "1c");
        assert_eq!(row.flags, ["END_MISMATCH"]);
        let invalid = theme::tier_style(Tier::Invalid, ThemeKind::Dark)
            .add_modifier(Modifier::REVERSED)
            .fg
            .unwrap();
        assert_eq!(row.spans[0].content.as_ref(), "1");
        assert_eq!(row.spans[0].style.fg, Some(invalid));
        // The type nibble is not what is wrong — it says END_GROUP, and
        // it does.
        assert_eq!(row.spans[1].content.as_ref(), "c");
        assert_eq!(row.spans[1].style.fg, Some(TYPE_HUE));
    }

    #[test]
    fn an_overlong_varint_shows_its_trailing_padding() {
        // field 2, varint, value 42 written in three bytes.
        let row = draw(&[0x10, 0xAA, 0x80, 0x00], Framing::Tagged, 2);
        assert_eq!(text(&row), "10[aa 80 00]");
        assert_eq!(row.flags, ["val_ohb"]);
        let yellow = theme::tier_style(Tier::NonCanonical, ThemeKind::Dark)
            .fg
            .unwrap();
        // The padding bytes are accused, and the gap between them is
        // accused with them: one fact, one unbroken block.
        let colored: Vec<&str> = row
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(yellow))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(colored, ["80", " ", "00"]);
        // The gap before them is *outside* the run and stays plain —
        // `aa` is a perfectly good first byte.
        assert_eq!(
            row.spans
                .iter()
                .filter(|s| s.content.as_ref() == " ")
                .map(|s| s.style.fg)
                .collect::<Vec<_>>(),
            [None, Some(yellow)],
        );
    }

    #[test]
    fn an_out_of_range_field_number_spares_the_type_nibble() {
        // Field number 0, wire type 0 — `TAG_OOR`, and the type bits are
        // perfectly good.
        let row = draw(&[0x00, 0x01], Framing::Tagged, 0);
        assert_eq!(text(&row), "00[01]");
        assert_eq!(row.flags, ["TAG_OOR"]);
        let invalid = theme::tier_style(Tier::Invalid, ThemeKind::Dark)
            .add_modifier(Modifier::REVERSED)
            .fg
            .unwrap();
        assert_eq!(row.spans[0].content.as_ref(), "0");
        assert_eq!(row.spans[0].style.fg, Some(invalid));
        assert_eq!(row.spans[1].content.as_ref(), "0");
        assert_eq!(row.spans[1].style.fg, Some(TYPE_HUE));
    }

    #[test]
    fn a_packed_records_tag_and_length_wear_the_landmark() {
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
        assert_eq!(text(&head), "0a:02[01");
        // The accent is not invented here: the row names `pack_size`,
        // the same keyword the annotation above it carries, and
        // `tier_of` is what turns that into a landmark.
        assert_eq!(head.flags, ["pack_size"]);
        let accent = theme::tier_style(Tier::Landmark, ThemeKind::Dark)
            .fg
            .unwrap();
        let landmarked: Vec<&str> = head
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(accent))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(landmarked, ["0", "a", "02"]);
        // The element bytes are not part of the landmark.
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
                .map(|s| s.style.fg),
            Some(Some(PAYLOAD_HUE)),
            "an element is payload, and payload is all it is",
        );
    }

    #[test]
    fn a_header_rows_payload_is_left_to_its_children() {
        // field 1, LEN, three payload bytes the row does not own —
        // the message-header case. No `]`: nothing has ended.
        let blob = [0x0A, 0x03, 0x08, 0x01, 0x00];
        let row = draw_of(&blob, 0..2, 5, Framing::Tagged, 1);
        assert_eq!(text(&row), "0a:03[");
        assert!(row.flags.is_empty());
    }

    #[test]
    fn unclaimed_trailing_bytes_are_drawn_plain() {
        let row = draw_of(&[0xAA, 0xBB], 0..2, 2, Framing::Raw, 1);
        assert_eq!(text(&row), "aa bb");
        // No framing claimed them, so they belong to none of the three
        // regions and borrow nothing (spec 0225 S11).
        assert!(row.spans.iter().all(|s| s.style.fg.is_none()));
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
        assert_eq!(text(&row), "00[01]");
        assert_eq!(row.flags, ["TAG_OOR"]);
        assert!(row.spans.iter().all(|s| s.style.fg.is_none()));
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
        // bytes. Nothing here is reversed (spec 0225 S11).
        let marker = row.spans.last().expect("the row ends with the elision");
        assert!(!marker.style.add_modifier.contains(Modifier::REVERSED));
    }

    /// The two cases spec 0225 S11 draws.
    ///
    /// A separator is reversed exactly when it lies between two bytes of
    /// the same accusation, so an accusation reads as one block rather
    /// than as a row of tiles.
    #[test]
    fn an_accusation_is_one_unbroken_block() {
        let block = |row: &WireRow| -> String {
            row.spans
                .iter()
                .flat_map(|s| {
                    let mark = if s.style.add_modifier.contains(Modifier::REVERSED) {
                        '^'
                    } else {
                        ' '
                    };
                    std::iter::repeat_n(mark, s.content.chars().count())
                })
                .collect()
        };
        // field 2, varint, a value written in four bytes: the last two
        // add nothing, and the run starts in the middle of the row.
        let row = draw(&[0x10, 0xAC, 0x86, 0x80, 0x00], Framing::Tagged, 2);
        assert_eq!(text(&row), "10[ac 86 80 00]");
        assert_eq!(block(&row), "         ^^^^^ ");
        // Field number 0 over a two-byte tag: the accusation opens on
        // the high nibble of the first byte, skips the wire type beside
        // it, and runs to the end of the tag.
        let row = draw(&[0x80, 0x80, 0x80, 0x80, 0x10, 0x01], Framing::Tagged, 0);
        assert_eq!(text(&row), "80 80 80 80 10[01]");
        assert_eq!(block(&row), "^ ^^^^^^^^^^^^    ");
    }
}
