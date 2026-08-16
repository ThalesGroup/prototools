// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0299: `message`, the override target that reads a
//! length-delimited payload as a message with no schema at all.
//!
//! The document root is the case it was written for — `Blob` prepends a
//! real field-1 tag, so a file the probe disqualifies (spec 0266)
//! collapses to one opaque string — but nothing here is about the root.
//! It is an override like any other, and the assertion that matters is
//! agreement with `prototext decode --raw`.

use super::super::*;
use super::support::*;

use crate::decode::{decode, DescriptorContext, RootType};
use crate::node_status::Status;
use prototext_core::serialize::render_text::decode_and_render;

/// Field 1 = `"abc"`, then a second field 1 whose length prefix
/// declares 16 payload bytes and whose payload is the 5 of `"short"`.
///
/// The cut tail is the point: it is one malformed token, arbitrarily
/// late, and it is what disqualifies the *whole* file from being read
/// as a message. `grpconf/stage/boblog` is this fixture at 20 198
/// bytes.
const CUT_SHORT: &[u8] = b"\x0a\x03abc\x0a\x10short";

/// An untyped `App` over `bytes` — what opening a file with neither a
/// descriptor set nor a `--type` builds.
fn untyped_app(bytes: &[u8]) -> App {
    let mut ctx = DescriptorContext::empty_for_test();
    let decoded = decode(wrapped(bytes), &mut ctx, RootType::Raw, 2).unwrap();
    fixture_app(decoded, ctx)
}

/// What `prototext decode --raw` prints for `bytes`: prototext-core's
/// own render with no root descriptor, at the fixtures' indent of 2 and
/// with protolens's own render settings (annotations on, no `Any` or
/// MessageSet expansion — protolens does both itself, as overrides).
///
/// Built rather than pasted in on purpose. A hand-copied expectation
/// would pin what this render happened to emit on the day it was
/// written; this pins the *claim*, which is that the two agree.
fn raw_render(bytes: &[u8]) -> Vec<String> {
    let opts = DecodeRenderOpts {
        annotations: true,
        indent_size: 2,
        expand_any: false,
        expand_message_set: false,
        emit_header: false,
        ..Default::default()
    };
    let text = decode_and_render(bytes, None, opts);
    String::from_utf8(text)
        .expect("the render is UTF-8")
        .lines()
        .map(str::to_string)
        .collect()
}

/// The document with the wrapper's own header and footer taken off and
/// its one level of indentation removed — the part of it that answers
/// to `bytes`, in `raw_render`'s coordinates.
fn interior(app: &App) -> Vec<String> {
    let lines = app.document_lines();
    let (header, rest) = lines.split_first().expect("a rendered document has lines");
    let (footer, body) = rest.split_last().expect("a message node has a footer");
    assert_eq!(header, "1 {  #@ message = 1", "the wrapper's own header");
    assert_eq!(footer, "}", "the wrapper's own footer");
    body.iter()
        .map(|l| {
            l.strip_prefix("  ")
                .expect("every interior line is indented by the wrapper")
                .to_string()
        })
        .collect()
}

