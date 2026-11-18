// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0446: `--report-out FILE`. Each test writes text FDPs, one per
//! `.textpb`, runs protoback with a report, and reads it back.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// `p.M` and `p.N`.
const P: &str =
    r#"name: "p.proto" package: "p" message_type { name: "M" } message_type { name: "N" }"#;

fn setup(test: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("report-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join("in").join(format!("{name}.textpb")), text).unwrap();
    }
    dir
}

struct Run {
    status: i32,
    stderr: String,
    report: Option<Value>,
}

fn run(dir: &Path, args: &[&str]) -> Run {
    let report = dir.join("report.json");
    let o = Command::new(env!("CARGO_BIN_EXE_protoback"))
        .args(["--use-variant", "descriptor", "-I"])
        .arg(dir.join("in"))
        .arg("-O")
        .arg(dir.join("out"))
        .arg("--report-out")
        .arg(&report)
        .args(args)
        .env_remove("REPROTO_VARIANT")
        .output()
        .unwrap();
    Run {
        status: o.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        report: std::fs::read_to_string(&report)
            .ok()
            .map(|t| serde_json::from_str(&t).unwrap()),
    }
}

fn warnings<'a>(report: &'a Value, code: &str) -> Vec<&'a Value> {
    report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == code)
        .collect()
}

