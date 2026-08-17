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
fn score_candidate(data: &[u8], start: usize) -> Option<usize> {
    let opts = ScoringOpts {
        policy: Policy::Scan,
        ..Default::default()
    };
    let entry = score_one(&data[start..], FDP, wkt_graph().graph(), &opts)?;
    if entry.vetoed || entry.unknowns != 0 || entry.mismatches != 0 {
        return None;
    }
    let end = start + entry.termination;
    if !declares_more_than_a_name(&data[start..end]) {
        return None;
    }
    Some(end)
}

/// Whether `record` carries a top-level field other than `name` (spec 0313 S4).
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
    fn make_fdp(name: &str) -> Vec<u8> {
        let mut buf = name_field(name);
        buf.extend_from_slice(&framed(2, b"p"));
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
        // A FDP whose name field is cut off mid-string — should not be
        // returned. `name_field`, not `make_fdp`, so that the bytes removed
        // are the name's own and the test still says what it says.
        let fdp = name_field("foo.proto");
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
            let fdp = make_fdp("foo.proto");
            let mut buf = fdp.clone();
            buf.extend_from_slice(tail);

            assert_eq!(
                scan_bytes(&buf),
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
            scan_bytes(cut),
            vec![(0, two.len())],
            "the boundary before the cut, not the cut"
        );

        let name_and_package = make_fdp("foo/bar.proto");
        let cut = &name_and_package[..name_and_package.len() - 1];
        assert!(
            scan_bytes(cut).is_empty(),
            "a clean prefix of one field is not a descriptor"
        );
    }

    /// Spec 0313 test 3 / S4. The false positive the scanner actually faces.
    /// `option java_package = "com.google.cloud.pubsublite.proto"` is field 1
    /// of `FileOptions` — tag `0x0a`, a plausible path, and at the anchor
    /// indistinguishable from a file name. Its first boundary is a flawless
    /// one-field descriptor, so cleanliness cannot reject it and the veto
    /// arrives only at the boundary after; without the floor the stub is
    /// handed back.
    #[test]
    fn a_java_package_name_is_not_a_descriptor() {
        let stub = name_field("com.google.cloud.pubsublite.proto");
        assert_eq!(score_candidate(&stub, 0), None);
        assert!(scan_bytes(&stub).is_empty());

        // One further depth-0 field and the same anchor is a real, if tiny,
        // descriptor: the floor is one field, not two.
        let mut real = stub.clone();
        real.extend_from_slice(&framed(2, b"pkg"));
        assert_eq!(score_candidate(&real, 0), Some(real.len()));
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
                    && e.out_of_range == 0,
                "record at {start} is not clean: vetoed={} unknowns={} \
                 mismatches={} non_canonical={} out_of_range={}",
                e.vetoed,
                e.unknowns,
                e.mismatches,
                e.non_canonical,
                e.out_of_range,
            );
            assert!(!e.truncated, "record at {start}: `Scan` cannot set this");
            assert_eq!(e.termination, end - start, "record at {start}");
        }
    }
}
