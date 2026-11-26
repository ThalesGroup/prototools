// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Custom editions features are written (spec 0435).
//!
//! A feature set's extensions (`features.(pb.cpp).string_type = VIEW`) are
//! written beside the standard features, at every level that can carry one:
//! compiling the output gives every element's options back, byte for byte.
//!
//! Ported from `test_custom_features.py`. Each element's options are
//! protoc's bytes for them (a wire walk), as in `test_option_names`. The
//! inputs of its last two tests, built with Python's protobuf (protoc's
//! descriptor.proto and cpp_features.proto moved to package `proto2` under
//! other paths, the user files pointed at them), are the `#@prototext`
//! fixtures `custom_features/`, made once by that code
//! (`protoback/tests/fixtures/gen/test_custom_features.py`); both tests' descriptor.proto
//! are the same bytes, one fixture. The last test's expected value, the
//! field's options as the original serialized them, is the generator's.

use crate::common::{
    compile_set, delimited, encode_fixture, name_and_options, protoback, tmp, tools, write,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FILES: &[(&str, &str)] = &[
    // Edition 2023: custom features on a file and on fields, alone and
    // beside a standard one.
    (
        "f/feat.proto",
        r#"edition = "2023";
package feat;
import "google/protobuf/cpp_features.proto";
option features.field_presence = IMPLICIT;
option features.(pb.cpp).string_type = CORD;
message M {
  string s = 1 [features.(pb.cpp).string_type = VIEW];
  string t = 2 [features.field_presence = EXPLICIT, features.(pb.cpp).string_type = STRING];
  string u = 3;
}
"#,
    ),
    // Edition 2024: on a message and on an enum.
    (
        "f/feat24.proto",
        r#"edition = "2024";
package feat24;
import "google/protobuf/cpp_features.proto";
import "google/protobuf/java_features.proto";
message M {
  option features.(pb.java).nest_in_file_class = YES;
  int32 x = 1;
}
enum E {
  option features.(pb.cpp).enum_name_uses_string_view = true;
  E_UNKNOWN = 0;
}
"#,
    ),
];

fn compile(root: &Path, out: &Path) -> Vec<u8> {
    let names: Vec<&str> = FILES.iter().map(|(n, _)| *n).collect();
    compile_set(root, &names, out)
}

/// Each element's options' bytes, by where it is (file, message, field,
/// enum).
type Options = BTreeMap<String, Vec<u8>>;

/// Every element's options, by where it is (`_options`).
fn options(set: &[u8]) -> Options {
    let mut out = BTreeMap::new();
    for f in delimited(set, 1) {
        let (file, opts) = name_and_options(f, 8);
        if !FILES.iter().any(|(n, _)| *n == file) {
            continue;
        }
        out.insert(file.clone(), opts);
        for m in delimited(f, 4) {
            let (mname, opts) = name_and_options(m, 7);
            out.insert(format!("{file}:{mname}"), opts);
            for fd in delimited(m, 2) {
                let (fname, opts) = name_and_options(fd, 8);
                out.insert(format!("{file}:{mname}.{fname}"), opts);
            }
        }
        for e in delimited(f, 5) {
            let (ename, opts) = name_and_options(e, 3);
            out.insert(format!("{file}:{ename}"), opts);
        }
    }
    out
}

/// The `rendered` fixture: the files compiled, rendered, compiled again;
/// both compilations' options, and the output directory.
fn rendered(test: &str) -> Option<(Options, Options, PathBuf)> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(test);
    for (name, text) in FILES {
        write(&dir.join("src").join(name), text);
    }
    let original = compile(&dir.join("src"), &dir.join("original.pb"));
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
    let again = compile(&out, &dir.join("again.pb"));
    Some((options(&original), options(&again), out))
}

#[test]
fn test_every_custom_feature_comes_back() {
    let Some((original, again, _)) = rendered("test_every_custom_feature_comes_back") else {
        return;
    };
    assert_eq!(again, original);
}

#[test]
fn test_custom_features_are_written_as_features() {
    let Some((_, _, out)) = rendered("test_custom_features_are_written_as_features") else {
        return;
    };
    let text = std::fs::read_to_string(out.join("f/feat.proto")).unwrap();
    // Paths, as protobuf writes custom features (spec 0439 S2).
    assert!(
        text.contains("option features.(pb.cpp).string_type = CORD;"),
        "{text}"
    );
    assert!(
        text.contains("features.(pb.cpp).string_type = VIEW"),
        "{text}"
    );
}

