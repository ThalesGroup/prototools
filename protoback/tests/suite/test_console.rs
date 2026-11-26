// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A message is printed as one line, whatever its length (spec 0433 S1).
//!
//! Ported from `test_console.py`, which ran reproto's console function
//! (`reproto.lib.console.rprint`) on a 199-character message, stderr not a
//! terminal: Rich assumed 80 columns there and wrapped it. protoback has
//! no such function to call from a test, so the port checks the same
//! property on its command line: a warning of more than 80 columns, stderr
//! not a terminal, on one line.

use crate::common::{protoback, tmp, write};

#[test]
fn test_a_long_message_is_one_line() {
    let dir = tmp("test_a_long_message_is_one_line");
    // A dependency whose name alone is over 80 columns: W5 names it.
    let missing = format!("{}.proto", ["word"; 40].join("_"));
    write(
        &dir.join("in/a.textpb"),
        &format!(r#"name: "a.proto" package: "a" dependency: "{missing}""#),
    );
    let run = protoback([
        "-I".as_ref(),
        dir.join("in").as_os_str(),
        "-O".as_ref(),
        dir.join("out").as_os_str(),
    ]);
    let line = run
        .stderr
        .lines()
        .find(|l| l.contains("[W5]"))
        .unwrap_or_else(|| panic!("no W5 in:\n{}", run.stderr));
    assert!(line.len() > 80, "{line}");
    assert!(
        line.contains(&missing),
        "the message is not one line:\n{}",
        run.stderr
    );
}
