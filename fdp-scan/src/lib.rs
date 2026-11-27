// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! The scanner protoscan and the `fdp_scan_lib` Python extension share
//! (spec 0407 S1): find the `FileDescriptorProto` records embedded in a
//! binary, by scoring each candidate against prototext's embedded WKT graph
//! (specs 0239, 0313).

use std::sync::OnceLock;

mod class;
mod inflate;

pub use inflate::{
    scan_inflating, scan_inflating_filtered, scan_inflating_with, InflateLimits, Layer, MemberKind,
    Origin, SkipReason, Visit,
};

use prototext_graph::score::load::LoadedGraph;
use prototext_graph::score::{score_one, Ending, Policy, ScoringOpts};

/// `FileDescriptorProto`'s field numbers that [`dangling_dependency_cut`]
/// and [`implausible_edition_cut`] read. The stock ones, whatever schema
/// the walk scored against: every `descriptor.proto` in use keeps them
/// (specs 0449 S1, 0456 S1).
const FDP_NAME: u64 = 1;
const FDP_PACKAGE: u64 = 2;
const FDP_DEPENDENCY: u64 = 3;
const FDP_PUBLIC_DEPENDENCY: u64 = 10;
const FDP_WEAK_DEPENDENCY: u64 = 11;
const FDP_SYNTAX: u64 = 12;
const FDP_EDITION: u64 = 14;

/// The editions a writer sets in `edition`: `EDITION_2023` to
/// `EDITION_UNSTABLE` (spec 0456 S1). Below are the legacy markers
/// (`EDITION_PROTO2`, `EDITION_PROTO3`, `EDITION_LEGACY`) and two test-only
/// values; above, three test-only values and `EDITION_MAX`. A range, not
/// the schema's names, so that a later edition is accepted unchanged.
const WRITTEN_EDITIONS: std::ops::RangeInclusive<u64> = 1000..=9999;

/// The only root a candidate is scored against (spec 0239 G3).
///
/// One root, not two. `SCAN` against `FileDescriptorProto` alone already
/// refuses to swallow a whole `FileDescriptorSet` — `name` is singular, so
/// the next `file` entry's field-1 tag terminates the walk — which is what
/// makes the boundary correct. Adding `FileDescriptorSet` would only let the
/// scanner *classify* its input, which is a different tool, and a root list
/// admitting exactly one exception is not a root list.
const FDP: &str = "google.protobuf.FileDescriptorProto";

/// `prototext`'s embedded WKT graph, parsed once.
///
/// It carries `descriptor.proto`, and reproto registers every message it
/// compiles as a root, so `FDP` is reachable by construction.
fn wkt_graph() -> &'static LoadedGraph {
    static GRAPH: OnceLock<LoadedGraph> = OnceLock::new();
    GRAPH.get_or_init(|| {
        LoadedGraph::from_static_bytes(prototext::WKT_GRAPH)
            .expect("the embedded WKT graph must load; it is built by prototext's build.rs")
    })
}

/// The `(start, end)` byte ranges of the `FileDescriptorProto` records
/// embedded in `data`, in order.
pub fn scan(data: &[u8]) -> Vec<(usize, usize)> {
    scan_explained(data).found
}

/// What [`scan`] finds, and what it refused (spec 0448 S7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Explained {
    /// Exactly what [`scan`] returns.
    pub found: Vec<(usize, usize)>,
    /// The candidates refused, in discovery order. Only anchors whose name
    /// ends in `.proto` are listed: every `0x0A` byte is an anchor, and
    /// listing the others would bury the ones worth reading.
    pub rejected: Vec<Rejected>,
    /// The records in `found` that the order check ended earlier than the
    /// scan without it ends them (spec 0449 S7), in order. Always empty unless
    /// [`ScanOptions::increasing_field_numbers`] is set.
    pub order_cuts: Vec<OrderCut>,
    /// The records in `found` that their frame ended earlier than the walk
    /// does (spec 0496), in order.
    pub frame_cuts: Vec<FrameCut>,
}

/// A record its frame ended (spec 0496): the walk read on past the length
/// the frame before it declares, into the enclosing message's fields. It is
/// in [`Explained::found`] with the frame's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameCut {
    /// Where the record starts, as in `found`.
    pub start: usize,
    /// Where its frame ends it, as in `found`.
    pub end: usize,
    /// Where the walk without the frame ended it.
    pub read_to: usize,
    /// The frame that ended it.
    pub frame: Frame,
}

/// A frame that declares a record's length (spec 0496).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// A protobuf length-delimited field, chained to the record before
    /// (S1, S2): a set's record.
    Protobuf,
    /// A Python marshal bytes object (S5): a compiled module's descriptor.
    Marshal,
}

impl Frame {
    /// The frame's name, as a record entry says it.
    pub fn name(self) -> &'static str {
        match self {
            Frame::Protobuf => "protobuf",
            Frame::Marshal => "marshal",
        }
    }
}

/// A record [`ScanOptions::increasing_field_numbers`] ended early. It is in
/// [`Explained::found`] all the same: whether to keep a record cut short is
/// the caller's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderCut {
    /// Where the record starts, as in `found`.
    pub start: usize,
    /// Where it now ends, as in `found`.
    pub end: usize,
    /// The offset of the out-of-order tag: `end` when it was a top-level
    /// field, past `end` when it was nested in the top-level field the record
    /// lost.
    pub at: usize,
}

/// How to scan. `Default` is what [`scan_explained`] and
/// [`scan_explained_with`] use.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Require every message's fields in increasing number order, as
    /// protoc's C++ writer emits them (spec 0449 S3). Fewer ambiguous and
    /// merged records on such inputs; on others, such as Go's, records cut
    /// short, each reported in [`Explained::order_cuts`].
    pub increasing_field_numbers: bool,
}

/// One refused candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// The offset of its field-1 tag.
    pub start: usize,
    /// The name it claims.
    pub name: String,
    pub reason: Reason,
}

/// Why a candidate was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The name ends in `.proto` but is not a plausible path (spec 0448 S5).
    ImplausibleName,
    /// The record holds only its `name` and does not stand alone (spec 0448
    /// S6).
    NameOnly,
    /// The walk refused it: a veto, an unknown field, or a missing
    /// `required` field (spec 0239 S4).
    Defect,
    /// Cut before a `public_dependency` or `weak_dependency` index with no
    /// matching `dependency`, which protoc's descriptor builder rejects, the
    /// record holds its name only (spec 0449 S1).
    DanglingDependency,
    /// Cut before an `edition` no writer sets, the record holds its name
    /// only (spec 0456 S1).
    ImplausibleEdition,
    /// The record declares nothing but its name, package and dependencies,
    /// and does not stand alone (spec 0483 S1).
    DeclaresNothing,
}

/// [`scan`], with the candidates it refused and why. `scan` is this function's
/// `found`, so the two cannot disagree.
pub fn scan_explained(data: &[u8]) -> Explained {
    scan_explained_with(data, wkt_graph(), FDP)
}

/// [`scan_explained`] against the caller's own descriptor schema: `graph`
/// built from it (`reproto --schema-db-out --emit-extension-ranges`), and
/// `root` the full name of its `FileDescriptorProto`.
///
/// For descriptors written against a fuller `descriptor.proto` than the
/// stock one. Load `graph` with `prototext_graph::score::load::load_graph`.
/// Their extra fields are then declared, so the walk checks their wire types
/// and contents instead of reading them as opaque bytes, and an extra
/// top-level field no longer ends the record (spec 0448 S4).
pub fn scan_explained_with(data: &[u8], graph: &LoadedGraph, root: &str) -> Explained {
    scan_explained_with_options(data, graph, root, &ScanOptions::default())
}

/// [`scan_explained`], with options (spec 0449 S7).
pub fn scan_explained_options(data: &[u8], options: &ScanOptions) -> Explained {
    scan_explained_with_options(data, wkt_graph(), FDP, options)
}

/// [`scan_explained_with`], with options (spec 0449 S7).
pub fn scan_explained_with_options(
    data: &[u8],
    graph: &LoadedGraph,
    root: &str,
    options: &ScanOptions,
) -> Explained {
    walk_candidates(data, graph, root, options)
}

// ============================================================================

/// Maximum byte length of a `.proto` file name embedded in a FileDescriptorProto.
/// Used to reject implausibly long strings during scanning.
const MAX_PROTO_NAME_LEN: usize = 200;

