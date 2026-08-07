// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::ops::Range;

use prost_reflect::{Cardinality, Kind, MessageDescriptor};

use super::super::sink::{GroupCloseFacts, NestedKind, ProbeSink, ScalarValue, Sink, TagFacts};
use super::super::{
    at_depth_cap, enter_level, render_message, FieldOrExt, EXPAND_ANY, EXPAND_MESSAGE_SET,
    HIDE_UNKNOWN,
};
use super::any_field::render_any_expansion;
use super::message_set_field::{is_message_set, render_message_set_expansion};

/// Per-field identity shared by `render_len_field`, `render_group_field`,
/// `render_any_expansion`, and `render_message_set_expansion` — bundled to
/// avoid `clippy::too_many_arguments`, mirroring the `ScalarCtx` pattern
/// (`scalar.rs`, spec 0110 §8).
pub(in super::super) struct FieldCtx<'a> {
    pub(in super::super) field_number: u64,
    pub(in super::super) field_schema: Option<&'a FieldOrExt>,
    pub(in super::super) tag: TagFacts,
}

/// Render a length-delimited field (string, bytes, message, packed, wire-bytes).
pub(in super::super) fn render_len_field<S: Sink>(
    ctx: FieldCtx<'_>,
    schema_present: bool,
    raw_range: Range<usize>,
    data: &[u8],
    sink: &mut S,
) {
    let FieldCtx {
        field_number,
        field_schema,
        tag,
    } = ctx;
    if sink.treat_len_as_opaque() {
        sink.scalar_field(
            field_number,
            field_schema,
            tag,
            ScalarValue::Bytes(data),
            raw_range,
            schema_present,
        );
        return;
    }

    // Spec 0171 §S4: at the render-recursion depth cap, hand the payload back
    // opaquely rather than descending into it. One check covers all four of
    // this function's recursive branches — nested message, spec-0097 probe,
    // Any expansion, MessageSet expansion — and it is made before anything is
    // written, so the parent's output needs no retraction.
    //
    // The schema is deliberately dropped: `ScalarValue::Bytes` with a
    // message-kind schema lands in `TextSink`'s wire-type-mismatch arm, which
    // annotates TYPE_MISMATCH and, with annotations off, emits nothing at all
    // — losing the bytes. `None` takes the unknown-field rendering instead,
    // which is always emitted and always re-encodes to tag + length + payload.
    if at_depth_cap() {
        sink.scalar_field(
            field_number,
            None,
            tag,
            ScalarValue::Bytes(data),
            raw_range,
            schema_present,
        );
        return;
    }

    let Some(fs) = field_schema else {
        // Unknown LEN field: three-step cascade (spec 0097).
        //
        // When no descriptor is active, every field is unknown — suppress
        // nothing regardless of hide_unknown_fields (spec 0097 S5).
        // When a descriptor is active but this field number is absent, honour
        // hide_unknown_fields (spec 0103).
        let hide_unknown = HIDE_UNKNOWN.with(|c| c.get());
        if hide_unknown && schema_present {
            return;
        }

        // Step 1: probe as nested message via `ProbeSink` (spec 0110 §2/Step 4).
        // Rendering failures inside the nested message do not affect this probe.
        //
        // This is the *only* place the verdict is computed. A sink building
        // the maximal tree recurses regardless of it (spec 0216 S14) — a
        // payload the probe declines is one a later type override could
        // still declare a message, and the render would then need child
        // nodes that were never created — but it is handed the verdict all
        // the same, on `NestedKind::Message`, so that recording what the
        // cascade would have decided never means re-deriving it.
        let probed_as_message = {
            let mut probe = ProbeSink::default();
            let (next_pos, _) = render_message(data, 0, None, None, false, &mut probe);
            probe.malformity_count() == 0 && next_pos == data.len()
        };
        if probed_as_message || sink.unknown_len_is_message() {
            let mark = sink.begin_nested(
                field_number,
                None,
                tag,
                NestedKind::Message {
                    probed_as_message: Some(probed_as_message),
                },
                raw_range.start,
                raw_range.end - data.len(),
            );
            {
                let _guard = enter_level(sink);
                render_message(data, 0, None, None, schema_present, sink);
            }
            sink.end_nested(mark, raw_range, None);
            return;
        }

        // Steps 2/3 (UTF-8 string, else raw bytes) are collapsed into
        // `scalar_field`'s `ScalarValue::Bytes` handling for `field_schema: None`.
        sink.scalar_field(
            field_number,
            None,
            tag,
            ScalarValue::Bytes(data),
            raw_range,
            schema_present,
        );
        return;
    };

    let is_repeated = fs.cardinality() == Cardinality::Repeated;

    // ── Packed repeated ───────────────────────────────────────────────────────
    let is_packable_kind = matches!(
        fs.kind(),
        Kind::Bool
            | Kind::Int32
            | Kind::Int64
            | Kind::Uint32
            | Kind::Uint64
            | Kind::Sint32
            | Kind::Sint64
            | Kind::Fixed32
            | Kind::Fixed64
            | Kind::Sfixed32
            | Kind::Sfixed64
            | Kind::Float
            | Kind::Double
            | Kind::Enum(_)
    );
    // Determine whether this LEN record encodes a packed repeated field.
    //
    // Normally we trust prost-reflect's precomputed `is_packed()`.  However,
    // prost-reflect has a bug (see docs/prototext/PROST-ISSUES.md §1): for proto3
    // repeated scalar/enum fields, `is_packed()` returns false when
    // `FieldOptions` is present-but-empty in the FDS (e.g. because an
    // unrelated custom option such as `google.api.field_behavior` is set on
    // another field in the same message).  The proto3 spec mandates packed
    // encoding for such fields unless `[packed=false]` is explicitly set.
    //
    // When the `prost-bug-workaround` feature is enabled and the conditions
    // for the bug are met (proto3, repeated, packable kind), we apply the
    // correct rule ourselves: packed unless `raw_packed_option()` is
    // `Some(false)`.
    let use_packed = if is_repeated && is_packable_kind {
        #[cfg(feature = "prost-bug-workaround")]
        {
            if fs.parent_file_syntax() == prost_reflect::Syntax::Proto3 {
                // Correct proto3 rule: packed unless explicitly set to false.
                fs.raw_packed_option() != Some(false)
            } else {
                fs.is_packed()
            }
        }
        #[cfg(not(feature = "prost-bug-workaround"))]
        {
            fs.is_packed()
        }
    } else {
        false
    };
    if use_packed {
        sink.scalar_field(
            field_number,
            Some(fs),
            tag,
            ScalarValue::Packed(data),
            raw_range,
            schema_present,
        );
        return;
    }

    // ── Nested message ────────────────────────────────────────────────────────
    // Note: groups are represented as Kind::Message in prost-reflect.  A GROUP
    // field received on a LEN wire record is a wire-type mismatch — fall
    // through to the generic String/Bytes/mismatch path below.  Intercepted
    // here (before that generic path) because Kind::Message on a LEN wire
    // record is the ordinary, non-mismatch case.
    if let Kind::Message(nested_msg_desc) = fs.kind() {
        if !fs.is_group() {
            // Any expansion intercept (spec 0089): if the field type is
            // google.protobuf.Any and EXPAND_ANY is set, try to expand the
            // value inline using the resolved type from type_url.
            if EXPAND_ANY.with(|c| c.get())
                && nested_msg_desc.full_name() == "google.protobuf.Any"
                && render_any_expansion(
                    FieldCtx {
                        field_number,
                        field_schema: Some(fs),
                        tag,
                    },
                    schema_present,
                    raw_range.clone(),
                    data,
                    sink,
                )
            {
                return;
            }

            // MessageSet expansion intercept (spec 0100): if the field type
            // is a MessageSet (structural heuristic), expand groups inline.
            if EXPAND_MESSAGE_SET.with(|c| c.get()) && is_message_set(&nested_msg_desc) {
                render_message_set_expansion(
                    &nested_msg_desc,
                    FieldCtx {
                        field_number,
                        field_schema: Some(fs),
                        tag,
                    },
                    schema_present,
                    raw_range,
                    data,
                    sink,
                );
                return;
            }

            let nested_schema: Option<&MessageDescriptor> = Some(&nested_msg_desc);

            let mark = sink.begin_nested(
                field_number,
                Some(fs),
                tag,
                NestedKind::Message {
                    // A declared message field: the schema decided, no probe ran.
                    probed_as_message: None,
                },
                raw_range.start,
                raw_range.end - data.len(),
            );
            {
                let _guard = enter_level(sink);
                render_message(data, 0, None, nested_schema, schema_present, sink);
            }
            sink.end_nested(mark, raw_range, None);
            return;
        }
    }

    // ── String / Bytes / wire-type mismatch — unified via `ScalarValue::Bytes` ─
    // `TextSink::scalar_field`'s `Bytes` arm already dispatches on
    // `field_schema.kind()` for String, Bytes, and (catch-all) mismatch.
    sink.scalar_field(
        field_number,
        Some(fs),
        tag,
        ScalarValue::Bytes(data),
        raw_range,
        schema_present,
    );
}

