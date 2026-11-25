// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Descriptors inside zip and gzip members (spec 0480).
//!
//! A container walker around the unchanged scanner: each buffer, the input
//! first, is scanned as [`crate::scan_explained_with_options`] scans it, and
//! the zip and gzip members it holds are read and walked in turn,
//! depth-first. The caller sees each buffer once, through a visitor, and
//! nothing is kept once visited (spec 0480 G5).

use flate2::{Crc, Decompress, FlushDecompress, Status};
use memchr::memmem;
use prototext_graph::score::load::LoadedGraph;

use crate::{scan_explained_with_options, wkt_graph, Explained, ScanOptions, FDP};

/// A zip local file header's signature.
const ZIP_LOCAL: &[u8] = b"PK\x03\x04";
/// A zip data descriptor's optional signature.
const ZIP_DESCRIPTOR: &[u8] = b"PK\x07\x08";
/// A gzip member's magic and its one compression method, deflate.
const GZIP: &[u8] = b"\x1f\x8b\x08";
/// What one inflate call writes at most: the scratch buffer's size.
const CHUNK: usize = 64 * 1024;
/// General-purpose bit 0: the member is encrypted.
const ZIP_ENCRYPTED: u16 = 1;
/// General-purpose bit 3: crc and sizes follow the data, in a descriptor.
const ZIP_TRAILING_SIZES: u16 = 1 << 3;
/// A size the zip64 extra field holds instead.
const ZIP64_SIZE: u32 = u32::MAX;

/// How far [`scan_inflating`] goes (spec 0480 S4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InflateLimits {
    /// Containers within containers: 3 reads a gzip in a zip in a zip.
    pub max_depth: usize,
    /// Bytes inflated per member. Memory follows the largest member at each
    /// level (spec 0480 G5), so this bounds it: 3 levels of 256 MiB.
    pub max_member: usize,
    /// Bytes inflated per input, which bounds time. Overlapping zip members
    /// and nesting, the ways a decompression bomb gets its size, stop here.
    pub max_total: usize,
}

impl Default for InflateLimits {
    fn default() -> Self {
        InflateLimits {
            max_depth: 3,
            max_member: 256 << 20,
            max_total: 1 << 30,
        }
    }
}

/// What kind of container a member is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberKind {
    /// A zip member, by the name its local header gives.
    Zip { name: String },
    /// A gzip member.
    Gzip,
}

/// One container of an [`Origin`]'s chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    /// The offset of its header in the enclosing buffer: a zip member's
    /// local header (its `P`), a gzip member's `1f 8b` (spec 0480 S6).
    pub at: usize,
    pub kind: MemberKind,
}

/// Where a buffer is: the chain of containers from the input inward, empty
/// for the input itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Origin {
    pub layers: Vec<Layer>,
}

impl Origin {
    /// The chain as text (spec 0480 S6), headed by the caller's `prefix`:
    /// `zip@7340032!assets/api.binpb`, `""` for the input with no prefix.
    pub fn chain(&self, prefix: Option<&str>) -> String {
        let mut parts: Vec<String> = prefix.map(str::to_string).into_iter().collect();
        for layer in &self.layers {
            parts.push(match &layer.kind {
                MemberKind::Zip { name } => format!("zip@{}!{}", layer.at, escape(name)),
                MemberKind::Gzip => format!("gzip@{}", layer.at),
            });
        }
        parts.join("!")
    }

    /// A range of this buffer as text (spec 0480 S6): the chain, then
    /// `start..end`.
    pub fn text(&self, prefix: Option<&str>, start: usize, end: usize) -> String {
        match self.chain(prefix) {
            chain if chain.is_empty() => format!("{start}..{end}"),
            chain => format!("{chain}!{start}..{end}"),
        }
    }
}

/// A member name with `%`, `!` and `;` escaped: `!` always separates
/// layers, and `;` the origins a record joins (spec 0480 S6, S8).
fn escape(name: &str) -> String {
    name.replace('%', "%25")
        .replace('!', "%21")
        .replace(';', "%3B")
}