/// Returns true if `name` looks like a plausible `.proto` import path: ends
/// in `.proto`, is not absolute, and every `/`-separated component is
/// non-empty, is not `.`/`..`, and is made of `[A-Za-z0-9_.+-]` only (spec
/// 0448 S5).
///
/// Deliberately *not* a legality check: protoc accepts any name the
/// filesystem does (`café.proto`, `a b.proto` and `x$y.proto` all compile).
/// This is about which names are worth believing. Every path in googleapis
/// (7 817 files) fits the set, as do the 7 554 names of a large private
/// corpus. What it refuses is code that happens to end in `.proto` once
/// cut, such as the Java codegen templates in every `libprotoc`
/// (`\n$deprecation$private com.google.proto…`). A real file outside the
/// set is refused visibly: [`scan_explained`] reports it.
fn is_plausible_path(name: &str) -> bool {
    name.ends_with(".proto")
        && !name.starts_with('/')
        && name.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && component
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
        })
}

fn walk_candidates(
    data: &[u8],
    graph: &LoadedGraph,
    root: &str,
    options: &ScanOptions,
) -> Explained {
    let mut result = Explained::default();
    let data_len = data.len();
    let mut offset = 0;
    // Where the last record found ends: a frame starting there chains the
    // next record to it (spec 0496).
    let mut chain_end: Option<usize> = None;

    while offset < data_len {
        if data[offset] == 0x0A {
            if let Some((name_len, varint_len)) = decode_varint(&data[offset + 1..]) {
                let name_start = offset + 1 + varint_len;
                let Some(name_end) = name_start.checked_add(name_len as usize) else {
                    offset += 1;
                    continue;
                };

                if name_end > data_len
                    || name_start > name_end
                    || name_end - name_start > MAX_PROTO_NAME_LEN
                {
                    offset += 1;
                    continue;
                }

                if let Ok(name_str) = std::str::from_utf8(&data[name_start..name_end]) {
                    if name_str.ends_with(".proto") {
                        let verdict = if is_plausible_path(name_str) {
                            score_candidate_with(data, offset, graph, root, options)
                        } else {
                            Err(Reason::ImplausibleName)
                        };
                        match verdict {
                            Ok(Accepted {
                                end: walked_end,
                                order_break,
                            }) => {
                                let (fdp_end, frame) = framed_end(
                                    data, offset, walked_end, chain_end, graph, root, options,
                                );
                                if let Some(frame) = frame {
                                    result.frame_cuts.push(FrameCut {
                                        start: offset,
                                        end: fdp_end,
                                        read_to: walked_end,
                                        frame,
                                    });
                                }
                                chain_end = Some(fdp_end);
                                result.found.push((offset, fdp_end));
                                // The frame's cut stands for what the order
                                // check would have cut.
                                if let Some(at) = order_break.filter(|_| fdp_end == walked_end) {
                                    result.order_cuts.push(OrderCut {
                                        start: offset,
                                        end: fdp_end,
                                        at: offset + at,
                                    });
                                }
                                offset = fdp_end;
                                continue;
                            }
                            Err(reason) => result.rejected.push(Rejected {
                                start: offset,
                                name: name_str.to_string(),
                                reason,
                            }),
                        }
                    }
                }
            }
        }
        offset += 1;
    }

    result
}

/// Score the candidate starting at `start` and return where its record ends,
/// or `None` if it is not a `FileDescriptorProto` (spec 0239 S3, S4).
///
/// The walk is handed **the rest of the buffer**, not a guessed length. That
/// is the whole mechanism: under `Policy::Scan` a root stops at the first tag
/// it cannot carry, and `FileDescriptorProto.name` is singular, so the next
/// record's field-1 tag is the boundary. This replaces a hand-rolled wire
/// walk whose stop rule was that same fact written out by hand, restricted to
/// field 1 — the schema also supplies undeclared field numbers and the other
/// five singular fields, for free.
///
/// The accept rule is on the **defect counters**, never on `score()`, which
/// is a sum over matched fields and so ranges over four orders of magnitude
/// with record size (8 … 171 309 across googleapis.desc). A genuine record
/// has zero defects at every size.
///
/// A veto no longer costs the boundary. Under `Policy::Scan` the walk reports
/// the last offset at which the candidate was clean (spec 0313 S3), so a
/// record followed by bytes that are not a record — the last member of an
/// embedded `FileDescriptorSet`, which used to be lost whole to whatever the
/// linker placed after it — comes back intact. The counters read here are
/// therefore the ones the *recovered* record earned, and the only defect a
/// recovered record can still carry is a `required` field it lacks, charged
/// where it ended.
///
/// What that leaves is the one judgment the walk cannot make, because it is
/// knowledge about `FileDescriptorProto` rather than about protobuf: whether
/// the recovered record is worth reporting as a `.proto` file at all.
///
/// An undeclared field nested in a declared one is no defect (spec 0448
/// S1): the walk reads it and counts it in `undeclared`, which this does not
/// read, so a record written against a fuller `descriptor.proto` keeps its
/// options whole. An undeclared top-level field still ends the record.
fn score_candidate_with(
    data: &[u8],
    start: usize,
    graph: &LoadedGraph,
    root: &str,
    options: &ScanOptions,
) -> Result<Accepted, Reason> {
    let opts = ScoringOpts {
        policy: Policy::Scan,
        increasing_field_numbers: options.increasing_field_numbers,
        ..Default::default()
    };
    let entry = score_one(&data[start..], root, graph.graph(), &opts).ok_or(Reason::Defect)?;
    if entry.vetoed || entry.unknowns != 0 || entry.mismatches != 0 {
        return Err(Reason::Defect);
    }
    let end = start + entry.termination;
    if end == start {
        // Nothing was clean, not even the name: not a record at all, and
        // `stands_alone` would otherwise read the anchor's own `0x0A`.
        return Err(Reason::Defect);
    }
    if !declares_more_than_a_name(&data[start..end]) && !stands_alone(entry.ending) {
        return Err(Reason::NameOnly);
    }
    let end = match dangling_dependency_cut(&data[start..end]) {
        None => end,
        // What is left of a record cut before its dangling index is a name
        // at most, and does not stand alone: the next byte is not a
        // record's end.
        Some(cut) if !declares_more_than_a_name(&data[start..start + cut]) => {
            return Err(Reason::DanglingDependency);
        }
        Some(cut) => start + cut,
    };
    let end = match implausible_edition_cut(&data[start..end]) {
        None => end,
        // As above: a name at most, which doesn't stand alone.
        Some(cut) if !declares_more_than_a_name(&data[start..start + cut]) => {
            return Err(Reason::ImplausibleEdition);
        }
        Some(cut) => start + cut,
    };
    let record = &data[start..end];
    // A record the cuts above shortened no longer ends where the walk did,
    // so it doesn't stand alone either.
    let stood_alone = stands_alone(entry.ending) && end == start + entry.termination;
    if declares_nothing(record) && !stood_alone {
        return Err(Reason::DeclaresNothing);
    }
    // The order rule ends many records exactly where they would have ended
    // anyway, at random bytes that happen to read as a lower field. Only a
    // record the option made *shorter* lost anything, so only that one is
    // reported (spec 0449 S7): the record is scored again without the
    // option, which costs a second walk on these records alone.
    let order_break = entry.order_break.filter(|_| {
        let mut without = options.clone();
        without.increasing_field_numbers = false;
        score_candidate_with(data, start, graph, root, &without)
            .is_ok_and(|unordered| unordered.end > end)
    });
    Ok(Accepted { end, order_break })
}

/// The length a frame at `at` declares, if the bytes from `at` read as one
/// ending at `start`: a tag of wire type 2 (length-delimited), then a varint
/// length. Read forwards, from where the previous record ended: read
/// backwards from `start`, a varint's continuation bytes (0x80 and above)
/// can't be told from the bytes before it. Public for the sets a caller
/// tells apart by their frames (git-seriate-fdp, spec 0495 S2): two records
/// are chained when a frame at the first's end declares the second's length.
pub fn frame_at(data: &[u8], at: usize, start: usize) -> Option<usize> {
    let (tag, tag_len) = decode_varint(data.get(at..start)?)?;
    let (length, length_len) = decode_varint(&data[at + tag_len..start])?;
    (tag & 7 == 2 && tag >> 3 != 0 && at + tag_len + length_len == start)
        .then(|| usize::try_from(length).ok())
        .flatten()
}