/// Render a GROUP field (proto2), with greedy rendering and post-hoc fixup.
pub(in super::super) fn render_group_field<S: Sink>(
    buf: &[u8],
    pos: &mut usize,
    ctx: FieldCtx<'_>,
    schema_present: bool,
    raw_start: usize,
    sink: &mut S,
) {
    let FieldCtx {
        field_number,
        field_schema,
        tag,
    } = ctx;
    // Determine nested schema.  `msg_desc` from `fs.kind()` is already live
    // and correct — no lookup needed (spec 0106 S1).  Mismatch/unknown-field
    // annotation details (field_decl, TYPE_MISMATCH, tag/close-tag modifiers)
    // are computed by `Sink::begin_nested`/`end_nested`'s own implementation.
    let nested_msg_desc: Option<MessageDescriptor> = field_schema.and_then(|fs| {
        if fs.is_group() {
            if let Kind::Message(msg_desc) = fs.kind() {
                Some(msg_desc)
            } else {
                None
            }
        } else {
            None
        }
    });
    let nested_schema_opt: Option<&MessageDescriptor> = nested_msg_desc.as_ref();

    let mark = sink.begin_nested(
        field_number,
        field_schema,
        tag,
        NestedKind::Group,
        raw_start,
        // Groups have no length prefix: `render_message` below continues
        // parsing the *same* `buf`/coordinate frame as this group's own
        // tag (spec 0110 § Design rationale) — `0` means "no frame reset".
        0,
    );

    // ── Recurse: parse and render child fields ────────────────────────────────
    let start = *pos;
    let (new_pos, end_tag) = {
        let _guard = enter_level(sink);
        render_message(
            buf,
            start,
            Some(field_number),
            nested_schema_opt,
            schema_present,
            sink,
        )
    };
    *pos = new_pos;

    let close_facts = end_tag.as_ref().map(|et| {
        let end_field = et.wfield.unwrap_or(0);
        GroupCloseFacts {
            end_tag_overhang_count: et.wfield_ohb,
            end_tag_is_out_of_range: et.wfield_oor.is_some(),
            mismatched_group_end: if end_field != field_number {
                Some(end_field)
            } else {
                None
            },
        }
    });

    sink.end_nested(mark, raw_start..*pos, close_facts);
}
