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

/// A `Command` for the `prototext` binary with the descriptor-set
/// environment variables removed (spec 0228 S8).
///
/// The dev-shell exports `PROTOTEXT_DESCRIPTOR_SET`, and `nix-build` does
/// not. Every test here passes `--descriptor-set` explicitly or means to
/// exercise the built-in fallback, so an inherited value would silently
/// make the two environments test different things.
fn prototext_cmd() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prototext"));
    cmd.env_remove("PROTOTEXT_DESCRIPTOR_SET")
        .env_remove("PROTOTEXT_DEFAULT_DESCRIPTOR");
    cmd
}

/// Run `prototext --descriptor-set <schema> decode --type <message>` on binary
/// input, then `prototext encode` on the text output.
/// Returns (text, re-encoded binary).
fn cli_roundtrip(
    wire: &[u8],
    schema_path: &Path,
    message: &str,
    annotations: bool,
) -> (Vec<u8>, Vec<u8>) {
    let mut decode_cmd = prototext_cmd();
    decode_cmd
        .arg("--descriptor-set")
        .arg(schema_path)
        .arg("decode")
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
        "prototext decode failed:\n{}",
        String::from_utf8_lossy(&decode_out.stderr)
    );
    let text = decode_out.stdout;

    let encode_out = prototext_cmd()
        .arg("encode")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn prototext")
        .wait_with_output_and_stdin(&text);

    assert!(
        encode_out.status.success(),
        "prototext encode failed:\n{}",
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

/// CLI: `prototext decode` then `prototext encode` must reproduce the original wire bytes.
///
/// Pipeline: craft_a() → wire → `prototext decode` → text → `prototext encode` → wire2
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

// ── §3.3 Unknown LEN field decoded as nested message (spec 0097) ─────────────

/// A hand-crafted SwissArmyKnife wire payload that contains a known field
/// (field 25 / int32Op = 42) followed by an unknown LEN field (field 9001)
/// whose payload is itself a valid protobuf message.
///
/// Spec 0097 S3 requires that the unknown LEN field be rendered as a nested
/// message (not raw bytes), and that the round-trip is lossless.
///
/// Wire layout (16 bytes):
///   \xc8\x01      — tag: field 25, wire type 0 (varint)
///   \x2a          — value: 42
///   \xca\xb2\x04  — tag: field 9001, wire type 2 (LEN)
///   \x09          — length: 9
///   \x08\x07      — inner field 1, varint 7
///   \x12\x05hello — inner field 2, string "hello"
#[test]
fn unknown_len_decoded_as_nested_message() {
    #[rustfmt::skip]
    let wire: &[u8] = &[
        0xc8, 0x01,              // tag: field 25, varint
        0x2a,                    // value: 42
        0xca, 0xb2, 0x04,        // tag: field 9001, LEN
        0x09,                    // length: 9
        0x08, 0x07,              // inner: field 1 varint 7
        0x12, 0x05,              // inner: field 2 LEN length 5
        b'h', b'e', b'l', b'l', b'o', // "hello"
    ];

    let sp = schema_path("fixtures/schemas/knife.pb");
    let (text, wire2) = cli_roundtrip(wire, &sp, "SwissArmyKnife", true);
    let text_str = String::from_utf8_lossy(&text);

    // The unknown field must be rendered as a nested message (brace syntax),
    // not as raw bytes.
    assert!(
        text_str.contains("9001 {"),
        "unknown LEN field must be rendered as nested message, got:\n{text_str}"
    );

    // Round-trip must be lossless.
    assert_eq!(
        wire2, wire,
        "binary→text→binary round-trip must be bit-exact\n  text:\n{text_str}"
    );
}

// ── Spec 0106 TC-7: --type typo suggests the closest match ───────────────────

/// `decode --type <typo>` against a schema containing a near-but-not-exact
/// match must report a closest-match-by-edit-distance suggestion, not a
/// full dump of every message name in the pool.
#[test]
fn decode_type_typo_suggests_closest_match() {
    let sp = schema_path("fixtures/schemas/knife.pb");
    let out = prototext_cmd()
        .arg("--descriptor-set")
        .arg(&sp)
        .arg("decode")
        .args(["--type", "SwissArmyKnif"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn prototext")
        .wait_with_output_and_stdin(&[]);
    assert!(
        !out.status.success(),
        "decode with a nonexistent --type must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("did you mean 'SwissArmyKnife'?"),
        "expected closest-match suggestion, got: {stderr}"
    );
}

// ── Spec 0238 S11: the CLI's ScoringOpts still carries what the flags say ────

/// `prototext score` must keep honoring `--no-expand-any`.
///
/// This drives the binary rather than `score_all` on purpose. `run.rs` builds
/// `ScoringOpts` at three sites, and spec 0238 S11 converted them to
/// `..Default::default()` so future fields cost nothing — which is exactly
/// the shape that lets a flag quietly stop being wired up, since a dropped
/// field then silently takes its default instead of failing to compile.
/// `ScoringOpts::default()` would agree with the CLI either way, so a
/// library-level test cannot see the difference.
///
/// The payload is a `google.protobuf.Option` whose `value` is an `Any`
/// wrapping a `Duration`: Any expansion is reached only from a *field*
/// pointing at the Any block, so the Any has to be nested rather than the
/// scored type itself. Expanded, the inner `seconds` is a third match.
#[test]
#[cfg(feature = "wkt-db")]
fn score_still_honors_no_expand_any() {
    #[rustfmt::skip]
    let payload: &[u8] = &[
        0x0a, 0x01, b'x',                       // Option.name = "x"
        0x12, 0x32,                             // Option.value, LEN 50
          0x0a, 0x2c,                           //   Any.type_url, LEN 44
            b't', b'y', b'p', b'e', b'.', b'g', b'o', b'o', b'g', b'l', b'e',
            b'a', b'p', b'i', b's', b'.', b'c', b'o', b'm', b'/',
            b'g', b'o', b'o', b'g', b'l', b'e', b'.', b'p', b'r', b'o', b't',
            b'o', b'b', b'u', b'f', b'.', b'D', b'u', b'r', b'a', b't', b'i',
            b'o', b'n',
          0x12, 0x02,                           //   Any.value, LEN 2
            0x08, 0x05,                         //     Duration.seconds = 5
    ];

    let score = |extra: &[&str]| -> String {
        let out = prototext_cmd()
            .args([
                "score",
                "--type",
                "google.protobuf.Option",
                "--assume-binary",
            ])
            .args(extra)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn prototext")
            .wait_with_output_and_stdin(payload);
        assert!(
            out.status.success(),
            "prototext score failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("score output is UTF-8")
    };

    assert!(
        score(&[]).contains("score: 3"),
        "expanding the Any must score the wrapped Duration's field too, got:\n{}",
        score(&[])
    );
    assert!(
        score(&["--no-expand-any"]).contains("score: 2"),
        "--no-expand-any must score the Any's value as plain bytes, got:\n{}",
        score(&["--no-expand-any"])
    );
}

// ── §3.2 No crash without annotations (all fixtures) ─────────────────────────

/// CLI: `prototext decode --no-annotations` must exit 0 for every fixture.
///
/// Without annotations the header is suppressed and encode is not possible,
/// so this test only checks that decode itself succeeds cleanly.
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
        let out = prototext_cmd()
            .arg("--descriptor-set")
            .arg(&sp)
            .arg("decode")
            .args(["--type", &message])
            .arg("--no-annotations")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn prototext")
            .wait_with_output_and_stdin(&wire);
        assert!(
            out.status.success(),
            "{name}: prototext decode --no-annotations must exit 0:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        ran += 1;
    }

    eprintln!("fixture_no_panic_no_annotations: {ran} passed, {skipped} skipped");
    assert!(ran > 0, "no fixtures ran");
}