/// Where a record the walk ended at `walked_end` really ends (spec 0496):
/// at its frame's declared length when that frame holds up, else where the
/// walk ended it. A record embedded in a larger message can be followed by
/// that message's other fields, which parse as valid fields of a
/// `FileDescriptorProto` (an enclosing field 3 reads as `dependency`), so
/// parsing cleanly doesn't mark a record's end; a frame does.
///
/// The frame is trusted only when it holds up three ways:
/// - **chained:** it starts exactly where the record found before ended,
///   and ends where this record starts, as records of a set do; two stray
///   bytes before a raw descriptor that happen to read as a frame aren't
///   chained to anything;
/// - **inside:** its declared length ends before the walk's end;
/// - **whole:** the prefix of exactly that length is accepted as a record
///   on its own, ending there.
///
/// A set's first record isn't chained, and needs no check: the next record
/// ends it. So a set of one record read on past its end is not cut.
///
/// Or a Python marshal bytes object's header right before it (S5), as a
/// compiled module stores its file's descriptor: whose next constant, the
/// module's name as a marshal short string (`z`, 0x7a), reads as field 15,
/// `option_dependency`. Its length is 4 bytes: stray bytes before a raw
/// descriptor don't declare a length that lands on one of its field
/// boundaries, so it needs no chaining; inside and whole, as above.
fn framed_end(
    data: &[u8],
    start: usize,
    walked_end: usize,
    chain_end: Option<usize>,
    graph: &LoadedGraph,
    root: &str,
    options: &ScanOptions,
) -> (usize, Option<Frame>) {
    let declared = [
        (
            chain_end.and_then(|at| frame_at(data, at, start)),
            Frame::Protobuf,
        ),
        (marshal_length(data, start), Frame::Marshal),
    ];
    for (length, frame) in declared {
        let Some(framed) = length.and_then(|l| start.checked_add(l)) else {
            continue;
        };
        if framed >= walked_end {
            continue;
        }
        if let Ok(prefix) = score_candidate_with(&data[..framed], start, graph, root, options) {
            if prefix.end == framed {
                return (framed, Some(frame));
            }
        }
    }
    (walked_end, None)
}

/// The length a Python marshal bytes object's header right before `start`
/// declares (spec 0496 S5): the type `s` (0x73, or 0xf3 with marshal's
/// reference flag), then a 4-byte little-endian length.
fn marshal_length(data: &[u8], start: usize) -> Option<usize> {
    let header = data.get(start.checked_sub(5)?..start)?;
    if !matches!(header[0], 0x73 | 0xf3) {
        return None;
    }
    usize::try_from(u32::from_le_bytes(header[1..5].try_into().ok()?)).ok()
}

/// A candidate [`score_candidate_with`] accepts.
#[derive(Debug, PartialEq, Eq)]
struct Accepted {
    /// Where the record ends, as an offset in the whole buffer.
    end: usize,
    /// Where the order check ended it, as an offset from the record's start
    /// (spec 0449 S6), when it ended it earlier than the walk without the
    /// check does.
    order_break: Option<usize>,
}

/// Whether a record that ended this way stands alone (spec 0448 S6): at the
/// end of the buffer, at an undeclarable tag such as a NUL, or at a second
/// `name` — the next record. Not when the walk was abandoned, nor at an
/// undeclared field.
///
/// What admits a name-only record — protoc's whole output for an empty file
/// with no package — while still refusing the `.proto` Java package names
/// [`declares_more_than_a_name`] exists for: `java_package` is field 1 of
/// `FileOptions`, and what follows it is more `FileOptions` fields, which
/// either fail to read as a descriptor's or are undeclared in one.
fn stands_alone(ending: Ending) -> bool {
    matches!(
        ending,
        Ending::EndOfBuffer | Ending::OutOfRange | Ending::RepeatedSingular
    )
}

/// Whether `record` carries a top-level field other than `name` (spec 0313
/// S4). A record that does not is still accepted when it stands alone (spec
/// 0448 S6, [`stands_alone`]).
///
/// Structural rather than a threshold on `score()`, which is a sum over
/// matched fields and so ranges over four orders of magnitude with record
/// size (8 … 171 309 across googleapis.desc). This states the thing that is
/// actually wrong with a false positive — it declares no package, no message,
/// no service, not even a syntax — instead of a number that has to be
/// re-justified whenever the corpus or the scoring changes.
///
/// It is needed because cleanliness cannot reject these. Every false positive
/// the scanner faces is a `.proto`-suffixed Java package name:
/// `option java_package = "com.google.cloud.pubsublite.proto"` is field 1 of
/// `FileOptions`, tag `0x0a`, which the anchor cannot tell from a file name.
/// The record recovered from one is a flawless descriptor carrying its name
/// and nothing else.
///
/// One field and not two: `name` + `package` is a real 19-byte descriptor, as
/// is `name` + `dependency` at 31 — a file whose only statement is an
/// `import`. Only the empty `.proto` file yields a descriptor with nothing
/// but its own name.
///
/// The record is known to parse, the walk having just read it, so anything
/// this cannot decode means the caller passed something else; `false` is the
/// safe answer to that.
fn declares_more_than_a_name(record: &[u8]) -> bool {
    let mut pos = 0;
    while pos < record.len() {
        let Some((tag, tag_len)) = decode_varint(&record[pos..]) else {
            return false;
        };
        if tag >> 3 != 1 {
            return true;
        }
        // Field 1 is `name`, a string, so the only wire type it can wear is
        // LEN and its payload is skipped by its own length prefix.
        if tag & 7 != 2 {
            return false;
        }
        pos += tag_len;
        let Some((len, len_len)) = decode_varint(&record[pos..]) else {
            return false;
        };
        let Some(next) = pos
            .checked_add(len_len)
            .and_then(|p| p.checked_add(len as usize))
            .filter(|&p| p <= record.len())
        else {
            return false;
        };
        pos = next;
    }
    false
}

/// Where `record` must end so that every `public_dependency` and
/// `weak_dependency` index in it names one of its `dependency` entries, or
/// `None` if they all do (spec 0449 S1).
///
/// protoc's descriptor builder rejects any other index ("Invalid public
/// dependency index.": protobuf 34.1, `descriptor.cc`, `BuildFileImpl`), so
/// no genuine descriptor has one, and what a cut removes is never the
/// record's own. Two things do carry one:
///
/// - bytes after a record that happen to frame an index field, which the
///   walk cannot tell from the record's (spec 0448 N2): a random `0x58` is a
///   `weak_dependency` tag. Cutting there recovers the record exactly;
///   refusing it would lose a genuine record to its neighbors;
/// - a Python `_pb2` module's separately serialized `FileOptions`, read from
///   a `.pyc`: `java_package` (field 1, a `.proto`-suffixed name) and
///   `java_multiple_files = true` (field 10, 1) are a `name` and a
///   `public_dependency: 1` with no import. The cut leaves its name alone,
///   and the caller refuses that.
///
/// The cut is before the first top-level field holding a dangling index,
/// counting the `dependency` entries before the cut, and is repeated until
/// none dangles: removing fields can only lower the count. Read after the
/// walk, from the recovered record, like [`declares_more_than_a_name`]:
/// knowledge about `FileDescriptorProto`, not about protobuf. Indices are
/// read packed or not; a negative one is a ten-byte varint, so as an
/// unsigned value it is past any count. The record is known to parse, so
/// anything this cannot decode means the caller passed something else, and
/// `None`, cutting nothing, is the safe answer to that.
fn dangling_dependency_cut(record: &[u8]) -> Option<usize> {
    let fields = top_level_fields(record)?;
    // The largest index each field holds, if it is an index field.
    let mut max_index: Vec<Option<u64>> = Vec::with_capacity(fields.len());
    for f in &fields {
        let mut max = None;
        if f.number == FDP_PUBLIC_DEPENDENCY || f.number == FDP_WEAK_DEPENDENCY {
            match f.wire {
                0 => max = f.varint,
                2 => {
                    let payload = &record[f.payload.clone()];
                    let mut p = 0;
                    while p < payload.len() {
                        let (value, len) = decode_varint(&payload[p..])?;
                        max = max.max(Some(value));
                        p += len;
                    }
                }
                _ => {}
            }
        }
        max_index.push(max);
    }

    let mut kept = fields.len();
    loop {
        let dependencies = fields[..kept]
            .iter()
            .filter(|f| f.number == FDP_DEPENDENCY && f.wire == 2)
            .count() as u64;
        let Some(first) = max_index[..kept]
            .iter()
            .position(|m| m.is_some_and(|i| i >= dependencies))
        else {
            break;
        };
        kept = first;
    }
    (kept < fields.len()).then(|| fields[kept].start)
}

