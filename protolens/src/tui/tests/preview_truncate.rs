// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Specs 0174 and 0318: the live preview's byte budget, where a cut at
//! that budget is allowed to land, and how faithful the result says it
//! is.
//!
//! `an_unknown_length_delimited_blob_can_be_read_as_a_packed_run` lives
//! here rather than with the other splice tests because it needs two of
//! this file's fixtures, and because reading an undescribed blob as a
//! packed run is the same subject: what the renderer makes of bytes no
//! schema explains.

use super::super::*;
use super::support::*;

/// Fixture shared by the preview-budget tests below: a `Holder` message
/// with one `bytes blob = 1` field carrying `payload` verbatim as its
/// raw interior. Returns the ready-to-splice `App` and `blob`'s own
/// tree index, with `override_target` already set to it.
fn preview_budget_fixture_bytes(payload: &[u8]) -> (App, usize) {
    use prost_types::field_descriptor_proto::{Label, Type};
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN};

    let fds = proto3_fds(
        "test_preview_budget.proto",
        vec![
            message(
                "Holder",
                vec![field("blob", 1, Label::Optional, Type::Bytes)],
            ),
            // `Empty` has no declared fields, so retyping `blob` to it
            // makes every entry of the reinterpreted payload land as an
            // unknown numeric field — which is exactly the pathological
            // shape spec 0174 bounds: the byte budget applies to the raw
            // input regardless of whether anything in it resolves
            // against the schema.
            message("Empty", Vec::new()),
            // The other candidate type `blob` gets retyped to: unlike
            // `Empty` it resolves the interior into real nested
            // *messages*, which is what spec 0174 G3 is about — the
            // surviving prefix must keep its nesting, not collapse into
            // one bytes line.
            message(
                "Wrapper",
                vec![field_of(
                    "items",
                    1,
                    Label::Repeated,
                    Type::Message,
                    ".test.Inner",
                )],
            ),
            message("Inner", vec![field("v", 1, Label::Optional, Type::Int64)]),
        ],
    );

    let mut blob = Vec::new();
    write_tag(1, WT_LEN, &mut blob);
    write_varint(payload.len() as u64, &mut blob);
    blob.extend_from_slice(payload);

    let mut app = fixture_under("preview-budget", &fds, "test.Holder", &blob);

    let blob_idx = app
        .nth_child(app.first_node, 0)
        .expect("tree must contain the blob field");
    app.override_target = Some(blob_idx);

    (app, blob_idx)
}

/// `preview_budget_fixture_bytes` with an interior of `field_count`
/// repetitions of a 2-byte `field 1 (varint) = 1` entry — so the
/// interior is exactly `2 * field_count` bytes, and every *even* cut
/// offset lands on a field boundary (no straddler) while every odd one
/// lands mid-field.
fn preview_budget_fixture(field_count: usize) -> (App, usize) {
    use prototext_core::helpers::{write_tag, write_varint, WT_VARINT};

    let mut payload = Vec::with_capacity(field_count * 2);
    for _ in 0..field_count {
        write_tag(1, WT_VARINT, &mut payload);
        write_varint(1, &mut payload);
    }
    preview_budget_fixture_bytes(&payload)
}

/// Spec 0318 S5: how faithful the preview `app` currently holds is. The
/// signal spec 0174 S4's `...` marker used to carry, moved off the
/// document and into the overlay's fold column.
fn preview_tier(app: &App) -> PreviewTier {
    app.preview_overlay
        .as_ref()
        .expect("a preview must be held")
        .tier
}

/// `lines` with each line's indentation and trailing `#@` annotation
/// stripped, so the assertions below read against the prototext itself.
fn bare_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.split("  #@").next().unwrap_or(l).trim().to_string())
        .collect()
}

