// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// PyO3 essentials
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::Bound;

// Stub-generation helpers
use pyo3_stub_gen::derive::gen_stub_pyfunction;

use std::sync::OnceLock;

use prototext_graph::score::load::LoadedGraph;
use prototext_graph::score::{score_one, Policy, ScoringOpts};

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

// ── scan ─────────────────────────────────────────────────────────────────────

/// Scan a binary buffer for FileDescriptorProto candidates.
///
/// Returns a list of (start, end) byte offsets for each candidate found.
#[gen_stub_pyfunction]
#[pyfunction]
fn scan(buffer: Bound<'_, PyBytes>) -> PyResult<Vec<(usize, usize)>> {
    let bytes = buffer.as_bytes();
    let candidates = walk_candidates(bytes);
    Ok(candidates)
}

/// Rust-to-Rust entry point — same logic as `scan()` without the PyO3 wrapper.
pub fn scan_bytes(data: &[u8]) -> Vec<(usize, usize)> {
    walk_candidates(data)
}

// ── Python module ─────────────────────────────────────────────────────────────

/// The Python module definition.
#[pymodule]
fn fdp_scan_lib(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(scan, m)?)?;
    Ok(())
}

/// Gather stub info for pyo3-stub-gen (called by the post_build binary).
///
/// The `use fdp_scan_lib::stub_info` in post_build.rs forces this lib to be
/// linked into the binary, ensuring all inventory items are present.
///
/// Uses std::env::var (runtime) rather than env!() (compile-time) so that the
/// binary works correctly when Cargo reuses it from a prior build's artifact
/// cache (e.g. when running under Crane/Nix where different derivations use
/// different sandbox paths).  The installPhase sets CARGO_MANIFEST_DIR before
/// invoking the binary.
pub fn stub_info() -> pyo3_stub_gen::Result<pyo3_stub_gen::StubInfo> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set when running fdp_scan_post_build");
    let pyproject = std::path::Path::new(&manifest_dir).join("pyproject.toml");
    pyo3_stub_gen::StubInfo::from_pyproject_toml(pyproject)
}

// ============================================================================

/// Maximum byte length of a `.proto` file name embedded in a FileDescriptorProto.
/// Used to reject implausibly long strings during scanning.
const MAX_PROTO_NAME_LEN: usize = 200;

/// Returns true if `name` looks like a plausible canonical `.proto`
/// import path: ends in `.proto`, is not absolute, and every
/// `/`-separated component is non-empty, is not `.`/`..`, and contains
/// no control characters.
///
/// This is deliberately *not* a POSIX path-legality check — POSIX
/// forbids only `NUL` and `/` in a filename, and control characters
/// (e.g. a literal newline) are technically legal there.  This checks
/// plausibility as a genuine, protoc-managed import path instead:
/// no real `.proto` import name is absolute, contains `.`/`..`
/// components, or embeds control characters, even though the
/// filesystem would tolerate all of these.
fn is_plausible_path(name: &str) -> bool {
    name.ends_with(".proto")
        && !name.starts_with('/')
        && name.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.chars().any(|c| c.is_control())
        })
}

fn walk_candidates(data: &[u8]) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let data_len = data.len();
    let mut offset = 0;

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
                    if name_str.len() <= MAX_PROTO_NAME_LEN && is_plausible_path(name_str) {
                        if let Some(fdp_end) = score_candidate(data, offset) {
                            result.push((offset, fdp_end));
                            offset = fdp_end;
                            continue;
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
/// A vetoed candidate yields no boundary at all: a veto fires inside a field
/// already consumed, so it leaves the counters polluted by bytes that may lie
/// past the true end of the record (spec 0238 N6).
fn score_candidate(data: &[u8], start: usize) -> Option<usize> {
    let opts = ScoringOpts {
        policy: Policy::Scan,
        ..Default::default()
    };
    let entry = score_one(&data[start..], FDP, wkt_graph().graph(), &opts)?;
    if entry.vetoed || entry.unknowns != 0 || entry.mismatches != 0 {
        return None;
    }
    Some(start + entry.termination)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal FileDescriptorProto encoding with just field 1 (name).
    /// Returns the encoded bytes.
    fn make_fdp(name: &str) -> Vec<u8> {
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

    #[test]
    fn test_fdp_with_null_separator() {
        // Two FDPs separated by a 0x00 byte — the classic terminator case.
        let fdp1 = make_fdp("foo.proto");
        let fdp2 = make_fdp("bar.proto");
        let mut buf = fdp1.clone();
        buf.push(0x00);
        let sep = 1;
        buf.extend_from_slice(&fdp2);

        let ranges = scan_bytes(&buf);

        assert_eq!(ranges.len(), 2, "expected two FDP ranges, got {ranges:?}");
        assert_eq!(ranges[0], (0, fdp1.len()));
        assert_eq!(ranges[1], (fdp1.len() + sep, buf.len()));
    }

    #[test]
    fn test_truncated_fdp_not_returned() {
        // A FDP whose name field is cut off mid-string — should not be returned.
        let fdp = make_fdp("foo.proto");
        let truncated = &fdp[..fdp.len() - 3]; // cut off last 3 bytes of name

        let ranges = scan_bytes(truncated);

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
        // `scan_bytes()` does *not* return an empty list here — it
        // correctly rejects the outer garbage span and finds this
        // coincidental but genuinely clean embedded name instead, which
        // is safe to accept (no leading `/`, no control characters).
        let garbage = "/p>\n{{- end}}\n{{end}}\n\n$google/api/expr/v1alpha1/value.proto";
        let fdp = make_fdp(garbage);

        let ranges = scan_bytes(&fdp);

        assert!(
            !ranges.contains(&(0, fdp.len())),
            "full garbage span should never be accepted as a candidate, got {ranges:?}"
        );
    }

    #[test]
    fn test_simple_garbage_name_rejected() {
        // Simpler garbage string with no embedded coincidental valid
        // substring: scan_bytes() should return no candidates at all.
        let garbage = "/p>\n{{- end}}\n{{end}}\n\nnot/a/real/path.proto";
        let fdp = make_fdp(garbage);

        let ranges = scan_bytes(&fdp);

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
        assert!(is_plausible_path("café.proto"));
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

    #[test]
    fn test_consecutive_fdps_split_correctly() {
        // Two consecutive FDPs with no 0x00 separator between them.
        let fdp1 = make_fdp("foo.proto");
        let fdp2 = make_fdp("bar.proto");
        let mut buf = fdp1.clone();
        buf.extend_from_slice(&fdp2);

        let ranges = scan_bytes(&buf);

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
        let mut buf = vec![(number << 3) as u8 | 2];
        let mut len = payload.len() as u64;
        loop {
            let byte = (len & 0x7F) as u8;
            len >>= 7;
            if len == 0 {
                buf.push(byte);
                break;
            }
            buf.push(byte | 0x80);
        }
        buf.extend_from_slice(payload);
        buf
    }

    /// A FileDescriptorProto carrying `messages` empty `message_type` entries
    /// (field 4), so that its score grows with `messages` while its defect
    /// counters stay at zero.
    fn make_fdp_with_messages(name: &str, messages: usize) -> Vec<u8> {
        let mut buf = make_fdp(name);
        for i in 0..messages {
            let descriptor = make_fdp(&format!("M{i}")); // DescriptorProto.name
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
        let found = scan_bytes(&data);
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
            Some(end),
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
                Some(fdp.len()),
                "an FDP with {messages} message_type entries was rejected"
            );
        }
    }
}
