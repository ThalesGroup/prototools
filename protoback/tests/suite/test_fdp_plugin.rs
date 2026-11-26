// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Regression tests for --fdp-plugin (spec 0369).
//!
//! The plugin used to run only inside the phase-2 rank loop, after phase 1
//! had built the files' targets from the unpatched dependencies: an import
//! a plugin added was never resolved. The plugin applies at every place a
//! descriptor is materialized, the first being the file's parse.
//!
//! Fixtures: plugin_subject.proto (imports nothing; the file the plugin
//! patches), plugin_target.proto (referenced by nothing; reachable only
//! through the patch).
//!
//! Ported from `test_fdp_plugin.py`. Its plugins are Python, given each
//! descriptor as Python protobuf's `FileDescriptorProto`: the interpreter
//! protoback runs them with (`PROTOBACK_PYTHON`, else `python3`) must have
//! protobuf (`common::tools`' `python-protobuf`).

use crate::common::{compile_proto, protoback, tmp, tools, Run};
use std::path::{Path, PathBuf};

/// A plugin adding an import of plugin_target.proto to plugin_subject.
const ADD_IMPORT: &str = "
def fdp_plugin(fdp):
    if fdp.name == 'plugin_subject.proto':
        fdp.dependency.append('plugin_target.proto')
";

/// The same, plus a field whose type lives in the imported file: it only
/// survives if plugin_target.proto is ranked ahead of its importer.
const ADD_IMPORT_AND_FIELD: &str = "
def fdp_plugin(fdp):
    if fdp.name != 'plugin_subject.proto':
        return
    fdp.dependency.append('plugin_target.proto')
    field = fdp.message_type[0].field.add()
    field.name = 'target'
    field.number = 2
    field.type = 11              # TYPE_MESSAGE
    field.label = 1              # LABEL_OPTIONAL
    field.type_name = '.plugin_test.Target'
";

const ADD_MISSING_IMPORT: &str = "
def fdp_plugin(fdp):
    if fdp.name == 'plugin_subject.proto':
        fdp.dependency.append('nowhere_to_be_found.proto')
";

/// Both fixtures compiled into `pb`, which is the -I root, so that
/// plugin_target.proto resolves there if and only if the discovery loop
/// looks for it; only the subject given. The plugin written beside.
fn setup(test: &str, plugin: &str) -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    if !tools(&["protoc", "python-protobuf"]) {
        return None;
    }
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let subject =
        compile_proto(&pb, &["plugin_subject.proto", "plugin_target.proto"]).swap_remove(0);
    let file = dir.join("plugin.py");
    std::fs::write(&file, plugin).unwrap();
    Some((pb, out, subject, file))
}

fn run(pb: &Path, out: &Path, subject: &Path, plugin: &Path) -> Run {
    protoback([
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", pb.display()),
        format!("--proto-out={}", out.display()),
        format!("--fdp-plugin={}", plugin.display()),
        subject.display().to_string(),
    ])
}