/// Render `idx` as `target` the way a live preview actually does it —
/// through `preview_override_highlight`, which holds the result as an
/// overlay (spec 0185 S3) and never touches the document.
///
/// Spec 0210 S1: the truncation tests below used to *splice* the preview
/// in. They cannot any more, and nothing in production ever did. The byte
/// budget applies only under `is_preview`, and the one production caller
/// of `splice_override` passes `false`, so a spliced truncation was only
/// ever reachable from a test. Spec 0318 S6 gives the further reason: the
/// fidelity tier is carried by the *overlay*, so only a preview held as
/// an overlay has one to read.
fn preview_lines(app: &mut App, idx: usize, target: &str) -> Vec<String> {
    app.override_target = Some(idx);
    app.override_candidates = vec![(target.to_string(), None)];
    app.override_highlight = 0;
    app.preview_override_highlight();
    match app.preview_overlay.as_ref() {
        Some(o) => o.lines.clone(),
        None => panic!("a preview of {target} must render: {}", app.message),
    }
}

/// Spec 0174 (superseding spec 0163): a candidate type structurally
/// mismatched against a large raw payload can make the recursive-descent
/// decoder mis-parse arbitrary bytes into a pathologically large
/// synthetic tree (observed on a real ~1.1MB field: over a million spans
/// from a single splice). `App::override_preview_byte_budget` bounds
/// this at the *input*: a *live preview* (`is_preview: true`) hands the
/// renderer at most that many interior bytes, so the decode, the render,
/// the span count and the line count are all bounded together, and the
/// render completes (no hang/panic) and says so through its fidelity
/// tier (spec 0318 S5). A confirmed override (`is_preview: false`) is
/// intentionally exempt — see the companion test below.
#[test]
fn preview_on_a_pathological_candidate_is_bounded_by_the_byte_budget() {
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    // The span count is the quantity that reached a million on the real
    // blob this spec came from, so it is asserted on directly rather than
    // through the arena a splice would have grown from it.
    let (_, _, rendered) = app
        .render_node_as(blob_idx, Some("test.Empty"), true, None)
        .expect("a pathological candidate render must still complete");

    // The budget admits at most one node per two interior bytes, i.e. a
    // quarter of `field_count` here.
    assert!(
        rendered.spans.len() < field_count / 2,
        "the render's footprint must be bounded by the byte budget, not \
         track the mis-parsed field count: spans={} field_count={field_count}",
        rendered.spans.len()
    );
    // Every record here is two bytes wide, so the soft target is itself
    // a record boundary: the cut is clean, and the preview says so.
    assert_eq!(
        rendered.tier,
        PreviewTier::Clean,
        "a preview cut at a record boundary is Clean"
    );
}

/// Companion to the test above (spec 0174 G5): the same pathological
/// candidate, but rendered as a *confirmed* override (`is_preview:
/// false`) rather than a live preview — must render completely and
/// untruncated, since this is the content that actually gets shown as the
/// real override, not a speculative guess.
#[test]
fn confirmed_override_is_not_truncated() {
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    let (_, _, confirmed) = app
        .render_node_as(blob_idx, Some("test.Empty"), false, None)
        .expect("a confirmed render must complete");
    assert_eq!(
        confirmed.tier,
        PreviewTier::Whole,
        "a confirmed override is never cut, so it is Whole"
    );

    app.splice_override(blob_idx, Some("test.Empty".to_string()), None)
        .expect("a confirmed override splice must complete");

    assert!(
        app.tree.len() >= field_count,
        "a confirmed override must render completely, not be truncated by \
         the preview-only byte budget: tree.len()={} field_count={field_count}",
        app.tree.len()
    );
}

/// Spec 0251 S6: `bytes` is the preview's product alone. A confirmed
/// render must not copy the node out of the blob — for a root override
/// that copy is the whole document, allocated before the render cache
/// is even consulted and discarded by the splice. A preview must still
/// own its buffer, because a budget-truncated one exists nowhere else
/// and its spans are relative to it.
#[test]
fn a_confirmed_render_copies_no_bytes() {
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    let (_, _, confirmed) = app
        .render_node_as(blob_idx, Some("test.Empty"), false, None)
        .expect("a confirmed render must complete");
    assert!(
        confirmed.bytes.is_none(),
        "a confirmed render must not carry a copy of the node's bytes"
    );

    let (_, _, preview) = app
        .render_node_as(blob_idx, Some("test.Empty"), true, None)
        .expect("a preview render must complete");
    let bytes = preview.bytes.expect("a preview owns its bytes");
    assert!(
        bytes.len() < app.blob.len(),
        "the truncated buffer must be shorter than the blob it came from"
    );
    for span in &preview.spans {
        assert!(
            usize::try_from(span.raw_range.end).unwrap_or(usize::MAX) <= bytes.len(),
            "every preview span must index into the buffer the preview owns"
        );
    }
}

