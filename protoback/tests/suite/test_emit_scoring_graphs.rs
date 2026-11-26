// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Tests for the --emit-scoring-yaml option (spec 0045).
//!
//! Each test compiles fixture .proto files with protoc, then runs protoback
//! with --emit-scoring-yaml and inspects the resulting YAML files. TC-6
//! (spec 0086) covers canonized output paths when a target rewrites file
//! names.
//!
//! Ported from `test_emit_scoring_graphs.py`; its tests' names lower-cased.
//! The YAML read with `serde_yaml`, where the original used PyYAML's
//! `safe_load`: a parser, not protoback's emitter (`scoring/pyyaml.rs`).
//! TC-6's binaries read with `descriptor_access`.

use crate::common::{compile_proto, fixture, protoback, tmp, tools, vendor_descriptor, write, Run};
use prototext_upb::descriptor_access::FileDescriptorProto;
use prototext_upb::{DynamicMessage, Pool};
use serde_yaml::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn run(pbs: &[PathBuf], out: &Path, extra: &[&str]) -> Run {
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", fixture("").display()),
        format!("--proto-out={}", out.display()),
        "--emit-scoring-yaml".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args.extend(pbs.iter().map(|p| p.display().to_string()));
    protoback(&args)
}

/// The fixtures compiled and emitted with `extra`; the output directory.
fn emit(test: &str, protos: &[&str], extra: &[&str]) -> Option<PathBuf> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let pbs = compile_proto(&pb, protos);
    let r = run(&pbs, &out, extra);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    Some(out)
}

fn load(path: &Path) -> Value {
    let v: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(v.is_mapping());
    v
}

/// A message's fields, by number.
fn fields(data: &Value, message: &str) -> BTreeMap<i64, Value> {
    data["messages"][message]["fields"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|f| (f["number"].as_i64().unwrap(), f.clone()))
        .collect()
}

fn entries(data: &Value) -> Vec<String> {
    data["entries"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap().to_string())
        .collect()
}

