// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Round trips: a fixture compiled by protoc, rendered by protoback,
//! compiled again, and the two descriptor sets byte-identical, the two
//! `.proto` texts equal once normalized (comments stripped, buf-formatted).
//!
//! Ported from `test_roundtrip.py`, its helper `_run_roundtrip` as
//! `run_roundtrip`, which `test_aggregate_options`, `test_emit_binary` and
//! `test_oneof_and_range_options` use too, and its fixture lists, which
//! `test_emit_binary` uses. Each parametrized test runs its
//! fixtures in turn, each named in a failure.

use crate::common::{
    fixture, normalize_proto_batch, pb_diff, pb_diff_fields, protoback, protoc, tmp, tools,
};
use std::collections::BTreeSet;
use std::path::Path;

/// Fixtures that must be beside a fixture for it to compile: what it
/// imports.
pub fn companions(fixture: &str) -> &'static [&'static str] {
    match fixture {
        "weak_import_proto2.proto" => &["weak_import_proto2_dep.proto"],
        "editions_roundtrip.proto" => &[
            "editions_custom_option_dep.proto",
            "weak_import_proto2_dep.proto",
        ],
        _ => &[],
    }
}

pub const DEFAULT_FIXTURES: &[&str] = &[
    "custom_options_proto3.proto",
    "default_values_proto2.proto",
    "enum.proto",
    "enum_value.proto",
    "extensions_proto2.proto",
    "field_comprehensive.proto",
    "group_proto2.proto",
    "file_comprehensive.proto",
    "message_comprehensive.proto",
    "name_resolution.proto",
    "service.proto",
    "message_set_proto2.proto",
    "weak_import_proto2.proto",
    "well_known_types.proto",
];

pub const EDITION_FIXTURES: &[&str] = &["editions_roundtrip.proto"];

/// Polyglot fixtures where the only no-polyglot .pb difference is `syntax`.
pub const POLYGLOT_FIXTURES_STRICT: &[&str] = &[
    "json_name.proto",
    "packed_proto2.proto",
    "packed_proto3.proto",
];

/// Polyglot fixtures using proto3-only descriptor fields (proto3_optional,
/// synthetic oneof_decl, oneof_index), which proto2 source cannot
/// reproduce: their no-polyglot round trip runs for crash-safety, its
/// field-diff assertion widened to PROTO3_ONLY_FIELDS.
pub const POLYGLOT_FIXTURES_LOSSY: &[&str] = &[
    "field_labels_proto3.proto", // spec 0019
    "synthetic_oneof.proto",     // spec 0019
];

/// Fields that may differ in the no-polyglot .pb comparison for lossy
/// fixtures; "name" because the diff reaches into missing oneof_decl
/// sub-messages and surfaces their name.
const PROTO3_ONLY_FIELDS: &[&str] = &[
    "syntax",
    "proto3_optional",
    "oneof_index",
    "oneof_decl",
    "name",
];

/// Under --force-proto2-output, proto3's implicitly packed repeated scalars
/// are written `[packed = true]`, so options.packed appears in the
/// recompiled .pb: allowed in all polyglot comparisons.
const PROTO3_PACKED_FIELDS: &[&str] = &["options", "packed"];

pub fn content(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap()
}

/// The tools the round trips need: protoc and buf, as `conftest.py`
/// required both.
pub const TOOLS: &[&str] = &["protoc", "buf"];