/// Spec 0251 S5: the render cache serves the preview path alone. A
/// confirmed render is the one that can be enormous, and caching it cost
/// two full clones of it for an entry no second lookup could reach.
#[test]
fn a_confirmed_splice_leaves_no_entry_behind() {
    let field_count = 40;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    app.render_node_as(blob_idx, Some("test.Empty"), false, None)
        .expect("a confirmed render must complete");
    assert_eq!(
        app.render_cache.len(),
        0,
        "a confirmed render must leave the cache untouched"
    );

    app.render_node_as(blob_idx, Some("test.Empty"), true, None)
        .expect("a preview render must complete");
    assert_eq!(
        app.render_cache.len(),
        1,
        "a preview render is what the cache is for"
    );
}

/// Spec 0251 S8 / open question 1: how big is a preview render really?
/// The cache holds nothing else after S5, so this is what sizes
/// `RENDER_CACHE_MAX_BYTES`. Spec 0318 test plan item 12 re-asks it now
/// that a cut may overshoot the budget as far as the hard cap. Reported,
/// not asserted — run with `--ignored --nocapture`.
#[test]
#[ignore]
fn measure_a_preview_renders_size() {
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN, WT_VARINT};

    let soft = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT;
    let report = |name: &str, r: &crate::tui::override_apply::RenderedAs| {
        let line_bytes: usize = r.lines.iter().map(String::len).sum();
        let span_bytes = r.spans.len() * std::mem::size_of::<NodeSpan>();
        println!(
            "{name}: {} lines, {line_bytes} B text, {} spans, \
             {span_bytes} B spans, {} B total",
            r.lines.len(),
            r.spans.len(),
            line_bytes + span_bytes
        );
    };

    // A two-byte field per line, so the line count is as high as `soft`
    // interior bytes can make it — and, two-byte records being what they
    // are, `soft` is itself a boundary and nothing overshoots.
    let (mut app, blob_idx) = preview_budget_fixture(soft * 2);
    let (_, _, r) = app
        .render_node_as(blob_idx, Some("test.Empty"), true, None)
        .expect("a preview render must complete");
    report("aligned at soft", &r);

    // The worst case S3's overshoot admits: as many two-byte records as
    // fit below `soft`, then one record long enough to carry the cut all
    // the way to `hard`. Nearly twice the bytes, one more line — a long
    // record is one row however long it is, which is why the render size
    // barely moves.
    let mut payload = Vec::new();
    for _ in 0..(soft / 2 - 1) {
        write_tag(1, WT_VARINT, &mut payload);
        write_varint(1, &mut payload);
    }
    write_tag(2, WT_LEN, &mut payload);
    write_varint(soft as u64, &mut payload);
    payload.extend(std::iter::repeat_n(b'z', soft));
    let (mut app, blob_idx) = preview_budget_fixture_bytes(&payload);
    let (_, _, r) = app
        .render_node_as(blob_idx, Some("test.Empty"), true, None)
        .expect("a preview render must complete");
    report("straddler carried to hard", &r);

    // What the boundary walk itself costs, on the shape that makes it do
    // the most work per byte: every top-level record two bytes wide, so
    // it takes `soft / 2` steps to reach the cut.
    let mut widest = Vec::new();
    for _ in 0..(soft * 2) {
        write_tag(1, WT_VARINT, &mut widest);
        write_varint(1, &mut widest);
    }
    let start = std::time::Instant::now();
    let rounds = 10_000;
    for _ in 0..rounds {
        std::hint::black_box(preview_truncate::cut_at(
            std::hint::black_box(&widest),
            soft,
            preview_truncate::TruncShape::RecordBoundary,
        ));
    }
    println!(
        "boundary walk over {} two-byte records: {:?} per cut",
        soft / 2,
        start.elapsed() / rounds
    );
}

