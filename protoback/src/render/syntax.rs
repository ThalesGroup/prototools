// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The syntax decisions of a field's rendering (reproto's `syntax.py`):
//! each on what it decides from alone, the syntax the file is written in
//! (`target`), its own (`syntax`), the field, and its resolved features
//! (`None` outside an editions file), so that each can be tested on its
//! own cases (spec 0488).

use prototext_upb::descriptor_access::FieldDescriptorProto;

use super::message::{LABEL_REPEATED, LABEL_REQUIRED};
use crate::features::{
    FIELD_PRESENCE_IMPLICIT, FIELD_PRESENCE_LEGACY_REQUIRED, MESSAGE_ENCODING_DELIMITED,
};

/// `proto2` or `editions`: a target that has groups.
fn proto2_or_editions(target: &str) -> bool {
    target == "proto2" || target == "editions"
}

/// `field_label`: the label written before a field's type, `presence` its
/// resolved field presence.
pub fn field_label(
    target: &str,
    f: &FieldDescriptorProto<'_>,
    is_oneof: bool,
    presence: Option<i32>,
) -> &'static str {
    if is_oneof {
        return "";
    }
    if f.label() == LABEL_REPEATED {
        return "repeated ";
    }
    if target == "editions" {
        return "";
    }
    if let Some(p) = presence {
        // An editions file written as proto2: presence picks the label.
        return if p == FIELD_PRESENCE_LEGACY_REQUIRED {
            "required "
        } else {
            "optional "
        };
    }
    if target == "proto3" {
        if f.label() == LABEL_REQUIRED {
            return "";
        }
        return if f.proto3_optional() { "optional " } else { "" };
    }
    if f.label() == LABEL_REQUIRED {
        "required "
    } else {
        "optional "
    }
}

/// `packed_option`: `true`, `false`, or nothing; `has`, the field sets
/// `packed`; `effective`, it is packed; `editions`, its features resolved.
pub fn packed_option(
    target: &str,
    syntax: &str,
    has: bool,
    effective: bool,
    editions: bool,
) -> Option<&'static str> {
    let v = if effective { "true" } else { "false" };
    if editions {
        if has {
            return Some(v);
        }
        if target == "proto2" {
            return effective.then_some("true");
        }
        return None;
    }
    if has {
        return Some(v);
    }
    if syntax == target {
        return None;
    }
    effective.then_some("true")
}

/// `allow_groups`: a field may be written as a group, `encoding` its
/// resolved message encoding.
pub fn allow_groups(target: &str, encoding: Option<i32>) -> bool {
    match encoding {
        Some(e) => e == MESSAGE_ENCODING_DELIMITED,
        None => proto2_or_editions(target),
    }
}

