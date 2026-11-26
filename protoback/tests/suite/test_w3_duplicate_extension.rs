// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! W3: a file reusing an extension number is pruned (spec 0438 S1).
//!
//! Two files extending one message with the same number each compile alone,
//! but the descriptor pool refuses the second. upb 36.2 then kept the refused
//! file's extensions while freeing them: protoback aborted. The second file
//! is now pruned before the pool, as a duplicate symbol is.
//!
//! Ported from `test_w3_duplicate_extension.py`.

use crate::common::{protoback, protoc, tmp, tools, write, Run};

const BASE: &str = "syntax = \"proto2\";\npackage p;\nmessage Config { extensions 100 to max; }\n";
const A: &str = "syntax = \"proto2\";\npackage a;\nimport \"base.proto\";\n\
                 message A { extend p.Config { optional A ext = 1000; } }\n";
const B: &str = "syntax = \"proto2\";\npackage b;\nimport \"base.proto\";\n\
                 message B { extend p.Config { optional B ext = 1000; } }\n";
/// A symbol of a.proto's and an extension number of it both.
const C: &str = "syntax = \"proto2\";\npackage a;\nimport \"base.proto\";\n\
                 message A { extend p.Config { optional int32 other = 1000; } }\n";

fn run(name: &str, sources: &[(&str, &str)], flags: &[&str]) -> Option<(Run, std::path::PathBuf)> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(name);
    let (src, inp) = (dir.join("src"), dir.join("in"));
    std::fs::create_dir_all(&inp).unwrap();
    for (file, text) in [("base.proto", BASE)].iter().chain(sources) {
        write(&src.join(file), text);
        // Each compiled alone: together, protoc refuses them.
        protoc([
            format!("-I{}", src.display()),
            format!("--descriptor_set_out={}.pb", inp.join(file).display()),
            file.to_string(),
        ]);
    }
    let mut args: Vec<std::ffi::OsString> = ["--use-variant", "descriptor"]
        .iter()
        .map(Into::into)
        .collect();
    args.extend(flags.iter().map(Into::into));
    args.push(format!("-I{}", inp.display()).into());
    args.extend(["-O".into(), dir.join("out").into_os_string()]);
    args.push(format!("--schema-db-out={}", dir.join("db.desc").display()).into());
    Some((protoback(args), dir))
}

#[test]
fn test_a_reused_extension_number_prunes_the_second_file() {
    let Some((run, dir)) = run(
        "test_a_reused_extension_number_prunes_the_second_file",
        &[("a.proto", A), ("b.proto", B)],
        &[],
    ) else {
        return;
    };
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr.contains(
            "Warning [W3]: file:b.proto pruned — duplicate extension numbers with:\n    \
             file:a.proto (p.Config 1000)\n"
        ),
        "{}",
        run.stderr
    );
    // Pruned before the pool: not refused by it.
    assert!(
        !run.stderr.contains("[W19]") && !run.stderr.contains("[W20]"),
        "{}",
        run.stderr
    );
    assert!(!dir.join("out/b.proto").exists());
    assert!(dir.join("out/a.proto").exists());
}

#[test]
fn test_fail_on_w3_counts_it() {
    let Some((run, _)) = run(
        "test_fail_on_w3_counts_it",
        &[("a.proto", A), ("b.proto", B)],
        &["--fail-on", "W3"],
    ) else {
        return;
    };
    assert_eq!(run.code, Some(3), "{}", run.stderr);
}

#[test]
fn test_symbols_and_extension_numbers_in_one_warning() {
    let Some((run, _)) = run(
        "test_symbols_and_extension_numbers_in_one_warning",
        &[("a.proto", A), ("c.proto", C)],
        &[],
    ) else {
        return;
    };
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr.contains(
            "Warning [W3]: file:c.proto pruned — duplicate symbols with:\n    \
             file:a.proto (1 symbol)\n  and duplicate extension numbers with:\n    \
             file:a.proto (p.Config 1000)\n"
        ),
        "{}",
        run.stderr
    );
}
