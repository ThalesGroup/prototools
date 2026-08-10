// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! CLI-level (black-box, subprocess) integration tests for `protolens`'s
//! batch `script` subcommand — spec 0271 S14, and the smoke test the
//! `grpconf/anomalies.script` walk exists to be.
//!
//! The transcript reports the *resolved* outcome of every directive, so
//! a script that has drifted out of sync with its blob shows up here as
//! a diagnostic line rather than as a talk that falls apart on stage.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_protolens")
}

/// The workspace root — `CARGO_MANIFEST_DIR` is `protolens/`.
fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("protolens/ has a parent")
        .to_path_buf()
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        // Spec 0228 S8, as in `batch_export.rs`: the dev shell exports a
        // descriptor set and nix-build does not, so both are cleared to
        // make the two environments resolve the same one.
        .env_remove("PROTOTEXT_DESCRIPTOR_SET")
        .env_remove("PROTOTEXT_DEFAULT_DESCRIPTOR")
        .output()
        .expect("failed to spawn protolens")
}

/// The `anomalies.pb` invocation the README documents, plus a subcommand.
fn anomalies(extra: &[&str]) -> Output {
    let root = repo();
    let descriptor = root.join("prototext-core/fixtures/descriptor.pb");
    let blob = root.join("grpconf/anomalies.pb");
    let mut args = vec![
        "--descriptor-set",
        descriptor.to_str().unwrap(),
        "--type",
        "google.protobuf.FileDescriptorProto",
    ];
    args.extend_from_slice(extra);
    args.push(blob.to_str().unwrap());
    args.push("script");
    run(&args)
}

/// Spec 0271 test-plan item 7. The script is found beside the blob, every
/// position in it resolves, and every step is walked.
///
/// Deliberately not a golden file of the whole transcript: the row ranges
/// in it move with any change to how a line is rendered, which would make
/// this a test of the renderer rather than of the script. What must not
/// drift is that each directive still finds what it names — and that is
/// exactly what an `error:` line reports.
#[test]
fn anomalies_script_walks_without_a_broken_position() {
    let out = anomalies(&[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Spec 0198 S2: a successful batch subcommand says nothing.
    assert!(
        out.stderr.is_empty(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let transcript = String::from_utf8(out.stdout).expect("the transcript is text");
    assert!(
        transcript.starts_with("script: anomalies.script\n"),
        "the implicit script beside the blob must be the one walked:\n{transcript}"
    );

    let broken: Vec<&str> = transcript
        .lines()
        .filter(|l| l.trim_start().starts_with("error:"))
        .collect();
    assert!(
        broken.is_empty(),
        "every position in anomalies.script must still resolve: {broken:?}"
    );

    let steps = transcript
        .lines()
        .filter(|l| l.starts_with("step "))
        .count();
    assert!(steps > 1, "the walk must have steps:\n{transcript}");
    assert!(
        transcript.contains(&format!("step {steps}/{steps}")),
        "and it must reach the last one:\n{transcript}"
    );
}

/// Spec 0271 S1: `--no-script` suppresses the implicit discovery, and
/// then the subcommand has nothing to walk.
#[test]
fn no_script_suppresses_the_script_beside_the_blob() {
    let out = anomalies(&["--no-script"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("needs a script"),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Spec 0271 S1: an explicit script that is not there is a hard error,
/// and it is reported before the descriptor set is even loaded.
#[test]
fn a_missing_explicit_script_is_refused() {
    let missing = std::env::temp_dir().join("protolens-no-such.script");
    let out = anomalies(&["--script", missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot read script"), "stderr:\n{stderr}");
    assert!(
        !stderr.contains("loading descriptor set"),
        "the refusal must come before the wait it would have caused:\n{stderr}"
    );
}
