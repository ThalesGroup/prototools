// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Synthesized SourceCodeInfo in --emit-binary output (spec 0141).
//!
//! Step 1: message-level (top-level and nested) Location path and span
//! synthesis. Step 2: enum, field (extensions included), service and method
//! Locations. All gated by --source-info/--no-source-info.
//!
//! Ported from `test_source_info.py`; its `roundtrip` marker dropped, its
//! parametrized test running its parameters in turn. The FileDescriptorProto
//! protoback wrote is read with `descriptor_access`, where the original
//! parsed it with Python's protobuf.

use crate::common::{fixture, protoback, protoc, tmp, tools};
use crate::test_roundtrip::TOOLS;
use prototext_upb::descriptor_access::{
    DescriptorProto, EnumDescriptorProto, FileDescriptorProto, ServiceDescriptorProto,
};
use prototext_upb::{DynamicMessage, Pool};
use std::collections::BTreeSet;

const FIXTURE: &str = "message_comprehensive.proto";

/// A Location as read: its path and span.
struct Loc {
    path: Vec<i32>,
    span: Vec<i32>,
}

/// `fixture` compiled, rendered with --emit-binary and `extra`; protoback's
/// FileDescriptorProto decoded, its Locations, and the .proto's lines.
fn render(
    test: &str,
    extra: &[&str],
    fixture_name: &str,
) -> (DynamicMessage, Vec<Loc>, bool, Vec<String>) {
    let dir = tmp(test);
    let (orig, out) = (dir.join("orig"), dir.join("out"));
    std::fs::create_dir_all(&orig).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let proto = orig.join(fixture_name);
    std::fs::copy(fixture(fixture_name), &proto).unwrap();
    let pb = proto.with_extension("pb");
    protoc([
        format!("--descriptor_set_out={}", pb.display()),
        format!("-I{}", orig.display()),
        proto.display().to_string(),
    ]);
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        "--emit-binary".into(),
        format!("-I{}", orig.display()),
        format!("--proto-out={}", out.display()),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args.push(pb.display().to_string());
    let r = protoback(&args);
    assert_eq!(r.code, Some(0), "protoback failed: {}", r.stderr);
    let stem = std::path::Path::new(fixture_name)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let msg = DynamicMessage::decode(
        &def,
        &std::fs::read(out.join(format!("{stem}.pb"))).unwrap(),
    )
    .unwrap();
    let fdp = FileDescriptorProto(msg.as_ref());
    let has = fdp.has_source_code_info();
    let locs = fdp
        .source_code_info()
        .map(|s| {
            s.location()
                .into_iter()
                .map(|l| Loc {
                    path: l.path(),
                    span: l.span(),
                })
                .collect()
        })
        .unwrap_or_default();
    let lines = std::fs::read_to_string(out.join(format!("{stem}.proto")))
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    (msg, locs, has, lines)
}

