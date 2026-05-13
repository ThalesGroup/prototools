// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use prost_reflect::{DescriptorPool, MessageDescriptor};

// ── Public types ──────────────────────────────────────────────────────────────

/// The parsed, indexed form of a `FileDescriptorSet`.
///
/// Owns a `DescriptorPool` and the fully-qualified name of the root message.
pub struct ParsedSchema {
    pool: DescriptorPool,
    root_full_name: String,
}

impl ParsedSchema {
    /// Construct an empty (no-schema) `ParsedSchema`.
    pub fn empty() -> Self {
        ParsedSchema {
            pool: DescriptorPool::new(),
            root_full_name: String::new(),
        }
    }

    /// Return the `MessageDescriptor` for the root message, or `None` for an
    /// empty schema (no-schema mode).
    pub fn root_descriptor(&self) -> Option<MessageDescriptor> {
        if self.root_full_name.is_empty() {
            None
        } else {
            self.pool.get_message_by_name(&self.root_full_name)
        }
    }

    /// Look up a message descriptor by fully-qualified name (no leading dot).
    pub fn get_descriptor(&self, fqn: &str) -> Option<MessageDescriptor> {
        self.pool.get_message_by_name(fqn)
    }

    /// Access the underlying descriptor pool.
    pub fn pool(&self) -> &DescriptorPool {
        &self.pool
    }
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that can occur while parsing a protobuf schema descriptor.
#[non_exhaustive]
#[derive(Debug)]
pub enum SchemaError {
    /// `schema_bytes` could not be decoded as a `FileDescriptorSet`.
    InvalidDescriptor(String),
    /// The requested root message was not found in the parsed descriptor.
    MessageNotFound(String),
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::InvalidDescriptor(msg) => write!(f, "invalid descriptor: {msg}"),
            SchemaError::MessageNotFound(msg) => write!(f, "message not found: {msg}"),
        }
    }
}

impl std::error::Error for SchemaError {}

// ── Public entry point ────────────────────────────────────────────────────────

/// Decode `schema_bytes` into a `DescriptorPool`, for use with
/// `schema_from_pool` when instantiating multiple types from the same bytes.
pub fn decode_pool(schema_bytes: &[u8]) -> Result<DescriptorPool, SchemaError> {
    DescriptorPool::decode(schema_bytes).map_err(|e| SchemaError::InvalidDescriptor(e.to_string()))
}