fn has_message(data: &Value, name: &str) -> bool {
    data["messages"].as_mapping().unwrap().contains_key(name)
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// field_comprehensive.proto: the YAML written; field kinds and child FQDNs
/// right.
#[test]
fn test_tc1_basic_emission() {
    let Some(out) = emit(
        "test_tc1_basic_emission",
        &["field_comprehensive.proto"],
        &[],
    ) else {
        return;
    };
    let path = out.join("field_comprehensive.yaml");
    assert!(
        path.exists(),
        "YAML file must be written alongside .proto output"
    );
    let data = load(&path);
    // PrimitiveTypes covers most scalar kinds (field numbers per fixture).
    let prim = fields(&data, "test.field.PrimitiveTypes");
    let kinds = [
        (1, "int32"),   // req_int32
        (2, "uint64"),  // opt_int64  (int64: UINT64 leaf)
        (3, "uint32"),  // rep_uint32
        (4, "uint64"),  // req_uint64
        (5, "uint32"),  // opt_sint32 (sint32: UINT32 leaf)
        (6, "uint64"),  // rep_sint64 (sint64: UINT64 leaf)
        (7, "float"),   // req_fixed32
        (8, "double"),  // opt_fixed64
        (9, "float"),   // rep_sfixed32
        (10, "double"), // req_sfixed64
        (11, "float"),  // opt_float
        (12, "double"), // rep_double
        (13, "bool"),   // req_bool
        (14, "string"), // opt_string
        (15, "bytes"),  // rep_bytes
    ];
    for (n, kind) in kinds {
        assert_eq!(s(&prim[&n]["type"]), kind, "field {n}");
    }
    // ComplexTypes: a message field, of type message, with its child.
    let complex = fields(&data, "test.field.ComplexTypes");
    assert_eq!(s(&complex[&1]["type"]), "message");
    assert_eq!(s(&complex[&1]["child"]), "test.field.NestedMessage");
    // req_enum = 4, a proto2 enum, so closed: its range emitted and kept
    // (spec 0176 Finding 2); contrast TC-4's open proto3 enum.
    assert_eq!(s(&complex[&4]["type"]), "enum");
    assert_eq!(
        complex[&4]["range"],
        serde_yaml::from_str::<Value>("[0, 2]").unwrap()
    ); // Status: UNKNOWN..INACTIVE
    assert!(complex[&4].get("child").is_none(), "enum field: no child");
    for f in prim.values() {
        assert!(f.get("child").is_none());
    }
    // label: required/repeated emitted; optional, the default, omitted.
    assert_eq!(s(&prim[&1]["label"]), "required");
    assert_eq!(s(&prim[&4]["label"]), "required");
    assert_eq!(s(&prim[&3]["label"]), "repeated");
    assert!(prim[&2].get("label").is_none());
    // Fields in ascending number order.
    let numbers: Vec<i64> = data["messages"]["test.field.PrimitiveTypes"]["fields"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|f| f["number"].as_i64().unwrap())
        .collect();
    let mut sorted = numbers.clone();
    sorted.sort();
    assert_eq!(numbers, sorted);
    // entries: every non-pruned message of the file, top-level and nested
    // alike (spec 0140), sorted, full FQDNs.
    let e = entries(&data);
    for name in [
        "test.field.PrimitiveTypes",
        "test.field.ComplexTypes",
        "test.field.Outer",
        "test.field.Outer.Middle",
        "test.field.Outer.Middle.Inner",
    ] {
        assert!(e.iter().any(|x| x == name), "{name}");
    }
    let mut es = e.clone();
    es.sort();
    assert_eq!(e, es);
}

/// Outer, Outer.Middle, Outer.Middle.Inner all top-level keys.
#[test]
fn test_tc2_nested_message_types() {
    let Some(out) = emit(
        "test_tc2_nested_message_types",
        &["field_comprehensive.proto"],
        &[],
    ) else {
        return;
    };
    let data = load(&out.join("field_comprehensive.yaml"));
    for name in [
        "test.field.Outer",
        "test.field.Outer.Middle",
        "test.field.Outer.Middle.Inner",
    ] {
        assert!(has_message(&data, name), "{name}");
    }
}

/// Pruning a nested message excludes it from both messages and entries
/// (spec 0140 G2).
#[test]
fn test_tc2b_pruned_nested_message_excluded() {
    let Some(out) = emit(
        "test_tc2b_pruned_nested_message_excluded",
        &["field_comprehensive.proto"],
        &["--prune", "desc:test.field.Outer.Middle"],
    ) else {
        return;
    };
    let data = load(&out.join("field_comprehensive.yaml"));
    let e = entries(&data);
    for name in ["test.field.Outer.Middle", "test.field.Outer.Middle.Inner"] {
        assert!(
            !has_message(&data, name) && !e.iter().any(|x| x == name),
            "{name}"
        );
    }
    assert!(has_message(&data, "test.field.Outer") && e.iter().any(|x| x == "test.field.Outer"));
}

/// address_book.yaml has PhoneNumber's FQDN as a child, not its definition.
#[test]
fn test_tc3_cross_file_reference() {
    let Some(out) = emit(
        "test_tc3_cross_file_reference",
        &["phone_number.proto", "address_book.proto"],
        &[],
    ) else {
        return;
    };
    let ab = load(&out.join("address_book.yaml"));
    // Person.phones (field 4) is repeated PhoneNumber: a message with child.
    let person = fields(&ab, "tutorial.Person");
    assert_eq!(s(&person[&4]["type"]), "message");
    assert_eq!(s(&person[&4]["child"]), "tutorial.PhoneNumber");
    assert!(!has_message(&ab, "tutorial.PhoneNumber"));
    let e = entries(&ab);
    assert!(e.iter().any(|x| x == "tutorial.Person"));
    assert!(!e.iter().any(|x| x == "tutorial.PhoneNumber"));
    let phone = load(&out.join("phone_number.yaml"));
    assert!(has_message(&phone, "tutorial.PhoneNumber"));
    assert!(entries(&phone).iter().any(|x| x == "tutorial.PhoneNumber"));
}

/// Every repeated scalar emits its element kind, whatever [packed] says
/// (spec 0175); the declaration travels as the `packed` key (spec 0371 S2),
/// present and true exactly where the field is declared packed.
#[test]
fn test_tc4_packed_option_is_not_consulted() {
    let Some(out) = emit(
        "test_tc4_packed_option_is_not_consulted",
        &["packed_proto3.proto"],
        &[],
    ) else {
        return;
    };
    let data = load(&out.join("packed_proto3.yaml"));
    let f = fields(&data, "mockup.Packed");
    for (n, kind) in [
        (1, "int32"),
        (2, "int32"),
        (3, "int32"),
        (9, "double"),
        (10, "float"),
    ] {
        assert_eq!(s(&f[&n]["type"]), kind, "field {n}");
        // `label: repeated` tells the scorer the field is packable.
        assert_eq!(s(&f[&n]["label"]), "repeated", "field {n}");
    }
    // enums_default = 6, `repeated Status` in proto3: an open enum, no
    // range, emitted as int32 (spec 0176); packable too.
    assert_eq!(s(&f[&6]["type"]), "int32");
    assert!(f[&6].get("range").is_none());
    assert_eq!(s(&f[&6]["label"]), "repeated");
    // Spec 0371 S2: proto3 declares a repeated packable field packed unless
    // `[packed = false]`; strings and bytes are never packable.
    let packed: Vec<i64> = f
        .iter()
        .filter(|(_, v)| v.get("packed") == Some(&Value::Bool(true)))
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(packed, [1, 2, 6, 7, 9, 10]);
    assert!(f
        .values()
        .all(|v| v.get("packed").is_none_or(|p| *p == Value::Bool(true))));
}

/// Group fields of type group (spec 0058); the group messages of kind GROUP.
#[test]
fn test_tc5_group_child_fqdn() {
    let Some(out) = emit(
        "test_tc5_group_child_fqdn",
        &["field_comprehensive.proto"],
        &[],
    ) else {
        return;
    };
    let data = load(&out.join("field_comprehensive.yaml"));
    let f = fields(&data, "test.field.GroupTest");
    assert_eq!(s(&f[&1]["type"]), "group");
    assert_eq!(s(&f[&1]["child"]), "test.field.GroupTest.RepeatedGroup");
    assert_eq!(s(&f[&3]["type"]), "group");
    assert_eq!(s(&f[&3]["child"]), "test.field.GroupTest.OptionalGroup");
    assert_eq!(
        s(&data["messages"]["test.field.GroupTest.RepeatedGroup"]["kind"]),
        "GROUP"
    );
    assert_eq!(
        s(&data["messages"]["test.field.GroupTest.OptionalGroup"]["kind"]),
        "GROUP"
    );
}

/// Openness from the enum's closedness, not the file's syntax: an edition
/// 2023 file's CLOSED enum keeps its range.
#[test]
fn test_tc9_editions_closed_enum_keeps_its_range() {
    let Some(out) = emit(
        "test_tc9_editions_closed_enum_keeps_its_range",
        &[
            "editions_roundtrip.proto",
            "editions_custom_option_dep.proto",
            "weak_import_proto2_dep.proto",
        ],
        &[],
    ) else {
        return;
    };
    let data = load(&out.join("editions_roundtrip.yaml"));
    let f = fields(&data, "mockup.editions.AllFeatures");
    assert_eq!(s(&f[&7]["type"]), "enum");
    assert_eq!(
        f[&7]["range"],
        serde_yaml::from_str::<Value>("[0, 2]").unwrap()
    ); // STATUS_UNKNOWN..STATUS_INACTIVE
}

fn message_set(test: &str) -> Option<Value> {
    let out = emit(test, &["message_set_proto2.proto"], &[])?;
    Some(load(&out.join("message_set_proto2.yaml")))
}

/// A message_set_wire_format type gets a synthetic field 1 to Item.
#[test]
fn test_tc_ms1_message_set_synthesizes_item_edge() {
    let Some(data) = message_set("test_tc_ms1_message_set_synthesizes_item_edge") else {
        return;
    };
    let ms = data["messages"]["mockup.MessageSetMessage"]["fields"]
        .as_sequence()
        .unwrap();
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0]["number"].as_i64(), Some(1));
    assert_eq!(s(&ms[0]["type"]), "message");
    assert_eq!(s(&ms[0]["child"]), "mockup.MessageSetMessage.Item");
    assert_eq!(s(&ms[0]["label"]), "repeated");
}

