// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `export --descriptor-binary`/`--descriptor-prototext`'s content
//! (spec 0156 G6/G7): builds a single-file `FileDescriptorSet`
//! describing one synthetic message type built from a cursor node's own
//! live-tree children, declaring (not embedding) the files needed to
//! resolve its fields' types, and locates the meta-schema
//! (`descriptor.proto`'s `FileDescriptorSet` message) needed to render
//! it as prototext.

use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
};
use prost_reflect::{DescriptorPool, FileDescriptor};
use std::collections::HashSet;

/// One resolved field for the synthetic message (spec 0156 G6c) —
/// `referenced_file` is `Some` only for `Message`/`Enum` fields, and
/// feeds `build`'s declared-dependency list (G6d).
pub struct ResolvedField {
    /// A `u32`, matching `NodeSpan::field_number`, which is where every
    /// one of these comes from — so `build`'s narrowing to
    /// `FieldDescriptorProto`'s `i32` is exact (spec 0212).
    pub number: u32,
    pub name: String,
    pub label: Label,
    pub r#type: Type,
    pub type_name: Option<String>,
    pub referenced_file: Option<FileDescriptor>,
}

/// G6b: the synthetic message name for a cursor whose own display name
/// (`field_name_for`) is `field_name` — that name with its first
/// character capitalized; if that first character is an ASCII digit
/// (the field-number-fallback case, including the document root, whose
/// `field_name_for` is always `"1"`), the whole string is prefixed with
/// `F` first (e.g. `"5"` -> `"F5"`, `"1"` -> `"F1"`), so the result is
/// always a syntactically valid proto identifier.
pub fn synthetic_message_name(field_name: &str) -> String {
    let prefixed = if field_name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("F{field_name}")
    } else {
        field_name.to_string()
    };
    let mut chars = prefixed.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => prefixed,
    }
}

/// G6c: the exported field name for a live child whose own display
/// name (`field_name_for`) is `field_name` — left unchanged unless its
/// first character is an ASCII digit (the field-number-fallback case),
/// in which case the whole string is prefixed with `f` (e.g. `"5"` ->
/// `"f5"`), so the result is always a syntactically valid proto
/// identifier (field names can't start with a digit). Unlike
/// `synthetic_message_name`, a resolvable field name's case is left
/// untouched — protobuf field names carry no capitalization
/// convention of their own.
pub fn synthetic_field_name(field_name: &str) -> String {
    if field_name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("f{field_name}")
    } else {
        field_name.to_string()
    }
}

/// Builds the `FileDescriptorSet` `export --descriptor-*` writes (spec
/// 0156 G6d): one synthetic file named `"{message_name}.export.proto"`
/// holding one message (`message_name`, `fields` — already fully
/// resolved by the caller per G6b/G6c), `syntax = "proto2"`, no
/// package. `dependency` declares (by name, like a proto `import`) the
/// files needed to resolve every field's `Message`/`Enum` type — their
/// own `FileDescriptorProto` content is never embedded, so the output
/// is always a single-file `FileDescriptorSet`.
pub fn build(message_name: &str, fields: Vec<ResolvedField>) -> FileDescriptorSet {
    let field_protos: Vec<FieldDescriptorProto> = fields
        .iter()
        .map(|f| FieldDescriptorProto {
            name: Some(f.name.clone()),
            number: Some(f.number as i32),
            label: Some(f.label as i32),
            r#type: Some(f.r#type as i32),
            type_name: f.type_name.clone(),
            ..Default::default()
        })
        .collect();

    let message = DescriptorProto {
        name: Some(message_name.to_string()),
        field: field_protos,
        ..Default::default()
    };

    // G6d: declared dependencies only (import-style), deduplicated by
    // file name — no embedding of a dependency's own content.
    let mut seen: HashSet<String> = HashSet::new();
    let mut dependency: Vec<String> = Vec::new();
    for f in &fields {
        if let Some(file) = &f.referenced_file {
            if seen.insert(file.name().to_string()) {
                dependency.push(file.name().to_string());
            }
        }
    }

    let synthetic = FileDescriptorProto {
        name: Some(format!("{message_name}.export.proto")),
        message_type: vec![message],
        dependency,
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };
    FileDescriptorSet {
        file: vec![synthetic],
    }
}