/// Whether `record` declares nothing but its name, its package and its
/// dependencies (spec 0483 S1): no message, enum, service, extension,
/// option, `syntax` or `edition`. Such a record is real only standing
/// alone, as a file that only imports, with nothing after it to cut it;
/// the rule of spec 0448 S6 for a name alone, extended. Measured before
/// deciding, on 27 234 real descriptors (seriate-tuto's corpora, the
/// protoback harness corpus with googleapis, and a downstream user's
/// history): 5 such records, all files that only re-export their imports,
/// which stand alone where they occur.
fn declares_nothing(record: &[u8]) -> bool {
    top_level_fields(record).is_some_and(|fields| {
        fields.iter().all(|f| {
            matches!(
                f.number,
                FDP_NAME
                    | FDP_PACKAGE
                    | FDP_DEPENDENCY
                    | FDP_PUBLIC_DEPENDENCY
                    | FDP_WEAK_DEPENDENCY
            )
        })
    })
}

/// Where `record` must end so that its `edition` is one a writer sets, or
/// `None` if it is (spec 0456 S1).
///
/// A descriptor sets `edition` only in an editions file, beside `syntax:
/// "editions"`, and only to a released edition ([`WRITTEN_EDITIONS`]); the
/// descriptor pool refuses anything else (`Setting edition requires that
/// syntax="editions"`). Field 14's tag is `0x70`, a `p`, common after a
/// record by chance, so this cuts rather than refuses, as
/// [`dangling_dependency_cut`] does: before the first implausible
/// `edition`. What else sets one is a string ending in `.proto` (a Java
/// package) followed by `p` and a small byte: the cut leaves its name
/// alone, and the caller refuses that.
fn implausible_edition_cut(record: &[u8]) -> Option<usize> {
    let fields = top_level_fields(record)?;
    let editions_syntax = fields.iter().any(|f| {
        f.number == FDP_SYNTAX && f.wire == 2 && record[f.payload.clone()] == *b"editions"
    });
    fields
        .iter()
        .find(|f| {
            f.number == FDP_EDITION
                && f.wire == 0
                && !(editions_syntax && f.varint.is_some_and(|v| WRITTEN_EDITIONS.contains(&v)))
        })
        .map(|f| f.start)
}

/// One top-level field of a record: where it starts, its number and wire
/// type, its value if a varint, its payload's range if length-delimited.
struct TopField {
    start: usize,
    number: u64,
    wire: u64,
    varint: Option<u64>,
    payload: std::ops::Range<usize>,
}

/// The top-level fields of `record`, `None` if it doesn't parse: the record
/// is known to parse, so that means the caller passed something else, and
/// the cuts then cut nothing.
fn top_level_fields(record: &[u8]) -> Option<Vec<TopField>> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < record.len() {
        let start = pos;
        let (tag, tag_len) = decode_varint(&record[pos..])?;
        pos += tag_len;
        let (mut varint, mut payload) = (None, pos..pos);
        match tag & 7 {
            0 => {
                let (value, len) = decode_varint(&record[pos..])?;
                pos += len;
                varint = Some(value);
            }
            1 => pos += 8,
            5 => pos += 4,
            2 => {
                let (len, len_len) = decode_varint(&record[pos..])?;
                let end = (pos + len_len)
                    .checked_add(len as usize)
                    .filter(|&end| end <= record.len())?;
                payload = pos + len_len..end;
                pos = end;
            }
            _ => return None,
        }
        fields.push(TopField {
            start,
            number: tag >> 3,
            wire: tag & 7,
            varint,
            payload,
        });
    }
    (pos == record.len()).then_some(fields)
}

fn decode_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut result = 0u64;
    let mut shift = 0;
    for (i, &b) in data.iter().enumerate() {
        result |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
        if shift > 64 {
            return None;
        }
    }
    None
}

// The unit tests are this library compiled as a test binary, which runs
// Python and so links libpython; the extension itself does not. See build.rs
// (spec 0402 S6).

#[cfg(test)]
mod tests {
    use super::*;

    /// A candidate scored against the embedded schema, as [`scan`] does.
    fn score_candidate(data: &[u8], start: usize) -> Result<usize, Reason> {
        score_candidate_with(data, start, wkt_graph(), FDP, &ScanOptions::default())
            .map(|accepted| accepted.end)
    }

    /// A message carrying just field 1, `name` — a `FileDescriptorProto` or a
    /// `DescriptorProto` depending on where it is put.
    ///
    /// Not usable on its own as a scanner fixture: a record with no top-level
    /// field but its name is refused (spec 0313 S4). Use [`make_fdp`].
    fn name_field(name: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        // field 1, wire type 2 (length-delimited)
        buf.push(0x0A);
        // varint length of name
        let name_bytes = name.as_bytes();
        let mut len = name_bytes.len() as u64;
        loop {
            let byte = (len & 0x7F) as u8;
            len >>= 7;
            if len == 0 {
                buf.push(byte);
                break;
            } else {
                buf.push(byte | 0x80);
            }
        }
        buf.extend_from_slice(name_bytes);
        buf
    }

    /// The smallest `FileDescriptorProto` the scanner will report: a `name`
    /// and a `package`, which is what a real 19-byte descriptor looks like.
    /// It declares nothing, so it is reported only standing alone (spec
    /// 0483 S1): where bytes after it cut it, use [`make_declaring_fdp`].
    fn make_fdp(name: &str) -> Vec<u8> {
        let mut buf = name_field(name);
        buf.extend_from_slice(&framed(2, b"p"));
        buf
    }

    /// [`make_fdp`] with a message `M`: a record that declares something,
    /// and so is kept when the bytes after it cut it (spec 0483 S1).
    fn make_declaring_fdp(name: &str) -> Vec<u8> {
        let mut buf = make_fdp(name);
        buf.extend_from_slice(&framed(4, &framed(1, b"M")));
        buf
    }

    #[test]
    fn test_fdp_with_null_separator() {
        // Two FDPs separated by a 0x00 byte — the classic terminator case.
        let fdp1 = make_fdp("foo.proto");
        let fdp2 = make_fdp("bar.proto");
        let mut buf = fdp1.clone();
        buf.push(0x00);
        let sep = 1;
        buf.extend_from_slice(&fdp2);

        let ranges = scan(&buf);

        assert_eq!(ranges.len(), 2, "expected two FDP ranges, got {ranges:?}");
        assert_eq!(ranges[0], (0, fdp1.len()));
        assert_eq!(ranges[1], (fdp1.len() + sep, buf.len()));
    }

    #[test]
    fn test_truncated_fdp_not_returned() {
        // A FDP whose name field is cut off mid-string — should not be
        // returned. `name_field`, not `make_fdp`, so that the bytes removed
        // are the name's own and the test still says what it says.
        let fdp = name_field("foo.proto");
        let truncated = &fdp[..fdp.len() - 3]; // cut off last 3 bytes of name

        let ranges = scan(truncated);

        assert!(
            ranges.is_empty(),
            "truncated FDP should not be returned, got {ranges:?}"
        );
    }

    #[test]
    fn test_garbage_name_ending_in_proto_rejected() {
        // Real-world false positive: an HTML/Go-template fragment whose
        // trailing bytes happen to end in ".proto".  The outer (garbage,
        // leading-`/`) span must never be accepted as a genuine FDP name
        // (spec 0105).
        //
        // Note: this exact garbage string embeds a `\n` immediately
        // followed by bytes that coincidentally decode as a valid varint
        // length matching a genuinely plausible trailing substring
        // (`google/api/expr/v1alpha1/value.proto`, 36 bytes).  So
        // `scan()` does *not* return an empty list here — it
        // correctly rejects the outer garbage span and finds this
        // coincidental but genuinely clean embedded name instead, which
        // is safe to accept (no leading `/`, no control characters).
        let garbage = "/p>\n{{- end}}\n{{end}}\n\n$google/api/expr/v1alpha1/value.proto";
        let fdp = make_fdp(garbage);

        let ranges = scan(&fdp);

        assert!(
            !ranges.contains(&(0, fdp.len())),
            "full garbage span should never be accepted as a candidate, got {ranges:?}"
        );
    }

    #[test]
    fn test_simple_garbage_name_rejected() {
        // Simpler garbage string with no embedded coincidental valid
        // substring: scan() should return no candidates at all.
        let garbage = "/p>\n{{- end}}\n{{end}}\n\nnot/a/real/path.proto";
        let fdp = make_fdp(garbage);

        let ranges = scan(&fdp);

        assert!(
            ranges.is_empty(),
            "garbage name ending in .proto should not be accepted, got {ranges:?}"
        );
    }

