// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The text format, parsed into the binary format, and protoc's text of
//! unknown fields re-framed (spec 0424 S5, spec 0426).

use prototext_upb::delimited::{correct, Declarations, Declared, Warning};
use prototext_upb::text_format::{parse, Options};
use prototext_upb::{DynamicMessage, Message, Pool, Value};

/// `tests/fixtures/options.proto` and `descriptor.proto`, and their text
/// form as `protoc --decode` prints it: the custom options by number.
const OPTIONS: &[u8] = include_bytes!("fixtures/options.pb");
const OPTIONS_TEXT: &str = include_str!("fixtures/options.txtpb");
/// Custom options protoc prints as `N { … }`, declared in their own file.
const DELIMITED: &[u8] = include_bytes!("fixtures/delimited.pb");
const DELIMITED_TEXT: &str = include_str!("fixtures/delimited.txtpb");

const LENIENT: Options = Options {
    allow_unknown_field: true,
    allow_unknown_extension: true,
};

/// The vendored `descriptor.proto`, alone.
const DESCRIPTOR: &[u8] = include_bytes!("../vendor/descriptor.pb");

const MESSAGE_OPTIONS: &str = "google.protobuf.MessageOptions";

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

/// Parse a set's text, then correct each file with what all of them
/// declare, as a loader does: the set, and the warnings.
fn parse_and_correct(pool: &Pool, text: &str) -> (Vec<u8>, Vec<Warning>) {
    let parsed = parse(&set_type(pool), text, LENIENT).unwrap();
    let files = files(pool, &parsed);
    let mut decls = Declarations::new();
    for f in &files {
        decls.add_file(pool, f).unwrap();
    }
    let mut warnings = Vec::new();
    let corrected: Vec<Vec<u8>> = files
        .iter()
        .map(|f| {
            let c = correct(pool, f, &decls);
            warnings.extend(c.warnings);
            c.bytes
        })
        .collect();
    (set_of(&corrected), warnings)
}

// ── Back to the binary set ──────────────────────────────────────────────────

#[test]
fn protocs_text_of_a_set_parses_back_to_the_set_custom_options_included() {
    let pool = descriptor_pool();
    let (bytes, warnings) = parse_and_correct(&pool, OPTIONS_TEXT);
    assert_eq!(bytes, OPTIONS);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn values_protoc_printed_alike_come_back_as_declared() {
    let pool = descriptor_pool();
    // Parsed alone, every `N { … }` is a group: not the set.
    let parsed = parse(&set_type(&pool), DELIMITED_TEXT, LENIENT).unwrap();
    assert_ne!(parsed, DELIMITED);
    // Corrected: the delimited value kept a group, its nested delimited
    // field too, the message value length-delimited, both strings exact.
    let (bytes, warnings) = parse_and_correct(&pool, DELIMITED_TEXT);
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
fn the_group_rule_agrees_with_upb() {
    // upb's resolution of the same declarations: the value of as_group is
    // framed as a group, of as_message length-delimited.
    let pool = Pool::new();
    assert!(pool.load_set(DELIMITED).skipped.is_empty());
    let group = pool.get_extension_by_name("dlm.as_group").unwrap();
    let message = pool.get_extension_by_name("dlm.as_message").unwrap();
    let inner = pool
        .get_message_by_name("dlm.Value")
        .unwrap()
        .get_field(2)
        .unwrap();
    assert!(group.is_group() && !message.is_group() && inner.is_group());
    // And the table, read from the files alone, says the same.
    let mut decls = Declarations::new();
    let alone = descriptor_pool();
    for f in files(&pool, DELIMITED) {
        decls.add_file(&alone, &f).unwrap();
    }
    let delimited = |t: &str, n| match decls.field(&alone, t, n) {
        Some(Declared::Message { delimited, .. }) => delimited,
        d => panic!("{d:?}"),
    };
    assert!(delimited(MESSAGE_OPTIONS, 50001));
    assert!(!delimited(MESSAGE_OPTIONS, 50002));
    assert!(delimited("dlm.Value", 2));
    assert_eq!(
        decls.field(&alone, MESSAGE_OPTIONS, 50003),
        Some(Declared::Bytes)
    );
}

#[test]
fn a_field_declared_nowhere_is_length_delimited_and_reported() {
    let pool = descriptor_pool();
    let parsed = parse(&set_type(&pool), DELIMITED_TEXT, LENIENT).unwrap();
    // The declarations of descriptor.proto only: not delimited.proto's.
    let files = files(&pool, &parsed);
    let mut decls = Declarations::new();
    decls.add_file(&pool, &files[0]).unwrap();
    let corrected = correct(&pool, &files[1], &decls);
    let numbers: Vec<u32> = corrected
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
    let file = DynamicMessage::decode(&fdp, &corrected.bytes).unwrap();
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
fn a_file_needing_no_correction_is_copied_byte_for_byte() {
    let pool = descriptor_pool();
    let mut decls = Declarations::new();
    let files = files(&pool, OPTIONS);
    for f in &files {
        decls.add_file(&pool, f).unwrap();
    }
    for f in &files {
        let c = correct(&pool, f, &decls);
        assert_eq!(&c.bytes, f);
        assert!(c.warnings.is_empty());
    }
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
            0x6b, 0x08, 0x01, 0x6c, // 13: a group, until the correction
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
    let pool = descriptor_pool();
    let (bytes, warnings) = parse_and_correct(&pool, &text);
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