/// Why a member found was not read (spec 0480 S5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    Encrypted,
    /// A zip compression method other than stored (0) and deflate (8).
    Method(u16),
    /// One container too deep.
    DepthLimit,
    /// Inflated past [`InflateLimits::max_member`].
    MemberLimit,
    /// The input's budget spent: no member after this one is inflated.
    InputLimit,
}

impl SkipReason {
    /// As records and messages show it.
    pub fn code(&self) -> String {
        match self {
            SkipReason::Encrypted => "encrypted".into(),
            SkipReason::Method(m) => format!("method {m}"),
            SkipReason::DepthLimit => "depth-limit".into(),
            SkipReason::MemberLimit => "member-limit".into(),
            SkipReason::InputLimit => "input-limit".into(),
        }
    }
}

/// What the visitor of [`scan_inflating`] is shown, in order: the input
/// first, then each member depth-first, its own members right after it.
#[derive(Debug)]
pub enum Visit<'a> {
    /// A buffer scanned: the input, or a member read.
    Scanned {
        origin: &'a Origin,
        /// Valid during the call only.
        data: &'a [u8],
        /// As [`crate::scan_explained`] gives it, ranges into `data`, less
        /// what the stored members of `data` report themselves.
        explained: &'a Explained,
    },
    /// A member found and not read. Its origin ends with its own layer.
    Skipped {
        origin: &'a Origin,
        reason: SkipReason,
    },
}

