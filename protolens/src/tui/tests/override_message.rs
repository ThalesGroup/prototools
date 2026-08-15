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