/// Create a `ParsedSchema` from an already-decoded pool and a root message name.
pub fn schema_from_pool(
    pool: DescriptorPool,
    root_msg_name: &str,
) -> Result<ParsedSchema, SchemaError> {
    let root_full_name = root_msg_name.trim_start_matches('.').to_string();
    if pool.get_message_by_name(&root_full_name).is_none() {
        let available = pool
            .all_messages()
            .map(|m| m.full_name().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(SchemaError::MessageNotFound(format!(
            "root message '{}' not found in schema (available: {})",
            root_full_name, available
        )));
    }
    Ok(ParsedSchema {
        pool,
        root_full_name,
    })
}

/// Parse `schema_bytes` into a `ParsedSchema`.
///
/// An empty `schema_bytes` or empty `root_msg_name` returns a schema whose
/// `root_descriptor()` is `None` (no-schema mode).
pub fn parse_schema(schema_bytes: &[u8], root_msg_name: &str) -> Result<ParsedSchema, SchemaError> {
    if schema_bytes.is_empty() || root_msg_name.is_empty() {
        return Ok(ParsedSchema::empty());
    }

    let pool = DescriptorPool::decode(schema_bytes)
        .map_err(|e| SchemaError::InvalidDescriptor(e.to_string()))?;

    // prost-reflect uses no leading dot; strip one if the caller passed it.
    let root_full_name = root_msg_name.trim_start_matches('.').to_string();

    if pool.get_message_by_name(&root_full_name).is_none() {
        let available = pool
            .all_messages()
            .map(|m| m.full_name().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(SchemaError::MessageNotFound(format!(
            "root message '{}' not found in schema (available: {})",
            root_full_name, available
        )));
    }

    Ok(ParsedSchema {
        pool,
        root_full_name,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message as ProstMessage;
    use prost_reflect::Kind;
    use prost_types::{
        DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
        FileDescriptorProto, FileDescriptorSet,
    };

    // proto_type integers used only to construct test descriptors
    const TYPE_ENUM: i32 = 14;
    const TYPE_INT32: i32 = 5;
    const LABEL_OPTIONAL: i32 = 1;

    /// Build a minimal FileDescriptorSet bytes containing one file with the
    /// given message descriptor and any top-level enums provided.
    fn build_fds(enums: Vec<EnumDescriptorProto>, message: DescriptorProto) -> Vec<u8> {
        let file = FileDescriptorProto {
            name: Some("test.proto".into()),
            syntax: Some("proto2".into()),
            enum_type: enums,
            message_type: vec![message],
            ..Default::default()
        };
        let fds = FileDescriptorSet { file: vec![file] };
        let mut buf = Vec::new();
        fds.encode(&mut buf).unwrap();
        buf
    }

    fn enum_value(name: &str, number: i32) -> EnumValueDescriptorProto {
        EnumValueDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            ..Default::default()
        }
    }

    fn enum_field(name: &str, number: i32, type_name: &str) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            r#type: Some(TYPE_ENUM),
            label: Some(LABEL_OPTIONAL),
            type_name: Some(type_name.into()),
            ..Default::default()
        }
    }

    fn int32_field(name: &str, number: i32) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            r#type: Some(TYPE_INT32),
            label: Some(LABEL_OPTIONAL),
            ..Default::default()
        }
    }

    // ── §8.1 Two-pass enum collection ─────────────────────────────────────────

    #[test]
    fn two_pass_enum_collection() {
        // Schema: enum Color { RED=0; GREEN=1; BLUE=2; }
        //         message Msg { optional Color color = 1; optional int32 id = 2; }
        let color_enum = EnumDescriptorProto {
            name: Some("Color".into()),
            value: vec![
                enum_value("RED", 0),
                enum_value("GREEN", 1),
                enum_value("BLUE", 2),
            ],
            ..Default::default()
        };
        let msg = DescriptorProto {
            name: Some("Msg".into()),
            field: vec![enum_field("color", 1, ".Color"), int32_field("id", 2)],
            ..Default::default()
        };
        let fds_bytes = build_fds(vec![color_enum], msg);
        let schema = parse_schema(&fds_bytes, "Msg").unwrap();
        let root = schema.root_descriptor().unwrap();

        let color_fd = root.get_field(1).expect("field 1 must exist");
        let Kind::Enum(enum_desc) = color_fd.kind() else {
            panic!("field 1 must be an enum");
        };
        let names: Vec<String> = enum_desc.values().map(|v| v.name().to_owned()).collect();
        assert_eq!(
            names,
            &["RED", "GREEN", "BLUE"],
            "enum values must be present"
        );

        let id_fd = root.get_field(2).expect("field 2 must exist");
        assert_eq!(id_fd.kind(), Kind::Int32, "field 2 must be int32");
    }

    // ── §8.3 Extension field visibility ───────────────────────────────────────

    /// Demonstrates that after spec-0011, `ParsedSchema` exposes extension
    /// descriptors registered on a message, making them available for
    /// rendering as `[pkg.ext_name]`.
    ///
    /// Schema:
    ///   package acme;
    ///   message Gadget { extensions 1000 to 1999; }
    ///   extend Gadget { optional int32 blade_count = 1000; }
    ///
    /// Wire bytes: field 1000, wire-type varint (0), value 42.
    ///   tag  = (1000 << 3) | 0 = 8000 → varint 0xC0 0x3E
    ///   value = 42 → varint 0x2A
    #[test]
    fn extension_field_visible_via_get_extension() {
        use prost_types::descriptor_proto::ExtensionRange;

        let extension_field = FieldDescriptorProto {
            name: Some("blade_count".into()),
            number: Some(1000),
            r#type: Some(TYPE_INT32),
            label: Some(LABEL_OPTIONAL),
            extendee: Some(".acme.Gadget".into()),
            ..Default::default()
        };

        let gadget_msg = DescriptorProto {
            name: Some("Gadget".into()),
            extension_range: vec![ExtensionRange {
                start: Some(1000),
                end: Some(2000),
                ..Default::default()
            }],
            ..Default::default()
        };

        let file = prost_types::FileDescriptorProto {
            name: Some("gadget.proto".into()),
            syntax: Some("proto2".into()),
            package: Some("acme".into()),
            message_type: vec![gadget_msg],
            extension: vec![extension_field],
            ..Default::default()
        };
        let fds = prost_types::FileDescriptorSet { file: vec![file] };
        let mut buf = Vec::new();
        fds.encode(&mut buf).unwrap();

        let schema = parse_schema(&buf, "acme.Gadget").unwrap();
        let root = schema.root_descriptor().unwrap();

        // The extension must be visible via get_extension().
        let ext = root
            .get_extension(1000)
            .expect("extension field 1000 must be visible");
        assert_eq!(ext.full_name(), "acme.blade_count");
        assert_eq!(ext.kind(), prost_reflect::Kind::Int32);
    }

    // ── §8.2 Enum named after primitive keyword ────────────────────────────────

    #[test]
    fn enum_named_float_not_mistaken_for_primitive() {
        // Schema: enum float { FLOAT_ZERO=0; FLOAT_ONE=1; }
        //         message Msg { optional float kind = 1; }
        let float_enum = EnumDescriptorProto {
            name: Some("float".into()),
            value: vec![enum_value("FLOAT_ZERO", 0), enum_value("FLOAT_ONE", 1)],
            ..Default::default()
        };
        let msg = DescriptorProto {
            name: Some("Msg".into()),
            field: vec![enum_field("kind", 1, ".float")],
            ..Default::default()
        };
        let fds_bytes = build_fds(vec![float_enum], msg);
        let schema = parse_schema(&fds_bytes, "Msg").unwrap();
        let root = schema.root_descriptor().unwrap();

        let kind_fd = root.get_field(1).expect("field 1 must exist");
        assert!(
            matches!(kind_fd.kind(), Kind::Enum(_)),
            "field named 'float' backed by an enum must have Kind::Enum"
        );
        let Kind::Enum(enum_desc) = kind_fd.kind() else {
            unreachable!()
        };
        assert!(
            enum_desc.values().count() > 0,
            "enum named 'float' must have non-empty values"
        );
    }
}