/// `b.proto`, importing `deps`, with one field of each type in `types`.
fn importer(deps: &[&str], types: &[&str]) -> String {
    let deps: String = deps
        .iter()
        .map(|d| format!(r#"dependency: "{d}" "#))
        .collect();
    let fields: String = types
        .iter()
        .enumerate()
        .map(|(n, t)| {
            format!(
                r#"field {{ name: "f{n}" number: {} label: LABEL_OPTIONAL type: TYPE_MESSAGE type_name: "{t}" }}"#,
                n + 1
            )
        })
        .collect();
    format!(r#"name: "b.proto" package: "b" {deps} message_type {{ name: "B" {fields} }}"#)
}

#[test]
fn report_lists_every_occurrence_whatever_q_shows() {
    let b = importer(&[], &[".p.Gone"; 5]);
    let dir = setup("occurrences", &[("b", &b)]);
    for q in [&[][..], &["-q"][..]] {
        let r = run(&dir, q);
        assert_eq!(r.status, 0, "{}", r.stderr);
        let report = r.report.unwrap();
        let w4 = warnings(&report, "W4");
        assert_eq!(w4.len(), 5, "{q:?}: {report:#}");
        assert_eq!(w4[0]["fields"]["file"], "b.proto");
        assert_eq!(w4[0]["fields"]["type"], ".p.Gone");
        assert_eq!(w4[0]["message"], "unresolvable type .p.Gone");
    }
}

#[test]
fn report_fields_per_code() {
    // W3: p2 repeats p.M; W5: b imports absent.proto; W23: c imports p2,
    // pruned; W22: an option declared nowhere.
    let p2 = r#"name: "p2.proto" package: "p" message_type { name: "M" }"#;
    let b = importer(&["p.proto", "absent.proto"], &[".p.M"]);
    let c = r#"name: "c.proto" package: "c" dependency: "p2.proto"
               message_type { name: "C" options { 50000: 1 } }"#;
    let dir = setup("fields", &[("p", P), ("p2", p2), ("b", &b), ("c", c)]);
    let r = run(&dir, &[]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    let report = r.report.unwrap();
    let w3 = &warnings(&report, "W3")[0]["fields"];
    assert_eq!(w3["pruned"], "p2.proto");
    assert_eq!(w3["with"][0]["file"], "p.proto");
    assert_eq!(w3["with"][0]["symbols"], 1);
    // Once, from phase 2, with its importer; stderr still counts two.
    let w5 = warnings(&report, "W5");
    assert_eq!(w5.len(), 1, "{report:#}");
    let w5 = &w5[0]["fields"];
    assert_eq!(
        (&w5["importer"], &w5["import"]),
        (&"b.proto".into(), &"absent.proto".into())
    );
    let w23 = &warnings(&report, "W23")[0]["fields"];
    assert_eq!(
        (&w23["importer"], &w23["import"]),
        (&"c.proto".into(), &"p2.proto".into())
    );
    let w22 = &warnings(&report, "W22")[0]["fields"];
    assert_eq!(w22["file"], "c.proto");
    assert_eq!(w22["options"], "google.protobuf.MessageOptions");
    assert_eq!(w22["number"], 50000);
    // The W3 prune, with its cause.
    let prunes = report["rewrites"]["prunes"].as_array().unwrap();
    assert!(
        prunes
            .iter()
            .any(|p| p["file"] == "p2.proto" && p["cause"]["duplicate_of"][0]["file"] == "p.proto"),
        "{report:#}"
    );
}

#[test]
fn report_refusals_name_their_import_or_reason() {
    // Kept both, p2 is refused for p.M, and u for its import p2.
    let p2 = r#"name: "p2.proto" package: "p" message_type { name: "M" }"#;
    let u = importer(&["p2.proto"], &[".p.M"]).replace("b.proto", "u.proto");
    let dir = setup("refusals", &[("p", P), ("p2", p2), ("u", &u)]);
    let r = run(&dir, &["--keep-duplicates"]);
    let report = r.report.unwrap();
    let w19 = warnings(&report, "W19");
    let of = |f: &str| w19.iter().find(|w| w["fields"]["file"] == f).unwrap()["fields"].clone();
    assert_eq!(of("p2.proto")["reason"], "duplicate symbol 'p.M'");
    assert_eq!(of("u.proto")["import"], "p2.proto");
}

#[test]
fn report_written_on_fail_on() {
    let b = importer(&[], &[".p.Gone"]);
    let dir = setup("fail-on", &[("b", &b)]);
    let r = run(&dir, &["--fail-on", "W4"]);
    assert_eq!(r.status, 3);
    let report = r.report.unwrap();
    assert_eq!(report["complete"], true);
    assert_eq!(report["run"]["exit_status"], 3);
}

#[test]
fn report_never_stale() {
    let dir = setup("stale", &[("p", P)]);
    std::fs::write(dir.join("report.json"), "previous run").unwrap();
    // A run that fails once options are parsed: a variant that is no file.
    let r = run(&dir, &["--proto-variant", "/nonexistent/variant.yaml"]);
    assert_eq!(r.status, 2, "{}", r.stderr);
    let report = r.report.expect("this run's report, not the previous one");
    assert_eq!(report["complete"], false);
    assert_eq!(report["run"]["exit_status"], 2);
    assert!(
        report["run"]["error"].as_str().unwrap().contains("variant"),
        "{report:#}"
    );
    // Through a rename: nothing left beside it.
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(left.is_empty());
}

#[test]
fn report_schema_and_rewrites() {
    let old = r#"name: "old.proto" package: "q" message_type { name: "T" }"#;
    let new = r#"name: "new.proto" package: "q" message_type { name: "T" }"#;
    let b = importer(&["old.proto", "p.proto"], &[".q.T", ".p.N"]);
    let skipped = r#"name: "skipped.proto" package: "s" message_type { name: "S" }"#;
    let dir = setup(
        "schema",
        &[
            ("p", P),
            ("old", old),
            ("new", new),
            ("b", &b),
            ("skipped", skipped),
        ],
    );
    let r = run(
        &dir,
        &[
            "--move",
            "old.proto=new.proto",
            "-p",
            "path:skipped.textpb",
            "-p",
            "desc:p.M",
            "-p",
            "desc:nothing.Here",
        ],
    );
    assert_eq!(r.status, 0, "{}", r.stderr);
    let report = r.report.unwrap();
    assert_eq!(report["report_version"], 1);
    assert_eq!(report["complete"], true);
    assert_eq!(report["tool"]["name"], "protoback");
    assert_eq!(report["run"]["variant"], "google.protobuf");
    assert!(report["run"]["cwd"].as_str().is_some_and(|c| !c.is_empty()));
    let inputs = report["run"]["inputs"].as_array().unwrap();
    assert!(
        inputs
            .iter()
            .any(|i| i["path"] == "b.textpb" && i["files"][0] == "b.proto"),
        "{report:#}"
    );
    let prunes = report["rewrites"]["prunes"].as_array().unwrap();
    let has = |f: &dyn Fn(&Value) -> bool| prunes.iter().any(f);
    assert!(has(
        &|p| p["path"] == "skipped.textpb" && p["cause"]["pattern"] == "path:skipped.textpb"
    ));
    assert!(has(
        &|p| p["file"] == "old.proto" && p["cause"]["move"][0] == "new.proto"
    ));
    assert!(has(
        &|p| p["pattern"] == "desc:p/M" && p["definitions"][0] == "desc:.p.M"
    ));
    let moves = report["rewrites"]["moves"].as_array().unwrap();
    assert_eq!(moves[0]["importer"], "b.proto");
    assert_eq!(moves[0]["imports_before"][0], "old.proto");
    assert_eq!(moves[0]["imports_after"][0], "new.proto");
    let idle = report["idle_directives"].as_array().unwrap();
    assert!(idle
        .iter()
        .any(|i| i["code"] == "W11" && i["directive"] == "desc:nothing/Here"));
}

#[test]
fn report_lists_another_variants_section_codeless() {
    let dir = setup("variants", &[("p", P)]);
    let config = dir.join("filter.yaml");
    std::fs::write(
        &config,
        "variants:\n  other:\n    prune:\n      - file:p.proto\n",
    )
    .unwrap();
    let r = run(&dir, &["-C", config.to_str().unwrap()]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(!r.stderr.contains("W26"), "{}", r.stderr);
    let report = r.report.unwrap();
    let idle = &report["idle_directives"][0];
    assert_eq!(idle["code"], Value::Null);
    assert_eq!(idle["directive"], "variants: other");
}
