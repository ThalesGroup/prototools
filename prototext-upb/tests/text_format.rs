// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The text format, parsed into the binary format, and descriptor sets
//! read in two passes, protoc's text of unknown fields framed by the
//! input's declarations (spec 0424 S5, spec 0426, spec 0427).

use prototext_upb::text_format::{
    descriptor_type, load_input, parse, parse_reporting, Options, Warning,
};
use prototext_upb::{DynamicMessage, Message, Pool, Value};

/// `tests/fixtures/options.proto` and `descriptor.proto`, and their text
/// form as `protoc --decode` prints it: the custom options by number.
const OPTIONS: &[u8] = include_bytes!("fixtures/options.pb");
const OPTIONS_TEXT: &str = include_str!("fixtures/options.txtpb");
/// The same set printed with `options.proto`: the custom options by name.
const OPTIONS_NAMED_TEXT: &str = include_str!("fixtures/options_named.txtpb");
/// Custom options protoc prints as `N { … }`, declared in their own file.
const DELIMITED: &[u8] = include_bytes!("fixtures/delimited.pb");
const DELIMITED_TEXT: &str = include_str!("fixtures/delimited.txtpb");
/// The vendored `descriptor.proto` under `package proto2`, as
/// `net/proto2/proto/descriptor.proto`, by protoc 36.2 (its release):
///
///   sed 's/^package google\.protobuf;/package proto2;/' \
///       vendor/descriptor.proto > $T/net/proto2/proto/descriptor.proto
///   protoc -I$T --descriptor_set_out=proto2_descriptor.pb \
///       $T/net/proto2/proto/descriptor.proto
const PROTO2_DESCRIPTOR: &[u8] = include_bytes!("fixtures/proto2_descriptor.pb");

const LENIENT: Options = Options {
    allow_unknown_field: true,
    allow_unknown_extension: true,
};

/// The vendored `descriptor.proto`, alone.
const DESCRIPTOR: &[u8] = include_bytes!("../vendor/descriptor.pb");

const MESSAGE_OPTIONS: &str = "google.protobuf.MessageOptions";
const DESCRIPTOR_FILE: &str = "google/protobuf/descriptor.proto";

/// A pool with only `descriptor.proto`: what reads a descriptor set, and
/// knows none of the extensions the sets parsed declare.
fn descriptor_pool() -> Pool {
    let pool = Pool::new();
    let report = pool.load_set(DESCRIPTOR);
    assert_eq!(report.loaded, ["google/protobuf/descriptor.proto"]);
    pool
}

fn set_type(pool: &Pool) -> Message {
    pool.get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap()
}

/// Each file of a serialized set, as bytes.
fn files(pool: &Pool, set: &[u8]) -> Vec<Vec<u8>> {
    let t = set_type(pool);
    let m = DynamicMessage::decode(&t, set).unwrap();
    let Value::List(l) = m.as_ref().get(&t.get_field(1).unwrap()) else {
        panic!()
    };
    l.iter()
        .map(|v| match v {
            Value::Message(Some(m)) => m.encode().unwrap(),
            v => panic!("{v:?}"),
        })
        .collect()
}

/// A set, from its files.
fn set_of(files: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in files {
        out.push(0x0a);
        let mut n = f.len();
        while n >= 0x80 {
            out.push((n as u8) | 0x80);
            n >>= 7;
        }
        out.push(n as u8);
        out.extend_from_slice(f);
    }
    out
}

/// A set's text read as a loader does (spec 0427): with descriptor.proto
/// alone, then again with the pool of the files the first pass gave. The
/// set, and the warnings of the second pass.
fn two_passes(text: &str) -> (Vec<u8>, Vec<Warning>) {
    let alone = descriptor_pool();
    let first = parse(&set_type(&alone), text, LENIENT).unwrap();
    let (pool, report) = load_input(&first, DESCRIPTOR_FILE);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let def = descriptor_type(&pool, DESCRIPTOR_FILE, "FileDescriptorSet").unwrap();
    let parsed = parse_reporting(&def, text, LENIENT).unwrap();
    (parsed.bytes, parsed.warnings)
}

