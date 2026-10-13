// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Schemas, read through upb (spec 0415 S6).
//!
//! The types the renderer works with are `prototext-upb`'s handles,
//! re-exported here so that callers name `prototext_core::schema::Message`
//! and the like, never a protobuf crate. Every pool this module builds
//! passes each file through [`gate`] before upb's parser sees it (S4).

pub use prototext_upb::descriptor;
pub use prototext_upb::{
    Cardinality, Edition, Enum, EnumValue, Field, File, Kind, LoadError, LoadReport, Message,
    Oneof, Pool,
};

// ── The gate (S4) ─────────────────────────────────────────────────────────────

/// Admit a serialized `FileDescriptorProto` to upb's parser only if
/// prototext's own wire parser reads it as one well-formed message: every
/// top-level field framed, the bytes consumed exactly, no malformation
/// (spec 0266's probe). Nested payloads are framed by their lengths, which
/// the probe checks; what is inside them is upb's to parse.
pub fn gate(bytes: &[u8]) -> Result<(), String> {
    if crate::serialize::render_text::probes_as_message(bytes) {
        Ok(())
    } else {
        Err("not a well-formed message (refused before parsing)".to_string())
    }
}

/// An empty pool whose every file passes through [`gate`].
pub fn new_pool() -> Pool {
    Pool::with_gate(gate)
}

// ── Public types ──────────────────────────────────────────────────────────────

/// The parsed, indexed form of a `FileDescriptorSet`: a pool and the
/// fully-qualified name of the root message.
pub struct ParsedSchema {
    pool: Pool,
    root_full_name: String,
    report: LoadReport,
}

impl ParsedSchema {
    /// Construct an empty (no-schema) `ParsedSchema`.
    pub fn empty() -> Self {
        ParsedSchema {
            pool: new_pool(),
            root_full_name: String::new(),
            report: LoadReport::default(),
        }
    }

    /// The root message, or `None` for an empty schema (no-schema mode).
    pub fn root_descriptor(&self) -> Option<Message> {
        if self.root_full_name.is_empty() {
            None
        } else {
            self.pool.get_message_by_name(&self.root_full_name)
        }
    }

    /// Look up a message by fully-qualified name (no leading dot).
    pub fn get_descriptor(&self, fqn: &str) -> Option<Message> {
        self.pool.get_message_by_name(fqn)
    }

