// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! On a terminal (spec 0433 S5): warnings bold yellow, attention bold
//! blue, the phases' labels kept, their bars drawn and cleared. protoback
//! runs under a pseudo-terminal Python's `pty` opens.

use std::path::{Path, PathBuf};
use std::process::Command;

const UNDER_A_PTY: &str = r#"
import os, pty, sys
pid, fd = pty.fork()
if pid == 0:
    os.execv(sys.argv[1], sys.argv[1:])
out = b''
while True:
    try:
        chunk = os.read(fd, 65536)
    except OSError:
        break
    if not chunk:
        break
    out += chunk
_, status = os.waitpid(pid, 0)
sys.stdout.buffer.write(out)
sys.exit(os.waitstatus_to_exitcode(status))
"#;

fn setup() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("terminal");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    let a = prototext_upb::text_format::parse_descriptor(
        r#"name: "a.proto" package: "a" message_type { name: "M"
           field { name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 } }"#,
        false,
    )
    .unwrap();
    std::fs::write(dir.join("in/a.pb"), a).unwrap();
    dir
}

#[test]
fn a_terminal_gets_colors_bars_and_labels() {
    let dir = setup();
    let out = Command::new("python3")
        .current_dir(&dir)
        .arg("-c")
        .arg(UNDER_A_PTY)
        .arg(env!("CARGO_BIN_EXE_protoback"))
        .args(["--use-variant", "descriptor", "-I", "in", "--proto-out=out"])
        .args(["--seed", "desc:.no.Such", "a.pb"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    // reproto's styles: bold yellow, bold blue.
    assert!(
        text.contains("\x1b[1;33mWarning [W12]: Seed not found: "),
        "{text:?}"
    );
    assert!(
        text.contains("\x1b[1;34mUsing embedded fallback: google/protobuf/descriptor.proto\x1b[0m"),
        "{text:?}"
    );
    // The labels, kept once the bars are gone.
    for label in [
        "Loading descriptors",
        "Computing reachability",
        "Rendering proto files",
    ] {
        assert!(text.contains(label), "{label}: {text:?}");
    }
    // A bar was drawn, and cleared (indicatif erases its line).
    assert!(text.contains("\x1b[2K"), "no bar cleared: {text:?}");
}