fn name(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Whether `path` is a pure message chain: [4,i] (,[3,j])*.
fn is_message_only_path(path: &[i32]) -> bool {
    !path.is_empty() && path[0] == 4 && (2..path.len()).step_by(2).all(|i| path[i] == 3)
}

/// The message a message-only path names.
fn message_name_at_path(fdp: &FileDescriptorProto, path: &[i32]) -> String {
    assert_eq!(
        path[0], 4,
        "unexpected top-level path field number: {path:?}"
    );
    let mut node: DescriptorProto = fdp.message_type().swap_remove(path[1] as usize);
    let mut i = 2;
    while i < path.len() {
        assert_eq!(path[i], 3, "unexpected nested path field number: {path:?}");
        node = node.nested_type().swap_remove(path[i + 1] as usize);
        i += 2;
    }
    name(node.name())
}

/// Any spec-0141 path as a dotted breadcrumb: messages (nested arbitrarily
/// deep), enums, enum values, fields, extensions (file- and message-level),
/// services and methods.
fn resolve_breadcrumb(fdp: &FileDescriptorProto, path: &[i32]) -> String {
    assert!(
        !path.is_empty() && path.len().is_multiple_of(2),
        "malformed path: {path:?}"
    );
    let (field, idx) = (path[0], path[1] as usize);
    match field {
        4 => {
            let node = fdp.message_type().swap_remove(idx);
            format!(
                "message:{}{}",
                name(node.name()),
                message_tail(node, &path[2..])
            )
        }
        5 => {
            let node = fdp.enum_type().swap_remove(idx);
            format!("enum:{}{}", name(node.name()), enum_tail(node, &path[2..]))
        }
        6 => {
            let node = fdp.service().swap_remove(idx);
            format!(
                "service:{}{}",
                name(node.name()),
                service_tail(node, &path[2..])
            )
        }
        7 => {
            assert!(
                path.len() == 2,
                "unexpected tail after file extension: {path:?}"
            );
            format!("extension:{}", name(fdp.extension()[idx].name()))
        }
        _ => panic!("unexpected top-level path field number: {path:?}"),
    }
}

fn message_tail(node: DescriptorProto, rest: &[i32]) -> String {
    if rest.is_empty() {
        return String::new();
    }
    let (field, idx) = (rest[0], rest[1] as usize);
    match field {
        2 => {
            assert!(rest.len() == 2, "unexpected tail after field: {rest:?}");
            format!(".field:{}", name(node.field()[idx].name()))
        }
        3 => {
            let nested = node.nested_type().swap_remove(idx);
            format!(
                ".message:{}{}",
                name(nested.name()),
                message_tail(nested, &rest[2..])
            )
        }
        4 => {
            let e = node.enum_type().swap_remove(idx);
            format!(".enum:{}{}", name(e.name()), enum_tail(e, &rest[2..]))
        }
        6 => {
            assert!(rest.len() == 2, "unexpected tail after extension: {rest:?}");
            format!(".extension:{}", name(node.extension()[idx].name()))
        }
        _ => panic!("unexpected nested path field number: {rest:?}"),
    }
}

fn enum_tail(node: EnumDescriptorProto, rest: &[i32]) -> String {
    if rest.is_empty() {
        return String::new();
    }
    assert_eq!(rest[0], 2, "unexpected enum tail field number: {rest:?}");
    assert!(
        rest.len() == 2,
        "unexpected tail after enum value: {rest:?}"
    );
    format!(".value:{}", name(node.value()[rest[1] as usize].name()))
}

fn service_tail(node: ServiceDescriptorProto, rest: &[i32]) -> String {
    if rest.is_empty() {
        return String::new();
    }
    assert_eq!(rest[0], 2, "unexpected service tail field number: {rest:?}");
    assert!(rest.len() == 2, "unexpected tail after method: {rest:?}");
    format!(".method:{}", name(node.method()[rest[1] as usize].name()))
}

/// (start_line, start_col, end_line, end_col) of a 3- or 4-element span.
fn span_lines(span: &[i32]) -> (usize, usize, usize, usize) {
    let s: Vec<usize> = span.iter().map(|&v| v as usize).collect();
    if s.len() == 3 {
        (s[0], s[1], s[0], s[2])
    } else {
        (s[0], s[1], s[2], s[3])
    }
}

/// A breadcrumb's last segment as (kind, name).
fn kind_and_name(breadcrumb: &str) -> (String, String) {
    let last = breadcrumb.rsplit('.').next().unwrap();
    let (k, n) = last.split_once(':').unwrap();
    (k.to_string(), n.to_string())
}

/// `line[from..]`, by characters as Python slices; empty past the end.
fn from(line: &str, col: usize) -> String {
    line.chars().skip(col).collect()
}

/// `line[..to]`, by characters.
fn upto(line: &str, col: usize) -> String {
    line.chars().take(col).collect()
}

/// A `kind Name {` ... `}` span (message, enum, service).
fn assert_open_close(lines: &[String], span: &[i32], prefix: &str, label: &str) {
    let (sl, sc, el, ec) = span_lines(span);
    assert!(
        from(&lines[sl], sc).starts_with(prefix),
        "{label}: open line {sl} doesn't start with {prefix:?} at col {sc}: {:?}",
        lines[sl]
    );
    assert!(
        upto(&lines[el], ec).trim_end().ends_with('}'),
        "{label}: close line {el} doesn't end with '}}' before col {ec}: {:?}",
        lines[el]
    );
}

/// A method span: `rpc Name(...`, ending in ';' or '}'.
fn assert_method_span(lines: &[String], span: &[i32], name: &str, label: &str) {
    let (sl, sc, el, ec) = span_lines(span);
    assert!(
        from(&lines[sl], sc).starts_with(&format!("rpc {name}(")),
        "{label}: open line {sl} doesn't start with 'rpc {name}(' at col {sc}: {:?}",
        lines[sl]
    );
    let t = upto(&lines[el], ec);
    let t = t.trim_end();
    assert!(
        t.ends_with(';') || t.ends_with('}'),
        "{label}: close line {el} doesn't end with ';' or '}}' before col {ec}: {:?}",
        lines[el]
    );
}

/// A field, value or extension span: its open line holds the name (case
/// aside, a group field's name being its type's lower-cased), its close
/// line ends in ';' or '}'.
fn assert_leaf_span(lines: &[String], span: &[i32], name: &str, label: &str) {
    let (sl, sc, el, ec) = span_lines(span);
    assert!(
        from(&lines[sl], sc)
            .to_lowercase()
            .contains(&name.to_lowercase()),
        "{label}: open line {sl} doesn't contain {name:?} at col {sc}: {:?}",
        lines[sl]
    );
    let t = upto(&lines[el], ec);
    let t = t.trim_end();
    assert!(
        t.ends_with(';') || t.ends_with('}'),
        "{label}: close line {el} doesn't end with ';' or '}}' before col {ec}: {:?}",
        lines[el]
    );
}

#[test]
fn test_source_info_message_locations() {
    if !tools(TOOLS) {
        return;
    }
    let (msg, locs, _, lines) = render("test_source_info_message_locations", &[], FIXTURE);
    let fdp = FileDescriptorProto(msg.as_ref());
    assert!(!locs.is_empty(), "expected synthesized source_code_info");
    let mut seen = BTreeSet::new();
    for loc in &locs {
        if !is_message_only_path(&loc.path) {
            continue;
        }
        let name = message_name_at_path(&fdp, &loc.path);
        assert_open_close(&lines, &loc.span, &format!("message {name} "), &name);
        seen.insert(name);
    }
    // OuterMessage > MiddleMessage > InnerMessage: three levels of nesting;
    // EmptyMessage: the single-line body-collapse case.
    for expected in [
        "OuterMessage",
        "MiddleMessage",
        "InnerMessage",
        "EmptyMessage",
    ] {
        assert!(seen.contains(expected), "missing Location for {expected}");
    }
}

/// Step 2: enum, field/extension, service and method Locations.
#[test]
fn test_source_info_g2_g3_g8_locations() {
    if !tools(TOOLS) {
        return;
    }
    let cases: &[(&str, &[&str])] = &[
        (
            "enum.proto",
            &[
                "enum:AllEnumFeatures",
                "enum:AllEnumFeatures.value:VALUE_ZERO",
                "enum:AllEnumFeatures.value:VALUE_DEPRECATED",
                "enum:ExtendedEnum",
                "enum:EmptyEnum",
                "extension:example_enum_option",
            ],
        ),
        (
            "enum_value.proto",
            &[
                "enum:EnumValueOptionsTest.value:ZERO",
                "enum:EnumValueOptionsTest.value:THREE",
                "enum:EnumValueOptionsTest.value:COMPREHENSIVE",
                "extension:example_value_option",
                "extension:value_number_option",
                "extension:value_tags",
            ],
        ),
        (
            "service.proto",
            &[
                "service:BasicService",
                "service:BasicService.method:SimpleRpc",
                "service:StreamingService.method:ClientStream",
                "service:OptionsService.method:DeprecatedMethod",
                "service:DeprecatedService.method:OldMethod",
                "service:EmptyService",
                "message:Request.field:query",
            ],
        ),
        (
            "message_comprehensive.proto",
            &[
                "message:ComprehensiveMessage.field:required_int32",
                "message:ComprehensiveMessage.field:repeatedgroup",
                "message:ComprehensiveMessage.extension:extension_field1",
                "message:ComprehensiveMessage.enum:NestedEnum",
                "message:ComprehensiveMessage.enum:NestedEnum.value:NESTED_FIRST",
                "message:OnlyExtensions.extension:extension_self",
                "message:ComprehensiveMessage.message:NestedMessage.message:DeeplyNested",
            ],
        ),
    ];
    for (fixture_name, expected) in cases {
        let (msg, locs, _, lines) = render(
            &format!("test_source_info_g2_g3_g8_locations-{fixture_name}"),
            &[],
            fixture_name,
        );
        let fdp = FileDescriptorProto(msg.as_ref());
        assert!(
            !locs.is_empty(),
            "{fixture_name}: expected synthesized source_code_info"
        );
        let mut seen = BTreeSet::new();
        for loc in &locs {
            let breadcrumb = resolve_breadcrumb(&fdp, &loc.path);
            let (kind, name) = kind_and_name(&breadcrumb);
            match kind.as_str() {
                "message" | "enum" | "service" => {
                    assert_open_close(&lines, &loc.span, &format!("{kind} {name} "), &breadcrumb)
                }
                "method" => assert_method_span(&lines, &loc.span, &name, &breadcrumb),
                "field" | "value" | "extension" => {
                    assert_leaf_span(&lines, &loc.span, &name, &breadcrumb)
                }
                k => panic!("unexpected kind: {k}"),
            }
            seen.insert(breadcrumb);
        }
        for e in *expected {
            assert!(
                seen.contains(*e),
                "{fixture_name}: missing Location for {e}"
            );
        }
    }
}

#[test]
fn test_source_info_disabled() {
    if !tools(TOOLS) {
        return;
    }
    let (_, _, has, _) = render("test_source_info_disabled", &["--no-source-info"], FIXTURE);
    assert!(!has);
}