    /// The underlying pool.
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// The files the schema's set could not load, and why (S3).
    pub fn load_report(&self) -> &LoadReport {
        &self.report
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

// ── Public entry points ───────────────────────────────────────────────────────

/// Load `schema_bytes`, a `FileDescriptorSet`, into a gated pool. Fails only
/// when the bytes are not a `FileDescriptorSet` at all; files that cannot
/// be loaded are in the report (S3).
pub fn decode_pool(schema_bytes: &[u8]) -> Result<(Pool, LoadReport), SchemaError> {
    let pool = new_pool();
    let report = pool.load_set(schema_bytes);
    match report.set_error {
        Some(ref e) => Err(SchemaError::InvalidDescriptor(e.clone())),
        None => Ok((pool, report)),
    }
}

fn not_found(pool: &Pool, root_full_name: &str, report: &LoadReport) -> SchemaError {
    let available = pool
        .all_messages()
        .iter()
        .map(|m| m.full_name().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut msg =
        format!("root message '{root_full_name}' not found in schema (available: {available})");
    if !report.skipped.is_empty() {
        let skipped: Vec<String> = report
            .skipped
            .iter()
            .map(|(file, why)| format!("{file}: {why}"))
            .collect();
        msg.push_str(&format!("; files skipped: {}", skipped.join("; ")));
    }
    SchemaError::MessageNotFound(msg)
}

/// Create a `ParsedSchema` from an already-loaded pool and a root message name.
pub fn schema_from_pool(pool: Pool, root_msg_name: &str) -> Result<ParsedSchema, SchemaError> {
    let root_full_name = root_msg_name.trim_start_matches('.').to_string();
    let report = LoadReport::default();
    if pool.get_message_by_name(&root_full_name).is_none() {
        return Err(not_found(&pool, &root_full_name, &report));
    }
    Ok(ParsedSchema {
        pool,
        root_full_name,
        report,
    })
}

/// Parse `schema_bytes` into a `ParsedSchema`.
///
/// An empty `schema_bytes` or empty `root_msg_name` returns a schema whose
/// `root_descriptor()` is `None` (no-schema mode). Fails when the bytes are
/// not a `FileDescriptorSet`, or the root message is not loadable; other
/// files that cannot be loaded are in [`ParsedSchema::load_report`].
pub fn parse_schema(schema_bytes: &[u8], root_msg_name: &str) -> Result<ParsedSchema, SchemaError> {
    if schema_bytes.is_empty() || root_msg_name.is_empty() {
        return Ok(ParsedSchema::empty());
    }
    let (pool, report) = decode_pool(schema_bytes)?;
    // Names are given without a leading dot; strip one if the caller passed it.
    let root_full_name = root_msg_name.trim_start_matches('.').to_string();
    if pool.get_message_by_name(&root_full_name).is_none() {
        return Err(not_found(&pool, &root_full_name, &report));
    }
    Ok(ParsedSchema {
        pool,
        root_full_name,
        report,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::descriptor::{
        DescriptorProto, Encode, EnumDescriptorProto, EnumValueDescriptorProto,
        FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };
    use super::*;

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

    /// `ParsedSchema` exposes extension descriptors registered on a message,
    /// making them available for rendering as `[pkg.ext_name]`.
    ///
    /// Schema:
    ///   package acme;
    ///   message Gadget { extensions 1000 to 1999; }
    ///   extend Gadget { optional int32 blade_count = 1000; }
    #[test]
    fn extension_field_visible_via_get_extension() {
        use super::descriptor::descriptor_proto::ExtensionRange;

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
            }],
            ..Default::default()
        };

        let file = FileDescriptorProto {
            name: Some("gadget.proto".into()),
            syntax: Some("proto2".into()),
            package: Some("acme".into()),
            message_type: vec![gadget_msg],
            extension: vec![extension_field],
            ..Default::default()
        };
        let fds = FileDescriptorSet { file: vec![file] };

        let schema = parse_schema(&fds.encode_to_vec(), "acme.Gadget").unwrap();
        let root = schema.root_descriptor().unwrap();

        // The extension must be visible via get_extension().
        let ext = root
            .get_extension(1000)
            .expect("extension field 1000 must be visible");
        assert_eq!(ext.full_name(), "acme.blade_count");
        assert_eq!(ext.kind(), Kind::Int32);
    }

    // ── §8.2 Enum named after primitive keyword ────────────────────────────────

    #[test]
    fn enum_named_float_not_mistaken_for_primitive() {
        // Schema: enum float { FLOAT_ZERO=0; FLOAT_ONE=1; }
        //         message Msg { optional float kind = 1; }
        let float_enum = EnumDescriptorProto {
            name: Some("float".into()),
            value: vec![enum_value("FLOAT_ZERO", 0), enum_value("FLOAT_ONE", 1)],
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
        let Kind::Enum(enum_desc) = kind_fd.kind() else {
            panic!("field named 'float' backed by an enum must have Kind::Enum");
        };
        assert!(
            enum_desc.values().count() > 0,
            "enum named 'float' must have non-empty values"
        );
    }

    // ── Spec 0415 S3/S4 ───────────────────────────────────────────────────────

    #[test]
    fn a_skipped_file_does_not_fail_the_schema_unless_it_holds_the_root() {
        let ok = FileDescriptorProto {
            name: Some("ok.proto".into()),
            message_type: vec![DescriptorProto {
                name: Some("Ok".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let broken = FileDescriptorProto {
            name: Some("broken.proto".into()),
            dependency: vec!["missing.proto".into()],
            message_type: vec![DescriptorProto {
                name: Some("Broken".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let bytes = FileDescriptorSet {
            file: vec![ok, broken],
        }
        .encode_to_vec();
        let schema = parse_schema(&bytes, "Ok").unwrap();
        assert_eq!(schema.load_report().skipped.len(), 1);
        let err = parse_schema(&bytes, "Broken").err().unwrap().to_string();
        assert!(
            err.contains("broken.proto: import 'missing.proto' is missing"),
            "{err}"
        );
    }

    /// A group-typed extension: prost-reflect answered `is_group() ==
    /// false` for every extension, so the group was a TYPE_MISMATCH; upb
    /// resolves it, and it is named as protoc names it, `[g.ext]`.
    ///
    ///   package g;
    ///   message Host { optional int32 id = 1; extensions 100 to 200; }
    ///   extend Host { optional group Ext = 100 { optional int32 a = 1; } }
    #[test]
    fn a_group_extension_renders_as_protoc_names_it() {
        use super::descriptor::descriptor_proto::ExtensionRange;
        use super::descriptor::field_descriptor_proto::{Label, Type};

        let int32 = |name: &str, number: i32| FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        };
        let file = FileDescriptorProto {
            name: Some("g.proto".into()),
            package: Some("g".into()),
            syntax: Some("proto2".into()),
            message_type: vec![
                DescriptorProto {
                    name: Some("Host".into()),
                    field: vec![int32("id", 1)],
                    extension_range: vec![ExtensionRange {
                        start: Some(100),
                        end: Some(201),
                    }],
                    ..Default::default()
                },
                DescriptorProto {
                    name: Some("Ext".into()),
                    field: vec![int32("a", 1)],
                    ..Default::default()
                },
            ],
            extension: vec![FieldDescriptorProto {
                name: Some("ext".into()),
                number: Some(100),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Group as i32),
                type_name: Some(".g.Ext".into()),
                extendee: Some(".g.Host".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let bytes = FileDescriptorSet { file: vec![file] }.encode_to_vec();
        let schema = parse_schema(&bytes, "g.Host").unwrap();
        // id = 7, then group 100 { a = 1 }.
        let blob = b"\x08\x07\xa3\x06\x08\x01\xa4\x06";

        let plain = crate::render_as_text(
            blob,
            schema.root_descriptor().as_ref(),
            crate::RenderOpts::default(),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(plain).unwrap(),
            "id: 7\n[g.ext] {\n a: 1\n}\n"
        );

        let annotated = crate::render_as_text(
            blob,
            schema.root_descriptor().as_ref(),
            crate::RenderOpts {
                include_annotations: true,
                ..Default::default()
            },
        )
        .unwrap();
        let wire = crate::render_as_bytes(&annotated, crate::RenderOpts::default()).unwrap();
        assert_eq!(&*wire, blob, "the group extension round-trips");
    }

    #[test]
    fn the_gate_refuses_a_file_with_a_malformed_top_level() {
        // A well-named file followed by a field whose length runs past the end.
        let mut fdp = FileDescriptorProto {
            name: Some("x.proto".into()),
            ..Default::default()
        }
        .encode_to_vec();
        fdp.extend_from_slice(b"\x12\x05ab");
        assert!(gate(&fdp).is_err());
        assert!(gate(b"\x0a\x07x.proto").is_ok());
    }
}