/// Spec 0299 G1: a document one bad token spoils is one opaque line
/// until the reader says otherwise, and what they get when they do is
/// exactly `prototext decode --raw`'s reading of the same bytes.
///
/// The truncation is still reported — it is real — but it is reported
/// on the record it belongs to instead of swallowing the file.
#[test]
fn overriding_the_root_to_message_renders_what_prototext_raw_renders() {
    let mut app = untyped_app(CUT_SHORT);
    assert_eq!(
        app.document_lines(),
        vec![r#"1: "\n\003abc\n\020short"  #@ string"#],
        "spec 0266: one invalid token disqualifies the whole payload",
    );

    app.run_command("override / --as message");

    assert_eq!(interior(&app), raw_render(CUT_SHORT));
    assert!(
        app.document_lines().len() > 2,
        "the payload must have been walked, not re-wrapped",
    );
}

/// The no-op half: on a well-formed payload the probe already descends,
/// so calling it a message must change nothing but the reader's
/// certainty.
#[test]
fn a_well_formed_payload_reads_the_same_either_way() {
    // `Inner { id: 5 }` under field 1 — nothing malformed anywhere, so
    // the probe descends on its own.
    let bytes = b"\x0a\x02\x08\x05";
    let mut app = untyped_app(bytes);
    let before = app.document_lines();

    app.run_command("override / --as message");

    assert_eq!(interior(&app), raw_render(bytes));
    assert_eq!(
        before[1..],
        app.document_lines()[1..],
        "the probe had already read it this way — only the wrapper's own \
         header changes, to say who decided",
    );
}

/// Spec 0299 S5's ladder order, which is spec 0135's: the pool is asked
/// first, so a real type named `message` still wins its own name — and
/// the keyword is only reachable when the pool has nothing to say.
#[test]
fn a_real_type_named_message_still_resolves_as_itself() {
    use crate::decode::WrapperTarget;
    use prost_types::field_descriptor_proto::Type;

    let mut bare = DescriptorContext::empty_for_test();
    let (target, field_type) = bare
        .wrapper_target_for("message", false)
        .expect("the keyword resolves with no pool at all");
    assert_eq!(field_type, Type::Message);
    let Some(WrapperTarget::Message(desc)) = target else {
        panic!("spec 0299: the keyword carries a message target");
    };
    assert_eq!(
        desc.fields().count(),
        0,
        "zero fields is the mechanism: every field met is an unknown one",
    );
    assert!(
        bare.pool().get_message_by_name(desc.full_name()).is_some(),
        "the target is registered in the pool",
    );

    let fds = proto3_fds_in(
        "",
        "test_named_message.proto",
        vec![message("message", vec![])],
    );
    let mut occupied = ctx_from_fds("named-message", &fds);
    let (target, _) = occupied
        .wrapper_target_for("message", false)
        .expect("a type of that name resolves");
    let Some(WrapperTarget::Message(desc)) = target else {
        panic!("a message target");
    };
    assert_eq!(
        desc.full_name(),
        "message",
        "the pool is asked first — the same rule that makes a message \
         named `bool` resolve as a message",
    );
}

/// Spec 0299 S6 / §3: the keyword is a `WT_LEN` reading, and a bare
/// FQDN that collides with it is shown with a leading dot so the two
/// are never read as each other.
#[test]
fn the_keyword_is_offered_for_len_and_shadows_a_bare_fqdn() {
    use prototext_core::helpers::{WT_LEN, WT_START_GROUP, WT_VARINT};

    assert!(decode::override_keywords_for_wire_type(WT_LEN).contains(&"message"));
    assert!(!decode::override_keywords_for_wire_type(WT_VARINT).contains(&"message"));
    assert!(decode::override_keywords_for_wire_type(WT_START_GROUP).is_empty());

    assert_eq!(override_display::format_fqdn_label("message"), ".message");
    assert_eq!(
        override_display::format_fqdn_label("test.message"),
        "test.message",
        "only a bare name can collide",
    );
}

// ── Spec 0302 tests ───────────────────────────────────────────────────────────

/// A TRUNCATED_BYTES field with two available payload bytes that form a
/// valid sub-field (`field 1, varint 5`). Declared length is 20; actual
/// payload is 2 bytes.
///
/// The truncated field: tag `\x0a` (field 1, LEN), length `\x14` (= 20),
/// then `\x08\x05` (field 1, varint 5). The walker sees 20 declared bytes
/// but only 2 are present.
const TRUNC_WITH_CHILDREN: &[u8] = b"\x0a\x14\x08\x05";

/// Spec 0302 S1: the arena walk descends into a TRUNCATED_BYTES field's
/// available bytes and allocates child slots for any valid sub-fields found
/// there, so that a later `message` override can splice them in without
/// `overlay_spans` panicking.
///
/// Before spec 0302 the TRUNCATED_BYTES node was a leaf; after it, the
/// node at arena slot 1 has one child (the `\x08\x05` varint field).
#[test]
fn truncated_bytes_arena_has_children() {
    let app = untyped_app(TRUNC_WITH_CHILDREN);

    // The wrapped blob has one top-level field (the wrapped content).
    // That field's payload = TRUNC_WITH_CHILDREN = one TRUNCATED_BYTES
    // field. Slot 0 is the wrapper's LEN field; slot 1 is the
    // TRUNCATED_BYTES field.
    let trunc_slot = {
        let fc = app.arena.first_child();
        fc[0] as usize // first child of root = the TRUNCATED_BYTES slot
    };
    let child_count_in_arena = {
        let fc = app.arena.first_child();
        fc[trunc_slot + 1] as usize - fc[trunc_slot] as usize
    };
    assert!(
        child_count_in_arena > 0,
        "spec 0302 S1: the arena must descend into the available bytes \
         of a TRUNCATED_BYTES field and allocate child slots \
         (got 0 children for slot {trunc_slot})",
    );
}

/// Spec 0302 G1 / S1: a `message` override on a TRUNCATED_BYTES node
/// commits successfully and the node becomes `is_message = true`.
///
/// Without the arena fix the commit would either panic (`overlay_spans`
/// asserting every rendered span has a slot) or silently produce another
/// TRUNCATED_BYTES line instead of nested content.
#[test]
fn message_override_on_truncated_bytes_commits() {
    let mut app = untyped_app(TRUNC_WITH_CHILDREN);

    // Open the root as a message first so its child (the TRUNCATED_BYTES
    // field) becomes navigable at `/1`.
    app.run_command("override / --as message");

    // The TRUNCATED_BYTES field is the first (and only) child of the root.
    let trunc_idx = app
        .resolve_path("/1")
        .expect("TRUNCATED_BYTES field must be reachable at /1 after root override");

    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("TRUNCATED_BYTES")),
        "the node must start out as TRUNCATED_BYTES before the override",
    );

    app.run_command(&format!(
        "override {} --as message",
        app.positional_path(trunc_idx)
    ));

    let trunc_idx_after = app
        .resolve_path("/1")
        .expect("node must still be reachable after the override");
    assert!(
        app.tree[trunc_idx_after].span.is_message,
        "spec 0302 G1: after committing a `message` override the node \
         must be is_message = true",
    );
    assert!(
        !app.document_lines()
            .iter()
            .any(|l| l.contains("TRUNCATED_BYTES")),
        "the committed render must not contain TRUNCATED_BYTES — it \
         opened the available bytes as a nested message",
    );
}

