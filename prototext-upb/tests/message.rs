// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Reflection over messages (spec 0424 S3, test plan 2).

use prototext_upb::{DynamicMessage, MessageRef, Pool, Scalar, Value};

/// `tests/fixtures/options.proto` and `descriptor.proto`, compiled by the
/// pinned protoc with `--include_imports`.
const OPTIONS: &[u8] = include_bytes!("fixtures/options.pb");

fn pool() -> Pool {
    let pool = Pool::new();
    let report = pool.load_set(OPTIONS);
    assert_eq!(
        report.loaded,
        ["google/protobuf/descriptor.proto", "options.proto"]
    );
    pool
}

fn decode_set(pool: &Pool) -> DynamicMessage {
    let fds = pool
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap();
    DynamicMessage::decode(&fds, OPTIONS).expect("decodes")
}

/// The field `name` of `m`, as a message (the `i`th one for a list).
fn sub<'a>(m: &MessageRef<'a>, name: &str, i: Option<usize>) -> MessageRef<'a> {
    let f = m.descriptor().get_field_by_name(name).expect(name);
    let v = m.get(&f);
    let v = match (v, i) {
        (Value::List(l), Some(i)) => l.get(i),
        (v, None) => v,
        (v, _) => panic!("{name}: {v:?}"),
    };
    match v {
        Value::Message(Some(m)) => m,
        v => panic!("{name}: {v:?}"),
    }
}

/// What `set_fields` lists: a field by its name, an extension by its full
/// name, as the recorded Python output names them.
fn names(m: &MessageRef<'_>) -> Vec<String> {
    m.set_fields()
        .into_iter()
        .map(|(f, _)| {
            if f.is_extension() {
                f.full_name().to_string()
            } else {
                f.name().to_string()
            }
        })
        .collect()
}

// ── Decode and encode ───────────────────────────────────────────────────────

#[test]
fn a_decoded_set_encodes_back_to_the_same_bytes() {
    let pool = pool();
    let fds = decode_set(&pool);
    assert_eq!(fds.encode().unwrap(), OPTIONS);
}

#[test]
fn custom_options_decode_as_extensions() {
    let pool = pool();
    let fds = decode_set(&pool);
    let file = sub(&fds.as_ref(), "file", Some(1));
    let options = sub(&file, "options", None);
    let tag = pool.get_extension_by_name("opts.file_tag").unwrap();
    assert!(options.has(&tag));
    assert!(matches!(options.get(&tag), Value::Bytes(b"file")));
}

#[test]
fn malformed_bytes_are_an_error() {
    let pool = pool();
    let fds = pool
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap();
    // Field 1, length-delimited, 9 bytes announced, 1 given.
    assert!(DynamicMessage::decode(&fds, &[0x0a, 0x09, 0x00]).is_err());
}

#[test]
fn random_bytes_never_panic() {
    let pool = pool();
    let fdp = pool
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for len in 0..400 {
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        if let Ok(m) = DynamicMessage::decode(&fdp, &bytes) {
            // Whatever decoded can be listed and encoded again.
            let _ = m.as_ref().set_fields();
            m.encode().unwrap();
        }
    }
}

// ── Listing (Python's ListFields) ───────────────────────────────────────────

/// What Python's `ListFields()` lists for each options message of the
/// fixture, recorded with Python protobuf 7.34.1 (upb) when the fixture was
/// written.
#[test]
fn set_fields_lists_as_python_list_fields() {
    let pool = pool();
    let fds = decode_set(&pool);
    let file = sub(&fds.as_ref(), "file", Some(1));
    let target = sub(&file, "message_type", Some(1));
    let color = sub(&file, "enum_type", Some(0));
    let svc = sub(&file, "service", Some(0));
    let cases: Vec<(&str, MessageRef<'_>, &[&str])> = vec![
        (
            "file",
            sub(&file, "options", None),
            &["opts.file_tag", "opts.file_numbers"],
        ),
        (
            "message Target",
            sub(&target, "options", None),
            &["deprecated", "opts.message_point"],
        ),
        (
            "field Target.name",
            sub(&sub(&target, "field", Some(0)), "options", None),
            &["deprecated", "opts.field_color"],
        ),
        (
            "field Target.values",
            sub(&sub(&target, "field", Some(1)), "options", None),
            &["packed"],
        ),
        (
            "oneof Target.choice",
            sub(&sub(&target, "oneof_decl", Some(0)), "options", None),
            &["opts.oneof_tag"],
        ),
        (
            "enum Color",
            sub(&color, "options", None),
            &["opts.enum_tag"],
        ),
        (
            "value Color.RED",
            sub(&sub(&color, "value", Some(0)), "options", None),
            &["opts.value_weight"],
        ),
        (
            "service Svc",
            sub(&svc, "options", None),
            &["opts.service_tag"],
        ),
        (
            "method Svc.Call",
            sub(&sub(&svc, "method", Some(0)), "options", None),
            &["opts.method_level"],
        ),
    ];
    for (what, m, expected) in cases {
        assert_eq!(names(&m), expected, "{what}");
    }
    assert_eq!(
        names(&file),
        [
            "name",
            "package",
            "dependency",
            "message_type",
            "enum_type",
            "service",
            "extension",
            "options"
        ]
    );
}