/// Spec 0174 §S2: `App::override_preview_byte_budget` is a plain field,
/// not a fixed constant — setting it to a custom value (as `main.rs`'s
/// `--override-preview-byte-budget` does) must actually change where a
/// live preview cuts, not just the default.
#[test]
fn preview_respects_a_custom_byte_budget() {
    let field_count = 50;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);
    app.override_preview_byte_budget = 20;

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    // 20 bytes / 2 bytes per entry = 10 entries, well under the 50 the
    // untruncated payload would have produced.
    assert_eq!(
        bare_lines(&lines).iter().filter(|l| *l == "1: 1").count(),
        10,
        "a lower custom budget must be honored, not fall back to the \
         default: lines={lines:?}"
    );
    assert_eq!(preview_tier(&app), PreviewTier::Clean);
}

/// Spec 0174 G3: the cut is on the *input* bytes, so whatever survives
/// it is decoded and rendered exactly as it would have been in the
/// untruncated document — the entries before the cut keep their full
/// nesting and their declared types, rather than collapsing into a
/// single opaque bytes line the way naive field-level truncation would
/// leave them.
#[test]
fn preview_renders_complete_nested_fields_up_to_the_cut() {
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN, WT_VARINT};

    // 20 repetitions of `items { v: 1 }` — 4 bytes each.
    let mut payload = Vec::new();
    for _ in 0..20 {
        write_tag(1, WT_LEN, &mut payload);
        write_varint(2, &mut payload);
        write_tag(1, WT_VARINT, &mut payload);
        write_varint(1, &mut payload);
    }
    let (mut app, blob_idx) = preview_budget_fixture_bytes(&payload);
    // A multiple of 4 => the cut lands exactly on an entry boundary.
    app.override_preview_byte_budget = 20;

    let lines = preview_lines(&mut app, blob_idx, "test.Wrapper");

    // Everything below `blob`'s own header.
    let interior: Vec<String> = bare_lines(&lines).into_iter().skip(1).collect();
    let mut expected: Vec<String> = Vec::new();
    for _ in 0..5 {
        expected.extend(["items {", "v: 1", "}"].map(str::to_string));
    }
    expected.push("}".to_string()); // `blob`'s own closing brace.
    assert_eq!(
        interior, expected,
        "the surviving entries must keep their nesting and declared \
         types: lines={lines:?}"
    );
}

/// Spec 0174 G4: cutting mid-entry makes the renderer emit its own
/// malformity annotation for the straddling bytes — which is an artifact
/// of *our* cut, not of the document, so it must never reach the user.
///
/// Spec 0318 S2 removes the straddler instead of papering over it: the
/// budget is a soft target, and the cut runs *forward* from it to the
/// next top-level record boundary. Here the entries are two bytes wide,
/// so a budget of 21 cuts at 22 — eleven whole entries, no partial one to
/// annotate.
#[test]
fn preview_shows_no_malformity_marker() {
    let field_count = 50;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);
    // Odd budget: the naive cut would land mid-entry, between an entry's
    // tag and its varint payload.
    app.override_preview_byte_budget = 21;

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    assert!(
        !lines.iter().any(|l| l.contains("TRUNCATED_BYTES")
            || l.contains("MALFORMED")
            || l.contains("UNEXPECTED_EOF")),
        "no malformity marker may leak out of a preview: lines={lines:?}"
    );
    assert_eq!(
        bare_lines(&lines).iter().filter(|l| *l == "1: 1").count(),
        11,
        "the cut must move forward to the boundary past the budget: \
         lines={lines:?}"
    );
    assert_eq!(preview_tier(&app), PreviewTier::Clean);
}