/// Spec 0302 S3: `status_type_label` returns `(\"message\", Some(\"message\"))`
/// for a node whose active override is the `message` keyword but whose
/// rendered span still has `is_message = false`.
///
/// This is the state that existed for every TRUNCATED_BYTES node before
/// the arena fix (the commit decoded to TRUNCATED_BYTES again), and it is
/// still reachable during a preview of any node where the commit has not
/// yet run. The test injects the override entry directly to avoid depending
/// on timing.
#[test]
fn status_line_shows_message_not_enum() {
    let mut app = untyped_app(TRUNC_WITH_CHILDREN);
    // Open the root as message so the child is rendered.
    app.run_command("override / --as message");

    let trunc_idx = app
        .resolve_path("/1")
        .expect("TRUNCATED_BYTES field must be reachable at /1");
    let path = app.positional_path(trunc_idx);

    // Plant an active override with type "message" without committing —
    // simulates the preview / pre-arena-fix state where is_message stays
    // false while the entry is present.
    app.overrides.activate(
        OverrideOrigin::Path { path },
        Some(decode::MESSAGE_KEYWORD.to_string()),
    );

    // The span must still be is_message = false (we did not splice).
    assert!(
        !app.tree[trunc_idx].span.is_message,
        "precondition: is_message must be false for this test to be meaningful",
    );

    let (label, tag) = app
        .status_type_label(trunc_idx)
        .expect("active `message` override must produce a status label");
    assert_eq!(
        label, "message",
        "spec 0302 S3: the label must be the bare keyword, not `.message`",
    );
    assert_eq!(
        tag,
        Some("message"),
        "spec 0302 S3: the tag must be `message`, not `enum`",
    );
}

