// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Regression tests for extension-field option rendering (spec 0041).
//!
//! Before the fix, reproto looked an extension up in its descriptor pool to
//! render its field options; for an extension whose defining file was not in
//! the pool, the lookup failed with "Couldn't find Extension N", said as a
//! spurious W6 even when the field carried no options. The fix built the
//! field's options from the FDP's own bytes.
//!
//! Fixtures: ext_no_options_config.proto (Config, extensions 100 to 200),
//! ext_no_options_extender.proto (extends Config, no options),
//! ext_with_options_extender.proto (extends Config, `[deprecated = true]`).
//!
//! Ported from `test_extension_field_options.py`.

use crate::common::{compile_proto, fixture, protoback, tmp, tools, Run};
use std::path::{Path, PathBuf};

fn run(pbs: &[PathBuf], out: &Path) -> Run {
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", fixture("").display()),
        format!("--proto-out={}", out.display()),
    ];
    args.extend(pbs.iter().map(|p| p.display().to_string()));
    protoback(&args)
}

fn setup(test: &str, extender: &str) -> Option<(Run, PathBuf)> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let pbs = compile_proto(&pb, &["ext_no_options_config.proto", extender]);
    Some((run(&pbs, &out), out))
}

/// An extension field with no field options produces no W6.
#[test]
fn test_extension_no_options_no_w6_warning() {
    let Some((r, out)) = setup(
        "test_extension_no_options_no_w6_warning",
        "ext_no_options_extender.proto",
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        !r.stderr.contains("Couldn't find Extension"),
        "{}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("field options could not be rendered"),
        "{}",
        r.stderr
    );
    let text = std::fs::read_to_string(out.join("ext_no_options_extender.proto")).unwrap();
    assert!(text.contains("optional Provider provider = 100;"), "{text}");
    assert!(!text.contains("WARNING"), "{text}");
}

/// An extension field with `[deprecated = true]` renders the option.
#[test]
fn test_extension_with_options_renders_correctly() {
    let Some((r, out)) = setup(
        "test_extension_with_options_renders_correctly",
        "ext_with_options_extender.proto",
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        !r.stderr.contains("Couldn't find Extension"),
        "{}",
        r.stderr
    );
    let text = std::fs::read_to_string(out.join("ext_with_options_extender.proto")).unwrap();
    assert!(text.contains("deprecated = true"), "{text}");
    assert!(!text.contains("WARNING"), "{text}");
}