// ── Back to the binary set ──────────────────────────────────────────────────

#[test]
fn protocs_text_of_a_set_parses_back_to_the_set_custom_options_included() {
    let (bytes, warnings) = two_passes(OPTIONS_TEXT);
    assert_eq!(bytes, OPTIONS);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn options_by_name_resolve_in_the_inputs_own_pool() {
    // descriptor.proto alone knows none of the names: each one skipped,
    // and said so.
    let alone = descriptor_pool();
    let first = parse_reporting(&set_type(&alone), OPTIONS_NAMED_TEXT, LENIENT).unwrap();
    assert_ne!(first.bytes, OPTIONS);
    assert!(first
        .warnings
        .iter()
        .all(|w| matches!(w, Warning::UnknownExtension { .. })));
    assert!(!first.warnings.is_empty());
    // The second pass: every name declared in the input.
    let (bytes, warnings) = two_passes(OPTIONS_NAMED_TEXT);
    assert_eq!(bytes, OPTIONS);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn values_protoc_printed_alike_come_back_as_declared() {
    // The delimited value a group, its nested delimited field too, the
    // message value length-delimited, both strings exact.
    let (bytes, warnings) = two_passes(DELIMITED_TEXT);
    assert_eq!(bytes, DELIMITED);
    // A guess for "shoppingProduct" (a group inside, not empty), none for
    // "UrlList" (only an empty one, which only a group can be).
    assert_eq!(
        warnings,
        [Warning::StringAsMessage {
            message_type: MESSAGE_OPTIONS.into(),
            number: 50003
        }]
    );
}

#[test]
fn a_field_declared_nowhere_is_length_delimited_and_reported() {
    // descriptor.proto alone: delimited.proto's declarations unknown.
    let alone = descriptor_pool();
    let parsed = parse_reporting(&set_type(&alone), DELIMITED_TEXT, LENIENT).unwrap();
    let numbers: Vec<u32> = parsed
        .warnings
        .iter()
        .map(|w| match w {
            Warning::Undeclared {
                message_type,
                number,
            } => {
                assert_eq!(message_type, MESSAGE_OPTIONS);
                *number
            }
            w => panic!("{w:?}"),
        })
        .collect();
    assert_eq!(numbers, [50001, 50002, 50003, 50004]);
    // Each one length-delimited: read with the full schema, the message
    // value is the message it was.
    let full = Pool::new();
    full.load_set(DELIMITED);
    let fdp = full
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let file = DynamicMessage::decode(&fdp, &files(&alone, &parsed.bytes)[1]).unwrap();
    let holder = prototext_upb::descriptor_access::FileDescriptorProto(file.as_ref())
        .message_type()
        .into_iter()
        .find(|m| m.name() == b"Holder")
        .unwrap();
    let options = holder.options().unwrap();
    let as_message = full.get_extension_by_name("dlm.as_message").unwrap();
    let Value::Message(Some(value)) = options.0.get(&as_message) else {
        panic!()
    };
    assert!(matches!(value.get_by_number(1), Some(Value::I32(3))));
}

#[test]
fn options_extend_the_inputs_own_descriptor_proto() {
    // descriptor.proto under another name and package, as a variant built
    // on net/proto2/proto/descriptor.proto has it.
    const PROTO2: &str = "net/proto2/proto/descriptor.proto";
    let alone = descriptor_pool();
    let fdp_type = alone
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let renamed = files(&alone, PROTO2_DESCRIPTOR).remove(0);
    let declaring = parse(
        &fdp_type,
        &format!(
            r#"name: "use.proto" package: "u" dependency: "{PROTO2}"
               extension {{ name: "tag" number: 50001 label: LABEL_OPTIONAL
                            type: TYPE_STRING extendee: ".proto2.MessageOptions" }}"#
        ),
        LENIENT,
    )
    .unwrap();
    let set = set_of(&[renamed, declaring]);
    let (pool, report) = load_input(&set, PROTO2);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert!(pool.file(DESCRIPTOR_FILE).is_none());
    let def = descriptor_type(&pool, PROTO2, "FileDescriptorProto").unwrap();
    assert_eq!(def.full_name(), "proto2.FileDescriptorProto");
    let message = r#"name: "m.proto" message_type { name: "M" options { OPTION } }"#;
    let by_name =
        parse_reporting(&def, &message.replace("OPTION", r#"[u.tag]: "t""#), LENIENT).unwrap();
    assert!(by_name.warnings.is_empty(), "{:?}", by_name.warnings);
    let by_number = parse(&def, &message.replace("OPTION", r#"50001: "t""#), LENIENT).unwrap();
    assert_eq!(by_name.bytes, by_number);
}

// ── The grammar ─────────────────────────────────────────────────────────────

/// Parse `text` as a `Point` or `Target` of the options fixture, and
/// decode the result.
fn options_pool() -> Pool {
    let pool = Pool::new();
    pool.load_set(OPTIONS);
    pool
}

fn parse_as(pool: &Pool, type_name: &str, text: &str) -> DynamicMessage {
    let def = pool.get_message_by_name(type_name).unwrap();
    let parsed = parse(&def, text, Options::default()).unwrap_or_else(|e| panic!("{e}"));
    DynamicMessage::decode(&def, &parsed).unwrap()
}

fn get<'a>(m: &'a DynamicMessage, name: &str) -> Value<'a> {
    let f = m.descriptor().get_field_by_name(name).expect(name);
    m.as_ref().get(&f)
}

fn error(pool: &Pool, type_name: &str, text: &str) -> String {
    let def = pool.get_message_by_name(type_name).unwrap();
    parse(&def, text, Options::default())
        .unwrap_err()
        .to_string()
}

#[test]
fn comments_separators_and_both_brackets() {
    let pool = options_pool();
    let m = parse_as(
        &pool,
        "opts.Target",
        "# a comment\nname: 'n'; # another\nat < x: 1, y: 2 >",
    );
    assert!(matches!(get(&m, "name"), Value::Bytes(b"n")));
    let Value::Message(Some(at)) = get(&m, "at") else {
        panic!()
    };
    let y = at.descriptor().get_field_by_name("y").unwrap();
    assert!(matches!(at.get(&y), Value::I32(2)));
    // The last line names a field Target does not have: an error, with its
    // place, when unknown names are not allowed.
    assert_eq!(
        error(&pool, "opts.Target", "name: 'n'\nat2: 1"),
        "2:1: message type \"opts.Target\" has no field named \"at2\""
    );
}

#[test]
fn values_of_each_kind() {
    let pool = options_pool();
    let m = parse_as(
        &pool,
        "opts.Target",
        r#"name: "a" 'b' "\x41\101é\n"
           values: [1, -2, 0x10]
           values: 017
           counts { key: "k" value: 5 }
           blob: "\001\377"
           at: { x: -7 labels: ["p", "q"] }
           a: 3"#,
    );
    assert!(matches!(get(&m, "name"), Value::Bytes(b) if b == "abAA\u{e9}\n".as_bytes()));
    let Value::List(values) = get(&m, "values") else {
        panic!()
    };
    let values: Vec<Value<'_>> = values.iter().collect();
    assert!(matches!(
        values[..],
        [
            Value::I64(1),
            Value::I64(-2),
            Value::I64(16),
            Value::I64(15)
        ]
    ));
    assert!(matches!(get(&m, "blob"), Value::Bytes(b"\x01\xff")));
    assert!(matches!(get(&m, "a"), Value::I32(3)));
    let Value::Map(counts) = get(&m, "counts") else {
        panic!()
    };
    assert!(matches!(
        counts.entries()[..],
        [(Value::Bytes(b"k"), Value::I32(5))]
    ));
}

#[test]
fn floats_enums_and_extensions() {
    let pool = options_pool();
    let options = parse_as(
        &pool,
        "google.protobuf.EnumValueOptions",
        "[opts.value_weight]: -1.5e1",
    );
    let w = pool.get_extension_by_name("opts.value_weight").unwrap();
    assert!(matches!(options.as_ref().get(&w), Value::F64(v) if v == -15.0));
    for (text, expected) in [
        ("inf", f64::INFINITY),
        ("-Infinity", f64::NEG_INFINITY),
        ("2f", 2.0),
    ] {
        let m = parse_as(
            &pool,
            "google.protobuf.EnumValueOptions",
            &format!("[opts.value_weight]: {text}"),
        );
        assert!(
            matches!(m.as_ref().get(&w), Value::F64(v) if v == expected),
            "{text}"
        );
    }
    let m = parse_as(
        &pool,
        "google.protobuf.EnumValueOptions",
        "[opts.value_weight]: -nan",
    );
    assert!(matches!(m.as_ref().get(&w), Value::F64(v) if v.is_nan()));

    let color = pool.get_extension_by_name("opts.field_color").unwrap();
    for text in ["[opts.field_color]: GREEN", "[opts.field_color]: 1"] {
        let m = parse_as(&pool, "google.protobuf.FieldOptions", text);
        assert!(matches!(m.as_ref().get(&color), Value::Enum(1)), "{text}");
    }
    // Color is a proto2 enum, closed: an unknown number is an error.
    assert!(error(
        &pool,
        "google.protobuf.FieldOptions",
        "[opts.field_color]: 7"
    )
    .contains("has no value 7"));
}

#[test]
fn a_bool_and_its_spellings() {
    let pool = options_pool();
    let tag = pool.get_extension_by_name("opts.service_tag").unwrap();
    for (text, v) in [
        ("true", true),
        ("True", true),
        ("t", true),
        ("1", true),
        ("false", false),
        ("f", false),
        ("0", false),
    ] {
        let m = parse_as(
            &pool,
            "google.protobuf.ServiceOptions",
            &format!("[opts.service_tag]: {text}"),
        );
        assert!(
            matches!(m.as_ref().get(&tag), Value::Bool(b) if b == v),
            "{text}"
        );
    }
}

#[test]
fn an_any_is_expanded() {
    // descriptor.proto has no Any: an Any of our own, written in the text
    // format and parsed, in a pool of its own.
    let fdp = descriptor_pool()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let any = parse(
        &fdp,
        "name: 'any.proto' package: 'google.protobuf'
         message_type {
           name: 'Any'
           field { name: 'type_url' number: 1 label: LABEL_OPTIONAL type: TYPE_STRING }
           field { name: 'value' number: 2 label: LABEL_OPTIONAL type: TYPE_BYTES }
         }",
        Options::default(),
    )
    .unwrap();
    let any_pool = Pool::new();
    any_pool.add_file(&any).unwrap();
    let def = any_pool.get_message_by_name("google.protobuf.Any").unwrap();
    let parsed = parse(
        &def,
        "[type.googleapis.com/google.protobuf.Any] { type_url: 'x' }",
        Options::default(),
    )
    .unwrap();
    let m = DynamicMessage::decode(&def, &parsed).unwrap();
    assert!(matches!(
        get(&m, "type_url"),
        Value::Bytes(b"type.googleapis.com/google.protobuf.Any")
    ));
    let Value::Bytes(value) = get(&m, "value") else {
        panic!()
    };
    let inner = DynamicMessage::decode(&def, value).unwrap();
    assert!(matches!(get(&inner, "type_url"), Value::Bytes(b"x")));
}

#[test]
fn what_cannot_be_set_twice_is_an_error() {
    let pool = options_pool();
    assert!(error(&pool, "opts.Target", "name: 'a' name: 'b'").contains("set twice"));
    assert!(error(&pool, "opts.Target", "a: 1 b: 'x'").contains("of oneof \"choice\""));
    assert!(error(&pool, "opts.Point", "x: 2147483648").contains("out of range"));
    assert!(error(&pool, "opts.Point", "x: 'one'").contains("expected an integer"));
    assert!(error(&pool, "opts.Point", "x 1").starts_with("1:3: expected \":\""));
    assert!(error(&pool, "opts.Point", "labels: 'unterminated").contains("unterminated"));
}

#[test]
fn unknown_names_are_skipped_when_allowed() {
    let pool = options_pool();
    let def = pool.get_message_by_name("opts.Point").unwrap();
    let text = "x: 1 nope { a: 1 b [1, 2] } [no.such]: 'v' also: [1, 2] y: 2";
    let parsed = parse(&def, text, LENIENT).unwrap();
    let m = DynamicMessage::decode(&def, &parsed).unwrap();
    assert!(matches!(get(&m, "x"), Value::I32(1)));
    assert!(matches!(get(&m, "y"), Value::I32(2)));
    assert!(parse(&def, text, Options::default()).is_err());
}

#[test]
fn fields_by_number_keep_their_wire_types() {
    let pool = options_pool();
    let def = pool.get_message_by_name("opts.Point").unwrap();
    let parsed = parse(
        &def,
        "x: 1 9: 300 10: 0x00000001 11: 0x0000000000000002 12: 'b' 13 { 1: 1 }",
        Options::default(),
    )
    .unwrap();
    assert_eq!(
        parsed,
        [
            0x08, 0x01, // x
            0x48, 0xac, 0x02, // 9: varint 300
            0x55, 1, 0, 0, 0, // 10: fixed32
            0x59, 2, 0, 0, 0, 0, 0, 0, 0, // 11: fixed64
            0x62, 1, b'b', // 12: length-delimited
            0x6a, 2, 0x08, 0x01, // 13: declared nowhere, length-delimited
        ]
    );
    // A known field given by number is read by its type.
    let by_number = parse(&def, "1: 5", Options::default()).unwrap();
    assert_eq!(by_number, [0x08, 0x05]);
    assert!(parse(&def, "14: 0x123", Options::default()).is_err());
}

#[test]
fn a_packed_list_is_one_record() {
    let pool = options_pool();
    let def = pool.get_message_by_name("opts.Target").unwrap();
    let parsed = parse(
        &def,
        "values: [1, 2] name: 'n' values: 3",
        Options::default(),
    )
    .unwrap();
    // values is `repeated sint64 … [packed = true]`: 1, 2, 3 zigzagged, in
    // one record where the field first appears.
    assert_eq!(parsed, [0x12, 3, 2, 4, 6, 0x0a, 1, b'n']);
}

/// A large set and its text, when `PROTOTEXT_UPB_TEXT_SET` names the set
/// and `PROTOTEXT_UPB_TEXT_FORM` its text as `protoc --decode` prints it
/// (googleapis, in the spec's test): the text parses back to the set.
#[test]
fn a_large_sets_text_parses_back_to_the_set() {
    let (Some(set), Some(text)) = (
        std::env::var_os("PROTOTEXT_UPB_TEXT_SET"),
        std::env::var_os("PROTOTEXT_UPB_TEXT_FORM"),
    ) else {
        return;
    };
    let set = std::fs::read(set).unwrap();
    let text = std::fs::read_to_string(text).unwrap();
    // Each stage timed, for the spec's measured outcome (0427).
    let t = std::time::Instant::now();
    let alone = descriptor_pool();
    let first = parse(&set_type(&alone), &text, LENIENT).unwrap();
    let t1 = t.elapsed();
    let (pool, report) = load_input(&first, DESCRIPTOR_FILE);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let t2 = t.elapsed();
    let def = descriptor_type(&pool, DESCRIPTOR_FILE, "FileDescriptorSet").unwrap();
    let parsed = parse_reporting(&def, &text, LENIENT).unwrap();
    let t3 = t.elapsed();
    eprintln!(
        "first pass {:?}, pool {:?}, second pass {:?}",
        t1,
        t2 - t1,
        t3 - t2
    );
    let (bytes, warnings) = (parsed.bytes, parsed.warnings);
    // Every option is declared in the set: the only guesses are strings
    // protoc printed as messages (spec 0426 S3).
    assert!(warnings
        .iter()
        .all(|w| matches!(w, Warning::StringAsMessage { .. })));
    eprintln!("{} warnings", warnings.len());
    if let Some(out) = std::env::var_os("PROTOTEXT_UPB_TEXT_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    assert!(bytes == set, "the text does not parse back to the set");
}

#[test]
fn a_descriptor_proto_without_the_descriptor_types_falls_back_to_the_vendored_one() {
    // A variant's descriptor.proto may declare the options messages alone
    // (reproto's spec 0159 tests): no FileDescriptorProto to parse with.
    let alone = descriptor_pool();
    let fdp_type = alone
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let minimal = parse(
        &fdp_type,
        r#"name: "google/protobuf/descriptor.proto" package: "google.protobuf"
           message_type { name: "MessageOptions" }"#,
        LENIENT,
    )
    .unwrap();
    let (pool, report) = load_input(&set_of(&[minimal]), DESCRIPTOR_FILE);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let def = descriptor_type(&pool, DESCRIPTOR_FILE, "FileDescriptorProto").unwrap();
    assert_eq!(def.full_name(), "google.protobuf.FileDescriptorProto");
    let parsed = parse(&def, r#"name: "a.proto""#, LENIENT).unwrap();
    assert_eq!(parsed, b"\x0a\x07a.proto");
}

#[test]
fn a_file_whose_import_is_missing_still_declares_its_options() {
    // delimited.proto, importing a file the input does not have and using
    // a type of it: upb refuses it as it is. Mended, without the import
    // and the field, it declares its options to the second pass.
    let alone = descriptor_pool();
    let mut files = files(&alone, DELIMITED);
    let descriptor_type_ = alone
        .get_message_by_name("google.protobuf.DescriptorProto")
        .unwrap();
    let uses_gone = parse(
        &descriptor_type_,
        r#"name: "UsesGone"
           field { name: "g" number: 1 label: LABEL_OPTIONAL
                   type: TYPE_MESSAGE type_name: ".gone.G" }
           field { name: "kept" number: 2 label: LABEL_OPTIONAL type: TYPE_INT32 }"#,
        LENIENT,
    )
    .unwrap();
    let mut dependency = b"\x1a\x0agone.proto".to_vec();
    dependency.push(0x22);
    dependency.push(uses_gone.len() as u8);
    dependency.extend(&uses_gone);
    files[1].extend(dependency);
    let set = set_of(&files);
    assert!(!Pool::new().load_set(&set).skipped.is_empty());
    let (pool, report) = load_input(&set, DESCRIPTOR_FILE);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let mended = pool.get_message_by_name("dlm.UsesGone").unwrap();
    assert!(mended.get_field(1).is_none() && mended.get_field(2).is_some());
    assert!(pool
        .get_extension_by_name("dlm.as_group")
        .unwrap()
        .is_group());
    let def = descriptor_type(&pool, DESCRIPTOR_FILE, "FileDescriptorSet").unwrap();
    let parsed = parse_reporting(&def, DELIMITED_TEXT, LENIENT).unwrap();
    assert_eq!(parsed.bytes, DELIMITED);
    assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
}
