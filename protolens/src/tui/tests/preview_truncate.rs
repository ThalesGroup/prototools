// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0174: the live preview's byte budget, and what a cut at that
//! budget is allowed to do to the rendering.
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

/// Number of `...` truncation markers (spec 0174 §S4).
fn ellipsis_line_count(lines: &[String]) -> usize {
    lines.iter().filter(|l| l.trim() == "...").count()
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
/// in. They cannot any more, and nothing in production ever did: a
/// truncated render carries the `...` marker, which deliberately has no
/// `NodeSpan` (spec 0174 §S4), so a document holding one has a body line
/// that no node claims — and a node counting its own lines has nowhere to
/// put such a line. The byte budget applies only under `is_preview`, and
/// the one production caller of `splice_override` passes `false`, so the
/// only way to reach that state was a test.
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
/// render completes (no hang/panic) with a visible `...` marker in place
/// of the omitted remainder. A confirmed override (`is_preview: false`)
/// is intentionally exempt — see the companion test below.
#[test]
fn preview_on_a_pathological_candidate_is_bounded_by_the_byte_budget() {
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    // The span count is the quantity that reached a million on the real
    // blob this spec came from, so it is asserted on directly rather than
    // through the arena a splice would have grown from it.
    let (_, _, rendered) = app
        .render_node_as(blob_idx, Some("test.Empty"), true)
        .expect("a pathological candidate render must still complete");

    // The budget admits at most one node per two interior bytes, i.e. a
    // quarter of `field_count` here.
    assert!(
        rendered.spans.len() < field_count / 2,
        "the render's footprint must be bounded by the byte budget, not \
         track the mis-parsed field count: spans={} field_count={field_count}",
        rendered.spans.len()
    );
    assert_eq!(
        ellipsis_line_count(&rendered.lines),
        1,
        "a truncated preview must show exactly one `...` marker"
    );
}

/// Companion to the test above (spec 0174 G5): the same pathological
/// candidate, but spliced as a *confirmed* override (`is_preview:
/// false`) rather than a live preview — must render completely, with no
/// truncation and no `...`, since this is the content that actually gets
/// shown as the real override, not a speculative guess.
#[test]
fn confirmed_override_is_not_truncated() {
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    app.splice_override(blob_idx, Some("test.Empty".to_string()), false)
        .expect("a confirmed override splice must complete");

    assert!(
        app.tree.len() >= field_count,
        "a confirmed override must render completely, not be truncated by \
         the preview-only byte budget: tree.len()={} field_count={field_count}",
        app.tree.len()
    );
    assert_eq!(
        ellipsis_line_count(&app.document_lines()),
        0,
        "a confirmed override must show no truncation marker"
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
        .render_node_as(blob_idx, Some("test.Empty"), false)
        .expect("a confirmed render must complete");
    assert!(
        confirmed.bytes.is_none(),
        "a confirmed render must not carry a copy of the node's bytes"
    );

    let (_, _, preview) = app
        .render_node_as(blob_idx, Some("test.Empty"), true)
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

    app.render_node_as(blob_idx, Some("test.Empty"), false)
        .expect("a confirmed render must complete");
    assert_eq!(
        app.render_cache.len(),
        0,
        "a confirmed render must leave the cache untouched"
    );

    app.render_node_as(blob_idx, Some("test.Empty"), true)
        .expect("a preview render must complete");
    assert_eq!(
        app.render_cache.len(),
        1,
        "a preview render is what the cache is for"
    );
}

/// Spec 0251 S8 / open question 1: how big is a preview render really?
/// The cache holds nothing else after S5, so this is what sizes
/// `RENDER_CACHE_MAX_BYTES`. Reported, not asserted — run with
/// `--ignored --nocapture`.
#[test]
#[ignore]
fn measure_a_preview_renders_size() {
    // The worst case the budget admits: a two-byte field per line, so
    // the line count is as high as 4096 interior bytes can make it.
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    let (_, _, r) = app
        .render_node_as(blob_idx, Some("test.Empty"), true)
        .expect("a preview render must complete");

    let line_bytes: usize = r.lines.iter().map(String::len).sum();
    let span_bytes = r.spans.len() * std::mem::size_of::<NodeSpan>();
    println!(
        "preview render: {} lines, {line_bytes} B text, {} spans, \
         {span_bytes} B spans, {} B total",
        r.lines.len(),
        r.spans.len(),
        line_bytes + span_bytes
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
    assert_eq!(ellipsis_line_count(&lines), 1);
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

    // Everything below `blob`'s own header, minus the marker.
    let interior: Vec<String> = bare_lines(&lines)
        .into_iter()
        .skip(1)
        .filter(|l| *l != "...")
        .collect();
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
/// §S4 replaces that line with the plain `...` marker.
#[test]
fn preview_shows_no_malformity_marker() {
    let field_count = 50;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);
    // Odd budget => the cut lands mid-entry, between an entry's tag and
    // its varint payload.
    app.override_preview_byte_budget = 21;

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    assert!(
        !lines.iter().any(|l| l.contains("TRUNCATED_BYTES")
            || l.contains("MALFORMED")
            || l.contains("UNEXPECTED_EOF")),
        "no malformity marker may leak out of a preview: lines={lines:?}"
    );
    assert_eq!(ellipsis_line_count(&lines), 1);
}

/// Spec 0174 §S4: the `...` is the *last* thing inside the truncated
/// node — just before its closing brace — so it reads as "and there is
/// more below", not as a sibling of what follows.
#[test]
fn truncated_preview_ends_with_an_ellipsis_line() {
    let field_count = 50;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);
    app.override_preview_byte_budget = 20;

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    let marker = lines
        .iter()
        .position(|l| l.trim() == "...")
        .expect("a truncated preview must carry a `...` marker");
    assert_eq!(
        lines[marker + 1].trim(),
        "}",
        "the marker must sit immediately before the node's closing brace: \
         lines={lines:?}"
    );
    // S4: the marker line carries no styles and no `NodeSpan` — it is
    // not selectable, not navigable, not part of any span range. Spec
    // 0187 S2 keeps the "no styles" half true by blanking the marker in
    // `window_text`, so the highlighter never sees a row that is not
    // prototext; the row still exists, so the buckets stay one-to-one
    // with the window.
    let window: Vec<DisplayRow> = (0..lines.len()).map(DisplayRow::Overlay).collect();
    app.refresh_window_styles(&window);
    assert_eq!(app.window_styles.len(), window.len());
    assert!(app.window_styles[marker].is_empty());
    // Spec 0210 S1: asked of the render's own spans rather than of the
    // document, which never holds a marker (see `preview_lines`).
    let (_, _, rendered) = app
        .render_node_as(blob_idx, Some("test.Empty"), true)
        .expect("the same render must succeed twice");
    // An enclosing message's span legitimately spans the marker; what may
    // not exist is a node whose *own* header or footer that line is, since
    // that is what makes a line selectable.
    assert!(
        !rendered
            .spans
            .iter()
            .any(|s| s.text_range.start == marker as u32 || s.text_range.end == marker as u32 + 1),
        "no span may own the marker line: {:?}",
        rendered.spans
    );
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
    assert_eq!(ellipsis_line_count(&lines), 1);

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
    assert_eq!(ellipsis_line_count(&lines), 1);
}

/// Spec 0174 G4's converse: a preview that fits within the budget is
/// byte-for-byte the confirmed rendering — no marker, nothing to
/// mistake for missing content.
#[test]
fn untruncated_preview_has_no_ellipsis_line() {
    let (mut app, blob_idx) = preview_budget_fixture(10);

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    assert_eq!(
        ellipsis_line_count(&lines),
        0,
        "an untruncated preview must show no marker: lines={lines:?}"
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
    assert_eq!(ellipsis_line_count(&lines), 1);
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
        ellipsis_line_count(&lines),
        0,
        "a singular varint must never be truncated: lines={lines:?}"
    );
    assert!(
        bare_lines(&lines).iter().any(|l| l == "id: 5"),
        "the value must survive intact: lines={lines:?}"
    );
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
    app.splice_override(blob_idx, Some("test.Empty".to_string()), false)
        .expect("retyping the blob to an empty message must succeed");
    let unknown = app
        .nth_child(blob_idx, 0)
        .expect("the empty message must hold the unknown LEN record");

    // Step two: ask for it as a run of varints.
    app.splice_override(unknown, Some("int32".to_string()), false)
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
