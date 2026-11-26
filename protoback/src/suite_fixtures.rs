// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! What the tests ported from reproto's suite (spec 0488) share, unit tests
//! (`#[cfg(test)]` here) and the suite's integration tests alike
//! (`tests/suite/`, which include this file): the fixtures (S4), a
//! directory to work in, protoc, and the check for the tools a test needs.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A fresh, empty directory for one test, `name` the test's, so that two
/// tests never share one: under Cargo's temporary directory for the
/// integration tests, the system's for unit tests (which Cargo gives none).
pub fn tmp(name: &str) -> PathBuf {
    let dir = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(d) => Path::new(d).join("suite").join(name),
        None => std::env::temp_dir().join(format!("protoback-suite-{name}-{}", std::process::id())),
    };
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fixture of `protoback/tests/fixtures/`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A `#@prototext` fixture's bytes, encoded by prototext: where the suite
/// built an input with Python's protobuf (`descriptor_pb2`), its bytes,
/// committed as text (spec 0488).
///
/// `sha256` is the digest of the bytes the original wrote, checked: the
/// encoder is prototext's, which protoback also uses to read text, so a
/// change to it fails here, on the fixture, and never feeds the test
/// another input in silence.
pub fn encoded(name: &str, sha256: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let text = std::fs::read(fixture(name)).unwrap();
    let binary = prototext_core::serialize::encode_text::encode_text_to_binary(&text);
    let digest: String = Sha256::digest(&binary)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        digest, sha256,
        "{name}: encoded to other bytes than its original's"
    );
    binary
}

/// protoc with `args`; its failure fails the test.
pub fn protoc<I, S>(args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let o = Command::new("protoc").args(args).output().unwrap();
    assert!(
        o.status.success(),
        "protoc: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Whether `program` is on `PATH`. The suite skipped every test without
/// protoc (`conftest.py`'s `check_dependencies`); a ported test that runs
/// protoc returns early without it, saying so.
pub fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The interpreter protoback runs a Python plugin with (`--fdp-plugin`):
/// `PROTOBACK_PYTHON`, else `python3`, as protoback picks it.
pub fn plugin_python() -> String {
    std::env::var("PROTOBACK_PYTHON").unwrap_or_else(|_| "python3".to_string())
}

/// Whether a tool `tools` names is there: a program on PATH, or
/// `python-protobuf`, `plugin_python()` able to import protobuf, which a
/// plugin is given its descriptors with.
fn have_tool(name: &str) -> bool {
    if name != "python-protobuf" {
        return have(name);
    }
    Command::new(plugin_python())
        .args(["-c", "import google.protobuf"])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Set by the repository's CI (`rustTests`, nix/rust.nix): a missing tool
/// fails the test instead of skipping it, so that the CI, which provides
/// them all, never passes by skipping (spec 0488 S5).
pub const REQUIRE_TOOLS: &str = "PROTOBACK_SUITE_REQUIRE_TOOLS";

/// Whether the external tools a test needs (protoc, buf, prototext) are on
/// PATH. Without one, the test skips, saying so: a downstream `cargo test`
/// (nixpkgs' check phase) has protoc alone. Under `REQUIRE_TOOLS`, it fails.
pub fn tools(names: &[&str]) -> bool {
    let missing: Vec<&str> = names.iter().copied().filter(|n| !have_tool(n)).collect();
    if missing.is_empty() {
        return true;
    }
    assert!(
        std::env::var_os(REQUIRE_TOOLS).is_none(),
        "{REQUIRE_TOOLS} is set and {missing:?} not found"
    );
    eprintln!("skipped: {missing:?} not found");
    false
}
