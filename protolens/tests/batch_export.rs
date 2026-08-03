// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! CLI-level (black-box, subprocess) integration tests for `protolens`'s
//! batch `export` subcommand — spec 0123 Test plan items 2-6, plus
//! spec 0156's rename/descriptor-format additions.
//!
//! `protolens` is a binary-only crate (no `lib.rs`), so unlike
//! `prototext/tests/e2e.rs` (which pulls in `protocraft` via a `#[path]`
//! include), these tests can't reach any of `protolens`'s own internal
//! modules. Fixtures are therefore rebuilt from scratch here using
//! `prost-types` (already a `[dev-dependencies]` entry), and the CLI is
//! exercised purely as a subprocess via `CARGO_BIN_EXE_protolens`.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use prost::Message as _;
use prost_types::descriptor_proto::ExtensionRange;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet, MessageOptions,
};

// ── Helpers ──────────────────────────────────────────────────────────────

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_protolens")
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("failed to spawn protolens")
}

/// Asserts that `out` is a *deliberate refusal*: every diagnostic path
/// in `main.rs` returns `ExitCode::FAILURE`, which is exit status 1.
///
/// `!out.status.success()` is not the same statement and is not enough.
/// A panic exits 101 and a signal exits with no code at all, and both
/// are "not success" — so a negative test written that way keeps
/// passing after the behavior it guards has stopped being a refusal and
/// started being a crash, which is exactly when it is most wanted.
#[track_caller]
fn assert_refused(out: &Output, what: &str) {
    assert_eq!(
        out.status.code(),
        Some(1),
        "{what}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A unique path under the system temp dir — never created, just named
/// (tests run in parallel and must not collide with each other, nor with
/// leftovers from prior runs).
fn temp_path(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "protolens-batch-export-{name}-{}-{n}.tmp",
        std::process::id()
    ))
}

/// Deletes its wrapped path on drop — keeps every test's temp files from
/// piling up regardless of pass/fail/panic.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl TempFile {
    fn path(&self) -> &str {
        self.0.to_str().expect("temp path must be valid UTF-8")
    }
}