// ── Spec 0303 tests ───────────────────────────────────────────────────────────

/// Spec 0303 G1/S2: after committing a `message` override on a
/// TRUNCATED_BYTES node, the header line carries `TRUNCATED_MESSAGE; MISSING:
/// N` with the correct count.
///
/// `TRUNC_WITH_CHILDREN`: tag `\x0a` (field 1, LEN), length `\x14` (= 20),
/// then `\x08\x05` (2 available bytes). Missing = 20 - 2 = 18.
#[test]
fn truncated_message_header_carries_missing() {
    let mut app = untyped_app(TRUNC_WITH_CHILDREN);
    app.run_command("override / --as message");

    let trunc_idx = app
        .resolve_path("/1")
        .expect("TRUNCATED_BYTES field must be reachable at /1");
    app.run_command(&format!(
        "override {} --as message",
        app.positional_path(trunc_idx)
    ));

    // The header line of the now-message node must carry the annotation.
    let lines = app.document_lines();
    let header = lines
        .iter()
        .find(|l| l.contains("TRUNCATED_MESSAGE"))
        .expect("spec 0303 G1: the header must contain TRUNCATED_MESSAGE after the commit");

    assert!(
        header.contains("MISSING: 18"),
        "spec 0303 S2: MISSING count must be 20 - 2 = 18, got: {header}",
    );
}

/// Spec 0303 G2/S6: `encode_text_to_binary` inflates the declared length by
/// the `MISSING` value, reconstructing the original declared length varint.
///
/// The prototext round-trip: override the TRUNCATED_BYTES child as `message`,
/// collect the document text, encode it back to binary, and assert the
/// re-encoded binary's length varint for the truncated field equals the
/// original declared length (20), not the actual payload length (2).
#[test]
fn encoder_inflates_length_for_truncated_message() {
    let mut app = untyped_app(TRUNC_WITH_CHILDREN);
    app.run_command("override / --as message");

    let trunc_idx = app
        .resolve_path("/1")
        .expect("TRUNCATED_BYTES field must be reachable at /1");
    app.run_command(&format!(
        "override {} --as message",
        app.positional_path(trunc_idx)
    ));

    // Collect the full annotated document text.
    let lines = app.document_lines();
    let prototext = format!("#@ prototext: protoc\n{}\n", lines.join("\n"));

    let wire = prototext_core::serialize::encode_text::encode_text_to_binary(prototext.as_bytes());

    // TRUNC_WITH_CHILDREN = \x0a \x14 \x08 \x05
    // The wrapper outer field gets its own tag+length (2 bytes tag-field-1 +
    // varint for wrapper length).  The inner TRUNCATED field must have its
    // own LEN varint = 20 (= \x14), not 2 (= \x02).
    assert!(
        wire.contains(&0x14),
        "spec 0303 G2: the re-encoded binary must contain the original declared \
         length varint (0x14 = 20), not the actual payload length",
    );
    assert!(
        !wire.ends_with(&[0x02, 0x08, 0x05]),
        "spec 0303 G2: the inner field's length varint must be 20, not 2",
    );
}

/// Spec 0303 S3: a normal (non-truncated) `message` override does NOT emit
/// `TRUNCATED_MESSAGE` — the annotation is specific to the truncated case.
#[test]
fn normal_message_override_has_no_truncated_annotation() {
    // Well-formed: field 1 = varint 5, inside field 1 outer (no truncation).
    let bytes = b"\x0a\x02\x08\x05";
    let mut app = untyped_app(bytes);
    app.run_command("override / --as message");

    let lines = app.document_lines();
    assert!(
        !lines.iter().any(|l| l.contains("TRUNCATED_MESSAGE")),
        "spec 0303 S3: a non-truncated field must not carry TRUNCATED_MESSAGE, \
         got lines: {lines:?}",
    );
}