    #[test]
    fn test_plausible_path_accepted() {
        // Sanity check: a genuine canonical import path is still accepted,
        // including non-ASCII names.
        assert!(is_plausible_path("google/protobuf/descriptor.proto"));
        assert!(is_plausible_path("foo/bar_baz-qux.proto"));
        assert!(is_plausible_path("simple.proto"));
        assert!(is_plausible_path("a+b/c.d-e_f.proto"));
    }

    #[test]
    fn test_non_plausible_path_rejected() {
        assert!(!is_plausible_path("/absolute/path.proto"));
        assert!(!is_plausible_path("foo//bar.proto"));
        assert!(!is_plausible_path("./foo.proto"));
        assert!(!is_plausible_path("../foo.proto"));
        assert!(!is_plausible_path("foo/../bar.proto"));
        assert!(!is_plausible_path("not_a_proto_file"));
        assert!(!is_plausible_path("foo\nbar.proto"));
    }

    /// Spec 0448 S5. Legal names protoc accepts, refused as implausible: the
    /// set is `[A-Za-z0-9_.+-]` per component. `café.proto` used to pass.
    #[test]
    fn names_outside_the_set_are_implausible() {
        for name in [
            "café.proto",
            "a b.proto",
            "x$y.proto",
            "deprecation$private com.google.proto",
            "   PARSER = new com.google.proto",
        ] {
            assert!(!is_plausible_path(name), "{name:?}");
        }
    }

    /// Spec 0448 S7: an implausible name is reported, not silently dropped,
    /// and a libprotoc template is one.
    #[test]
    fn a_codegen_template_is_reported_as_an_implausible_name() {
        // `$` is the length, 36, so the name starts at `deprecation`.
        let template = b"\n$deprecation$private com.google.protobuf.MapFieldBuilder<";
        let explained = scan_explained(template);
        assert!(explained.found.is_empty());
        assert_eq!(
            explained.rejected,
            vec![Rejected {
                start: 0,
                name: "deprecation$private com.google.proto".to_string(),
                reason: Reason::ImplausibleName,
            }]
        );
    }

    #[test]
    fn test_consecutive_fdps_split_correctly() {
        // Two consecutive FDPs with no 0x00 separator between them.
        let fdp1 = make_fdp("foo.proto");
        let fdp2 = make_fdp("bar.proto");
        let mut buf = fdp1.clone();
        buf.extend_from_slice(&fdp2);

        let ranges = scan(&buf);

        assert_eq!(
            ranges.len(),
            2,
            "expected two separate FDP ranges, got {ranges:?}"
        );
        assert_eq!(ranges[0], (0, fdp1.len()));
        assert_eq!(ranges[1], (fdp1.len(), buf.len()));
    }

    /// Encode `payload` as one length-delimited field-`number` record.
    fn framed(number: u32, payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        for mut v in [u64::from(number) << 3 | 2, payload.len() as u64] {
            loop {
                let byte = (v & 0x7F) as u8;
                v >>= 7;
                if v == 0 {
                    buf.push(byte);
                    break;
                }
                buf.push(byte | 0x80);
            }
        }
        buf.extend_from_slice(payload);
        buf
    }

    /// A set of descriptors as a larger message holds them (spec 0496):
    /// each record framed as field 1, then the message's own field 3, two
    /// strings, which read as a `FileDescriptorProto`'s `dependency`.
    fn enclosing_message(records: &[&[u8]]) -> (Vec<u8>, Vec<(usize, usize)>) {
        let mut buf = vec![0u8; 8];
        let mut ranges = Vec::new();
        for record in records {
            let framed_record = framed(1, record);
            let header = framed_record.len() - record.len();
            ranges.push((buf.len() + header, buf.len() + framed_record.len()));
            buf.extend_from_slice(&framed_record);
        }
        buf.extend_from_slice(&framed(3, b"src/a.sdl"));
        buf.extend_from_slice(&framed(3, b"src/b.sdl"));
        (buf, ranges)
    }

    #[test]
    fn a_chained_frame_ends_the_last_record_of_a_set() {
        let (a, b) = (
            make_declaring_fdp("x/a.proto"),
            make_declaring_fdp("x/b.proto"),
        );
        let (data, ranges) = enclosing_message(&[&a, &b]);
        let explained = scan_explained(&data);
        assert_eq!(
            explained.found, ranges,
            "the last record read on past its frame"
        );
        assert_eq!(
            explained.frame_cuts,
            [FrameCut {
                start: ranges[1].0,
                end: ranges[1].1,
                read_to: data.len(),
                frame: Frame::Protobuf,
            }]
        );
    }

    #[test]
    fn a_set_of_one_record_is_not_cut() {
        // Its frame is chained to nothing: as a stray pair of bytes would
        // be (spec 0496's limit).
        let a = make_declaring_fdp("x/a.proto");
        let (data, ranges) = enclosing_message(&[&a]);
        let explained = scan_explained(&data);
        assert_eq!(explained.found, [(ranges[0].0, data.len())]);
        assert!(explained.frame_cuts.is_empty());
    }

    #[test]
    fn bytes_that_read_as_a_frame_before_a_raw_record_are_no_frame() {
        // A raw record with an import, preceded by two bytes that read as a
        // frame whose length ends right before the import: whole, inside,
        // but chained to nothing. The record keeps its import.
        let mut record = make_declaring_fdp("x/a.proto");
        let prefix = record.len();
        record.extend_from_slice(&framed(3, b"x/dep.proto"));
        let mut data = vec![0u8; 8];
        data.extend_from_slice(&[0x12, u8::try_from(prefix).unwrap()]);
        let start = data.len();
        data.extend_from_slice(&record);
        let explained = scan_explained(&data);
        assert_eq!(explained.found, [(start, data.len())]);
        assert!(explained.frame_cuts.is_empty());
    }

    #[test]
    fn a_set_its_frames_agree_with_reports_no_cut() {
        let (a, b) = (
            make_declaring_fdp("x/a.proto"),
            make_declaring_fdp("x/b.proto"),
        );
        let mut data = vec![0u8; 8];
        let mut ranges = Vec::new();
        for record in [&a, &b] {
            let framed_record = framed(1, record);
            let header = framed_record.len() - record.len();
            ranges.push((data.len() + header, data.len() + framed_record.len()));
            data.extend_from_slice(&framed_record);
        }
        data.push(0); // a NUL after the set: no field to read on into
        let explained = scan_explained(&data);
        assert_eq!(explained.found, ranges);
        assert!(explained.frame_cuts.is_empty());
    }

    /// A compiled Python module's constants, as marshal writes them (spec
    /// 0496 S5): `record` as a bytes object (type `type_byte`, a 4-byte
    /// little-endian `declared` length), then the module's name as a short
    /// string (`z`, which reads as field 15, `option_dependency`). Where the
    /// record starts, and the data.
    fn compiled_module(record: &[u8], type_byte: u8, declared: usize) -> (usize, Vec<u8>) {
        let mut data = vec![0xe3, 0, 0, 0, 0]; // a code object's head, roughly
        data.push(type_byte);
        data.extend_from_slice(&u32::try_from(declared).unwrap().to_le_bytes());
        let start = data.len();
        data.extend_from_slice(record);
        let name = b"x.a_pb2";
        data.push(0x7a);
        data.push(u8::try_from(name.len()).unwrap());
        data.extend_from_slice(name);
        data.push(0x29); // a tuple of constants, next
        (start, data)
    }

    #[test]
    fn a_marshal_frame_ends_a_compiled_modules_descriptor() {
        let record = make_declaring_fdp("x/a.proto");
        for type_byte in [0x73, 0xf3] {
            let (start, data) = compiled_module(&record, type_byte, record.len());
            let explained = scan_explained(&data);
            assert_eq!(
                explained.found,
                [(start, start + record.len())],
                "{type_byte:#x}"
            );
            assert_eq!(explained.frame_cuts.len(), 1);
            assert_eq!(explained.frame_cuts[0].frame, Frame::Marshal);
            assert!(explained.frame_cuts[0].read_to > start + record.len());
        }
    }

    #[test]
    fn a_marshal_length_off_the_records_boundaries_is_no_frame() {
        // Declaring one byte less than the record: inside, but not whole.
        let record = make_declaring_fdp("x/a.proto");
        let (start, data) = compiled_module(&record, 0x73, record.len() - 1);
        let explained = scan_explained(&data);
        assert!(explained.frame_cuts.is_empty());
        assert!(
            explained.found[0].1 > start + record.len(),
            "read on, as before"
        );
        // No marshal header at all: the same.
        let (_, data) = compiled_module(&record, 0x00, record.len());
        assert!(scan_explained(&data).frame_cuts.is_empty());
    }