fn write_temp(name: &str, bytes: &[u8]) -> TempFile {
    let path = temp_path(name);
    std::fs::write(&path, bytes).unwrap();
    TempFile(path)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// `Outer { inner: Inner { x: int32 } }`, wired as
/// `Outer{ inner: Inner{ x: 1 } }` = `0x0A 0x02 0x08 0x01`
/// (field 1 LEN, payload = field 1 varint 1).
fn outer_inner_fixture() -> (TempFile, TempFile) {
    let inner = DescriptorProto {
        name: Some("Inner".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("x".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let outer = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("inner".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Inner".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer, inner],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };
    let descriptor = write_temp("outer-inner-desc", &fds.encode_to_vec());
    let blob = write_temp("outer-inner-blob", &[0x0A, 0x02, 0x08, 0x01]);
    (descriptor, blob)
}

/// Same `TestMessageSet`/`ExtPayload`/`Container` schema and
/// `Container{ extensions: TestMessageSet{ Item{ type_id: 100, message:
/// ExtPayload{ label: "hi" } } } }` blob as `tui.rs`'s (private)
/// `message_set_fixture` unit-test helper (spec 0120) — rebuilt here for
/// the reason given in this file's header comment.
fn message_set_fixture() -> (TempFile, TempFile) {
    let message_set_msg = DescriptorProto {
        name: Some("TestMessageSet".to_string()),
        options: Some(MessageOptions {
            message_set_wire_format: Some(true),
            ..Default::default()
        }),
        extension_range: vec![ExtensionRange {
            start: Some(1),
            end: Some(536870912),
            ..Default::default()
        }],
        ..Default::default()
    };
    let ext_payload_msg = DescriptorProto {
        name: Some("ExtPayload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("label".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let extension_field = FieldDescriptorProto {
        name: Some("ext_payload".to_string()),
        number: Some(100),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Message as i32),
        type_name: Some(".ms_test.ExtPayload".to_string()),
        extendee: Some(".ms_test.TestMessageSet".to_string()),
        ..Default::default()
    };
    let container_msg = DescriptorProto {
        name: Some("Container".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("extensions".to_string()),
            number: Some(2),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".ms_test.TestMessageSet".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("ms_test.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("ms_test".to_string()),
        message_type: vec![message_set_msg, ext_payload_msg, container_msg],
        extension: vec![extension_field],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };
    let descriptor = write_temp("message-set-desc", &fds.encode_to_vec());

    // Container { extensions: TestMessageSet {
    //   Item { type_id: 100, message: ExtPayload { label: "hi" } }
    // } }.
    let ext_payload_bytes = [0x0au8, 0x02, b'h', b'i'];
    let mut item_bytes = vec![0x0bu8, 0x10, 100u8];
    item_bytes.push(0x1a);
    item_bytes.push(ext_payload_bytes.len() as u8);
    item_bytes.extend_from_slice(&ext_payload_bytes);
    item_bytes.push(0x0c); // END_GROUP
    let mut blob_bytes = vec![0x12u8, item_bytes.len() as u8];
    blob_bytes.extend_from_slice(&item_bytes);

    let blob = write_temp("message-set-blob", &blob_bytes);
    (descriptor, blob)
}

// ── Item 2: `export /` matches the fixture's known-good rendering ───────

#[test]
fn export_root_produces_the_expected_prototext() {
    let (descriptor, blob) = outer_inner_fixture();
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
    ]);
    assert!(
        out.status.success(),
        "export / must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("#@ prototext: protoc\n"),
        "text output must start with the prototext header, got: {text}"
    );
    assert!(
        text.contains("inner {"),
        "expected the nested `inner` field, got: {text}"
    );
    assert!(
        text.contains("x: 1"),
        "expected the inner `x` field (x: 1), got: {text}"
    );
}

// ── Item 3: `--load-overrides` alongside a MessageSet auto-override ──────

#[test]
fn export_with_load_overrides_still_shows_message_set_auto_expansion() {
    let (descriptor, blob) = message_set_fixture();
    let descriptor_bytes = std::fs::read(&descriptor.0).unwrap();
    let blob_bytes = std::fs::read(&blob.0).unwrap();

    // `load_overrides` *replaces* the whole collection (spec 0117 §4),
    // including spec 0120's auto-detected entries seeded at `App::new`
    // time — so the loaded collection must itself carry the two
    // MessageSet auto-override tiers (tier 1: the `Item` group retyped to
    // the synthetic `protolens_internal.Item` — `decode.rs`'s
    // `MESSAGE_SET_ITEM_FQDN`, which is deliberately *not* spelled
    // `MessageSetItem`; tier 2: its `message` field retyped to the resolved
    // extension type), at their positional paths (`/1/1` = the sole
    // `Item`, `/1/1/2` = `Item`'s 2nd child, `message`) — exactly what a
    // real `:save-overrides` from an already-auto-expanded session would
    // have written. The document root's own type is deliberately *not*
    // included here: it's meant to be preserved automatically across a
    // wholesale collection replace, not something a hand-authored
    // overrides file is expected to carry.
    let yaml = format!(
        r#"version: 1
target:
  blob_sha256: "{}"
  descriptor_set_sha256: "{}"
overrides:
  - path: "/1/1"
    type: protolens_internal.Item
    active: true
    name: Item
  - path: "/1/1/2"
    type: ms_test.ExtPayload
    active: true
"#,
        sha256_hex(&blob_bytes),
        sha256_hex(&descriptor_bytes),
    );
    let overrides = write_temp("message-set-overrides", yaml.as_bytes());

    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "ms_test.Container",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        overrides.path(),
    ]);
    assert!(
        out.status.success(),
        "export with a hash-matching --load-overrides must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "matching target hashes must not warn: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("label") && text.contains("hi"),
        "MessageSet Item must auto-expand to the resolved extension type \
         (not raw bytes) even with --load-overrides applied, got:\n{text}"
    );
}

// ── Item 4: an unresolvable export path is a hard error, no TUI ─────────

#[test]
fn export_unresolvable_path_fails_without_entering_the_tui() {
    let (descriptor, blob) = outer_inner_fixture();
    // Command::output() would hang forever waiting for a child that has
    // entered the TUI's raw-mode input loop (stdin/stdout are not a TTY
    // here, but the TUI still blocks reading events) — this test's own
    // completion is itself proof that batch mode returned instead of
    // calling `tui::run`.
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/99",
    ]);
    assert_refused(&out, "export with an unresolvable path must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not resolve"),
        "expected a resolution diagnostic, got: {stderr}"
    );
}