/// Spec 0303 S7: `TRUNCATED_MESSAGE` is in the INVALID annotation tier.
#[test]
fn truncated_message_is_invalid_tier() {
    use crate::annotation::{tier_of, Tier};
    assert_eq!(
        tier_of("TRUNCATED_MESSAGE"),
        Some(Tier::Invalid),
        "spec 0303 S7: TRUNCATED_MESSAGE must be @annotation.invalid",
    );
}

/// Spec 0303 S8: `clause` returns an explanation for `TRUNCATED_MESSAGE`.
#[test]
fn annotation_explains_truncated_message() {
    use crate::annotation::clause;
    let explanation = clause("TRUNCATED_MESSAGE").expect("spec 0303 S8: must have a clause");
    assert!(
        explanation.contains("declared length") && explanation.contains("available bytes"),
        "spec 0303 S8: explanation must mention the declared length and available bytes, \
         got: {explanation}",
    );
}

/// Spec 0302 S4: the `none` keyword is the lowercase string `\"none\"`,
/// matching every other override keyword. The first candidate in
/// lexicographic mode is `\"none\"`, and activating it produces a raw
/// render (no splice — spec 0237).
#[test]
fn none_keyword_is_lowercase() {
    let mut app = untyped_app(CUT_SHORT);
    // Open the root as a message so children are visible.
    app.run_command("override / --as message");

    // Point the override pane at the root so `recompute_override_candidates`
    // fills the list in lexicographic order.
    app.override_target = Some(app.first_node);
    app.override_sort = SortMode::Lexicographic;
    app.recompute_override_candidates();

    assert_eq!(
        app.override_candidates.first().map(|(s, _)| s.as_str()),
        Some(decode::NONE_KEYWORD),
        "spec 0302 S4: the first override candidate must be the \
         lowercase `none` keyword, not `protolens_internal.None` or `None`",
    );
    assert_eq!(decode::NONE_KEYWORD, "none");

    // Committing `none` on the root resets it to raw bytes — no splice.
    app.run_command("override / --as none");
    assert_eq!(
        app.document_lines(),
        vec![r#"1: "\n\003abc\n\020short"  #@ string"#],
        "after `none` the root must revert to the raw single-line render",
    );
}

/// Spec 0247 S10 composed with spec 0299: a truncation is reported by the
/// node it belongs to, and every node above it goes red for it.
///
/// `/` itself is never truncated — `Blob` writes the wrapper's length from
/// the buffer it actually has — so `/` says nothing about truncation on its
/// own line and must still wear the color. That combination is the whole
/// reason no annotation needs to be invented for the declined render.
#[test]
fn a_truncated_record_reddens_every_node_above_it() {
    let mut app = untyped_app(CUT_SHORT);
    app.run_command("override / --as message");

    let lines = app.document_lines();
    let row = lines
        .iter()
        .position(|l| l.contains("TRUNCATED_BYTES"))
        .expect("the cut record reports itself");
    let cut = app
        .line_pos(row)
        .expect("the row is inside the document")
        .node;

    assert_ne!(
        cut, app.first_node,
        "the truncation is never the root's own — which is what makes the \
         walk below non-vacuous, and `/` red only by inheritance",
    );
    assert_eq!(
        app.status_own[cut],
        Status::Invalid,
        "the record whose length overruns the buffer is the one accusing",
    );

    let mut node = cut;
    while let Some(parent) = app.parent(node) {
        assert_eq!(
            app.status_of(parent),
            Status::Invalid,
            "node {parent} is above the truncation and must show it",
        );
        assert_eq!(
            app.status_own[parent],
            Status::Ok,
            "node {parent} is intact itself; the red is inherited, not its own",
        );
        node = parent;
    }
    assert_eq!(node, app.first_node, "the walk reached the root");
}