/// The named file is read off -I, ranked, and rendered; before the fix it
/// was never read and the import dangled.
#[test]
fn test_plugin_added_import_is_discovered() {
    let Some((pb, out, subject, plugin)) =
        setup("test_plugin_added_import_is_discovered", ADD_IMPORT)
    else {
        return;
    };
    let r = run(&pb, &out, &subject, &plugin);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(out.join("plugin_subject.proto").exists());
    assert!(
        out.join("plugin_target.proto").exists(),
        "the plugin-added import was never loaded from -I"
    );
    let rendered = std::fs::read_to_string(out.join("plugin_subject.proto")).unwrap();
    assert!(
        rendered.contains(r#"import "plugin_target.proto";"#),
        "{rendered}"
    );
}

/// The silent failure: the field deleted with only a W4.
#[test]
fn test_plugin_added_import_keeps_its_fields() {
    let Some((pb, out, subject, plugin)) = setup(
        "test_plugin_added_import_keeps_its_fields",
        ADD_IMPORT_AND_FIELD,
    ) else {
        return;
    };
    let r = run(&pb, &out, &subject, &plugin);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    let rendered = std::fs::read_to_string(out.join("plugin_subject.proto")).unwrap();
    assert!(
        rendered.contains("Target target = 2;"),
        "plugin-added field was stripped:\n{rendered}"
    );
    assert!(
        !r.stderr.to_lowercase().contains("unresolvable"),
        "{}",
        r.stderr
    );
}

/// The import is emitted once, though the plugin runs at every parse: each
/// parse hands it a pristine descriptor (S1).
#[test]
fn test_plugin_added_import_appears_once() {
    let Some((pb, out, subject, plugin)) =
        setup("test_plugin_added_import_appears_once", ADD_IMPORT)
    else {
        return;
    };
    let r = run(&pb, &out, &subject, &plugin);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    let rendered = std::fs::read_to_string(out.join("plugin_subject.proto")).unwrap();
    assert_eq!(
        rendered.matches(r#"import "plugin_target.proto";"#).count(),
        1,
        "import emitted more than once:\n{rendered}"
    );
}

/// An unresolvable added import is stripped, kept as an orphan comment
/// (spec 0053), and reported.
#[test]
fn test_plugin_added_import_unresolvable_is_stripped() {
    let Some((pb, out, subject, plugin)) = setup(
        "test_plugin_added_import_unresolvable_is_stripped",
        ADD_MISSING_IMPORT,
    ) else {
        return;
    };
    let r = run(&pb, &out, &subject, &plugin);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    let text = std::fs::read_to_string(out.join("plugin_subject.proto")).unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    assert!(
        !lines.contains(&r#"import "nowhere_to_be_found.proto";"#),
        "unresolvable import left live in the output"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("nowhere_to_be_found.proto") && l.starts_with("//")),
        "a stripped import must be retained as an orphan comment (spec 0053)"
    );
    assert!(
        r.stderr.contains("nowhere_to_be_found.proto"),
        "stripping an import must be reported"
    );
}

/// A raising plugin aborts the run, named as the culprit, nothing written.
fn assert_plugin_abort(r: &Run, out: &Path) {
    assert_ne!(
        r.code,
        Some(0),
        "a raising plugin must abort the run:\n{}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("Traceback"),
        "a plugin fault must not surface as a crash:\n{}",
        r.stderr
    );
    assert!(
        r.stderr.contains("plugin raised"),
        "the diagnostic must name the plugin as the culprit:\n{}",
        r.stderr
    );
    let written = std::fs::read_dir(out)
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.path().extension().is_some_and(|x| x == "proto"));
    assert!(!written, "no output may be written");
}

fn raising(test: &str, plugin: &str) -> Option<(Run, PathBuf)> {
    let (pb, out, subject, file) = setup(test, plugin)?;
    Some((run(&pb, &out, &subject, &file), out))
}

/// Baseline: an exception no handler on the path would catch.
#[test]
fn test_plugin_exception_aborts_the_run() {
    let Some((r, out)) = raising(
        "test_plugin_exception_aborts_the_run",
        "\ndef fdp_plugin(fdp):\n    raise RuntimeError('boom')\n",
    ) else {
        return;
    };
    assert_plugin_abort(&r, &out);
    assert!(r.stderr.contains("RuntimeError"), "{}", r.stderr);
}

/// Not blamed on the input as an unparseable file.
#[test]
fn test_plugin_decode_error_is_not_a_corrupt_input() {
    let Some((r, out)) = raising(
        "test_plugin_decode_error_is_not_a_corrupt_input",
        "\nfrom google.protobuf.message import DecodeError\n\n\ndef fdp_plugin(fdp):\n    raise DecodeError('not actually a decode failure')\n",
    ) else {
        return;
    };
    assert_plugin_abort(&r, &out);
    assert!(
        !r.stderr.contains("Skipping unparseable file"),
        "a plugin fault must not be reported as a corrupt input file"
    );
}

/// Not blamed on a descriptor conflict.
#[test]
fn test_plugin_type_error_is_not_a_pool_conflict() {
    let Some((r, out)) = raising(
        "test_plugin_type_error_is_not_a_pool_conflict",
        "\ndef fdp_plugin(fdp):\n    raise TypeError('not actually a pool conflict')\n",
    ) else {
        return;
    };
    assert_plugin_abort(&r, &out);
    assert!(
        !r.stderr.contains("Could not add descriptor"),
        "a plugin fault must not be reported as a pool conflict"
    );
}

/// Raised while patching a built-in copy: aborts too, not swallowed.
#[test]
fn test_plugin_exception_on_a_fallback_aborts() {
    let Some((r, out)) = raising(
        "test_plugin_exception_on_a_fallback_aborts",
        "\ndef fdp_plugin(fdp):\n    if fdp.name == 'google/protobuf/descriptor.proto':\n        raise AttributeError('raised while patching a fallback')\n",
    ) else {
        return;
    };
    assert_plugin_abort(&r, &out);
    assert!(r.stderr.contains("AttributeError"), "{}", r.stderr);
}