/// [`crate::scan_explained_options`], looking into zip and gzip members
/// (spec 0480).
pub fn scan_inflating(
    data: &[u8],
    options: &ScanOptions,
    limits: &InflateLimits,
    visit: impl FnMut(Visit<'_>),
) {
    scan_inflating_with(data, wkt_graph(), FDP, options, limits, visit);
}

/// [`scan_inflating`] against the caller's own descriptor schema, as
/// [`crate::scan_explained_with_options`].
pub fn scan_inflating_with(
    data: &[u8],
    graph: &LoadedGraph,
    root: &str,
    options: &ScanOptions,
    limits: &InflateLimits,
    visit: impl FnMut(Visit<'_>),
) {
    let mut walk = Walk {
        graph,
        root,
        options,
        limits: *limits,
        visit,
        spent: 0,
        exhausted: false,
        buffers: vec![Vec::new(); limits.max_depth],
        scratch: vec![0; CHUNK],
    };
    walk.walk(data, &mut Origin::default(), 0);
}

/// A member's header, parsed.
enum Header {
    Zip(ZipHeader),
    /// A gzip member: where its deflate data starts.
    Gzip {
        body: usize,
    },
}

struct ZipHeader {
    name: String,
    flags: u16,
    method: u16,
    crc: u32,
    compressed: u64,
    uncompressed: u64,
    /// Where its data starts.
    data: usize,
}

impl ZipHeader {
    /// Where its data ends, when the header says.
    fn end(&self, len: usize) -> Option<usize> {
        if self.flags & ZIP_TRAILING_SIZES != 0 {
            return None;
        }
        let end = self
            .data
            .checked_add(usize::try_from(self.compressed).ok()?)?;
        (end <= len).then_some(end)
    }
}

/// How inflating a member went.
enum Inflated {
    /// The deflate stream ended after `consumed` bytes of input.
    Done {
        consumed: usize,
        crc: u32,
        size: u64,
    },
    /// Not a deflate stream: a signature found by chance.
    Failed,
    Limit(SkipReason),
}

struct Walk<'g, F> {
    graph: &'g LoadedGraph,
    root: &'g str,
    options: &'g ScanOptions,
    limits: InflateLimits,
    visit: F,
    /// Bytes inflated so far, for [`InflateLimits::max_total`].
    spent: usize,
    /// The input's budget is spent: nothing more is inflated.
    exhausted: bool,
    /// One per level, reused for each member inflated at that level.
    buffers: Vec<Vec<u8>>,
    scratch: Vec<u8>,
}

impl<F: FnMut(Visit<'_>)> Walk<'_, F> {
    fn walk(&mut self, data: &[u8], origin: &mut Origin, depth: usize) {
        let headers = headers(data);
        let deep = depth >= self.limits.max_depth;

        // Stored members are checked first: their descriptors are theirs to
        // report, not the parent's, unless they are one level too deep.
        let mut stored: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
        for (at, header) in &headers {
            let Header::Zip(h) = header else { continue };
            if stored.last().is_some_and(|(_, r)| *at < r.end) {
                continue;
            }
            if h.method == 0 && h.flags & ZIP_ENCRYPTED == 0 && h.compressed == h.uncompressed {
                if let Some(end) = h.end(data.len()) {
                    if crc32(&data[h.data..end]) == h.crc {
                        stored.push((*at, h.data..end));
                    }
                }
            }
        }

        let mut explained = scan_explained_with_options(data, self.graph, self.root, self.options);
        if !deep {
            let inside = |start: usize| stored.iter().any(|(_, r)| r.contains(&start));
            explained.found.retain(|&(start, _)| !inside(start));
            explained.rejected.retain(|r| !inside(r.start));
            explained.order_cuts.retain(|c| !inside(c.start));
        }
        (self.visit)(Visit::Scanned {
            origin,
            data,
            explained: &explained,
        });
        drop(explained);

        let mut claimed = 0;
        for (at, header) in headers {
            if let Some((_, range)) = stored.iter().find(|(s, _)| *s == at) {
                claimed = claimed.max(range.end);
                let Header::Zip(h) = header else { continue };
                origin.layers.push(Layer {
                    at,
                    kind: MemberKind::Zip { name: h.name },
                });
                if deep {
                    self.skipped(origin, SkipReason::DepthLimit);
                } else {
                    self.walk(&data[range.clone()], origin, depth + 1);
                }
                origin.layers.pop();
                continue;
            }
            if at < claimed {
                continue;
            }
            match header {
                Header::Zip(h) => self.zip_member(data, at, h, origin, depth, &mut claimed),
                Header::Gzip { body } => {
                    self.gzip_member(data, at, body, origin, depth, &mut claimed)
                }
            }
        }
    }

    fn zip_member(
        &mut self,
        data: &[u8],
        at: usize,
        h: ZipHeader,
        origin: &mut Origin,
        depth: usize,
        claimed: &mut usize,
    ) {
        let known_end = h.end(data.len());
        let reason = if h.flags & ZIP_ENCRYPTED != 0 {
            Some(SkipReason::Encrypted)
        } else if h.method != 0 && h.method != 8 {
            Some(SkipReason::Method(h.method))
        } else {
            None
        };
        // A stored member that failed its check, or whose size only a
        // trailing descriptor gives, can't be read: its bytes stay the
        // parent's, scanned in the clear.
        if reason.is_none() && h.method == 0 {
            return;
        }
        if reason.is_none() && self.exhausted {
            return;
        }
        let (flags, crc, uncompressed, start) = (h.flags, h.crc, h.uncompressed, h.data);
        origin.layers.push(Layer {
            at,
            kind: MemberKind::Zip { name: h.name },
        });
        if let Some(reason) = reason {
            *claimed = (*claimed).max(known_end.unwrap_or(at));
            self.skipped(origin, reason);
        } else {
            self.member(
                data,
                start,
                depth,
                origin,
                claimed,
                |data, consumed, sum, size| {
                    let end = start + consumed;
                    if flags & ZIP_TRAILING_SIZES == 0 {
                        let size_ok = uncompressed == u64::from(ZIP64_SIZE) || uncompressed == size;
                        return (sum == crc && size_ok).then_some(end);
                    }
                    // The descriptor: an optional signature, then the crc.
                    let at = if data.get(end..end + 4) == Some(ZIP_DESCRIPTOR) {
                        end + 4
                    } else {
                        end
                    };
                    (read_u32(data, at)? == sum).then_some(at + 12)
                },
            );
        }
        origin.layers.pop();
    }

    fn gzip_member(
        &mut self,
        data: &[u8],
        at: usize,
        body: usize,
        origin: &mut Origin,
        depth: usize,
        claimed: &mut usize,
    ) {
        if self.exhausted {
            return;
        }
        origin.layers.push(Layer {
            at,
            kind: MemberKind::Gzip,
        });
        self.member(
            data,
            body,
            depth,
            origin,
            claimed,
            |data, consumed, sum, size| {
                // The trailer: crc and size, modulo 2^32.
                let end = body + consumed;
                let ok = read_u32(data, end)? == sum && read_u32(data, end + 4)? == size as u32;
                ok.then_some(end + 8)
            },
        );
        origin.layers.pop();
    }

    /// Inflates the member whose deflate data starts at `start` and whose
    /// layer `origin` ends with, checks it with `check` (given the bytes
    /// consumed, the crc and size inflated, it answers where the member
    /// ends, or `None`), then walks it, or reports it skipped.
    fn member(
        &mut self,
        data: &[u8],
        start: usize,
        depth: usize,
        origin: &mut Origin,
        claimed: &mut usize,
        check: impl Fn(&[u8], usize, u32, u64) -> Option<usize>,
    ) {
        let deep = depth >= self.limits.max_depth;
        // One level too deep, the member is still inflated, its bytes
        // dropped: only a checked member is reported skipped.
        let mut buffer = if deep {
            None
        } else {
            Some(std::mem::take(&mut self.buffers[depth]))
        };
        if let Some(b) = buffer.as_mut() {
            b.clear();
        }
        match self.inflate(&data[start..], buffer.as_mut()) {
            Inflated::Done {
                consumed,
                crc,
                size,
            } => {
                if let Some(end) = check(data, consumed, crc, size) {
                    *claimed = (*claimed).max(end);
                    match &buffer {
                        Some(inflated) => self.walk(inflated, origin, depth + 1),
                        None => self.skipped(origin, SkipReason::DepthLimit),
                    }
                }
            }
            Inflated::Failed => {}
            Inflated::Limit(reason) => self.skipped(origin, reason),
        }
        if let Some(b) = buffer {
            self.buffers[depth] = b;
        }
    }

    /// Inflates the raw deflate stream at the start of `input` into `out`
    /// (or nowhere), within the limits.
    fn inflate(&mut self, input: &[u8], mut out: Option<&mut Vec<u8>>) -> Inflated {
        let mut stream = Decompress::new(false);
        let mut crc = Crc::new();
        let mut size = 0usize;
        loop {
            let (read, written) = (stream.total_in(), stream.total_out());
            let Ok(status) = stream.decompress(
                &input[read as usize..],
                &mut self.scratch,
                FlushDecompress::None,
            ) else {
                return Inflated::Failed;
            };
            let n = (stream.total_out() - written) as usize;
            crc.update(&self.scratch[..n]);
            if let Some(out) = out.as_mut() {
                out.extend_from_slice(&self.scratch[..n]);
            }
            size += n;
            self.spent += n;
            if self.spent > self.limits.max_total {
                self.exhausted = true;
                return Inflated::Limit(SkipReason::InputLimit);
            }
            if size > self.limits.max_member {
                return Inflated::Limit(SkipReason::MemberLimit);
            }
            if status == Status::StreamEnd {
                return Inflated::Done {
                    consumed: stream.total_in() as usize,
                    crc: crc.sum(),
                    size: size as u64,
                };
            }
            if n == 0 && stream.total_in() == read {
                // No progress: the input ended before the stream did.
                return Inflated::Failed;
            }
        }
    }

    fn skipped(&mut self, origin: &Origin, reason: SkipReason) {
        (self.visit)(Visit::Skipped { origin, reason });
    }
}

/// Every plausible member header in `data`, in order.
fn headers(data: &[u8]) -> Vec<(usize, Header)> {
    let mut found: Vec<(usize, Header)> = memmem::find_iter(data, ZIP_LOCAL)
        .filter_map(|at| Some((at, Header::Zip(zip_header(data, at)?))))
        .chain(memmem::find_iter(data, GZIP).filter_map(|at| {
            Some((
                at,
                Header::Gzip {
                    body: gzip_header(data, at)?,
                },
            ))
        }))
        .collect();
    found.sort_by_key(|(at, _)| *at);
    found
}

/// The zip local file header at `at`, if it reads as one: a known version, a
/// name and an extra field that fit, no NUL in the name.
fn zip_header(data: &[u8], at: usize) -> Option<ZipHeader> {
    let version = read_u16(data, at + 4)?;
    let flags = read_u16(data, at + 6)?;
    let method = read_u16(data, at + 8)?;
    let crc = read_u32(data, at + 14)?;
    let compressed = read_u32(data, at + 18)?;
    let uncompressed = read_u32(data, at + 22)?;
    let name_len = usize::from(read_u16(data, at + 26)?);
    let extra_len = usize::from(read_u16(data, at + 28)?);
    // Versions needed to extract go up to 6.3 (63).
    if version & 0xff > 63 || name_len == 0 {
        return None;
    }
    let name = data.get(at + 30..at + 30 + name_len)?;
    let extra = data.get(at + 30 + name_len..at + 30 + name_len + extra_len)?;
    if name.contains(&0) {
        return None;
    }
    let (mut compressed, mut uncompressed) = (u64::from(compressed), u64::from(uncompressed));
    if compressed == u64::from(ZIP64_SIZE) || uncompressed == u64::from(ZIP64_SIZE) {
        (uncompressed, compressed) = zip64_sizes(extra, uncompressed, compressed)?;
    }
    Some(ZipHeader {
        name: String::from_utf8_lossy(name).into_owned(),
        flags,
        method,
        crc,
        compressed,
        uncompressed,
        data: at + 30 + name_len + extra_len,
    })
}

/// The sizes a zip64 extra field (id 1) holds: each one the header gives as
/// `0xffffffff`, uncompressed first.
fn zip64_sizes(mut extra: &[u8], uncompressed: u64, compressed: u64) -> Option<(u64, u64)> {
    while extra.len() >= 4 {
        let id = read_u16(extra, 0)?;
        let len = usize::from(read_u16(extra, 2)?);
        let body = extra.get(4..4 + len)?;
        if id == 1 {
            let mut fields = body
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().unwrap()));
            let mut pick = |given: u64| {
                if given == u64::from(ZIP64_SIZE) {
                    fields.next()
                } else {
                    Some(given)
                }
            };
            let uncompressed = pick(uncompressed)?;
            return Some((uncompressed, pick(compressed)?));
        }
        extra = &extra[4 + len..];
    }
    None
}

