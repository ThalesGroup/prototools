// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Every program answers --version (spec 0413 S5).
//!
//! Ported from `test_entry_points.py`, adapted: reproto's programs were its
//! `[project.scripts]`, `reproto` and `reproto-instantiate-schema`;
//! protoback's are itself and `protoback instantiate`, the second's port, a
//! subcommand. Each prints its name and its version, the crate's; reproto
//! also named the Python protobuf it ran on (spec 0467 S2), which protoback
//! has none of: reproto-only.

use crate::common::protoback_command;

#[test]
fn test_every_program_answers_version() {
    for name in [&["protoback"][..], &["protoback", "instantiate"][..]] {
        let mut args: Vec<&str> = name[1..].to_vec();
        args.push("--version");
        let o = protoback_command(&args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let out = String::from_utf8_lossy(&o.stdout);
        assert_eq!(
            out.trim_end(),
            format!("{} {}", name.join(" "), env!("CARGO_PKG_VERSION")),
            "{out}"
        );
    }
}
