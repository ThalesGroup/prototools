// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoback prints its own completion scripts (spec 0422, 0433 S7).
//!
//! Ported from `test_completion_scripts.py`, adapted: reproto completed
//! through Click, protoback through clap (`PROTOBACK_COMPLETE=<shell>`),
//! whose scripts and response formats are its own. Each test keeps its
//! property:
//!
//! - S2: the bash script is protoback's. reproto's was compared with a
//!   committed `completions.sh`; protoback writes its own, nothing
//!   committed to compare with, so the script is checked to be protoback's
//!   and to call it back.
//! - S3, S4 (sources): zsh's and fish's scripts are protoback's, as were.
//! - N2 (reproto-instantiate-schema kept Click's scripts, reproto's not
//!   leaking into it): reproto-only; `protoback instantiate` is a
//!   subcommand, completed by protoback's own script.
//! - S3, S4 (responses): zsh and fish get the same candidates as bash,
//!   `-I`'s descriptor files and directories, not the other files; in
//!   clap's formats, the candidates first.
//!
//! Each call runs the program in a fresh process, as a shell does.

use crate::common::tmp;
use std::path::Path;
use std::process::Command;

fn run(env: &[(&str, &str)], args: &[&str], cwd: Option<&Path>) -> String {
    let mut c = Command::new(env!("CARGO_BIN_EXE_protoback"));
    c.args(args).envs(env.iter().copied());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn source(shell: &str) -> String {
    run(&[("PROTOBACK_COMPLETE", shell)], &[], None)
}

#[test]
fn test_s2_bash_source_is_protobacks() {
    let script = source("bash");
    assert!(script.contains(r#"PROTOBACK_COMPLETE="bash""#), "{script}");
    assert!(
        script.contains("-F _prototools_complete_protoback protoback"),
        "{script}"
    );
}

#[test]
fn test_s3_s4_zsh_and_fish_sources_are_protobacks() {
    for (shell, calls_back) in [
        ("zsh", r#"PROTOBACK_COMPLETE="zsh""#),
        ("fish", "PROTOBACK_COMPLETE=fish"),
    ] {
        let script = source(shell);
        assert!(script.contains(calls_back), "{shell}: {script}");
        assert!(
            script.contains("--command protoback")
                || script.contains("compdef _clap_dynamic_completer_protoback protoback"),
            "{shell}: {script}"
        );
        assert!(!script.contains("%("), "{shell}: placeholder left");
    }
}

/// zsh and fish get the same candidates as bash.
#[test]
fn test_s3_s4_responses_carry_protobacks_types() {
    let dir = tmp("test_s3_s4_responses_carry_protobacks_types");
    std::fs::create_dir_all(dir.join("descs/sub")).unwrap();
    std::fs::write(dir.join("descs/a.pb"), b"").unwrap();
    std::fs::write(dir.join("descs/notes.txt"), b"").unwrap();
    for shell in ["zsh", "fish"] {
        let out = run(
            &[("PROTOBACK_COMPLETE", shell), ("_CLAP_COMPLETE_INDEX", "3")],
            &["--", "protoback", "-I", "descs", ""],
            Some(&dir),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[..2], ["a.pb", "sub/"], "{shell}:\n{out}");
        assert!(!out.contains("notes.txt"), "{shell}:\n{out}");
    }
}