/// Where the deflate data of the gzip member at `at` starts, if its header
/// reads as one: reserved flags clear, a known extra-flags value and
/// operating system, optional fields that fit.
fn gzip_header(data: &[u8], at: usize) -> Option<usize> {
    let flags = *data.get(at + 3)?;
    let extra_flags = *data.get(at + 8)?;
    let os = *data.get(at + 9)?;
    if flags & 0xe0 != 0 || !matches!(extra_flags, 0 | 2 | 4) || !(os <= 13 || os == 255) {
        return None;
    }
    let mut pos = at + 10;
    if flags & 0x04 != 0 {
        pos += 2 + usize::from(read_u16(data, pos)?);
    }
    for bit in [0x08, 0x10] {
        if flags & bit != 0 {
            pos += memchr::memchr(0, data.get(pos..)?)? + 1;
        }
    }
    if flags & 0x02 != 0 {
        pos += 2;
    }
    (pos < data.len()).then_some(pos)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(bytes);
    crc.sum()
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_text() {
        let origin = Origin {
            layers: vec![
                Layer {
                    at: 7,
                    kind: MemberKind::Zip {
                        name: "a!b%c;d.jar".into(),
                    },
                },
                Layer {
                    at: 0,
                    kind: MemberKind::Gzip,
                },
            ],
        };
        assert_eq!(
            origin.text(None, 1, 2),
            "zip@7!a%21b%25c%3Bd.jar!gzip@0!1..2"
        );
        assert_eq!(
            origin.text(Some("f/m"), 1, 2),
            "f/m!zip@7!a%21b%25c%3Bd.jar!gzip@0!1..2"
        );
        assert_eq!(Origin::default().text(None, 3, 4), "3..4");
        assert_eq!(Origin::default().text(Some("f"), 3, 4), "f!3..4");
    }
}
