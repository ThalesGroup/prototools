// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Services, methods, extension ranges and the extensions of a message
//! (spec 0424 S4).

use prototext_upb::{DynamicMessage, ExtensionRange, Pool, Value};

/// `tests/fixtures/options.proto` and `descriptor.proto`.
const OPTIONS: &[u8] = include_bytes!("fixtures/options.pb");

fn pool() -> Pool {
    let pool = Pool::new();
    assert!(pool.load_set(OPTIONS).skipped.is_empty());
    pool
}

fn names(fields: &[prototext_upb::Field]) -> Vec<&str> {
    fields.iter().map(|f| f.full_name()).collect()
}

#[test]
fn a_service_lists_its_methods() {
    let pool = pool();
    let file = pool.file("options.proto").unwrap();
    let services: Vec<_> = file.services().collect();
    assert_eq!(services.len(), 1);
    let svc = &services[0];
    assert_eq!(svc.full_name(), "opts.Svc");
    assert_eq!(svc.name(), "Svc");
    assert_eq!(svc.parent_file(), file);
    assert_eq!(pool.get_service_by_name("opts.Svc").as_ref(), Some(svc));
    assert!(pool.get_service_by_name("opts.Nope").is_none());

    let methods: Vec<_> = svc.methods().collect();
    assert_eq!(methods.len(), 1);
    let call = &methods[0];
    assert_eq!(call.full_name(), "opts.Svc.Call");
    assert_eq!(call.name(), "Call");
    assert_eq!(call.parent_service(), *svc);
    assert_eq!(svc.get_method_by_name("Call").as_ref(), Some(call));
    assert_eq!(call.input_type().full_name(), "opts.Target");
    assert_eq!(call.output_type().full_name(), "opts.Point");
    assert!(!call.is_client_streaming() && !call.is_server_streaming());
}

#[test]
fn services_and_methods_know_their_declaration_paths() {
    let pool = pool();
    let svc = pool.get_service_by_name("opts.Svc").unwrap();
    assert_eq!(svc.declaration_path(), [6, 0]);
    let call = svc.get_method_by_name("Call").unwrap();
    assert_eq!(call.declaration_path(), [6, 0, 2, 0]);
}

#[test]
fn extension_ranges_end_past_their_last_number() {
    let pool = pool();
    let target = pool.get_message_by_name("opts.Target").unwrap();
    assert_eq!(
        target.extension_ranges(),
        [ExtensionRange {
            start: 100,
            end: 200
        }]
    );
    let point = pool.get_message_by_name("opts.Point").unwrap();
    assert!(point.extension_ranges().is_empty());
}

#[test]
fn all_extensions_of_a_message_by_number() {
    let pool = pool();
    let message = |name: &str| pool.get_message_by_name(name).unwrap();
    assert_eq!(
        names(&message("google.protobuf.FileOptions").all_extensions()),
        ["opts.file_tag", "opts.file_numbers"]
    );
    assert_eq!(
        names(&message("google.protobuf.MessageOptions").all_extensions()),
        ["opts.message_point"]
    );
    assert_eq!(
        names(&message("opts.Target").all_extensions()),
        ["opts.extra"]
    );
    assert!(message("opts.Point").all_extensions().is_empty());
}

#[test]
fn all_extensions_sees_the_files_loaded_so_far() {
    // Each file's bytes, from the set read as a message.
    let full = pool();
    let set_type = full
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap();
    let set = DynamicMessage::decode(&set_type, OPTIONS).unwrap();
    let file_field = set_type.get_field_by_name("file").unwrap();
    let Value::List(files) = set.as_ref().get(&file_field) else {
        panic!()
    };
    let bytes: Vec<Vec<u8>> = files
        .iter()
        .map(|f| match f {
            Value::Message(Some(m)) => m.encode().unwrap(),
            v => panic!("{v:?}"),
        })
        .collect();

    // descriptor.proto alone: FileOptions has no extension yet.
    let pool = Pool::new();
    pool.add_file(&bytes[0]).unwrap();
    let file_options = pool
        .get_message_by_name("google.protobuf.FileOptions")
        .unwrap();
    assert!(file_options.all_extensions().is_empty());
    pool.add_file(&bytes[1]).unwrap();
    assert_eq!(file_options.all_extensions().len(), 2);
}