// ── Item 4a: bad --load-overrides is a hard error ─────────────────────────

#[test]
fn export_load_overrides_missing_file_is_a_hard_error() {
    let (descriptor, blob) = outer_inner_fixture();
    let missing = temp_path("missing-overrides");
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        missing.to_str().unwrap(),
    ]);
    assert_refused(
        &out,
        "--load-overrides pointing at a missing file must fail",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--load-overrides"),
        "expected a --load-overrides diagnostic, got: {stderr}"
    );
}

#[test]
fn export_load_overrides_malformed_yaml_is_a_hard_error() {
    let (descriptor, blob) = outer_inner_fixture();
    let overrides = write_temp("malformed-overrides", b"{not valid yaml: [1, 2\n");
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        overrides.path(),
    ]);
    assert_refused(
        &out,
        "--load-overrides pointing at malformed YAML must fail",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--load-overrides"),
        "expected a --load-overrides diagnostic, got: {stderr}"
    );
}

/// By contrast (spec 0123 Test plan item 4a's clarifying note): a
/// hash-mismatched but otherwise-valid overrides collection is a
/// stderr-only warning, not a hard error — same policy as the TUI's own
/// `:restore`.
#[test]
fn export_load_overrides_hash_mismatch_warns_but_still_succeeds() {
    let (descriptor, blob) = outer_inner_fixture();
    let yaml = format!(
        "version: 1\ntarget:\n  blob_sha256: \"{}\"\n  descriptor_set_sha256: \"{}\"\noverrides: []\n",
        "0".repeat(64),
        "0".repeat(64),
    );
    let overrides = write_temp("hash-mismatch-overrides", yaml.as_bytes());
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        overrides.path(),
    ]);
    assert!(
        out.status.success(),
        "a hash mismatch must warn, not fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning") && stderr.contains("hash mismatch"),
        "expected a hash-mismatch warning, got: {stderr}"
    );
}

/// ...but not for the descriptor formats (quality audit E6). Those
/// export a *schema* assembled from the overrides themselves, so a
/// collection written against a different blob or descriptor set makes
/// the output wrong rather than merely stale — and the consumer of a
/// descriptor set has no way to notice. Fails, and writes nothing.
#[test]
fn export_descriptor_binary_with_a_hash_mismatch_is_a_hard_error() {
    let (descriptor, blob) = outer_inner_fixture();
    let yaml = format!(
        "version: 1\ntarget:\n  blob_sha256: \"{}\"\n  descriptor_set_sha256: \"{}\"\noverrides: []\n",
        "0".repeat(64),
        "0".repeat(64),
    );
    let overrides = write_temp("descriptor-hash-mismatch-overrides", yaml.as_bytes());
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--format",
        "descriptor-binary",
        "--load-overrides",
        overrides.path(),
    ]);
    assert_refused(&out, "a hash mismatch must be fatal for a schema export");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error") && stderr.contains("different blob or descriptor set"),
        "expected a fatal hash-mismatch diagnostic, got: {stderr}"
    );
}

/// Quality audit E7: `--format=descriptor-*` without `--load-overrides`
/// is refused before the descriptor set is even opened — the check used
/// to run after the whole startup, which is seconds on a large set.
/// A nonexistent `--descriptor-set` is the probe: reaching the load at
/// all would produce *its* diagnostic instead of this one.
#[test]
fn descriptor_format_without_load_overrides_fails_before_startup() {
    let (_descriptor, blob) = outer_inner_fixture();
    let out = run(&[
        "--descriptor-set",
        "/nonexistent/descriptor.desc",
        blob.path(),
        "export",
        "/",
        "--format",
        "descriptor-prototext",
    ]);
    assert_refused(&out, "the flag pairing must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("requires --load-overrides"),
        "the flag pairing must be refused before the descriptor set is read, got: {stderr}"
    );
}