/// Spec 0219 G3/S6: the byte budget applies to a packed target too, and
/// its cut lands on an element boundary.
///
/// Alignment is the whole test. `decode_packed_elems` is all-or-nothing:
/// a cut inside a varint, or one leaving a fixed-width payload that is
/// not a whole multiple of the element size, collapses the entire record
/// into a single `INVALID_PACKED_RECORDS` line — so the preview would
/// claim the bytes are junk where the confirmed override renders a clean
/// run, the preview/commit divergence spec 0185 G3 forbids.
#[test]
fn a_packed_preview_is_cut_at_an_element_boundary() {
    // 20 elements of two bytes each: 300 encodes as `AC 02`, so no cut
    // at an odd offset can be a boundary.
    let payload: Vec<u8> = std::iter::repeat_n([0xACu8, 0x02], 20).flatten().collect();

    // A varint run, budget deliberately odd: 15 walks back to 14, i.e.
    // seven whole elements.
    let (mut app, blob_idx) = preview_budget_fixture_bytes(&payload);
    app.override_preview_byte_budget = 15;
    let lines = preview_lines(&mut app, blob_idx, "int32");
    assert!(
        !lines.iter().any(|l| l.contains("INVALID_PACKED_RECORDS")),
        "an element-aligned cut must still decode: lines={lines:?}"
    );
    assert_eq!(
        bare_lines(&lines)
            .iter()
            .filter(|l| *l == "blob: 300")
            .count(),
        7,
        "the cut must keep a whole number of varint elements: lines={lines:?}"
    );
    assert_eq!(preview_tier(&app), PreviewTier::Clean);

    // The same bytes as a fixed-width run: 15 rounds down to 12, i.e.
    // three whole four-byte elements.
    let (mut app, blob_idx) = preview_budget_fixture_bytes(&payload);
    app.override_preview_byte_budget = 15;
    let lines = preview_lines(&mut app, blob_idx, "fixed32");
    assert!(
        !lines.iter().any(|l| l.contains("INVALID_PACKED_RECORDS")),
        "a fixed-width cut must land on a whole element: lines={lines:?}"
    );
    assert_eq!(
        bare_lines(&lines)
            .iter()
            .filter(|l| l.starts_with("blob: "))
            .count(),
        3,
        "the cut must keep a whole number of fixed32 elements: lines={lines:?}"
    );
    assert_eq!(preview_tier(&app), PreviewTier::Clean);
}

/// Spec 0174 G4's converse: a preview that fits within the budget is
/// byte-for-byte the confirmed rendering, and spec 0318 S5 has it say so
/// — `Whole`, nothing withheld.
#[test]
fn untruncated_preview_is_whole() {
    let (mut app, blob_idx) = preview_budget_fixture(10);

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    assert_eq!(
        preview_tier(&app),
        PreviewTier::Whole,
        "an untruncated preview is Whole: lines={lines:?}"
    );
}

/// Spec 0174 §S3 `TruncShape::CharBoundary`: a `string` target is cut at
/// the last UTF-8 character boundary at or before the budget, never
/// mid-character — otherwise the renderer would see (and flag) invalid
/// UTF-8 that the document does not actually contain.
#[test]
fn preview_of_a_long_string_stays_valid_utf8() {
    // 50 two-byte characters; an odd budget guarantees the naive cut
    // would land mid-character.
    let payload = "é".repeat(50);
    let (mut app, blob_idx) = preview_budget_fixture_bytes(payload.as_bytes());
    app.override_preview_byte_budget = 21;

    let lines = preview_lines(&mut app, blob_idx, "string");

    let value = lines
        .iter()
        .find(|l| l.contains('"'))
        .expect("the string value must be rendered");
    assert!(
        value.contains(&"é".repeat(10)) && !value.contains(&"é".repeat(11)),
        "the cut must fall back to the last character boundary at or \
         before the budget: {value}"
    );
    assert!(
        !value.contains("INVALID_STRING"),
        "the cut must never leave a partial character behind: {value}"
    );
    assert_eq!(preview_tier(&app), PreviewTier::Clean);
}

/// Spec 0174 §S3 `TruncShape::Never`: a singular numeric value is
/// bounded by construction (10 bytes at most), so it is never cut — a
/// budget lower than its own width must not corrupt it.
#[test]
fn preview_of_a_singular_varint_is_never_truncated() {
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.override_target = Some(id_idx);
    app.override_preview_byte_budget = 1;

    let lines = preview_lines(&mut app, id_idx, "int64");

    assert_eq!(
        preview_tier(&app),
        PreviewTier::Whole,
        "a singular varint must never be truncated: lines={lines:?}"
    );
    assert!(
        bare_lines(&lines).iter().any(|l| l == "id: 5"),
        "the value must survive intact: lines={lines:?}"
    );
}