/// `is_synthetic_oneof`: protoc's rule (`OneofDescriptor::is_synthetic`),
/// whatever the target syntax: one field, which has `proto3_optional`
/// (spec 0429 S13); in an editions file, a `_`-named oneof whose one field
/// is IMPLICIT, `presence` that field's resolved presence.
pub fn is_synthetic_oneof(
    name: &str,
    members: &[&FieldDescriptorProto<'_>],
    presence: Option<i32>,
) -> bool {
    if members.len() != 1 {
        return false;
    }
    match presence {
        Some(p) => name.starts_with('_') && p == FIELD_PRESENCE_IMPLICIT,
        None => members[0].proto3_optional(),
    }
}

/// `should_render_default`: a field's default value is written.
pub fn should_render_default(
    target: &str,
    f: &FieldDescriptorProto<'_>,
    presence: Option<i32>,
) -> bool {
    f.has_default_value()
        && match presence {
            Some(p) => p != FIELD_PRESENCE_IMPLICIT,
            None => target != "proto3",
        }
}

/// The unit tests of `test_editions_rendering.py` (spec 0488 S2), T1 to
/// T12, on these functions; names lower-cased. reproto's `Context` stood
/// for the target and file syntax, its `ResolvedFeatures` for the one
/// feature each decision reads (for `packed_option`, that the file has
/// features at all: the T5 and T6 tests' PACKED and EXPANDED are what
/// `effective` says); its `_field` stub is built here from text.
#[cfg(test)]
mod test_editions_rendering {
    use super::*;
    use crate::features::{FIELD_PRESENCE_IMPLICIT, FIELD_PRESENCE_LEGACY_REQUIRED};
    use prototext_upb::descriptor_access::FileDescriptorProto;
    use prototext_upb::{DynamicMessage, Pool};

    /// `FieldPresence.EXPLICIT`, `MessageEncoding.LENGTH_PREFIXED`.
    const FIELD_PRESENCE_EXPLICIT: i32 = 1;
    const MESSAGE_ENCODING_LENGTH_PREFIXED: i32 = 1;

    const OPTIONAL: &str = "LABEL_OPTIONAL";
    const REQUIRED: &str = "LABEL_REQUIRED";
    const REPEATED: &str = "LABEL_REPEATED";

    /// A file holding `fields` (`_field`: name `x`, number 1, int32, with
    /// the label, and `proto3_optional` or a default of 42 when asked).
    fn file(fields: &[(&str, bool, bool)]) -> DynamicMessage {
        let body: String = fields
            .iter()
            .map(|(label, optional, default)| {
                format!(
                    "field {{ name: \"x\" number: 1 label: {label} type: TYPE_INT32{}{} }} ",
                    if *optional {
                        " proto3_optional: true"
                    } else {
                        ""
                    },
                    if *default {
                        " default_value: \"42\""
                    } else {
                        ""
                    },
                )
            })
            .collect();
        let text = format!("name: \"t.proto\" message_type {{ name: \"M\" {body}}}");
        let bytes = prototext_upb::text_format::parse_descriptor(&text, false).unwrap();
        let def = Pool::vendored()
            .get_message_by_name("google.protobuf.FileDescriptorProto")
            .unwrap();
        DynamicMessage::decode(&def, &bytes).unwrap()
    }

    /// The label of a one-field file's field.
    fn label(
        target: &str,
        field: (&str, bool, bool),
        is_oneof: bool,
        presence: Option<i32>,
    ) -> &'static str {
        let f = file(&[field]);
        let m = FileDescriptorProto(f.as_ref())
            .message_type()
            .swap_remove(0);
        field_label(target, &m.field()[0], is_oneof, presence)
    }

    /// features=None: proto2 labels unchanged.
    #[test]
    fn test_t1_field_label_proto2_unchanged() {
        assert_eq!(
            label("proto2", (OPTIONAL, false, false), false, None),
            "optional "
        );
        assert_eq!(
            label("proto2", (REQUIRED, false, false), false, None),
            "required "
        );
        assert_eq!(
            label("proto2", (REPEATED, false, false), false, None),
            "repeated "
        );
        // is_oneof always yields ''
        assert_eq!(label("proto2", (OPTIONAL, false, false), true, None), "");
    }

    /// features=None: proto3 labels unchanged.
    #[test]
    fn test_t1_field_label_proto3_unchanged() {
        assert_eq!(label("proto3", (OPTIONAL, false, false), false, None), "");
        assert_eq!(
            label("proto3", (OPTIONAL, true, false), false, None),
            "optional "
        );
        assert_eq!(
            label("proto3", (REPEATED, false, false), false, None),
            "repeated "
        );
    }

    #[test]
    fn test_t2_field_label_editions_explicit() {
        assert_eq!(
            label(
                "proto2",
                (OPTIONAL, false, false),
                false,
                Some(FIELD_PRESENCE_EXPLICIT)
            ),
            "optional "
        );
    }

    /// IMPLICIT: written as editions, ''; as proto2, 'optional ', its
    /// closest wire-compatible label.
    #[test]
    fn test_t3_field_label_editions_implicit() {
        let p = Some(FIELD_PRESENCE_IMPLICIT);
        assert_eq!(label("editions", (OPTIONAL, false, false), false, p), "");
        assert_eq!(
            label("proto2", (OPTIONAL, false, false), false, p),
            "optional "
        );
    }

    #[test]
    fn test_t4_field_label_editions_legacy_required() {
        assert_eq!(
            label(
                "proto2",
                (OPTIONAL, false, false),
                false,
                Some(FIELD_PRESENCE_LEGACY_REQUIRED)
            ),
            "required "
        );
    }

    /// PACKED, not set: written as editions, nothing (the features block);
    /// as proto2, 'true', proto2 unpacked by default.
    #[test]
    fn test_t5_packed_option_editions_packed_default() {
        assert_eq!(
            packed_option("editions", "editions", false, true, true),
            None
        );
        assert_eq!(
            packed_option("proto2", "editions", false, true, true),
            Some("true")
        );
    }

    /// EXPANDED, not set: nothing either way, unpacked proto2's default.
    #[test]
    fn test_t6_packed_option_editions_expanded() {
        assert_eq!(
            packed_option("editions", "editions", false, false, true),
            None
        );
        assert_eq!(
            packed_option("proto2", "editions", false, false, true),
            None
        );
    }

    /// Set, in an editions file: its value (a legacy round trip).
    #[test]
    fn test_t6_packed_option_editions_has_field_emits() {
        assert_eq!(
            packed_option("proto2", "proto2", true, true, true),
            Some("true")
        );
        assert_eq!(
            packed_option("proto2", "proto2", true, false, true),
            Some("false")
        );
    }

    #[test]
    fn test_t7_allow_groups_editions_delimited() {
        assert!(allow_groups("proto2", Some(MESSAGE_ENCODING_DELIMITED)));
    }

    #[test]
    fn test_t8_allow_groups_editions_length_prefixed() {
        assert!(!allow_groups(
            "proto2",
            Some(MESSAGE_ENCODING_LENGTH_PREFIXED)
        ));
    }

    /// `is_synthetic_oneof` on `n` members named `name`, IMPLICIT or not.
    fn synthetic(name: &str, n: usize, presence: i32) -> bool {
        let f = file(&vec![(OPTIONAL, false, false); n]);
        let m = FileDescriptorProto(f.as_ref())
            .message_type()
            .swap_remove(0);
        let fields = m.field();
        let members: Vec<&FieldDescriptorProto<'_>> = fields.iter().collect();
        is_synthetic_oneof(name, &members, Some(presence))
    }

    /// One IMPLICIT member: a synthetic oneof.
    #[test]
    fn test_t9_is_synthetic_oneof_editions_implicit() {
        assert!(synthetic("_x", 1, FIELD_PRESENCE_IMPLICIT));
    }

    /// IMPLICIT, but a name without its `_`: a real oneof.
    #[test]
    fn test_t9_is_synthetic_oneof_editions_implicit_no_underscore() {
        assert!(!synthetic("real_oneof", 1, FIELD_PRESENCE_IMPLICIT));
    }

    /// IMPLICIT, but two members: a real oneof.
    #[test]
    fn test_t9_is_synthetic_oneof_editions_implicit_multi_member() {
        assert!(!synthetic("_x", 2, FIELD_PRESENCE_IMPLICIT));
    }

    /// One EXPLICIT member: a real oneof.
    #[test]
    fn test_t10_is_synthetic_oneof_editions_explicit() {
        assert!(!synthetic("_x", 1, FIELD_PRESENCE_EXPLICIT));
    }

    fn default(has: bool, presence: i32) -> bool {
        let f = file(&[(OPTIONAL, false, has)]);
        let m = FileDescriptorProto(f.as_ref())
            .message_type()
            .swap_remove(0);
        should_render_default("proto2", &m.field()[0], Some(presence))
    }

    /// IMPLICIT: no default, though one is set.
    #[test]
    fn test_t11_should_render_default_editions_implicit() {
        assert!(!default(true, FIELD_PRESENCE_IMPLICIT));
    }

    #[test]
    fn test_t12_should_render_default_editions_explicit() {
        assert!(default(true, FIELD_PRESENCE_EXPLICIT));
    }

    #[test]
    fn test_t12_should_render_default_editions_explicit_no_default() {
        assert!(!default(false, FIELD_PRESENCE_EXPLICIT));
    }
}
