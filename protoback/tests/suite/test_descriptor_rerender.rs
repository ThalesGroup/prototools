// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A node is reported once, whatever is written (spec 0429 S11).
//!
//! With -b (or --schema-db-out), the descriptor is built from renders of the
//! children after the text's: those report nothing. A warning raised two
//! messages deep used to print 2^(d+1) times, 8 here.
//!
//! Ported from `test_descriptor_rerender.py`. Its input, built there with
//! Python's protobuf (`_write_input`), is those bytes committed as the
//! `#@prototext` fixture `descriptor_rerender.pb`, encoded here; its three
//! parameters run in turn.

use crate::common::{encode_fixture, protoback, tmp};

/// The SHA-256 of the bytes `_write_input` wrote, which the fixture renders.
const INPUT_SHA256: &str = "780e5718d4d63163862fefec12821d810a8dd3e96d54d6dbda49179130ef2a81";

#[test]
fn test_a_nested_warning_is_printed_once() {
    let flag_sets: [&[&str]; 3] = [&[], &["-b"], &["--detailed-warnings", "-b"]];
    for (n, flags) in flag_sets.into_iter().enumerate() {
        let dir = tmp(&format!("test_a_nested_warning_is_printed_once-{n}"));
        encode_fixture("descriptor_rerender.pb", INPUT_SHA256, &dir.join("m.pb"));
        let mut args: Vec<std::ffi::OsString> = flags.iter().map(Into::into).collect();
        args.extend(["--use-variant".into(), "descriptor".into()]);
        args.push(format!("-I{}", dir.display()).into());
        args.extend(["-O".into(), dir.join("out").into_os_string(), "m.pb".into()]);
        let run = protoback(args);
        assert_eq!(run.code, Some(0), "{flags:?}: {}", run.stderr);
        let lines: Vec<&str> = run.stderr.lines().filter(|l| l.contains("[B1]")).collect();
        assert_eq!(
            lines,
            [
                "Warning [B1]: message 'C': nested extend block for '.p.Base' is not \
              valid in proto3; omitting"
            ],
            "{flags:?}: {}",
            run.stderr
        );
    }
}