/// Spec 0318 S2: the kept prefix is a sequence of whole top-level
/// records — which is exactly what a shorter message is. Asserted by
/// re-walking the kept bytes: a whole-record prefix ends with no
/// remainder.
#[test]
fn record_boundary_cut_keeps_whole_records() {
    use crate::tui::preview_truncate::{cut_at, TruncShape};
    use prototext_core::helpers::{write_tag, write_varint, WT_VARINT};

    let mut payload = Vec::new();
    for _ in 0..3_000 {
        write_tag(1, WT_VARINT, &mut payload);
        write_varint(1, &mut payload);
    }
    let soft = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT;

    let (kept, tier) = cut_at(&payload, soft, TruncShape::RecordBoundary)
        .expect("a 6000-byte payload must be cut at a 4096-byte budget");
    assert_eq!(tier, PreviewTier::Clean);
    assert_eq!(
        kept, soft,
        "two-byte records make the soft target itself a boundary"
    );

    // Re-walk the kept bytes: whole records leave no remainder.
    let mut pos = 0usize;
    while pos < kept {
        assert_eq!(payload[pos], 0x08, "each record must start with its tag");
        pos += 2;
    }
    assert_eq!(pos, kept, "the prefix must not end mid-record");
}

/// Spec 0318 G2: because no field straddles the cut, the preview shows no
/// malformity the full node would not have shown. `TRUNCATED_BYTES` and
/// spec 0303's missing-byte count are the two annotations a mid-record cut
/// used to manufacture.
#[test]
fn record_boundary_preview_has_no_truncation_annotation() {
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    assert_eq!(preview_tier(&app), PreviewTier::Clean);
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("TRUNCATED_BYTES") || l.contains("missing")),
        "a record-boundary cut invents no annotation: {:?}",
        lines.iter().rev().take(4).collect::<Vec<_>>()
    );
}

/// Spec 0318 S4: when one record is longer than the room, there is no
/// boundary to cut at. The preview cuts at the hard cap anyway and lets
/// the rendering say what it is — showing the reader less than they asked
/// for with no way to explain it would be worse. `Ragged` is the tier
/// that admits it.
#[test]
fn a_record_longer_than_the_room_is_ragged() {
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN};

    // One field whose payload alone exceeds the hard cap.
    let mut payload = Vec::new();
    write_tag(1, WT_LEN, &mut payload);
    write_varint(10_000, &mut payload);
    payload.extend(std::iter::repeat_n(b'z', 10_000));

    let (mut app, blob_idx) = preview_budget_fixture_bytes(&payload);
    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    assert_eq!(preview_tier(&app), PreviewTier::Ragged);
    assert!(
        lines.iter().any(|l| l.contains("TRUNCATED_BYTES")),
        "S4 is behavior, not a fallback to hide: lines={lines:?}"
    );
}

/// Spec 0318 S5's reason for returning the tier from `cut_at` rather than
/// letting the caller derive it from the kept length: a boundary landing
/// exactly on the hard cap is `Clean`, and a mid-record cut at that same
/// offset is `Ragged`. The number cannot tell them apart.
#[test]
fn a_boundary_exactly_at_hard_is_clean() {
    use crate::tui::preview_truncate::{cut_at, hard_cap, TruncShape};
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN};

    let soft = 10usize;
    let hard = hard_cap(soft);

    // A first record exactly `hard` bytes wide: tag + length + 18 bytes
    // of payload. The first boundary at or after `soft` is `hard` itself.
    let mut payload = Vec::new();
    write_tag(1, WT_LEN, &mut payload);
    write_varint(18, &mut payload);
    payload.extend(std::iter::repeat_n(b'z', 18));
    assert_eq!(payload.len(), hard);
    // A second record, so the payload does not simply fit.
    write_tag(2, WT_LEN, &mut payload);
    write_varint(4, &mut payload);
    payload.extend(std::iter::repeat_n(b'z', 4));

    assert_eq!(
        cut_at(&payload, soft, TruncShape::RecordBoundary),
        Some((hard, PreviewTier::Clean)),
        "a boundary at the cap is still a boundary"
    );

    // The same offset reached by giving up: one record too long to walk
    // out of, cut at `hard` mid-record.
    let mut long = Vec::new();
    write_tag(1, WT_LEN, &mut long);
    write_varint(100, &mut long);
    long.extend(std::iter::repeat_n(b'z', 100));
    assert_eq!(
        cut_at(&long, soft, TruncShape::RecordBoundary),
        Some((hard, PreviewTier::Ragged)),
        "the same kept length, the opposite verdict"
    );
}