// ── Spec 0221: a refused override is reported, not swallowed ────────────

/// An overrides file naming a type that is not in the descriptor set.
/// Hashes match, so nothing *else* has anything to complain about and
/// the refusal is the only thing that can put a word on stderr.
fn unappliable_overrides_yaml(blob: &[u8], descriptor: &[u8], entries: &str) -> TempFile {
    let yaml = format!(
        "version: 1\ntarget:\n  blob_sha256: \"{}\"\n  descriptor_set_sha256: \"{}\"\noverrides:\n{entries}",
        sha256_hex(blob),
        sha256_hex(descriptor),
    );
    write_temp("unappliable-overrides", yaml.as_bytes())
}

/// Spec 0221 G1/S3: an override that could not be applied makes the
/// export fail, loudly, and produce no output file. Before 0221 this
/// wrote a rendering with the override missing and exited 0 — and for
/// `--format=descriptor-binary`, which *requires* `--load-overrides`,
/// that is a wrong schema handed to whatever consumes it.
#[test]
fn export_with_an_unappliable_override_fails_loudly() {
    let (descriptor, blob) = outer_inner_fixture();
    let overrides = unappliable_overrides_yaml(
        &std::fs::read(blob.path()).unwrap(),
        &std::fs::read(descriptor.path()).unwrap(),
        "  - path: \"/1\"\n    type: test.NoSuchType\n    active: true\n",
    );
    let out_path = temp_path("unappliable-output");
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        overrides.path(),
        "--output",
        out_path.to_str().unwrap(),
    ]);
    assert_refused(
        &out,
        "an override that could not be applied must not report success",
    );
    assert!(
        !out_path.exists(),
        "no output file may be written when an override was refused"
    );
}

/// Spec 0221 G4/S2: the diagnostic is written for someone who has never
/// read this spec, so each of its four parts is asserted on its own —
/// dropping one has to be a test failure, not a cosmetic diff.
#[test]
fn a_refusal_line_names_file_node_type_and_reason() {
    let (descriptor, blob) = outer_inner_fixture();
    let overrides = unappliable_overrides_yaml(
        &std::fs::read(blob.path()).unwrap(),
        &std::fs::read(descriptor.path()).unwrap(),
        "  - path: \"/1\"\n    type: test.NoSuchType\n    active: true\n",
    );
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        overrides.path(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--load-overrides") && stderr.contains(overrides.path()),
        "the line must name the flag and the file it came from: {stderr}"
    );
    assert!(
        stderr.contains("/1"),
        "the line must name the node, which is how the file names it: {stderr}"
    );
    assert!(
        stderr.contains("test.NoSuchType"),
        "the line must name the type that was asked for: {stderr}"
    );
    assert!(
        stderr.contains("not found in descriptor set"),
        "the line must carry the underlying reason: {stderr}"
    );
}

/// Spec 0221 G3/S1: two unappliable overrides produce two lines. Before
/// 0221 the refusal was assigned to `self.message`, not pushed, so the
/// second destroyed the first and the user could not learn there had
/// been more than one.
#[test]
fn refusals_are_collected_not_overwritten() {
    let (descriptor, blob) = outer_inner_fixture();
    let overrides = unappliable_overrides_yaml(
        &std::fs::read(blob.path()).unwrap(),
        &std::fs::read(descriptor.path()).unwrap(),
        concat!(
            "  - path: \"/1\"\n    type: test.NoSuchType\n    active: true\n",
            "  - path: \"/1/1\"\n    type: test.AlsoMissing\n    active: true\n",
        ),
    );
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        overrides.path(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("test.NoSuchType") && stderr.contains("test.AlsoMissing"),
        "both refusals must be reported, not just the last: {stderr}"
    );
}