    #[test]
    fn frame_at_reads_a_tag_and_a_length() {
        let data = [0xff, 0x0a, 0x96, 0x01, 0x0a];
        assert_eq!(frame_at(&data, 1, 4), Some(150));
        // Not ending at the record, of another wire type, or no tag: no frame.
        assert_eq!(frame_at(&data, 1, 3), None);
        assert_eq!(frame_at(&[0x08, 0x05, 0x0a], 0, 2), None);
        assert_eq!(frame_at(&[0x05, 0x0a], 0, 1), None);
    }

    /// A FileDescriptorProto carrying `messages` empty `message_type` entries
    /// (field 4), so that its score grows with `messages` while its defect
    /// counters stay at zero.
    fn make_fdp_with_messages(name: &str, messages: usize) -> Vec<u8> {
        let mut buf = make_fdp(name);
        for i in 0..messages {
            let descriptor = name_field(&format!("M{i}")); // DescriptorProto.name
            buf.extend_from_slice(&framed(4, &descriptor));
        }
        buf
    }

    /// The corpus `googleapis.desc`, or `None` when the env var is unset.
    ///
    /// The corpus is a Nix store artifact, not part of `workspaceSrc`, so the
    /// tests that need it are `#[ignore]`d rather than conditionally skipped.
    fn corpus() -> Vec<u8> {
        let path = std::env::var("PROTOSCAN_CORPUS_DESC")
            .expect("set PROTOSCAN_CORPUS_DESC to a googleapis.desc path");
        std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read '{path}': {e}"))
    }

    /// Ground truth: the `(start, end)` of every `file` payload, read off the
    /// `FileDescriptorSet` framing — independent of both the old stop rule and
    /// the new one (spec 0239 test 1).
    fn framing_boundaries(data: &[u8]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut offset = 0;
        while offset < data.len() {
            assert_eq!(data[offset], 0x0A, "not a FileDescriptorSet at {offset}");
            let (len, varint_len) =
                decode_varint(&data[offset + 1..]).expect("truncated length prefix");
            let start = offset + 1 + varint_len;
            let end = start + len as usize;
            out.push((start, end));
            offset = end;
        }
        out
    }

    #[test]
    #[ignore = "needs a googleapis.desc in PROTOSCAN_CORPUS_DESC"]
    fn test_scan_finds_every_record() {
        let data = corpus();
        let expected = framing_boundaries(&data);
        let found = scan(&data);
        assert_eq!(found, expected, "boundaries disagree with the framing");
    }

    #[test]
    #[ignore = "needs a googleapis.desc in PROTOSCAN_CORPUS_DESC"]
    fn test_a_record_never_swallows_its_successor() {
        // The bug this spec fixes: handed the whole 25.6 MB buffer, the first
        // record must report its own length. `FileDescriptorProto.name` is
        // singular, so the next record's field-1 tag terminates the walk
        // (spec 0238 S12 rule 2).
        let data = corpus();
        let (start, end) = framing_boundaries(&data)[0];
        assert_eq!(
            score_candidate(&data, start),
            Ok(end),
            "the first record swallowed its successors"
        );
    }

    #[test]
    fn test_accept_rule_is_size_independent() {
        // The accept rule reads the defect counters, never `score()`, which is
        // a sum over matched fields and so grows without bound with record
        // size (spec 0239 G2/S4). Both of these are genuine records; a
        // threshold on `score()` would have to admit both.
        for messages in [0, 2000] {
            let fdp = make_fdp_with_messages("size/independence.proto", messages);
            assert_eq!(
                score_candidate(&fdp, 0),
                Ok(fdp.len()),
                "an FDP with {messages} message_type entries was rejected"
            );
        }
    }

    // ── Spec 0313: a record ends at its last clean boundary ──────────────────

    /// Spec 0313 test 1, in its synthetic form: a complete record followed by
    /// arbitrary bytes is still recovered, at its true end.
    ///
    /// Both tails, because the pair is what killed the narrow fix. `0x77` is
    /// wire type 7, which no tag can wear, so the walk dies parsing it.
    /// `0x18 0x01` is a perfectly legal tag for `dependency` — declared, and
    /// repeated, so spec 0238 S12's lookahead lets it through — carrying the
    /// wrong wire type, so the walk dies a field later and somewhere else. A
    /// rule keyed on either death recovers one record and loses the other.
    #[test]
    fn a_record_followed_by_rubbish_is_still_recovered() {
        for tail in [&[0x77u8][..], &[0x18, 0x01][..]] {
            let fdp = make_declaring_fdp("foo.proto");
            let mut buf = fdp.clone();
            buf.extend_from_slice(tail);

            assert_eq!(
                scan(&buf),
                vec![(0, fdp.len())],
                "trailing bytes {tail:02x?}"
            );
        }
    }

    /// Spec 0313 test 5 / N1. A cut record is reported as the last depth-0
    /// boundary before the cut. That is the accepted consequence of having no
    /// rule of its own for truncation: what comes back is never itself a cut
    /// record — every field in it was read whole — but nothing says the source
    /// was longer.
    ///
    /// And when the cut lands before the record's second depth-0 field, the
    /// structural floor refuses it outright rather than handing back a stub
    /// (S4).
    #[test]
    fn a_cut_record_reports_its_clean_prefix() {
        let whole = make_fdp_with_messages("foo/bar.proto", 3);
        let two = make_fdp_with_messages("foo/bar.proto", 2);
        let cut = &whole[..whole.len() - 3]; // most of the third entry gone

        assert_eq!(
            scan(cut),
            vec![(0, two.len())],
            "the boundary before the cut, not the cut"
        );

        let name_and_package = make_fdp("foo/bar.proto");
        let cut = &name_and_package[..name_and_package.len() - 1];
        assert!(
            scan(cut).is_empty(),
            "a clean prefix of one field is not a descriptor"
        );
    }

    /// Spec 0313 test 3 / S4, as spec 0448 S6 narrows it. The false positive
    /// the scanner actually faces: `option java_package =
    /// "com.google.cloud.pubsublite.proto"` is field 1 of `FileOptions` — tag
    /// `0x0a`, a plausible path, and at the anchor indistinguishable from a
    /// file name. Its first boundary is a flawless one-field descriptor, so
    /// cleanliness cannot reject it; what gives it away is what follows it,
    /// more `FileOptions` fields.
    #[test]
    fn a_java_package_name_is_not_a_descriptor() {
        let stub = name_field("com.google.cloud.pubsublite.proto");
        let mut in_options = stub.clone();
        in_options.extend_from_slice(&framed(8, b"Proto")); // java_outer_classname
        assert_eq!(score_candidate(&in_options, 0), Err(Reason::NameOnly));
        let explained = scan_explained(&in_options);
        assert!(explained.found.is_empty());
        assert_eq!(explained.rejected[0].reason, Reason::NameOnly);

        // One further depth-0 field and the same anchor is a real, if tiny,
        // descriptor: the floor is one field, not two.
        let mut real = stub.clone();
        real.extend_from_slice(&framed(2, b"pkg"));
        assert_eq!(score_candidate(&real, 0), Ok(real.len()));
    }

    /// Spec 0448 S6. A name-only record — protoc's whole output for an empty
    /// file with no package — is found when it stands alone: at the end of
    /// the buffer, before a NUL, or before the next record.
    #[test]
    fn a_standalone_name_only_record_is_found() {
        let empty = name_field("empty/e.proto");
        assert_eq!(scan(&empty), vec![(0, empty.len())], "at the end");

        let mut nul = empty.clone();
        nul.push(0);
        assert_eq!(scan(&nul), vec![(0, empty.len())], "before a NUL");

        let next = make_fdp("next.proto");
        let mut pair = empty.clone();
        pair.extend_from_slice(&next);
        assert_eq!(
            scan(&pair),
            vec![(0, empty.len()), (empty.len(), pair.len())],
            "before the next record"
        );

        let mut rubbish = empty.clone();
        rubbish.push(0x77);
        assert!(scan(&rubbish).is_empty(), "before other bytes: refused");
    }

    /// A descriptor with one field more than the stock `descriptor.proto`
    /// declares, nested at `options` or at the top, its fields in increasing
    /// number order as protoc writes them.
    fn with_undeclared(nested: bool) -> Vec<u8> {
        let mut fdp = make_fdp("undeclared.proto");
        if nested {
            // FileOptions field 5: not declared, outside its extension ranges.
            fdp.extend_from_slice(&framed(8, &framed(5, b"x")));
        }
        fdp.extend_from_slice(&framed(12, b"proto3")); // syntax
        if !nested {
            // FileDescriptorProto has no extension ranges: field 171 is out.
            fdp.extend_from_slice(&framed(171, b"x"));
        }
        fdp
    }