// ── Values ──────────────────────────────────────────────────────────────────

#[test]
fn values_come_back_with_their_types() {
    let pool = pool();
    let fds = decode_set(&pool);
    let file = sub(&fds.as_ref(), "file", Some(1));
    let ext = |name: &str| pool.get_extension_by_name(name).unwrap();

    // A repeated option keeps the order it was written in.
    let options = sub(&file, "options", None);
    let Value::List(numbers) = options.get(&ext("opts.file_numbers")) else {
        panic!()
    };
    let numbers: Vec<i32> = numbers
        .iter()
        .map(|v| match v {
            Value::I32(n) => n,
            v => panic!("{v:?}"),
        })
        .collect();
    assert_eq!(numbers, [3, 1, 2]);

    // A message-valued option, its fields and its list.
    let target = sub(&file, "message_type", Some(1));
    let Value::Message(Some(point)) = sub(&target, "options", None).get(&ext("opts.message_point"))
    else {
        panic!()
    };
    let field = |name: &str| point.descriptor().get_field_by_name(name).unwrap();
    assert!(matches!(point.get(&field("x")), Value::I32(1)));
    assert!(matches!(point.get(&field("y")), Value::I32(-2)));
    let Value::List(labels) = point.get(&field("labels")) else {
        panic!()
    };
    let labels: Vec<Value<'_>> = labels.iter().collect();
    assert!(matches!(
        labels[..],
        [Value::Bytes(b"b"), Value::Bytes(b"a")]
    ));

    // An enum is its number; a double, a bool, a uint32.
    let name_options = sub(&sub(&target, "field", Some(0)), "options", None);
    assert!(matches!(
        name_options.get(&ext("opts.field_color")),
        Value::Enum(1)
    ));
    let color = sub(&file, "enum_type", Some(0));
    let red = sub(&sub(&color, "value", Some(0)), "options", None);
    assert!(matches!(red.get(&ext("opts.value_weight")), Value::F64(w) if w == 1.5));
    let svc = sub(&file, "service", Some(0));
    assert!(matches!(
        sub(&svc, "options", None).get(&ext("opts.service_tag")),
        Value::Bool(true)
    ));
    let call = sub(&sub(&svc, "method", Some(0)), "options", None);
    assert!(matches!(call.get(&ext("opts.method_level")), Value::U32(2)));
}

#[test]
fn an_unset_field_reads_as_its_default() {
    let pool = pool();
    let target = pool.get_message_by_name("opts.Target").unwrap();
    let m = DynamicMessage::new(&target);
    let m = m.as_ref();
    let f = |name: &str| target.get_field_by_name(name).unwrap();
    assert!(!m.has(&f("blob")));
    assert!(matches!(m.get(&f("blob")), Value::Bytes(b"\x01\xff")));
    assert!(matches!(m.get(&f("at")), Value::Message(None)));
    assert!(matches!(m.get(&f("values")), Value::List(l) if l.is_empty()));
    assert!(matches!(m.get(&f("counts")), Value::Map(map) if map.is_empty()));
    assert!(m.set_fields().is_empty());
}

// ── Writing ─────────────────────────────────────────────────────────────────

/// A Target built field by field, of every shape.
fn build_target(pool: &Pool) -> DynamicMessage {
    let target = pool.get_message_by_name("opts.Target").unwrap();
    let f = |name: &str| target.get_field_by_name(name).unwrap();
    let mut m = DynamicMessage::new(&target);
    let mut w = m.as_mut();
    w.set(&f("name"), Scalar::Bytes(b"hello")).unwrap();
    for v in [5, -1, 300] {
        w.push(&f("values"), Scalar::I64(v)).unwrap();
    }
    w.insert(&f("counts"), Scalar::Bytes(b"k"), Scalar::I32(7))
        .unwrap();
    {
        let mut at = w.message_mut(&f("at")).unwrap();
        let x = at.descriptor().get_field_by_name("x").unwrap();
        at.set(&x, Scalar::I32(4)).unwrap();
        let labels = at.descriptor().get_field_by_name("labels").unwrap();
        at.push(&labels, Scalar::Bytes(b"z")).unwrap();
    }
    let extra = pool.get_extension_by_name("opts.extra").unwrap();
    w.set(&extra, Scalar::I32(9)).unwrap();
    m
}