/// Spec 0221's bound: 0198 S2 says a *successful* batch subcommand
/// writes nothing to stderr, and 0221 must not have widened that. The
/// success path stays silent.
#[test]
fn export_with_every_override_applied_still_says_nothing() {
    let (descriptor, blob) = outer_inner_fixture();
    let overrides = hash_matching_overrides_yaml(
        &std::fs::read(blob.path()).unwrap(),
        &std::fs::read(descriptor.path()).unwrap(),
    );
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        overrides.path(),
    ]);
    assert!(out.status.success());
    assert!(
        out.stderr.is_empty(),
        "a successful export must stay silent: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Spec 0221 S6: a file whose origins no longer resolve against this
/// blob used to lose those entries without a word, which reads exactly
/// like a file that applied cleanly. It now warns — and still succeeds,
/// since a dropped entry is not a refused one.
#[test]
fn load_overrides_warns_about_dropped_entries() {
    let (descriptor, blob) = outer_inner_fixture();
    let overrides = unappliable_overrides_yaml(
        &std::fs::read(blob.path()).unwrap(),
        &std::fs::read(descriptor.path()).unwrap(),
        "  - path: \"/99\"\n    type: test.Inner\n    active: true\n",
    );
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--load-overrides",
        overrides.path(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a dropped entry is a warning, not a failure: {stderr}"
    );
    assert!(
        stderr.contains("1 override(s) dropped"),
        "expected the dropped count on stderr: {stderr}"
    );
}

// ── Item 6: round-trip regression, export + encode == original blob ─────

/// `export /`'s text output, piped through `prototext encode`'s
/// equivalent library function (`encode_text_to_binary` — used directly
/// rather than spawning the sibling `prototext` binary, since
/// `CARGO_BIN_EXE_<name>` is only guaranteed for the current package's own
/// binaries), must byte-for-byte reproduce the original blob.
#[test]
fn export_root_round_trips_losslessly_through_encode() {
    let (descriptor, blob) = outer_inner_fixture();
    let original = std::fs::read(&blob.0).unwrap();
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
    ]);
    assert!(
        out.status.success(),
        "export / must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reencoded = prototext_core::serialize::encode_text::encode_text_to_binary(&out.stdout);
    assert_eq!(
        reencoded, original,
        "export + encode round-trip must be byte-for-byte lossless"
    );
}

#[test]
fn export_message_set_round_trips_losslessly_through_encode() {
    let (descriptor, blob) = message_set_fixture();
    let original = std::fs::read(&blob.0).unwrap();
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "ms_test.Container",
        blob.path(),
        "export",
        "/",
    ]);
    assert!(
        out.status.success(),
        "export / must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reencoded = prototext_core::serialize::encode_text::encode_text_to_binary(&out.stdout);
    assert_eq!(
        reencoded, original,
        "export + encode round-trip must be byte-for-byte lossless"
    );
}

// ── Spec 0156 G9/G6/G7: the two descriptor formats ────────────────────────

fn hash_matching_overrides_yaml(blob: &[u8], descriptor: &[u8]) -> TempFile {
    let yaml = format!(
        "version: 1\ntarget:\n  blob_sha256: \"{}\"\n  descriptor_set_sha256: \"{}\"\noverrides: []\n",
        sha256_hex(blob),
        sha256_hex(descriptor),
    );
    write_temp("descriptor-export-overrides", yaml.as_bytes())
}

#[test]
fn export_descriptor_binary_without_load_overrides_is_a_hard_error() {
    let (descriptor, blob) = outer_inner_fixture();
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--format",
        "descriptor-binary",
    ]);
    assert_refused(
        &out,
        "export --format=descriptor-binary without --load-overrides must fail",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--load-overrides"),
        "expected a --load-overrides diagnostic, got: {stderr}"
    );
}