    /// Spec 0448 S1. An undeclared field nested in a declared one no longer
    /// cuts the record, whatever follows it.
    #[test]
    fn an_undeclared_nested_field_is_kept() {
        let fdp = with_undeclared(true);
        for tail in [&[][..], &[0x00][..], &[0x77][..], &[0x18, 0x01][..]] {
            let mut buf = fdp.clone();
            buf.extend_from_slice(tail);
            assert_eq!(scan(&buf), vec![(0, fdp.len())], "tail {tail:02x?}");
        }
        let opts = ScoringOpts {
            policy: Policy::Scan,
            ..Default::default()
        };
        let e = score_one(&fdp, FDP, wkt_graph().graph(), &opts).expect("a root");
        assert_eq!((e.undeclared, e.unknowns), (1, 0));
    }

    /// Spec 0448 S2. An undeclared top-level field still ends the record:
    /// its bytes cannot be told from whatever follows the record, and an
    /// opaque length-delimited one can claim any length.
    #[test]
    fn an_undeclared_top_level_field_ends_the_record() {
        let fdp = with_undeclared(false);
        let before = fdp.len() - framed(171, b"x").len();
        assert_eq!(scan(&fdp), vec![(0, before)]);
    }

    /// A vendor `descriptor.proto` declaring more than the stock one: a
    /// `FileOptions` field 5 and a top-level field 171, as scoring YAML the
    /// way reproto emits it (spec 0448 S4).
    fn vendor_graph() -> &'static LoadedGraph {
        static GRAPH: OnceLock<LoadedGraph> = OnceLock::new();
        GRAPH.get_or_init(|| {
            let yaml = "extension_ranges: true\nentries:\n- vendor.FileDescriptorProto\n\
                messages:\n  vendor.FileDescriptorProto:\n    fields:\n\
                \x20   - number: 1\n      type: string\n\
                \x20   - number: 2\n      type: string\n\
                \x20   - number: 8\n      type: message\n      child: vendor.FileOptions\n\
                \x20   - number: 12\n      type: string\n\
                \x20   - number: 171\n      type: bytes\n\
                \x20 vendor.FileOptions:\n    fields:\n\
                \x20   - number: 1\n      type: string\n\
                \x20   - number: 5\n      type: string\n    ext_ranges: [[1000, 536870911]]\n";
            let (bytes, _, _) = prototext_graph::build_scoring_graph::build_from_strings(
                &[yaml.to_string()],
                false,
                false,
                |_, _| {},
            )
            .expect("the vendor graph builds");
            LoadedGraph::from_static_bytes(Box::leak(bytes.into_boxed_slice()))
                .expect("the vendor graph loads")
        })
    }

    /// Spec 0448 S4. Against the caller's schema, the fields the stock one
    /// lacks are declared: both records come back whole, a top-level field
    /// 171 included, and nothing is read as undeclared.
    #[test]
    fn a_callers_schema_declares_what_the_stock_one_lacks() {
        const ROOT: &str = "vendor.FileDescriptorProto";
        for nested in [true, false] {
            let fdp = with_undeclared(nested);
            let mut buf = fdp.clone();
            buf.push(0x77); // rubbish after the record
            let found = scan_explained_with(&buf, vendor_graph(), ROOT).found;
            assert_eq!(found, vec![(0, fdp.len())], "nested={nested}");

            let opts = ScoringOpts {
                policy: Policy::Scan,
                ..Default::default()
            };
            let e = score_one(&fdp, ROOT, vendor_graph().graph(), &opts).expect("a root");
            assert_eq!((e.undeclared, e.unknowns), (0, 0), "nested={nested}");
        }
        // The stock schema, for contrast, ends the record before field 171.
        let fdp = with_undeclared(false);
        assert!(scan(&fdp)[0].1 < fdp.len());
    }

    /// Spec 0448 S4. A wrong wire type on a field only the caller's schema
    /// declares is now caught, where the stock schema would read it blind.
    #[test]
    fn a_callers_schema_checks_the_fields_it_adds() {
        // `syntax` declares something (spec 0483 S1), and this vendor schema
        // declares it, where it has no `message_type`.
        let mut fdp = make_fdp("vendor.proto");
        fdp.extend_from_slice(&framed(12, b"proto3"));
        let kept = fdp.len();
        fdp.extend_from_slice(&framed(8, &[0x28, 0x01])); // FileOptions 5 as a varint
        let found = scan_explained_with(&fdp, vendor_graph(), "vendor.FileDescriptorProto").found;
        assert_eq!(found, vec![(0, kept)]);
        assert_eq!(scan(&fdp), vec![(0, fdp.len())], "stock: read blind, kept");
    }

    /// Spec 0448 S7: `Defect` is the walk's refusal.
    #[test]
    fn a_defect_is_reported() {
        // `package` with the wrong wire type right after `name`: the walk
        // vetoes there, and what is left is a name-only stub followed by
        // other bytes.
        let mut bad = name_field("a.proto");
        bad.extend_from_slice(&[0x10, 0x01]); // package (string) as a varint
        let explained = scan_explained(&bad);
        assert!(explained.found.is_empty());
        assert_eq!(explained.rejected.len(), 1);
        assert_eq!(explained.rejected[0].reason, Reason::NameOnly);

        // A record still dirty when the buffer ends is refused: an anomaly
        // is reported at the next depth-0 boundary (spec 0313 S3), and the
        // end of the buffer is none. Here a field number 0, which no schema
        // can declare, inside `options`, the record's last field.
        let mut fdp = make_fdp("defect.proto");
        fdp.extend_from_slice(&framed(8, &[0x00, 0x00]));
        let explained = scan_explained(&fdp);
        assert_eq!(explained.found, vec![], "{explained:?}");
        assert_eq!(explained.rejected[0].reason, Reason::Defect);
    }

    fn varint(mut v: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                buf.push(byte);
                return buf;
            }
            buf.push(byte | 0x80);
        }
    }

    fn varint_field(number: u32, value: u64) -> Vec<u8> {
        let mut buf = varint(u64::from(number) << 3);
        buf.extend(varint(value));
        buf
    }

    /// Spec 0449 S1. A Python module's `FileOptions`, `java_package` and
    /// `java_multiple_files = true`, read as a descriptor: a `name` and a
    /// `public_dependency: 1` with no import. Refused alone and between
    /// random bytes.
    #[test]
    fn test_scan_refuses_a_dangling_public_dependency() {
        let mut options = name_field("com.example.decoy.proto");
        options.extend(varint_field(10, 1));
        for (before, after) in [(&b""[..], &b""[..]), (&b"\x91\x07"[..], &b"\x77\x13"[..])] {
            let mut buf = before.to_vec();
            buf.extend_from_slice(&options);
            buf.extend_from_slice(after);
            let explained = scan_explained(&buf);
            assert!(explained.found.is_empty(), "{explained:?}");
            assert_eq!(explained.rejected.len(), 1);
            assert_eq!(explained.rejected[0].start, before.len());
            assert_eq!(explained.rejected[0].reason, Reason::DanglingDependency);
        }
    }

    /// Spec 0456 S1. A `.proto` string, then `0x70 0x02`: a name and an
    /// `edition: 2`, refused alone and between random bytes. An editions
    /// record is kept whole, and cut before the same bytes after it; an
    /// `edition` without `syntax: "editions"` is cut.
    /// A template bundle's record (spec 0483 Background): a `.proto` path,
    /// its package, its Java outer class in field 3, then message names in
    /// field 7, which don't read as `FieldDescriptorProto`s.
    fn template_record(name: &str) -> Vec<u8> {
        let mut buf = name_field(name);
        buf.extend_from_slice(&framed(2, b"y"));
        buf.extend_from_slice(&framed(3, b"com.example.protos.y.Runtime"));
        for message in ["Alpha", "Beta"] {
            buf.extend_from_slice(&framed(7, message.as_bytes()));
        }
        buf
    }

    #[test]
    fn a_template_record_is_refused() {
        let data = template_record("x/y/proto/runtime.proto");
        let explained = scan_explained(&data);
        assert!(explained.found.is_empty(), "{explained:?}");
        assert_eq!(explained.rejected[0].reason, Reason::DeclaresNothing);
    }

    /// Spec 0483 N2: a dependency is not held to the name rule. Real
    /// descriptors import files that no `.proto` source wrote, compiled by
    /// other compilers: a schema language's `.sdl`, the old `.protodevel`.
    #[test]
    fn a_dependency_need_not_end_in_dot_proto() {
        let mut fdp = make_declaring_fdp("x/db.proto");
        fdp.extend_from_slice(&framed(3, b"x/schemas/sys_root.sdl"));
        fdp.extend_from_slice(&framed(3, b"x/proto/y.protodevel"));
        fdp.push(0);
        assert_eq!(scan(&fdp), [(0, fdp.len() - 1)]);
    }

    #[test]
    fn a_cut_record_that_declares_nothing_is_refused() {
        // A name and a package, then field 7 strings that cut the walk.
        let mut data = name_field("x/y/proto/runtime.proto");
        data.extend_from_slice(&framed(2, b"y"));
        data.extend_from_slice(&framed(7, b"Alpha"));
        let explained = scan_explained(&data);
        assert!(explained.found.is_empty(), "{explained:?}");
        assert_eq!(explained.rejected[0].reason, Reason::DeclaresNothing);
    }

    #[test]
    fn an_imports_only_file_standing_alone_is_found() {
        let mut fdp = name_field("x/reexport.proto");
        fdp.extend_from_slice(&framed(2, b"x"));
        fdp.extend_from_slice(&framed(3, b"x/a.proto"));
        fdp.extend_from_slice(&framed(3, b"x/b.proto"));
        // At the end of the buffer, and before a NUL.
        assert_eq!(scan(&fdp), [(0, fdp.len())]);
        let mut data = fdp.clone();
        data.push(0);
        assert_eq!(scan(&data), [(0, fdp.len())]);
    }

    #[test]
    fn test_scan_cuts_an_implausible_edition() {
        let mut found = name_field("com.example.crawl.proto");
        found.extend(varint_field(14, 2));
        for (before, after) in [(&b""[..], &b""[..]), (&b"\x91\x07"[..], &b"\x77\x13"[..])] {
            let mut buf = before.to_vec();
            buf.extend_from_slice(&found);
            buf.extend_from_slice(after);
            let explained = scan_explained(&buf);
            assert!(explained.found.is_empty(), "{explained:?}");
            assert_eq!(explained.rejected.len(), 1);
            assert_eq!(explained.rejected[0].start, before.len());
            assert_eq!(explained.rejected[0].reason, Reason::ImplausibleEdition);
        }

        let mut editions = make_declaring_fdp("a.proto");
        editions.extend(framed(12, b"editions"));
        editions.extend(varint_field(14, 1001));
        assert_eq!(scan(&editions), vec![(0, editions.len())]);
        let mut trailed = editions.clone();
        trailed.extend(varint_field(14, 2));
        assert_eq!(scan(&trailed), vec![(0, editions.len())]);

        let mut no_syntax = make_declaring_fdp("a.proto");
        let cut = no_syntax.len();
        no_syntax.extend(varint_field(14, 1000));
        assert_eq!(scan(&no_syntax), vec![(0, cut)]);
    }

    /// Spec 0449 S1. The same index with its import is a real descriptor.
    #[test]
    fn test_scan_accepts_a_public_dependency_with_its_import() {
        let mut fdp = make_fdp("a.proto");
        fdp.extend(framed(3, b"b.proto"));
        fdp.extend(varint_field(10, 0));
        assert_eq!(scan(&fdp), vec![(0, fdp.len())]);
    }

    /// Spec 0449 S1, for `weak_dependency`, packed and not: a real record
    /// with a dangling index after its own fields ends before it.
    #[test]
    fn test_scan_cuts_a_record_before_a_dangling_weak_dependency() {
        let mut one_import = make_declaring_fdp("a.proto");
        one_import.extend(framed(3, b"b.proto"));

        for index in [varint_field(11, 1), framed(11, &[0x00, 0x01])] {
            let mut buf = one_import.clone();
            buf.extend(index);
            let explained = scan_explained(&buf);
            assert_eq!(explained.found, vec![(0, one_import.len())]);
            assert!(explained.rejected.is_empty());
        }

        let mut resolved = one_import.clone();
        resolved.extend(framed(11, &[0x00]));
        assert_eq!(scan(&resolved), vec![(0, resolved.len())]);
    }

    /// Spec 0449 S1. Random bytes after a record that happen to frame an
    /// index field (`0x58`, `weak_dependency`, then a varint) are cut off:
    /// the record comes back exact rather than refused.
    #[test]
    fn test_scan_cuts_a_junk_index_after_a_record() {
        let fdp = make_declaring_fdp("a.proto");
        let mut buf = fdp.clone();
        buf.extend_from_slice(&[0x58, 0xd8, 0x27, 0x9a]);
        assert_eq!(scan(&buf), vec![(0, fdp.len())]);
    }

    /// Spec 0449 S7. A record whose top-level fields are out of order, as Go
    /// writes some: with the option it ends before the lower field and is
    /// reported in `order_cuts`; without, it is found whole and nothing is
    /// reported.
    #[test]
    fn test_scan_reports_a_record_the_order_check_cut() {
        let mut fdp = make_fdp("go.proto");
        fdp.extend(framed(8, &framed(1, b"example.com/go"))); // options
        let cut = fdp.len();
        fdp.extend(framed(4, &framed(1, b"M"))); // message_type, lower than 8

        let on = ScanOptions {
            increasing_field_numbers: true,
        };
        let explained = scan_explained_with_options(&fdp, wkt_graph(), FDP, &on);
        assert_eq!(explained.found, vec![(0, cut)]);
        assert_eq!(
            explained.order_cuts,
            vec![OrderCut {
                start: 0,
                end: cut,
                at: cut
            }]
        );

        let explained = scan_explained_with(&fdp, wkt_graph(), FDP);
        assert_eq!(explained.found, vec![(0, fdp.len())]);
        assert!(explained.order_cuts.is_empty());
    }

    /// Spec 0449 S7. Nested, the cut lies inside the field the record lost.
    #[test]
    fn test_scan_reports_a_nested_order_cut_past_the_end() {
        let mut fdp = make_declaring_fdp("go.proto");
        let cut = fdp.len();
        let mut options = framed(8, b"Outer"); // java_outer_classname
        let nested = options.len();
        options.extend(framed(1, b"example.com/go")); // java_package, lower
        let at = cut + framed(8, &options).len() - options.len() + nested;
        fdp.extend(framed(8, &options));

        let on = ScanOptions {
            increasing_field_numbers: true,
        };
        let explained = scan_explained_with_options(&fdp, wkt_graph(), FDP, &on);
        assert_eq!(explained.found, vec![(0, cut)]);
        assert_eq!(
            explained.order_cuts,
            vec![OrderCut {
                start: 0,
                end: cut,
                at
            }]
        );
    }

    #[test]
    fn scan_is_scan_explained_found() {
        let mut buf = make_fdp("a.proto");
        buf.extend_from_slice(b"\n$deprecation$private com.google.protobuf.Map");
        buf.extend_from_slice(&with_undeclared(true));
        assert_eq!(scan(&buf), scan_explained(&buf).found);
    }

    /// Spec 0313 test 4 / S2. Every member of the corpus, scored on its own
    /// declared extent, is clean under the strict definition — including the
    /// two sloppiness counters the accept rule of spec 0239 ignores. This is
    /// what licenses naming them in S2, and it must be re-run if either gains
    /// a new site.
    #[test]
    #[ignore = "needs a googleapis.desc in PROTOSCAN_CORPUS_DESC"]
    fn every_real_descriptor_is_clean_at_its_own_end() {
        let data = corpus();
        let opts = ScoringOpts {
            policy: Policy::Scan,
            ..Default::default()
        };
        for (start, end) in framing_boundaries(&data) {
            let e = score_one(&data[start..end], FDP, wkt_graph().graph(), &opts)
                .expect("FileDescriptorProto is a root of the embedded graph");
            assert!(
                !e.vetoed
                    && e.unknowns == 0
                    && e.mismatches == 0
                    && e.non_canonical == 0
                    && e.out_of_range == 0
                    // Spec 0371 S9: protoc writes every repeated scalar
                    // exactly as declared, so a real descriptor is never
                    // charged for packing.
                    && e.packing == 0,
                "record at {start} is not clean: vetoed={} unknowns={} \
                 mismatches={} non_canonical={} out_of_range={} packing={}",
                e.vetoed,
                e.unknowns,
                e.mismatches,
                e.non_canonical,
                e.out_of_range,
                e.packing,
            );
            assert_eq!(e.truncated, 0, "record at {start}: `Scan` cannot set this");
            assert_eq!(e.termination, end - start, "record at {start}");
        }
    }
}