#[test]
fn a_built_message_encodes_and_decodes_back() {
    let pool = pool();
    let m = build_target(&pool);
    let bytes = m.encode().unwrap();
    let again = DynamicMessage::decode(m.descriptor(), &bytes).unwrap();
    assert_eq!(again.encode().unwrap(), bytes);
    let r = again.as_ref();
    assert_eq!(names(&r), ["name", "values", "counts", "at", "opts.extra"]);
    let f = |name: &str| r.descriptor().get_field_by_name(name).unwrap();
    assert!(matches!(r.get(&f("name")), Value::Bytes(b"hello")));
    let Value::List(values) = r.get(&f("values")) else {
        panic!()
    };
    let values: Vec<Value<'_>> = values.iter().collect();
    assert!(matches!(
        values[..],
        [Value::I64(5), Value::I64(-1), Value::I64(300)]
    ));
    let Value::Map(counts) = r.get(&f("counts")) else {
        panic!()
    };
    assert!(matches!(
        counts.entries()[..],
        [(Value::Bytes(b"k"), Value::I32(7))]
    ));
    let at = sub(&r, "at", None);
    let x = at.descriptor().get_field_by_name("x").unwrap();
    assert!(matches!(at.get(&x), Value::I32(4)));
}

#[test]
fn a_list_of_messages_grows_by_push_message() {
    let pool = pool();
    let fdp = pool
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let message_type = fdp.get_field_by_name("message_type").unwrap();
    let mut m = DynamicMessage::new(&fdp);
    for name in ["A", "B"] {
        let mut w = m.as_mut();
        let mut msg = w.push_message(&message_type).unwrap();
        let name_field = msg.descriptor().get_field_by_name("name").unwrap();
        msg.set(&name_field, Scalar::Bytes(name.as_bytes()))
            .unwrap();
    }
    let r = m.as_ref();
    let names: Vec<Value<'_>> = (0..2)
        .map(|i| {
            let d = sub(&r, "message_type", Some(i));
            let f = d.descriptor().get_field_by_name("name").unwrap();
            d.get(&f)
        })
        .collect();
    assert!(matches!(
        names[..],
        [Value::Bytes(b"A"), Value::Bytes(b"B")]
    ));
}

#[test]
fn clear_unsets_and_a_clone_is_independent() {
    let pool = pool();
    let mut m = build_target(&pool);
    let copy = m.clone();
    let name = m.descriptor().get_field_by_name("name").unwrap();
    let values = m.descriptor().get_field_by_name("values").unwrap();
    m.as_mut().clear(&name);
    m.as_mut().clear(&values);
    assert!(!m.as_ref().has(&name));
    assert!(!m.as_ref().has(&values));
    assert!(copy.as_ref().has(&name));
    assert!(copy.as_ref().has(&values));
    assert_ne!(m.encode().unwrap(), copy.encode().unwrap());
}

#[test]
fn a_value_of_the_wrong_type_is_an_error() {
    let pool = pool();
    let target = pool.get_message_by_name("opts.Target").unwrap();
    let f = |name: &str| target.get_field_by_name(name).unwrap();
    let mut m = DynamicMessage::new(&target);
    let mut w = m.as_mut();
    assert!(w.set(&f("name"), Scalar::I32(1)).is_err());
    assert!(w.set(&f("values"), Scalar::I64(1)).is_err(), "repeated");
    assert!(
        w.push(&f("name"), Scalar::Bytes(b"x")).is_err(),
        "not a list"
    );
    assert!(w.message_mut(&f("name")).is_err(), "not a message");
    assert!(w
        .insert(&f("counts"), Scalar::I32(1), Scalar::I32(1))
        .is_err());
}

#[test]
#[should_panic(expected = "does not belong to message")]
fn a_field_of_another_message_panics() {
    let pool = pool();
    let target = pool.get_message_by_name("opts.Target").unwrap();
    let point = pool.get_message_by_name("opts.Point").unwrap();
    let x = point.get_field_by_name("x").unwrap();
    let m = DynamicMessage::new(&target);
    m.as_ref().get(&x);
}
