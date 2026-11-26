// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Tests for --emit-binary output (spec 0076).
//!
//! Three cases:
//!
//! - Case 1, no translation (proto2, proto3, editions without forced
//!   downconversion): protoback --emit-binary on the round-trip fixtures;
//!   the target file's FileDescriptorProto taken from protoc's set, both
//!   sides decoded as annotated prototext, source_code_info stripped,
//!   compared textually.
//! - Case 2, proto3/editions to proto2 via --force-proto2-output: a golden
//!   comparison, editions_rendering.proto as input.
//! - Case 3, editions to proto2 via --force-proto2-for-editions: likewise.
//!
//! Ported from `test_emit_binary.py`; its `roundtrip` marker dropped, as the
//! suite has no selection by marker. Its fixtures' file taken from protoc's
//! set as the set's own bytes for it, where the original reserialized them
//! with Python's protobuf: the same fields, rendered the same.

use crate::common::{extract_fdp, fixture, protoback, protoc, render, tmp, tools};
use crate::test_roundtrip::{
    companions, content, DEFAULT_FIXTURES, EDITION_FIXTURES, POLYGLOT_FIXTURES_LOSSY,
    POLYGLOT_FIXTURES_STRICT, TOOLS,
};
use std::path::Path;

const FDP: &str = "google.protobuf.FileDescriptorProto";

fn protoback_args(orig: &Path, out: &Path, extra: Vec<String>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        "--emit-binary".into(),
        format!("-I{}", orig.display()),
        format!("--proto-out={}", out.display()),
    ];
    args.extend(extra);
    args
}

/// `text` without its source_code_info block: protoback synthesizes its own
/// (spec 0141), with no reason to match protoc's textually.
fn strip_source_code_info(text: &str) -> String {
    let (mut out, mut depth, mut in_sci) = (String::new(), 0i64, false);
    for line in text.split_inclusive('\n') {
        let stripped = line.trim_start();
        if !in_sci {
            if stripped.starts_with("source_code_info {") {
                in_sci = true;
                depth = 1;
                continue;
            }
            out.push_str(line);
        } else {
            depth += stripped.matches('{').count() as i64 - stripped.matches('}').count() as i64;
            if depth <= 0 {
                in_sci = false;
            }
        }
    }
    out
}

/// --emit-binary's output matches the original FDP, source_code_info aside.
#[test]
fn test_emit_binary_no_translation() {
    if !tools(TOOLS) {
        return;
    }
    let all = DEFAULT_FIXTURES
        .iter()
        .chain(EDITION_FIXTURES)
        .chain(POLYGLOT_FIXTURES_STRICT)
        .chain(POLYGLOT_FIXTURES_LOSSY);
    for name in all {
        let dir = tmp(&format!("test_emit_binary_no_translation-{name}"));
        let (orig, out) = (dir.join("orig"), dir.join("out"));
        std::fs::create_dir_all(&orig).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let fixture_path = orig.join(name);
        std::fs::write(&fixture_path, content(name)).unwrap();

        let mut companion_pbs = Vec::new();
        for companion in companions(name) {
            let path = orig.join(companion);
            std::fs::write(&path, std::fs::read(fixture(companion)).unwrap()).unwrap();
            let pb = orig.join(Path::new(companion).with_extension("pb"));
            protoc([
                format!("--descriptor_set_out={}", pb.display()),
                format!("-I{}", orig.display()),
                path.display().to_string(),
            ]);
            companion_pbs.push(pb.display().to_string());
        }
        let stem = Path::new(name)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (orig_set, emit_pb) = (
            orig.join(format!("{stem}.pb")),
            out.join(format!("{stem}.pb")),
        );
        protoc([
            format!("--descriptor_set_out={}", orig_set.display()),
            format!("-I{}", orig.display()),
            fixture_path.display().to_string(),
        ]);
        let orig_fdp = extract_fdp(&std::fs::read(&orig_set).unwrap(), name);

        // well_known_types.proto needs the well-known variants for its
        // imports to resolve.
        let mut extra: Vec<String> = Vec::new();
        if *name == "well_known_types.proto" {
            for v in ["any", "empty", "timestamp", "duration"] {
                extra.extend(["--use-variant".into(), v.into()]);
            }
        }
        extra.push(orig_set.display().to_string());
        extra.extend(companion_pbs);
        let r = protoback(protoback_args(&orig, &out, extra));
        assert_eq!(r.code, Some(0), "{name}: protoback failed: {}", r.stderr);
        assert!(emit_pb.exists(), "{name}: emit.pb not created");

        let orig_text = strip_source_code_info(&render(&orig_fdp, FDP));
        let emit_text = strip_source_code_info(&render(&std::fs::read(&emit_pb).unwrap(), FDP));
        assert!(
            orig_text == emit_text,
            "emit.pb differs from orig FDP for {name}.\n--- orig (source_code_info stripped) ---\n{orig_text}--- emit ---\n{emit_text}"
        );
    }
}

/// Cases 2 and 3: editions_rendering.proto translated, against a golden.
fn run_emit_binary_golden(test: &str, flag: &str, golden: &str) {
    if !tools(TOOLS) {
        return;
    }
    let dir = tmp(test);
    let (orig, out) = (dir.join("orig"), dir.join("out"));
    std::fs::create_dir_all(&orig).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let proto = orig.join("editions_rendering.proto");
    std::fs::write(&proto, content("editions_rendering.proto")).unwrap();
    let pb = orig.join("editions_rendering.pb");
    protoc([
        format!("--descriptor_set_out={}", pb.display()),
        "--include_imports".into(),
        format!("-I{}", orig.display()),
        proto.display().to_string(),
    ]);
    // --no-source-info: these goldens predate spec 0141 and encode
    // translation behavior (groups, labels, packed options, feature
    // residuals), not source-info synthesis.
    let r = protoback(protoback_args(
        &orig,
        &out,
        vec![
            flag.into(),
            "--no-source-info".into(),
            pb.display().to_string(),
        ],
    ));
    assert_eq!(r.code, Some(0), "protoback failed: {}", r.stderr);
    let emit = out.join("editions_rendering.pb");
    assert!(emit.exists(), "emit.pb not created");
    let actual = render(&std::fs::read(&emit).unwrap(), FDP);
    let golden_text = content(golden);
    assert!(
        actual == golden_text,
        "emit.pb differs from golden {golden}.\n--- golden ---\n{golden_text}\n--- actual ---\n{actual}"
    );
}

/// --force-proto2-output: DELIMITED to TYPE_GROUP, LEGACY_REQUIRED to
/// LABEL_REQUIRED, EXPANDED repeated scalars to options.packed = true, no
/// residual features.
#[test]
fn test_emit_binary_force_proto2_output() {
    run_emit_binary_golden(
        "test_emit_binary_force_proto2_output",
        "--force-proto2-output",
        "editions_rendering.emit_binary.force_proto2_output.golden.pb",
    );
}

/// --force-proto2-for-editions: DELIMITED stays TYPE_MESSAGE,
/// LEGACY_REQUIRED to LABEL_REQUIRED, no residual features.
#[test]
fn test_emit_binary_force_proto2_for_editions() {
    run_emit_binary_golden(
        "test_emit_binary_force_proto2_for_editions",
        "--force-proto2-for-editions",
        "editions_rendering.emit_binary.force_proto2_for_editions.golden.pb",
    );
}
