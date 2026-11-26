// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Type names as protoc resolves them (spec 0434, invariant 0001).
//!
//! Each case: compile the original files with protoc, render them, compile
//! the output again. The output must compile, and every type name,
//! extendee and method type must come back as the original's: the name
//! written resolves, under protoc's rule, to the type the FDP names. The
//! positive controls pin the spelling: googleapis' convention (spec 0434
//! S4).
//!
//! Ported from `test_type_names.py`, its parametrized test running its
//! cases in turn; the references read from protoc's sets by a wire walk,
//! where the original parsed them with Python's protobuf.

use crate::common::{compile_set, delimited, protoback, tmp, tools, write};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

type Case = (&'static str, &'static [(&'static str, &'static str)]);

const CASES: &[Case] = &[
    // A: the extendee shadowed by a message of the file's own package.
    (
        "a_extendee_own_package",
        &[
            (
                "a/base.proto",
                "syntax = \"proto2\";\npackage alpha;\nmessage Thing { extensions 1000 to max; }\n",
            ),
            (
                "a/sub.proto",
                "syntax = \"proto2\";\npackage alpha.beta;\nimport \"a/base.proto\";\nmessage Thing { optional int32 id = 1; }\nextend .alpha.Thing { optional Thing sub = 1001; }\n",
            ),
        ],
    ),
    // B: a partial name whose first component a nearer package captures.
    (
        "b_partial_name_captured_by_package",
        &[
            ("b/one.proto", "syntax = \"proto2\";\npackage p.q.inner;\nmessage Source {}\n"),
            ("b/two.proto", "syntax = \"proto2\";\npackage p.q.r.inner;\nmessage Plan {}\n"),
            (
                "b/use.proto",
                "syntax = \"proto2\";\npackage p.q.r.use;\nimport \"b/one.proto\";\nimport \"b/two.proto\";\nmessage Attr {\n  optional .p.q.inner.Source source = 1;\n  optional .p.q.r.inner.Plan plan = 2;\n}\n",
            ),
        ],
    ),
    // C: a full name whose first component a sub-package captures.
    (
        "c_full_name_captured_by_subpackage",
        &[
            ("c/kind.proto", "syntax = \"proto2\";\npackage top.kinds;\nenum Kind { KIND_UNKNOWN = 0; }\n"),
            ("c/ui.proto", "syntax = \"proto2\";\npackage x.y.top;\nmessage Ui {}\n"),
            (
                "c/tags.proto",
                "syntax = \"proto2\";\npackage x.y;\nimport \"c/kind.proto\";\nimport \"c/ui.proto\";\nmessage Tags {\n  optional .top.kinds.Kind kind = 1;\n  optional .x.y.top.Ui ui = 2;\n}\n",
            ),
        ],
    ),
    // D: a method named as its message type.
    (
        "d_method_named_as_its_type",
        &[(
            "d/svc.proto",
            "syntax = \"proto2\";\npackage svc.pkg;\nmessage DoIt {}\nservice S { rpc DoIt(.svc.pkg.DoIt) returns (.svc.pkg.DoIt); }\n",
        )],
    ),
    // E: as A, the shadowing message in another file of the package.
    (
        "e_extendee_shadowed_from_another_file",
        &[
            ("e/req.proto", "syntax = \"proto2\";\npackage quota;\nmessage Request { extensions 1000 to max; }\n"),
            ("e/common.proto", "syntax = \"proto2\";\npackage quota.pb;\nmessage Request {}\n"),
            (
                "e/ext.proto",
                "syntax = \"proto2\";\npackage quota.pb;\nimport \"e/req.proto\";\nimport \"e/common.proto\";\nextend .quota.Request { optional int32 consistent = 1001; }\n",
            ),
        ],
    ),
    // An extendee is looked up with LOOKUP_ALL: a field of the message the
    // extension is declared in shadows it, as does an enum value.
    (
        "f_extendee_shadowed_by_field_and_enum_value",
        &[
            (
                "f/base.proto",
                "syntax = \"proto2\";\npackage base;\nmessage Target { extensions 1000 to max; }\nmessage Other { extensions 1000 to max; }\n",
            ),
            (
                "f/ext.proto",
                "syntax = \"proto2\";\npackage base.ext;\nimport \"f/base.proto\";\nenum Color { Other = 0; }\nmessage Holder {\n  optional int32 Target = 1;\n  extend .base.Target { optional int32 by_field = 1001; }\n}\nextend .base.Other { optional int32 by_value = 1001; }\n",
            ),
        ],
    ),
    // The same in the extendee's own package, where the short name is the
    // convention's: a field of the enclosing message captures it.
    (
        "h_own_package_extendee_shadowed_by_field",
        &[(
            "h/ext.proto",
            "syntax = \"proto2\";\npackage own;\nmessage Target { extensions 1000 to max; }\nmessage Holder {\n  optional int32 Target = 1;\n  extend .own.Target { optional int32 by_field = 1001; }\n}\n",
        )],
    ),
    // A map value's type is resolved from the synthesized entry message.
    (
        "g_map_value_named_as_the_entry",
        &[(
            "g/map.proto",
            "syntax = \"proto3\";\npackage maps;\nmessage ThingEntry { int32 x = 1; }\nmessage Holder {\n  map<string, .maps.ThingEntry> thing = 1;\n}\n",
        )],
    ),
    // Positive controls: short forms kept where they resolve, other
    // packages' types in full without the dot (googleapis' convention).
    (
        "z_controls",
        &[
            ("z/other.proto", "syntax = \"proto3\";\npackage google.type.demo;\nmessage Date { int32 day = 1; }\n"),
            (
                "z/main.proto",
                "syntax = \"proto3\";\npackage google.api.demo;\nimport \"google/protobuf/timestamp.proto\";\nimport \"z/other.proto\";\nmessage Outer {\n  message Inner { int32 x = 1; }\n  Inner inner = 1;\n  Sibling sibling = 2;\n  google.protobuf.Timestamp at = 3;\n  google.type.demo.Date day = 4;\n  int32 Sibling = 5;\n}\nmessage Sibling { Outer.Inner inner = 1; }\nservice S { rpc Get(google.protobuf.Timestamp) returns (Sibling); }\n",
            ),
        ],
    ),
];

