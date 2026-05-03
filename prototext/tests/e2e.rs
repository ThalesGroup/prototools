// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! End-to-end tests driven by the craft_a fixture registry (spec 0009).

use std::path::{Path, PathBuf};
use std::process::Command;

// Pull in the protocraft module (test-only).
#[path = "../src/protocraft/mod.rs"]
mod protocraft;

use protocraft::craft_a;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn index_schema(name: &str) -> Option<(String, String)> {
    let index_path = repo_root().join("fixtures/index.toml");
    let text = std::fs::read_to_string(&index_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", index_path.display(), e));
    let doc: toml::Value = text
        .parse()
        .unwrap_or_else(|e| panic!("cannot parse index.toml: {e}"));
    doc.get("fixture")
        .and_then(|v| v.as_array())
        .unwrap_or(&vec![])
        .iter()
        .find(|entry| entry["name"].as_str() == Some(name))
        .map(|entry| {
            (
                entry["schema"].as_str().unwrap().to_owned(),
                entry["message"].as_str().unwrap().to_owned(),
            )
        })
}

fn schema_path(schema_rel: &str) -> PathBuf {
    let generated = ["descriptor.pb", "knife.pb", "enum_collision.pb"];
    if let Some(name) = generated
        .iter()
        .find(|&&n| schema_rel == format!("fixtures/schemas/{n}"))
    {
        return PathBuf::from(env!("OUT_DIR")).join(name);
    }
    repo_root().join(schema_rel)
}

/// Run `prototext -d --descriptor <schema> --type <message>` on binary input,
/// then `prototext -e` on the text output.  Returns (text, re-encoded binary).
fn cli_roundtrip(
    wire: &[u8],
    schema_path: &Path,
    message: &str,
    annotations: bool,
) -> (Vec<u8>, Vec<u8>) {
    let bin = env!("CARGO_BIN_EXE_prototext");

    let mut decode_cmd = Command::new(bin);
    decode_cmd
        .args(["-d", "--descriptor"])
        .arg(schema_path)
        .args(["--type", message]);
    if !annotations {
        decode_cmd.arg("--no-annotations");
    }
    let decode_out = decode_cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn prototext")
        .wait_with_output_and_stdin(wire);

    assert!(
        decode_out.status.success(),
        "prototext -d failed:\n{}",
        String::from_utf8_lossy(&decode_out.stderr)
    );
    let text = decode_out.stdout;

    let encode_out = Command::new(bin)
        .arg("-e")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn prototext")
        .wait_with_output_and_stdin(&text);

    assert!(
        encode_out.status.success(),
        "prototext -e failed:\n{}",
        String::from_utf8_lossy(&encode_out.stderr)
    );

    (text, encode_out.stdout)
}

trait SpawnExt {
    fn wait_with_output_and_stdin(self, input: &[u8]) -> std::process::Output;
}

impl SpawnExt for std::process::Child {
    fn wait_with_output_and_stdin(mut self, input: &[u8]) -> std::process::Output {
        use std::io::Write;
        if let Some(mut stdin) = self.stdin.take() {
            stdin.write_all(input).ok();
        }
        self.wait_with_output().expect("wait_with_output failed")
    }
}

// ── §3.1 Lossless round-trip with annotations (all fixtures) ─────────────────

/// CLI: `prototext -d` then `prototext -e` must reproduce the original wire bytes.
///
/// Pipeline: craft_a() → wire → `prototext -d` → text → `prototext -e` → wire2
/// Assert: wire2 == wire (bit-exact).
#[test]
fn fixture_roundtrip_annotated_craft_a() {
    let mut ran = 0;
    let mut skipped = 0;

    for &(name, func) in craft_a::ALL_FIXTURES {
        let Some((schema_rel, message)) = index_schema(name) else {
            eprintln!("SKIP  {name} (not in index.toml)");
            skipped += 1;
            continue;
        };

        let wire = func();
        let sp = schema_path(&schema_rel);
        let (text, wire2) = cli_roundtrip(&wire, &sp, &message, true);

        assert_eq!(
            wire2,
            wire,
            "{name}: binary→text→binary round-trip must be bit-exact\n  text:\n{}",
            String::from_utf8_lossy(&text),
        );
        ran += 1;
    }

    eprintln!("fixture_roundtrip_annotated_craft_a: {ran} passed, {skipped} skipped");
    assert!(ran > 0, "no fixtures ran");
}

// ── §3.2 No crash without annotations (all fixtures) ─────────────────────────

/// CLI: `prototext -d --no-annotations` must exit 0 for every fixture.
#[test]
fn fixture_no_panic_no_annotations() {
    let mut ran = 0;
    let mut skipped = 0;

    for &(name, func) in craft_a::ALL_FIXTURES {
        let Some((schema_rel, message)) = index_schema(name) else {
            eprintln!("SKIP  {name} (not in index.toml)");
            skipped += 1;
            continue;
        };

        let wire = func();
        let sp = schema_path(&schema_rel);
        // cli_roundtrip with annotations=false asserts exit 0 internally.
        cli_roundtrip(&wire, &sp, &message, false);
        ran += 1;
    }

    eprintln!("fixture_no_panic_no_annotations: {ran} passed, {skipped} skipped");
    assert!(ran > 0, "no fixtures ran");
}