/// G7: locates `descriptor.proto`'s `FileDescriptorSet` message inside
/// `pool`, by file-name suffix + simple-name presence of both
/// `FileDescriptorSet` and `FileDescriptorProto` — deliberately not
/// keyed to any one package (works for `google.protobuf.*`,
/// `proto2.*`, or any other variant's canonized/uncanonized names).
/// `None` when no such file/message pair is found (G7's hard-error
/// case, raised by the caller).
pub fn locate_file_descriptor_set_type(
    pool: &DescriptorPool,
) -> Option<prost_reflect::MessageDescriptor> {
    pool.files()
        .find(|f| f.name().ends_with("descriptor.proto"))
        .and_then(|f| {
            let has_fdp = f.messages().any(|m| m.name() == "FileDescriptorProto");
            has_fdp
                .then(|| f.messages().find(|m| m.name() == "FileDescriptorSet"))
                .flatten()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_reflect::prost::Message as _;

    #[test]
    fn synthetic_message_name_capitalizes_resolvable_field_name() {
        assert_eq!(synthetic_message_name("person"), "Person");
    }

    #[test]
    fn synthetic_message_name_prefixes_field_number_fallback() {
        assert_eq!(synthetic_message_name("5"), "F5");
    }

    #[test]
    fn synthetic_message_name_prefixes_document_root() {
        assert_eq!(synthetic_message_name("1"), "F1");
    }

    #[test]
    fn synthetic_field_name_leaves_a_resolvable_field_name_untouched() {
        assert_eq!(synthetic_field_name("inner"), "inner");
    }

    #[test]
    fn synthetic_field_name_prefixes_field_number_fallback() {
        assert_eq!(synthetic_field_name("5"), "f5");
    }

    #[test]
    fn synthetic_field_name_prefixes_document_root() {
        assert_eq!(synthetic_field_name("1"), "f1");
    }

    fn pool_from(files: Vec<FileDescriptorProto>) -> DescriptorPool {
        let fds = FileDescriptorSet { file: files };
        DescriptorPool::decode(fds.encode_to_vec().as_slice()).unwrap()
    }

    fn message(name: &str) -> DescriptorProto {
        DescriptorProto {
            name: Some(name.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn locate_file_descriptor_set_type_resolves_the_canonical_name() {
        let pool = pool_from(vec![FileDescriptorProto {
            name: Some("google/protobuf/descriptor.proto".to_string()),
            package: Some("google.protobuf".to_string()),
            message_type: vec![message("FileDescriptorSet"), message("FileDescriptorProto")],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        }]);
        let found = locate_file_descriptor_set_type(&pool).expect("must resolve");
        assert_eq!(found.name(), "FileDescriptorSet");
    }

    /// G7's heuristic is name-suffix + simple-name presence, package-
    /// agnostic — an uncanonized `proto2`-packaged file with the same
    /// filename suffix still resolves.
    #[test]
    fn locate_file_descriptor_set_type_resolves_an_uncanonized_variant() {
        let pool = pool_from(vec![FileDescriptorProto {
            name: Some("net/proto2/proto/descriptor.proto".to_string()),
            package: Some("proto2".to_string()),
            message_type: vec![message("FileDescriptorSet"), message("FileDescriptorProto")],
            syntax: Some("proto2".to_string()),
            ..Default::default()
        }]);
        let found = locate_file_descriptor_set_type(&pool).expect("must resolve");
        assert_eq!(found.name(), "FileDescriptorSet");
    }

    #[test]
    fn locate_file_descriptor_set_type_is_none_without_a_matching_file() {
        let pool = pool_from(vec![FileDescriptorProto {
            name: Some("unrelated.proto".to_string()),
            message_type: vec![message("FileDescriptorSet"), message("FileDescriptorProto")],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        }]);
        assert!(locate_file_descriptor_set_type(&pool).is_none());
    }

    #[test]
    fn locate_file_descriptor_set_type_is_none_when_a_required_message_is_missing() {
        let pool = pool_from(vec![FileDescriptorProto {
            name: Some("google/protobuf/descriptor.proto".to_string()),
            package: Some("google.protobuf".to_string()),
            // `FileDescriptorProto` is missing.
            message_type: vec![message("FileDescriptorSet")],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        }]);
        assert!(locate_file_descriptor_set_type(&pool).is_none());
    }

    #[test]
    fn build_produces_no_dependency_when_all_fields_are_primitive() {
        let fields = vec![ResolvedField {
            number: 1,
            name: "x".to_string(),
            label: Label::Optional,
            r#type: Type::Int32,
            type_name: None,
            referenced_file: None,
        }];
        let fds = build("Msg", fields);
        assert_eq!(fds.file.len(), 1);
        assert_eq!(fds.file[0].dependency, Vec::<String>::new());
        assert_eq!(fds.file[0].syntax.as_deref(), Some("proto2"));
        assert_eq!(fds.file[0].message_type[0].name(), "Msg");
        assert_eq!(fds.file[0].message_type[0].field[0].name(), "x");
    }

    /// G6d: two fields referencing messages in two different files both
    /// contribute a declared `dependency` entry — no exclusion of the
    /// cursor's own file — but neither file's content is embedded, so
    /// the output stays a single-file `FileDescriptorSet`.
    #[test]
    fn build_declares_every_referenced_file_as_a_dependency_without_embedding_it() {
        let own_file = FileDescriptorProto {
            name: Some("own.proto".to_string()),
            message_type: vec![message("Own")],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        };
        let other_file = FileDescriptorProto {
            name: Some("other.proto".to_string()),
            message_type: vec![message("Other")],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        };
        let pool = pool_from(vec![own_file, other_file]);
        let own_desc = pool.get_message_by_name("Own").unwrap();
        let other_desc = pool.get_message_by_name("Other").unwrap();

        let fields = vec![
            ResolvedField {
                number: 1,
                name: "own_field".to_string(),
                label: Label::Optional,
                r#type: Type::Message,
                type_name: Some(".Own".to_string()),
                referenced_file: Some(own_desc.parent_file()),
            },
            ResolvedField {
                number: 2,
                name: "other_field".to_string(),
                label: Label::Optional,
                r#type: Type::Message,
                type_name: Some(".Other".to_string()),
                referenced_file: Some(other_desc.parent_file()),
            },
        ];
        let fds = build("Msg", fields);
        assert_eq!(fds.file.len(), 1, "no dependency's content is embedded");
        let dependency = &fds.file[0].dependency;
        assert!(dependency.contains(&"own.proto".to_string()));
        assert!(dependency.contains(&"other.proto".to_string()));
        assert_eq!(dependency.len(), 2);
    }
}
