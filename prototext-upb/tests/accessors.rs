// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The generated accessors over `descriptor.proto`, in use (spec 0424 S6).

use prototext_upb::descriptor_access::{
    field_descriptor_proto_label as label, field_descriptor_proto_type as ty, FileDescriptorSet,
    FileDescriptorSetMut,
};
use prototext_upb::{DynamicMessage, Pool};

/// `tests/fixtures/options.proto` and `descriptor.proto`.
const OPTIONS: &[u8] = include_bytes!("fixtures/options.pb");
/// A cut-down `descriptor.proto` in package `proto2`.
const VARIANT: &[u8] = include_bytes!("fixtures/variant.pb");

fn decode(pool: &Pool, type_name: &str, bytes: &[u8]) -> DynamicMessage {
    let def = pool.get_message_by_name(type_name).expect(type_name);
    DynamicMessage::decode(&def, bytes).expect("decodes")
}

#[test]
fn a_descriptor_set_reads_through_its_accessors() {
    let pool = Pool::new();
    pool.load_set(OPTIONS);
    let set = decode(&pool, "google.protobuf.FileDescriptorSet", OPTIONS);
    let set = FileDescriptorSet::new(set.as_ref()).unwrap();
    let files = set.file();
    assert_eq!(files.len(), 2);
    let file = &files[1];
    assert_eq!(file.name(), b"options.proto");
    assert_eq!(file.package(), b"opts");
    assert_eq!(file.dependency(), [b"google/protobuf/descriptor.proto"]);

    let messages = file.message_type();
    let target = messages.iter().find(|m| m.name() == b"Target").unwrap();
    let fields = target.field();
    let name = &fields[0];
    assert_eq!((name.name(), name.number()), (&b"name"[..], 1));
    assert_eq!(name.label(), label::LABEL_OPTIONAL);
    assert_eq!(name.r#type(), ty::TYPE_STRING);
    let options = name.options().unwrap();
    assert!(options.has_deprecated() && options.deprecated());
    assert!(!options.has_packed());
    let values = &fields[1];
    assert_eq!(values.label(), label::LABEL_REPEATED);
    assert_eq!(values.r#type(), ty::TYPE_SINT64);
    assert!(values.options().unwrap().packed());
    let blob = &fields[3];
    assert_eq!(blob.default_value(), b"\\001\\377");

    let ranges = target.extension_range();
    assert_eq!((ranges[0].start(), ranges[0].end()), (100, 200));
    assert_eq!(target.oneof_decl()[0].name(), b"choice");
    assert_eq!(file.service()[0].method()[0].input_type(), b".opts.Target");
    assert!(file.source_code_info().is_none());
}

#[test]
fn a_descriptor_set_writes_through_its_accessors() {
    let pool = Pool::new();
    pool.load_set(OPTIONS);
    let mut set = decode(&pool, "google.protobuf.FileDescriptorSet", OPTIONS);
    {
        let mut w = FileDescriptorSetMut(set.as_mut());
        let mut file = w.push_file().unwrap();
        file.set_name(b"new.proto").unwrap();
        file.set_package(b"fresh").unwrap();
        let mut message = file.push_message_type().unwrap();
        message.set_name(b"M").unwrap();
        let mut field = message.push_field().unwrap();
        field.set_name(b"x").unwrap();
        field.set_number(1).unwrap();
        field.set_label(label::LABEL_OPTIONAL).unwrap();
        field.set_type(ty::TYPE_INT32).unwrap();
        field.options_mut().unwrap().set_deprecated(true).unwrap();
        field.clear_label();
        assert!(!field.as_ref().has_label());
    }
    let bytes = set.encode().unwrap();
    // What was written is a file the pool loads.
    let again = decode(&pool, "google.protobuf.FileDescriptorSet", &bytes);
    let again = FileDescriptorSet::new(again.as_ref()).unwrap();
    let file = &again.file()[2];
    assert_eq!(file.name(), b"new.proto");
    let fresh = Pool::new();
    assert!(fresh.load_set(OPTIONS).skipped.is_empty());
    fresh.add_file(&file.0.encode().unwrap()).unwrap();
    let x = fresh.get_message_by_name("fresh.M").unwrap();
    assert_eq!(x.get_field(1).unwrap().name(), "x");
}

#[test]
fn another_package_and_an_older_shape_read_and_keep_the_rest() {
    let pool = Pool::new();
    pool.load_set(VARIANT);
    let set = decode(&pool, "proto2.FileDescriptorSet", OPTIONS);
    let reader = FileDescriptorSet::new(set.as_ref()).unwrap();
    let file = &reader.file()[1];
    assert_eq!(file.name(), b"options.proto");
    let field = &file.message_type()[0].field()[0];
    assert_eq!((field.name(), field.number()), (&b"x"[..], 1));
    // Fields this descriptor.proto lacks read as absent.
    assert!(!field.has_label() && field.label() == 0);
    assert!(field.options().is_none());
    assert!(file.service().is_empty() && file.dependency().is_empty());
    // They cannot be written.
    let mut set = set;
    let mut w = FileDescriptorSetMut(set.as_mut());
    let mut extra = w.push_file().unwrap();
    assert!(extra.set_syntax(b"proto3").is_err());
    extra.clear_syntax();
    extra.set_name(b"extra.proto").unwrap();
    let bytes = set.encode().unwrap();

    // What they held is kept, as unknown bytes, which upb writes after a
    // message's known fields: in another place, so not the same bytes.
    assert!(!bytes.starts_with(OPTIONS));
    // Nothing is lost: read with the full descriptor.proto, every field is
    // back, and encoding writes them in field-number order again.
    let full = Pool::new();
    full.load_set(OPTIONS);
    let again = decode(&full, "google.protobuf.FileDescriptorSet", &bytes);
    assert!(again.encode().unwrap().starts_with(OPTIONS));
}