/// The round trip of `fixture_name`, `content` written in `orig`, rendered
/// into `new` (`_run_roundtrip`).
pub fn run_roundtrip(
    fixture_name: &str,
    content: &str,
    orig: &Path,
    new: &Path,
    extra: &[&str],
    compare_pb: bool,
    compare_proto: bool,
) {
    std::fs::create_dir_all(orig).unwrap();
    std::fs::create_dir_all(new).unwrap();
    let fixture_path = orig.join(fixture_name);
    std::fs::write(&fixture_path, content).unwrap();

    // Companions (imported dependencies) beside it, each compiled to its own
    // .pb, for protoback to load as context.
    let mut companion_pbs = Vec::new();
    for companion in companions(fixture_name) {
        let path = orig.join(companion);
        std::fs::write(&path, std::fs::read(fixture(companion)).unwrap()).unwrap();
        let pb = orig.join(Path::new(companion).with_extension("pb"));
        protoc([
            format!("--descriptor_set_out={}", pb.display()),
            format!("-I{}", orig.display()),
            path.display().to_string(),
        ]);
        companion_pbs.push(pb);
    }
    let stem = Path::new(fixture_name)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let (orig_pb, new_pb, new_proto) = (
        orig.join(format!("{stem}.pb")),
        new.join(format!("{stem}.pb")),
        new.join(fixture_name),
    );

    // Step 1: the original compiled.
    protoc([
        format!("--descriptor_set_out={}", orig_pb.display()),
        format!("-I{}", orig.display()),
        fixture_path.display().to_string(),
    ]);

    // Step 2: protoback renders it again.
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", orig.display()),
        format!("--proto-out={}", new.display()),
        orig_pb.display().to_string(),
    ];
    args.extend(companion_pbs.iter().map(|p| p.display().to_string()));
    args.extend(extra.iter().map(|s| s.to_string()));
    if fixture_name == "well_known_types.proto" && !extra.contains(&"all") {
        for v in ["any", "empty", "timestamp", "duration"] {
            args.extend(["--use-variant".into(), v.into()]);
        }
    }
    let run = protoback(&args);
    assert_eq!(
        run.code,
        Some(0),
        "{fixture_name}: protoback failed: {}",
        run.stderr
    );
    assert!(
        new_proto.exists(),
        "{fixture_name}: regenerated proto not created"
    );

    // Step 3: the rendering compiled, its companions beside it.
    for companion in companions(fixture_name) {
        std::fs::write(
            new.join(companion),
            std::fs::read(fixture(companion)).unwrap(),
        )
        .unwrap();
    }
    protoc([
        format!("-I{}", new.display()),
        format!("--descriptor_set_out={}", new_pb.display()),
        new_proto.display().to_string(),
    ]);

    // Step 4: the descriptor sets compared.
    if compare_pb {
        let (pb1, pb2) = (
            std::fs::read(&orig_pb).unwrap(),
            std::fs::read(&new_pb).unwrap(),
        );
        assert!(
            pb1 == pb2,
            "descriptor sets differ for {fixture_name}:\n{}",
            pb_diff(&pb1, &pb2)
        );
    }
    // Step 5: the .proto texts compared, comments and whitespace aside.
    if compare_proto {
        let output = std::fs::read_to_string(&new_proto).unwrap();
        let normalized = normalize_proto_batch(
            &new.parent().unwrap().join("normalize"),
            &[("fixture", content), ("output", &output)],
        );
        assert!(
            normalized[0] == normalized[1],
            ".proto text differs after normalization for {fixture_name}"
        );
    }
}

#[test]
fn test_roundtrip_edition() {
    if !tools(TOOLS) {
        return;
    }
    for name in EDITION_FIXTURES {
        let dir = tmp(&format!("test_roundtrip_edition-{name}"));
        run_roundtrip(
            name,
            &content(name),
            &dir.join("orig"),
            &dir.join("new"),
            &[],
            true,
            true,
        );
    }
}

#[test]
fn test_roundtrip() {
    if !tools(TOOLS) {
        return;
    }
    for name in DEFAULT_FIXTURES {
        let dir = tmp(&format!("test_roundtrip-{name}"));
        run_roundtrip(
            name,
            &content(name),
            &dir.join("orig"),
            &dir.join("new"),
            &[],
            true,
            true,
        );
    }
}

/// Run twice: with --force-proto2-output, where the text is not compared
/// and the .pb may differ by the allowed fields alone; without it, in full.
#[test]
fn test_roundtrip_polyglot() {
    if !tools(TOOLS) {
        return;
    }
    for name in POLYGLOT_FIXTURES_STRICT
        .iter()
        .chain(POLYGLOT_FIXTURES_LOSSY)
    {
        let content = content(name);
        let mut allowed: BTreeSet<String> = if POLYGLOT_FIXTURES_LOSSY.contains(name) {
            PROTO3_ONLY_FIELDS.iter().map(|s| s.to_string()).collect()
        } else {
            ["syntax".to_string()].into()
        };
        allowed.extend(PROTO3_PACKED_FIELDS.iter().map(|s| s.to_string()));

        let dir = tmp(&format!("test_roundtrip_polyglot-{name}"));
        let (orig, new) = (dir.join("no_polyglot/orig"), dir.join("no_polyglot/new"));
        run_roundtrip(
            name,
            &content,
            &orig,
            &new,
            &["--force-proto2-output"],
            false,
            false,
        );
        let stem = name.strip_suffix(".proto").unwrap();
        let (a, b) = (
            std::fs::read(orig.join(format!("{stem}.pb"))).unwrap(),
            std::fs::read(new.join(format!("{stem}.pb"))).unwrap(),
        );
        let differing = pb_diff_fields(&a, &b);
        assert!(
            differing.is_subset(&allowed),
            "{name}: with --force-proto2-output, .pb differs beyond allowed fields {allowed:?}: {differing:?}\n{}",
            pb_diff(&a, &b)
        );
        run_roundtrip(
            name,
            &content,
            &dir.join("polyglot/orig"),
            &dir.join("polyglot/new"),
            &[],
            true,
            true,
        );
    }
}

