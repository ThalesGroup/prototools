// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Editions rendering (spec 0027, 0072): editions_rendering.proto rendered
//! against its goldens, as editions (T13) and with --force-proto2-output
//! (T14: IMPLICIT written `optional`, DELIMITED an inline group, PACKED
//! `[packed = true]`).
//!
//! Ported from `test_editions_rendering.py`, its command-line tests; names
//! lower-cased; its `roundtrip` marker dropped. Lines compared without
//! their trailing whitespace, as it compared them.

use crate::common::{fixture, protoback, tmp, tools, write};

fn golden(test: &str, flags: &[&str], golden: &str) {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp(test);
    let (orig, out) = (dir.join("orig"), dir.join("out"));
    let proto = orig.join("editions_rendering.proto");
    write(
        &proto,
        &std::fs::read_to_string(fixture("editions_rendering.proto")).unwrap(),
    );
    let pb = orig.join("editions_rendering.pb");
    let o = std::process::Command::new("protoc")
        .arg(format!("--descriptor_set_out={}", pb.display()))
        .arg("--include_imports")
        .arg(format!("-I{}", orig.display()))
        .arg(&proto)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "protoc failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    std::fs::create_dir_all(&out).unwrap();
    let mut args: Vec<String> = vec!["--use-variant".into(), "descriptor".into()];
    args.extend(flags.iter().map(|f| f.to_string()));
    args.extend([
        format!("-I{}", orig.display()),
        format!("--proto-out={}", out.display()),
        pb.display().to_string(),
    ]);
    let r = protoback(&args);
    assert_eq!(r.code, Some(0), "protoback failed: {}", r.stderr);
    // Placed at the package path: found wherever it is.
    let mut stack = vec![out.clone()];
    let mut generated = None;
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .is_some_and(|n| n == "editions_rendering.proto")
            {
                generated = Some(p);
            }
        }
    }
    let actual = std::fs::read_to_string(generated.expect("no editions_rendering.proto")).unwrap();
    let expected = std::fs::read_to_string(fixture(golden)).unwrap();
    let norm = |t: &str| t.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    assert!(
        norm(&actual) == norm(&expected),
        "Output differs from golden.\n--- golden ---\n{expected}\n--- actual ---\n{actual}"
    );
}

#[test]
fn test_t13_editions_rendering_golden() {
    golden(
        "test_t13_editions_rendering_golden",
        &[],
        "editions_rendering.golden.proto",
    );
}

#[test]
fn test_t14_force_proto2_output_golden() {
    golden(
        "test_t14_force_proto2_output_golden",
        &["--force-proto2-output"],
        "editions_rendering.force_proto2.golden.proto",
    );
}