/// Spec 0318 S2: any tag or length varint the walk cannot parse ends it.
/// Guessing past malformed bytes is how a preview would invent a boundary
/// the data does not have, so S4 takes over instead — and nothing panics
/// on the way (`parse_wiretag` asserts `start < buflen`).
#[test]
fn a_bad_length_varint_before_soft_does_not_panic() {
    use crate::tui::preview_truncate::{cut_at, hard_cap, TruncShape};
    use prototext_core::helpers::{write_tag, WT_LEN};

    let soft = 8usize;

    // A length varint whose continuation bits run off the end of the
    // payload: it never terminates, so it does not parse.
    let mut unterminated = Vec::new();
    write_tag(1, WT_LEN, &mut unterminated);
    unterminated.extend(std::iter::repeat_n(0x80u8, 20));
    assert_eq!(
        cut_at(&unterminated, soft, TruncShape::RecordBoundary),
        Some((hard_cap(soft), PreviewTier::Ragged))
    );

    // A length that parses but points past the end.
    let mut overlong = Vec::new();
    write_tag(1, WT_LEN, &mut overlong);
    prototext_core::helpers::write_varint(1_000_000, &mut overlong);
    overlong.extend(std::iter::repeat_n(b'z', 30));
    assert_eq!(
        cut_at(&overlong, soft, TruncShape::RecordBoundary),
        Some((hard_cap(soft), PreviewTier::Ragged))
    );

    // Wire type 6 is not a wire type at all.
    let mut bad_tag = vec![(1 << 3) | 6];
    bad_tag.extend(std::iter::repeat_n(b'z', 30));
    assert_eq!(
        cut_at(&bad_tag, soft, TruncShape::RecordBoundary),
        Some((hard_cap(soft), PreviewTier::Ragged))
    );
}

/// Spec 0318 S2: group framing nests, so only depth 0 yields a boundary.
/// An `END_GROUP` closing a nested group does not end a top-level record,
/// and cutting there would leave a group open that the reader's node has
/// closed.
#[test]
fn group_payload_cuts_at_top_level_only() {
    use crate::tui::preview_truncate::{cut_at, TruncShape};
    use prototext_core::helpers::{
        write_tag, write_varint, WT_END_GROUP, WT_START_GROUP, WT_VARINT,
    };

    // Four bytes per group: open, a two-byte varint field, close.
    let mut payload = Vec::new();
    for _ in 0..10 {
        write_tag(1, WT_START_GROUP, &mut payload);
        write_tag(2, WT_VARINT, &mut payload);
        write_varint(1, &mut payload);
        write_tag(1, WT_END_GROUP, &mut payload);
    }
    assert_eq!(payload.len(), 40);

    // A budget landing inside the second group. Offsets 5, 6 and 7 are
    // all inside it — the inner close at 7 is at depth 1 — so the first
    // boundary at or after 5 is 8.
    assert_eq!(
        cut_at(&payload, 5, TruncShape::RecordBoundary),
        Some((8, PreviewTier::Clean)),
        "a nested close is not a top-level boundary"
    );
}

/// Spec 0318 S5: a node the budget already fits is `Whole`. `cut_at`
/// says so by cutting nothing at all, which is what the caller reads as
/// the tier.
#[test]
fn a_short_node_is_whole() {
    use crate::tui::preview_truncate::{cut_at, TruncShape};

    let payload = vec![b'z'; 8];
    for shape in [
        TruncShape::RecordBoundary,
        TruncShape::AnyByte,
        TruncShape::CharBoundary,
        TruncShape::PackedVarint,
        TruncShape::PackedFixed(4),
        TruncShape::Never,
    ] {
        assert_eq!(cut_at(&payload, 8, shape), None, "{shape:?} at its budget");
        assert_eq!(cut_at(&payload, 16, shape), None, "{shape:?} under it");
    }
}