fn case(name: &str) -> &'static [(&'static str, &'static str)] {
    CASES.iter().find(|(n, _)| *n == name).unwrap().1
}

/// A string field `number` of a message's bytes, empty when absent.
fn text(m: &[u8], number: u64) -> String {
    delimited(m, number)
        .first()
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default()
}

/// Every type name, extendee and method type of the files `names`, by where
/// it is (`_references`).
fn references(set: &[u8], names: &[&str]) -> BTreeMap<String, String> {
    fn field(refs: &mut BTreeMap<String, String>, at: &str, f: &[u8]) {
        let name = text(f, 1);
        let (type_name, extendee) = (text(f, 6), text(f, 2));
        if !type_name.is_empty() {
            refs.insert(format!("{at}.{name}:type"), type_name);
        }
        if !extendee.is_empty() {
            refs.insert(format!("{at}.{name}:extendee"), extendee);
        }
    }
    fn message(refs: &mut BTreeMap<String, String>, at: &str, m: &[u8]) {
        let here = format!("{at}.{}", text(m, 1));
        for f in delimited(m, 2).into_iter().chain(delimited(m, 6)) {
            field(refs, &here, f);
        }
        for n in delimited(m, 3) {
            message(refs, &here, n);
        }
    }
    let mut refs = BTreeMap::new();
    for file in delimited(set, 1) {
        if !names.contains(&text(file, 1).as_str()) {
            continue;
        }
        let package = text(file, 2);
        for m in delimited(file, 4) {
            message(&mut refs, &package, m);
        }
        for f in delimited(file, 7) {
            field(&mut refs, &package, f);
        }
        for s in delimited(file, 6) {
            let service = text(s, 1);
            for meth in delimited(s, 2) {
                let at = format!("{package}.{service}.{}", text(meth, 1));
                refs.insert(format!("{at}:in"), text(meth, 2));
                refs.insert(format!("{at}:out"), text(meth, 3));
            }
        }
    }
    refs
}

fn roundtrip(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
    let src = dir.join("src");
    for (name, body) in files {
        write(&src.join(name), body);
    }
    let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
    let original = compile_set(&src, &names, &dir.join("original.pb"));
    let out = dir.join("out");
    let r = protoback([
        "-q".into(),
        format!("-I{}", dir.join("original.pb").display()),
        "-O".into(),
        out.display().to_string(),
        "--use-variant".into(),
        "descriptor".into(),
        ".".into(),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let again = compile_set(&out, &names, &dir.join("again.pb"));
    assert_eq!(references(&again, &names), references(&original, &names));
    out
}

#[test]
fn test_every_name_written_resolves_to_its_type() {
    if !tools(&["protoc"]) {
        return;
    }
    let mut names: Vec<&str> = CASES.iter().map(|(n, _)| *n).collect();
    names.sort();
    for name in names {
        roundtrip(
            &tmp(&format!(
                "test_every_name_written_resolves_to_its_type-{name}"
            )),
            case(name),
        );
    }
}

#[test]
fn test_names_are_written_as_googleapis_writes_them() {
    if !tools(&["protoc"]) {
        return;
    }
    let out = roundtrip(
        &tmp("test_names_are_written_as_googleapis_writes_them"),
        case("z_controls"),
    );
    let text = std::fs::read_to_string(out.join("z/main.proto")).unwrap();
    // The file's own package: short, relative to the scope; a field named
    // as a type does not shadow it for a field (LOOKUP_TYPES). Other
    // packages, well-known types included: in full, without the dot, from a
    // google.* package too.
    for s in [
        "Inner inner = 1;",
        "Sibling sibling = 2;",
        "Outer.Inner inner = 1;",
        "google.protobuf.Timestamp at = 3;",
        "google.type.demo.Date day = 4;",
        "rpc Get(google.protobuf.Timestamp) returns (Sibling);",
    ] {
        assert!(text.contains(s), "{s}:\n{text}");
    }
}

#[test]
fn test_a_captured_name_gets_its_leading_dot() {
    if !tools(&["protoc"]) {
        return;
    }
    let out = roundtrip(
        &tmp("test_a_captured_name_gets_its_leading_dot"),
        case("c_full_name_captured_by_subpackage"),
    );
    let text = std::fs::read_to_string(out.join("c/tags.proto")).unwrap();
    assert!(text.contains(".top.kinds.Kind kind = 1;"), "{text}");
    assert!(text.contains("x.y.top.Ui ui = 2;"), "{text}");
}

#[test]
fn test_an_own_package_name_captured_gets_its_leading_dot() {
    if !tools(&["protoc"]) {
        return;
    }
    let out = roundtrip(
        &tmp("test_an_own_package_name_captured_gets_its_leading_dot"),
        case("h_own_package_extendee_shadowed_by_field"),
    );
    let text = std::fs::read_to_string(out.join("h/ext.proto")).unwrap();
    assert!(text.contains("extend .own.Target {"), "{text}");
}