/// The synthesized Item is a GROUP of type_id (int32 = 2) and message
/// (bytes = 3).
#[test]
fn test_tc_ms2_item_node_shape() {
    let Some(data) = message_set("test_tc_ms2_item_node_shape") else {
        return;
    };
    assert!(has_message(&data, "mockup.MessageSetMessage.Item"));
    assert_eq!(
        s(&data["messages"]["mockup.MessageSetMessage.Item"]["kind"]),
        "GROUP"
    );
    let f = fields(&data, "mockup.MessageSetMessage.Item");
    assert_eq!(s(&f[&2]["type"]), "int32");
    assert_eq!(s(&f[&3]["type"]), "bytes");
    assert!(f[&2].get("child").is_none() && f[&3].get("child").is_none());
}

/// A plain message is emitted normally.
#[test]
fn test_tc_ms3_non_message_set_unaffected() {
    let Some(data) = message_set("test_tc_ms3_non_message_set_unaffected") else {
        return;
    };
    let p = data["messages"]["mockup.Payload"]["fields"]
        .as_sequence()
        .unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0]["number"].as_i64(), Some(1));
    assert_eq!(s(&p[0]["type"]), "string");
    assert!(!has_message(&data, "mockup.Payload.Item"));
}

/// The synthesized Item is a messages key, never an entry (spec 0140 G3).
#[test]
fn test_tc_ms4_item_node_excluded_from_entries() {
    let Some(data) = message_set("test_tc_ms4_item_node_excluded_from_entries") else {
        return;
    };
    let e = entries(&data);
    assert!(has_message(&data, "mockup.MessageSetMessage.Item"));
    assert!(!e.iter().any(|x| x == "mockup.MessageSetMessage.Item"));
    assert!(e.iter().any(|x| x == "mockup.MessageSetMessage"));
}

