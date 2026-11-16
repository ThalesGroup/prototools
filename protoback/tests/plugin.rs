// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `--fdp-plugin-cmd` (spec 0431): a plugin in any language, spoken to in
//! frames. The plugin here is bare Python (no protobuf), handling frames
//! by hand: it renames `a.proto`'s package by appending a `package` field,
//! the last value of a singular field being the one read, and logs each
//! descriptor it is sent. `MODE` makes it fail in each way S4 names.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PLUGIN: &str = r#"
import os, sys
inp, out = sys.stdin.buffer, sys.stdout.buffer
mode = os.environ.get('MODE', '')
log = open(os.environ['LOG'], 'a')

def read():
    n, shift = 0, 0
    while True:
        b = inp.read(1)
        if not b:
            return None
        n |= (b[0] & 0x7F) << shift
        if not b[0] & 0x80:
            break
        shift += 7
    return inp.read(n)

def write(body):
    n, v = len(body), bytearray()
    while n >= 0x80:
        v.append((n & 0x7F) | 0x80)
        n >>= 7
    v.append(n)
    out.write(bytes(v) + body)
    out.flush()

if mode == 'no-ready':
    sys.exit(0)
write(b'\x00')
A = b'\x0a\x07a.proto'
while (data := read()) is not None:
    log.write(data.hex() + '\n')
    log.flush()
    if not data.startswith(A):
        write(b'\x00' + data)
    elif mode == 'exit':
        sys.exit(0)
    elif mode == 'status':
        write(b'\x07' + data)
    elif mode == 'garbage':
        write(b'\x00\xff\xff\xff')
    elif mode == 'error':
        write(b'\x01' + b'no thanks')
    else:
        write(b'\x00' + data + b'\x12\x07renamed')
"#;

/// A fresh directory holding `a.pb`, a message in package `a`, and the
/// plugin.
fn setup(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("plugin-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    let a = prototext_upb::text_format::parse_descriptor(
        r#"name: "a.proto" package: "a" message_type { name: "M"
           field { name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 } }"#,
        false,
    )
    .unwrap();
    std::fs::write(dir.join("in/a.pb"), a).unwrap();
    std::fs::write(dir.join("plugin.py"), PLUGIN).unwrap();
    dir
}

fn run(dir: &Path, mode: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_protoback"))
        .current_dir(dir)
        .env("MODE", mode)
        .env("LOG", dir.join("log"))
        .args(["--use-variant", "descriptor", "-I", "in", "--proto-out=out"])
        .arg("--fdp-plugin-cmd")
        .arg("python3 plugin.py")
        .arg("a.pb")
        .output()
        .unwrap()
}

#[test]
fn a_command_plugin_changes_a_descriptor() {
    let dir = setup("changes");
    let out = run(&dir, "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let proto = std::fs::read_to_string(dir.join("out/a.proto")).unwrap();
    assert!(proto.contains("package renamed;"), "{proto}");
}

#[test]
fn each_distinct_descriptor_is_sent_once() {
    let dir = setup("once");
    let out = run(&dir, "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = std::fs::read_to_string(dir.join("log")).unwrap();
    let sent: Vec<&str> = log.lines().collect();
    let distinct: std::collections::BTreeSet<&str> = sent.iter().copied().collect();
    assert!(!sent.is_empty());
    // a.proto reaches reproto's plugin at load and in phase 2; protoback
    // sends it once (spec 0431 S5).
    assert_eq!(
        sent.len(),
        distinct.len(),
        "a descriptor sent twice:\n{log}"
    );
}

#[test]
fn a_plugin_that_exits_raises_or_misframes_fails_naming_the_file() {
    for (mode, expected) in [
        ("exit", "plugin failed on 'a.proto': the plugin exited"),
        (
            "status",
            "plugin failed on 'a.proto': malformed frame (status byte 7)",
        ),
        (
            "garbage",
            "plugin failed on 'a.proto': malformed frame (not a FileDescriptorProto)",
        ),
        ("error", "plugin failed on 'a.proto': no thanks"),
        ("no-ready", "Error: python3 plugin.py: the plugin exited"),
    ] {
        let dir = setup(&format!("fails-{mode}"));
        let out = run(&dir, mode);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{mode}: {stderr}");
        assert!(stderr.contains(expected), "{mode}: {stderr}");
        assert!(
            !dir.join("out/a.proto").exists(),
            "{mode}: an output was written"
        );
    }
}