#[test]
fn export_descriptor_binary_with_load_overrides_succeeds_and_is_decodable() {
    let (descriptor, blob) = outer_inner_fixture();
    let descriptor_bytes = std::fs::read(&descriptor.0).unwrap();
    let blob_bytes = std::fs::read(&blob.0).unwrap();
    let overrides = hash_matching_overrides_yaml(&blob_bytes, &descriptor_bytes);

    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--format",
        "descriptor-binary",
        "--load-overrides",
        overrides.path(),
    ]);
    assert!(
        out.status.success(),
        "export --format=descriptor-binary --load-overrides must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let fds = FileDescriptorSet::decode(out.stdout.as_slice())
        .expect("stdout must be a decodable FileDescriptorSet");
    let synthetic = fds
        .file
        .iter()
        .find_map(|f| f.message_type.iter().find(|m| m.name() == "F1"))
        .expect("expected a synthetic message named F1 (root's field_number)");
    assert_eq!(
        synthetic.field.iter().map(|f| f.name()).collect::<Vec<_>>(),
        vec!["inner"],
        "the synthetic message must have one field per live child"
    );
}

#[test]
fn export_descriptor_prototext_with_load_overrides_and_descriptor_proto_present_succeeds() {
    let (descriptor, blob) = outer_inner_fixture();

    // Append a minimal `descriptor.proto`-suffixed file, carrying
    // `FileDescriptorSet`/`FileDescriptorProto` by simple name (G7's
    // heuristic is name-based only, not a real-schema match) alongside
    // the fixture's own `test.proto`, into one combined descriptor-set.
    let mut fds =
        FileDescriptorSet::decode(std::fs::read(&descriptor.0).unwrap().as_slice()).unwrap();
    let field_descriptor_proto = DescriptorProto {
        name: Some("FieldDescriptorProto".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("name".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("number".to_string()),
                number: Some(3),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("label".to_string()),
                number: Some(4),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("type".to_string()),
                number: Some(5),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("type_name".to_string()),
                number: Some(6),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let descriptor_proto = DescriptorProto {
        name: Some("DescriptorProto".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("name".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("field".to_string()),
                number: Some(2),
                label: Some(Label::Repeated as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".meta.FieldDescriptorProto".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let file_descriptor_proto = DescriptorProto {
        name: Some("FileDescriptorProto".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("name".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("message_type".to_string()),
                number: Some(4),
                label: Some(Label::Repeated as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".meta.DescriptorProto".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let file_descriptor_set = DescriptorProto {
        name: Some("FileDescriptorSet".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("file".to_string()),
            number: Some(1),
            label: Some(Label::Repeated as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".meta.FileDescriptorProto".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    fds.file.push(FileDescriptorProto {
        name: Some("descriptor.proto".to_string()),
        package: Some("meta".to_string()),
        message_type: vec![
            file_descriptor_set,
            file_descriptor_proto,
            descriptor_proto,
            field_descriptor_proto,
        ],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    });
    let descriptor = write_temp("outer-inner-plus-meta-desc", &fds.encode_to_vec());
    let descriptor_bytes = std::fs::read(&descriptor.0).unwrap();
    let blob_bytes = std::fs::read(&blob.0).unwrap();
    let overrides = hash_matching_overrides_yaml(&blob_bytes, &descriptor_bytes);

    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "export",
        "/",
        "--format",
        "descriptor-prototext",
        "--load-overrides",
        overrides.path(),
    ]);
    assert!(
        out.status.success(),
        "export --format=descriptor-prototext --load-overrides must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.is_empty(),
        "expected non-empty prototext output, got: {text}"
    );
}

#[test]
fn export_format_binary_and_prototext_still_succeed() {
    let (descriptor, blob) = outer_inner_fixture();
    for format in ["binary", "prototext"] {
        let out = run(&[
            "--descriptor-set",
            descriptor.path(),
            "--type",
            "test.Outer",
            blob.path(),
            "export",
            "/",
            "--format",
            format,
        ]);
        assert!(
            out.status.success(),
            "export --format={format} must succeed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ── Spec 0157: schemaless launch (no --descriptor-set) ──────────────────

#[test]
fn export_binary_with_no_descriptor_set_succeeds_and_produces_a_raw_rendering() {
    let (_descriptor, blob) = outer_inner_fixture();
    let original = std::fs::read(&blob.0).unwrap();
    let out = run(&[blob.path(), "export", "/", "--format", "binary"]);
    assert!(
        out.status.success(),
        "export --format=binary with no --descriptor-set must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        out.stdout, original,
        "root's raw binary export must reproduce the whole blob"
    );
}

#[test]
fn export_descriptor_binary_with_no_descriptor_set_degrades_every_field_to_a_tier4_guess() {
    let (_descriptor, blob) = outer_inner_fixture();
    let blob_bytes = std::fs::read(&blob.0).unwrap();
    let overrides = hash_matching_overrides_yaml(&blob_bytes, &[]);

    let out = run(&[
        blob.path(),
        "export",
        "/",
        "--format",
        "descriptor-binary",
        "--load-overrides",
        overrides.path(),
    ]);
    assert!(
        out.status.success(),
        "export --format=descriptor-binary with no --descriptor-set must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let fds = FileDescriptorSet::decode(out.stdout.as_slice())
        .expect("stdout must be a decodable FileDescriptorSet");
    let synthetic = fds
        .file
        .iter()
        .find_map(|f| f.message_type.iter().find(|m| m.name() == "F1"))
        .expect("expected a synthetic message named F1 (root's field_number)");
    assert_eq!(
        synthetic.field.iter().map(|f| f.name()).collect::<Vec<_>>(),
        vec!["f1"],
        "with no schema, the field name falls back to the digit-guarded field number"
    );
    assert_eq!(
        synthetic.field[0].r#type(),
        Type::Bytes,
        "with no schema, a LEN-wire-type field degrades to the tier-4 Bytes guess"
    );
}

#[test]
fn export_descriptor_prototext_with_no_descriptor_set_fails_with_meta_schema_not_found() {
    let (_descriptor, blob) = outer_inner_fixture();
    let blob_bytes = std::fs::read(&blob.0).unwrap();
    let overrides = hash_matching_overrides_yaml(&blob_bytes, &[]);

    let out = run(&[
        blob.path(),
        "export",
        "/",
        "--format",
        "descriptor-prototext",
        "--load-overrides",
        overrides.path(),
    ]);
    assert_refused(
        &out,
        "export --format=descriptor-prototext with no --descriptor-set must fail",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no descriptor.proto"),
        "expected a meta-schema-not-found diagnostic, got: {stderr}"
    );
}

#[test]
fn type_with_no_descriptor_set_fails_before_reading_the_blob() {
    let (_descriptor, blob) = outer_inner_fixture();
    let out = run(&["--type", "test.Outer", blob.path(), "export", "/"]);
    assert_refused(&out, "--type without --descriptor-set must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--type requires --descriptor-set"),
        "expected the --type-requires-descriptor-set diagnostic, got: {stderr}"
    );
}

// ── Spec 0198: the `quit` subcommand ────────────────────────────────────

/// Spec 0198 test plan items 1 and 2. `quit` runs every startup phase and
/// then returns, which is a thing only a subprocess test can observe: as
/// with `export_unresolvable_path_fails_without_entering_the_tui` above,
/// `Command::output()` would hang forever on a child that reached
/// `tui::run`, so this test completing at all is the proof that it did
/// not.
#[test]
fn quit_runs_the_startup_phases_and_returns() {
    let (descriptor, blob) = outer_inner_fixture();
    let out = run(&[
        "--descriptor-set",
        descriptor.path(),
        "--type",
        "test.Outer",
        blob.path(),
        "quit",
    ]);
    assert!(
        out.status.success(),
        "quit must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "quit produces no output: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Spec 0198 S2: the phase lines *are* `quit`'s contribution — a
    // single number with no breakdown would make a regression findable
    // only by bisection.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("rendering root node as") && stderr.contains("indexing"),
        "quit must announce its phases: {stderr}"
    );
}

/// Spec 0198 test plan item 4. `--type` is an O(1) pool lookup and
/// `--raw` skips the inference sweep outright (spec 0168 G6), so these
/// are the two root-option combinations whose phase lines differ — and
/// the two that would break if `quit` grew options of its own.
#[test]
fn quit_accepts_the_root_options() {
    let (_descriptor, blob) = outer_inner_fixture();
    let out = run(&["--raw", blob.path(), "quit"]);
    assert!(
        out.status.success(),
        "quit --raw must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("inferring root type"),
        "--raw skips the inference sweep: {stderr}"
    );
}