/// A file of a vendor descriptor's types (spec 0447's fixture).
const VENDOR_USER: &str = r#"name: "c.proto" package: "c" dependency: "vendor/proto/descriptor.proto"
message_type { name: "M" field { name: "m" number: 1 label: LABEL_OPTIONAL
  type: TYPE_ENUM type_name: ".vendor.FileOptions.OptimizeMode" } }
"#;

/// --target protoc rewrites the vendor descriptor's path and package: every
/// output uses the rewritten names (specs 0086, 0159, 0447 S3).
#[test]
fn test_tc6_canonized_output_paths() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_tc6_canonized_output_paths");
    vendor_descriptor(&dir, "", "");
    let inputs = dir.join("in");
    write(&inputs.join("c.textpb"), VENDOR_USER);
    let out = dir.join("out");
    // Named: nothing here extends its options, for S1 to infer it.
    let r = protoback([
        "--target".into(),
        "protoc".into(),
        "--descriptor".into(),
        "vendor/proto/descriptor.proto".into(),
        format!("-I{}", inputs.display()),
        format!("--proto-out={}", out.display()),
        "--emit-scoring-yaml".into(),
        "--emit-binary".into(),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let canonical = out.join("google/protobuf");
    assert!(canonical.join("descriptor.proto").exists());
    assert!(!out.join("vendor/proto/descriptor.proto").exists());
    assert!(canonical.join("descriptor.yaml").exists());
    assert!(!out.join("vendor/proto/descriptor.yaml").exists());
    assert!(std::fs::read_to_string(out.join("c.proto"))
        .unwrap()
        .contains(r#"import "google/protobuf/descriptor.proto""#));

    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let desc_msg = DynamicMessage::decode(
        &def,
        &std::fs::read(canonical.join("descriptor.pb")).unwrap(),
    )
    .unwrap();
    let desc = FileDescriptorProto(desc_msg.as_ref());
    assert_eq!(text(desc.name()), "google/protobuf/descriptor.proto");
    assert_eq!(text(desc.package()), "google.protobuf");
    let options = desc
        .message_type()
        .into_iter()
        .find(|m| text(m.name()) == "FileOptions")
        .unwrap();
    let optimize = options
        .field()
        .into_iter()
        .find(|f| text(f.name()) == "optimize_for")
        .unwrap();
    assert_eq!(
        text(optimize.type_name()),
        ".google.protobuf.FileOptions.OptimizeMode"
    );

    let user_msg = DynamicMessage::decode(&def, &std::fs::read(out.join("c.pb")).unwrap()).unwrap();
    let user = FileDescriptorProto(user_msg.as_ref());
    let deps: Vec<String> = user.dependency().into_iter().map(text).collect();
    assert_eq!(deps, ["google/protobuf/descriptor.proto"]);
    let field = user.message_type().swap_remove(0).field().swap_remove(0);
    assert_eq!(
        text(field.type_name()),
        ".google.protobuf.FileOptions.OptimizeMode"
    );
}

/// map<K, V> fields' generated *Entry types are messages and entries (spec
/// 0140 G4).
#[test]
fn test_tc7_map_entry_types_are_entries() {
    let Some(out) = emit(
        "test_tc7_map_entry_types_are_entries",
        &["field_comprehensive.proto"],
        &[],
    ) else {
        return;
    };
    let data = load(&out.join("field_comprehensive.yaml"));
    let name = "test.field.MapFields.StringToIntEntry";
    assert!(has_message(&data, name));
    assert!(entries(&data).iter().any(|x| x == name));
}

/// entries, across several files, each sorted, with no duplicate (spec 0140
/// G5).
#[test]
fn test_tc8_entries_sorted_dedup_multi_file() {
    let Some(out) = emit(
        "test_tc8_entries_sorted_dedup_multi_file",
        &["phone_number.proto", "address_book.proto"],
        &[],
    ) else {
        return;
    };
    for name in ["phone_number.yaml", "address_book.yaml"] {
        let e = entries(&load(&out.join(name)));
        let mut sorted = e.clone();
        sorted.sort();
        assert_eq!(e, sorted, "{name}: entries must be sorted");
        sorted.dedup();
        assert_eq!(
            e.len(),
            sorted.len(),
            "{name}: entries must be duplicate-free"
        );
    }
}