/// Spec 0318 S1: splitting the old `Exact` in two left the `bytes` half
/// unchanged. To a `bytes` field every byte sequence is a valid value, so
/// a shorter one aligns to nothing and annotates nothing.
#[test]
fn bytes_target_still_cuts_anywhere() {
    use crate::tui::preview_truncate::{cut_at, TruncShape};

    let payload = vec![b'z'; 100];
    for soft in [1usize, 7, 33, 99] {
        assert_eq!(
            cut_at(&payload, soft, TruncShape::AnyByte),
            Some((soft, PreviewTier::Clean)),
            "a bytes cut lands exactly on the budget"
        );
    }
}
/// Spec 0219 G2: reading a length-delimited record as a packed run is
/// not limited to records the schema already calls packed — it is the
/// reading *any* `WT_LEN` node gets when retyped to a packable
/// primitive, which is the whole point on a blob no schema describes.
///
/// The node here is doubly unknown: it sits inside a message with no
/// declared fields, so there is neither a `[packed=true]` declaration
/// nor a parent field of any kind to derive packedness from, and it was
/// never rendered as a run so it carries no `packed_record_start`
/// either. Both of spec 0219's rejected alternatives fail it.
#[test]
fn an_unknown_length_delimited_blob_can_be_read_as_a_packed_run() {
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN};

    // `2: { 5, 6, 7, 8 }` as raw wire bytes.
    let mut payload = Vec::new();
    write_tag(2, WT_LEN, &mut payload);
    write_varint(4, &mut payload);
    payload.extend_from_slice(&[0x05, 0x06, 0x07, 0x08]);

    let (mut app, blob_idx) = preview_budget_fixture_bytes(&payload);

    // Step one: make the interior unknown. `test.Empty` declares no
    // fields, so field 2 lands as a bare LEN record with no schema
    // behind it at all.
    app.splice_override(blob_idx, Some("test.Empty".to_string()), None)
        .expect("retyping the blob to an empty message must succeed");
    let unknown = app
        .nth_child(blob_idx, 0)
        .expect("the empty message must hold the unknown LEN record");

    // Step two: ask for it as a run of varints.
    app.splice_override(unknown, Some("int32".to_string()), None)
        .expect("retyping an unknown LEN record to int32 must succeed");

    let bare = bare_lines(&app.document_lines());
    let elems: Vec<&String> = bare.iter().filter(|l| l.starts_with("2: ")).collect();
    assert_eq!(
        elems,
        vec!["2: 5", "2: 6", "2: 7", "2: 8"],
        "the blob must read as one line per packed element: {:?}",
        app.document_lines()
    );
}

/// The budget makes a preview *narrower* than the row it stands in for.
/// Spec 0185 G4 anticipated only the other direction — a mismatched
/// candidate rendering wide — but an overlay narrows the visible content
/// exactly as a fold or an override splice does, and owes the same
/// `clamp_pan_offset`.
///
/// Without it, `$` on a long blob line and then `t` left `pan_offset`
/// past the right edge of every row on screen: `pan_spans` yielded
/// nothing for any of them and the main pane went blank.
#[test]
fn a_budgeted_preview_clamps_a_pan_made_for_the_untruncated_row() {
    let (mut app, blob_idx) = preview_budget_fixture_bytes(&vec![b'z'; 400]);
    app.override_preview_byte_budget = 16;
    // A pane the untruncated line does not fit in, which is what gives
    // `$` something to pan.
    app.main_area = ratatui::layout::Rect::new(0, 0, 40, 8);
    app.cursor = blob_idx;
    // The fixture pre-arms `override_target`; `t` would read that as
    // "already open" and close the pane instead.
    app.override_target = None;

    app.handle_key(KeyEvent::new(KeyCode::Char('$'), KeyModifiers::NONE));
    let panned = app.pan_offset;
    assert!(panned > 0, "the caret at a long line's end pans right");

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(
        app.preview_overlay.is_some(),
        "`t` must put a preview up: {}",
        app.message
    );
    assert!(
        app.max_visible_line_len() < panned,
        "the fixture must actually narrow: the budgeted preview's widest \
         row has to fall short of where the reader had panned",
    );

    let bound = app.max_pan_offset();
    assert!(
        app.pan_offset <= bound,
        "the pan must be clamped to what the overlay shows ({} > {bound})",
        app.pan_offset,
    );
    assert!(
        app.max_visible_line_len() > app.pan_offset,
        "so some row still has a character left of the pan — the pane is \
         not blank",
    );
}