// A descriptor.proto of another package, named by --descriptor, its names
// kept (--target keep): custom features extend that package's FeatureSet,
// and are read from it. protoc's own descriptor.proto and
// cpp_features.proto, moved to package `proto2` under other paths, stand for
// such a root; a stock protoc cannot compile against them, so the rendered
// text is checked.
const ROOT: &str = "internal/proto2/descriptor.proto";
const CPP: &str = "internal/pb/cpp_features.proto";
const ROOT_SHA256: &str = "7d1cf2ef597521e9460fd96580d5434e3b423d8785fbff88953913b3c4a9710a";

/// The fixtures `custom_features/<name>` encoded into `inputs`, as
/// `(name, sha256)`.
fn inputs(inputs: &Path, files: &[(&str, &str)]) {
    for (name, sha) in files {
        encode_fixture(&format!("custom_features/{name}"), sha, &inputs.join(name));
    }
}

#[test]
fn test_custom_features_under_a_root_of_another_package() {
    let dir = tmp("test_custom_features_under_a_root_of_another_package");
    let inp = dir.join("in");
    inputs(
        &inp,
        &[
            ("internal/proto2/descriptor.pb", ROOT_SHA256),
            (
                "internal/pb/cpp_features.pb",
                "a7cb5ec6dff41167d17f190fa96db65cc024a08dfc4927501eac5b112a36dcc5",
            ),
            (
                "f/feat.pb",
                "3afce6560eeff1d8486c20dcfd3cffeb3b6e2fdf70f45bbe548fb412fae82248",
            ),
        ],
    );
    let out = dir.join("out");
    let r = protoback([
        "-q".into(),
        format!("-I{}", inp.display()),
        "-O".into(),
        out.display().to_string(),
        "--descriptor".into(),
        ROOT.into(),
        "f/feat.pb".into(),
        ROOT.replace(".proto", ".pb"),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let text = std::fs::read_to_string(out.join("f/feat.proto")).unwrap();
    for s in [
        format!(r#"import "{CPP}";"#),
        "option features.(pb.cpp).string_type = CORD;".into(),
        "features.(pb.cpp).string_type = VIEW".into(),
        "features.field_presence = EXPLICIT".into(),
    ] {
        assert!(text.contains(&s), "{s}:\n{text}");
    }
}

/// A custom option declared on proto2.FieldOptions (the root above, names
/// kept), set on a field: written, and its bytes kept.
#[test]
fn test_a_custom_option_on_a_root_of_another_package_is_kept() {
    // `(ext.level) = 7`, field 50001, as the original serialized the
    // field's options.
    const ORIGINAL: &[u8] = b"\x88\xb5\x18\x07";
    let dir = tmp("test_a_custom_option_on_a_root_of_another_package_is_kept");
    let inp = dir.join("in");
    inputs(
        &inp,
        &[
            ("internal/proto2/descriptor.pb", ROOT_SHA256),
            (
                "ext.pb",
                "83d8019dcfdef3f7b836080e6093e81ddcee5405a76b45d1381fa9f996982fce",
            ),
            (
                "u.pb",
                "7d47be6e74f5345716a70ed76474f50f1d09c40a2c002cec498f6586908f2b19",
            ),
        ],
    );
    let out = dir.join("out");
    let r = protoback([
        "-q".into(),
        "-b".into(),
        format!("-I{}", inp.display()),
        "-O".into(),
        out.display().to_string(),
        "--descriptor".into(),
        ROOT.into(),
        "u.pb".into(),
        ROOT.replace(".proto", ".pb"),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let text = std::fs::read_to_string(out.join("u.proto")).unwrap();
    assert!(text.contains("[(ext.level) = 7]"), "{text}");
    // -b writes each file's FileDescriptorProto, alone.
    let written = std::fs::read(out.join("u.pb")).unwrap();
    let message = delimited(&written, 4)[0];
    let field = delimited(message, 2)[0];
    assert_eq!(name_and_options(field, 8).1, ORIGINAL);
}