/// --use-variant all must not break the topological sort for importers of
/// well-known files (spec 0051): a fallback loaded must not leave spurious
/// W5 warnings.
#[test]
fn test_roundtrip_use_variant_all_wkt() {
    if !tools(TOOLS) {
        return;
    }
    let dir = tmp("test_roundtrip_use_variant_all_wkt");
    let (orig, new) = (dir.join("orig"), dir.join("new"));
    std::fs::create_dir_all(&orig).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let content = content("well_known_types.proto");
    let fixture_path = orig.join("well_known_types.proto");
    std::fs::write(&fixture_path, &content).unwrap();
    let orig_pb = orig.join("well_known_types.pb");
    protoc([
        format!("--descriptor_set_out={}", orig_pb.display()),
        format!("-I{}", orig.display()),
        fixture_path.display().to_string(),
    ]);
    let run = protoback([
        "--use-variant".into(),
        "descriptor".into(),
        "--use-variant".into(),
        "all".into(),
        format!("-I{}", orig.display()),
        format!("--proto-out={}", new.display()),
        orig_pb.display().to_string(),
    ]);
    assert_eq!(run.code, Some(0), "protoback failed:\n{}", run.stderr);
    assert!(
        !run.stderr.contains("missing dependency file"),
        "Spurious W5 warning(s), fallback topo-sort bug regression:\n{}",
        run.stderr
    );
    run_roundtrip(
        "well_known_types.proto",
        &content,
        &orig,
        &new,
        &["--use-variant", "all"],
        true,
        true,
    );
}

/// --use-variant all must provide fallbacks for api, type, field_mask and
/// source_context (spec 0052): a file importing api.proto renders.
#[test]
fn test_roundtrip_use_variant_all_api() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_roundtrip_use_variant_all_api");
    let (orig, new) = (dir.join("orig"), dir.join("new"));
    std::fs::create_dir_all(&orig).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let fixture_path = orig.join("wkt_api_ref.proto");
    std::fs::write(&fixture_path, content("wkt_api_ref.proto")).unwrap();
    let orig_pb = orig.join("wkt_api_ref.pb");
    protoc([
        format!("--descriptor_set_out={}", orig_pb.display()),
        format!("-I{}", orig.display()),
        fixture_path.display().to_string(),
    ]);
    let run = protoback([
        "--use-variant".into(),
        "descriptor".into(),
        "--use-variant".into(),
        "all".into(),
        "--emit-scoring-yaml".into(),
        format!("-I{}", orig.display()),
        format!("--proto-out={}", new.display()),
        orig_pb.display().to_string(),
    ]);
    assert_eq!(
        run.code,
        Some(0),
        "protoback failed (spec 0052 regression):\n{}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("missing dependency file"),
        "Unexpected W5 warning for a google/protobuf WKT:\n{}",
        run.stderr
    );
}

/// The scoring-graph emitter must not fail when an importer's dependency was
/// pruned as a duplicate-symbol file (spec 0053).
#[test]
fn test_scoring_graph_pruned_dependency() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_scoring_graph_pruned_dependency");
    let (orig, new) = (dir.join("orig"), dir.join("new"));
    std::fs::create_dir_all(&orig).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let mut pbs = Vec::new();
    for name in [
        "dup_sym_a.proto",
        "dup_sym_b.proto",
        "dup_sym_importer.proto",
    ] {
        let path = orig.join(name);
        std::fs::write(&path, content(name)).unwrap();
        let pb = orig.join(name.replace(".proto", ".pb"));
        protoc([
            format!("--descriptor_set_out={}", pb.display()),
            format!("-I{}", orig.display()),
            path.display().to_string(),
        ]);
        pbs.push(pb.display().to_string());
    }
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        "--emit-scoring-yaml".into(),
        format!("-I{}", orig.display()),
        format!("--proto-out={}", new.display()),
    ];
    args.extend(pbs);
    let run = protoback(&args);
    assert_eq!(
        run.code,
        Some(0),
        "protoback failed (spec 0053 regression):\n{}",
        run.stderr
    );
    assert!(
        new.join("dup_sym_importer.yaml").exists(),
        "Scoring graph not produced: {}",
        run.stderr
    );
    let proto = new.join("dup_sym_importer.proto");
    assert!(
        proto.exists(),
        "Reconstructed dup_sym_importer.proto not produced"
    );
    // If dup_sym_a was pruned, its import must appear as an orphan line.
    let content = std::fs::read_to_string(&proto).unwrap();
    if run.stderr.contains("stripped pruned dependency") {
        assert!(
            content.contains(r#"///import "dup_sym_a.proto";"#),
            "Stripped import not rendered as orphan:\n{content}"
        );
    }
}

/// The fixtures are found.
#[test]
fn test_fixture_discovery() {
    let protos: Vec<_> = std::fs::read_dir(fixture(""))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "proto"))
        .collect();
    assert!(
        !protos.is_empty(),
        "No fixture files found in protoback/tests/fixtures"
    );
}
