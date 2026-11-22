// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::check::staleness;
use super::decide::Analysis;
use super::problems::{Problem, ProblemKind};
use super::Source;

/// The codes read back: a file refused (W19, W6), a type no file defines
/// (W4), an import no file has (W5), a type a move cost an importer (W24,
/// spec 0455 S1).
const FILE_CODES: [&str; 5] = ["W19", "W6", "W4", "W5", "W24"];

/// One warning of the report, read.
struct Finding<'r> {
    code: &'r str,
    fields: &'r Value,
    /// The file it is about, as a path of the history where there is one.
    path: String,
    note: String,
}

impl Finding<'_> {
    /// A refusal for an import's (spec 0455 S3): W5 also names an
    /// `import`, the one missing.
    fn knock_on(&self) -> Option<&str> {
        matches!(self.code, "W19" | "W6")
            .then(|| self.fields["import"].as_str())
            .flatten()
    }
}

/// Fold `report` into `problems`. Refused, with the reason, when the report
/// wasn't built with a `fixups.yaml` computed from `sources`.
pub fn apply(
    report: &Value,
    a: &Analysis,
    sources: &[Source],
    problems: &mut Vec<Problem>,
) -> Result<(), String> {
    check_fixups(report, sources)?;

    // A descriptor name's path: the kept file, else any.
    let mut path_of: HashMap<&str, &str> = HashMap::new();
    for (p, f) in &a.files {
        if !a.pruned.contains(p) || !path_of.contains_key(f.info.name.as_str()) {
            path_of.insert(&f.info.name, p);
        }
    }
    let path = |name: &str| path_of.get(name).copied().unwrap_or(name).to_string();

    let mut findings: Vec<Finding> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new(); // (path, reason): W19 and W6 say it twice
    for w in report["warnings"].as_array().into_iter().flatten() {
        let code = w["code"].as_str().unwrap_or("");
        if !FILE_CODES.contains(&code) {
            continue;
        }
        let fields = &w["fields"];
        let Some(name) = fields["file"].as_str().or(fields["importer"].as_str()) else {
            continue;
        };
        let path = path(name);
        let what = fields["reason"]
            .as_str()
            .or(fields["type"].as_str())
            .or(fields["import"].as_str())
            .map(str::to_string)
            // W24: a definition, and the move that cost it.
            .unwrap_or_else(|| format!("{} {}", fields["definition"], fields["move"]));
        if seen.contains(&(path.clone(), what.clone())) {
            continue;
        }
        seen.push((path.clone(), what));
        let message = w["message"].as_str().unwrap_or("");
        findings.push(Finding {
            code,
            fields,
            path,
            note: format!("{code}: {message}"),
        });
    }

    // Each file refused for its import: the import's path (S3).
    let knocked: HashMap<String, String> = findings
        .iter()
        .filter_map(|f| Some((f.path.clone(), path(f.knock_on()?))))
        .collect();
    // Where each file's refusal attached.
    let mut refusal_at: HashMap<String, usize> = HashMap::new();

    // First, everything but knock-on refusals and what is said of a file
    // refused that way; the roots' place is then known.
    for f in &findings {
        if f.knock_on().is_some() || (f.code == "W4" && knocked.contains_key(&f.path)) {
            continue;
        }
        let at = match f.code {
            "W24" => w24_target(f, problems),
            "W5" => {
                let import = f.fields["import"].as_str().unwrap_or("");
                problems
                    .iter()
                    .position(|p| {
                        p.files.first() == Some(&f.path)
                            && p.kind == ProblemKind::MissingImport
                            && p.detail.contains(&format!("imports {import},"))
                    })
                    .or_else(|| first_problem(&f.path, problems))
            }
            "W19" | "W6" => problems
                .iter()
                .position(|p| p.kind == ProblemKind::Refused && p.files.first() == Some(&f.path))
                .or_else(|| first_problem(&f.path, problems)),
            _ => first_problem(&f.path, problems),
        };
        let at = attach(at, f, problems);
        if matches!(f.code, "W19" | "W6") {
            refusal_at.entry(f.path.clone()).or_insert(at);
        }
    }

    // Then the knock-on refusals, each under its root (S3).
    for f in findings.iter().filter(|f| f.knock_on().is_some()) {
        let mut at = None;
        let mut chain = vec![f.path.as_str()];
        let mut next = knocked.get(&f.path);
        while let Some(import) = next {
            if chain.contains(&import.as_str()) {
                break; // a loop: no root
            }
            if !knocked.contains_key(import) {
                at = refusal_at.get(import).copied();
                break;
            }
            chain.push(import);
            next = knocked.get(import);
        }
        let at = attach(at.or_else(|| first_problem(&f.path, problems)), f, problems);
        refusal_at.entry(f.path.clone()).or_insert(at);
    }
    // Then a W4 on a file refused for its import, where that refusal went
    // (S4).
    for f in &findings {
        if f.code == "W4" && knocked.contains_key(&f.path) {
            let at = refusal_at
                .get(&f.path)
                .copied()
                .or_else(|| first_problem(&f.path, problems));
            attach(at, f, problems);
        }
    }

    for idle in report["idle_directives"].as_array().into_iter().flatten() {
        let code = idle["code"].as_str().unwrap_or("");
        let directive = idle["directive"].as_str().unwrap_or("");
        let reason = idle["reason"].as_str().unwrap_or("");
        problems.push(Problem {
            kind: ProblemKind::StaleEntry,
            files: vec![directive.to_string()],
            detail: format!("{code}: {reason}"),
            subject: None,
            build: Vec::new(),
        });
    }
    problems.sort();
    Ok(())
}

/// The first problem of `path` a finding can attach to.
fn first_problem(path: &str, problems: &[Problem]) -> Option<usize> {
    problems.iter().position(|p| {
        p.files.first().map(String::as_str) == Some(path) && p.kind != ProblemKind::Manual
    })
}

/// The `stale-importer` problem of a W24's importer whose import is one of
/// its move's old files (S2).
fn w24_target(f: &Finding, problems: &[Problem]) -> Option<usize> {
    let olds: Vec<&str> = f.fields["move"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    problems.iter().position(|p| {
        p.kind == ProblemKind::StaleImporter
            && p.files.first() == Some(&f.path)
            && olds
                .iter()
                .any(|o| p.detail.starts_with(&format!("imports {o} (moved;")))
    })
}

/// `f` under problem `at`, or a `build` problem of its own: its index.
fn attach(at: Option<usize>, f: &Finding, problems: &mut Vec<Problem>) -> usize {
    match at {
        Some(i) => {
            problems[i].build.push(f.note.clone());
            i
        }
        None => {
            problems.push(Problem {
                kind: ProblemKind::Build,
                files: vec![f.path.clone()],
                detail: f.note.clone(),
                subject: None,
                build: Vec::new(),
            });
            problems.len() - 1
        }
    }
}

/// The report's `-C` file, read where the build ran, must carry the
/// current stamp.
fn check_fixups(report: &Value, sources: &[Source]) -> Result<(), String> {
    let argv: Vec<&str> = report["run"]["argv"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let cwd = report["run"]["cwd"].as_str().unwrap_or(".");
    let fixups = argv
        .windows(2)
        .find(|w| w[0] == "-C")
        .map(|w| w[1])
        .or_else(|| {
            argv.iter()
                .find_map(|a| a.strip_prefix("-C").filter(|r| !r.is_empty()))
        })
        .ok_or("the report's build had no -C file: it wasn't built with advise's fixups.yaml")?;
    let path = Path::new(cwd).join(fixups);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read the report's -C file {}: {e}", path.display()))?;
    match staleness(&text, sources) {
        None => Ok(()),
        Some(_) => Err(format!(
            "the report was built with {}, which wasn't computed from the current sources: \
             run advise --out first, then build again",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advise::decide::analyze;
    use crate::advise::fixups::stamp;
    use crate::advise::problems::problems;
    use crate::advise::{Input, SnapshotFile};
    use prototext_upb::text_format::parse_descriptor;
    use serde_json::json;

    fn input() -> Input {
        let file = |path: &str, text: &str| SnapshotFile {
            path: path.into(),
            fdp: parse_descriptor(text, false).unwrap(),
            written_at: 0,
            written_by: String::new(),
        };
        Input {
            files: vec![file("a.pb", r#"name: "a.proto" dependency: "gone.proto""#)],
            sources: vec![Source {
                identity: "s".into(),
                build_time: 1,
                order_hint: None,
                group: None,
                paths: ["a.pb".to_string()].into_iter().collect(),
                metadata: vec![],
            }],
        }
    }

    fn report(dir: &Path, fixups: &str) -> Value {
        std::fs::write(dir.join("fixups.yaml"), fixups).unwrap();
        json!({
            "run": { "argv": ["protoback", "-C", "fixups.yaml", "."], "cwd": dir },
            "warnings": [
                { "code": "W5", "message": "missing dependency file:gone.proto",
                  "fields": { "importer": "a.proto", "import": "gone.proto" } },
                { "code": "W4", "message": "unresolvable type .x.Y",
                  "fields": { "file": "b.proto", "type": ".x.Y" } },
                { "code": "W22", "message": "ignored", "fields": {} }
            ],
            "idle_directives": [
                { "code": "W25", "directive": "old.proto: new.proto", "reason": "nothing imports it" }
            ]
        })
    }

    #[test]
    fn attached_reported_and_stale() {
        let dir = std::env::temp_dir().join(format!("advise-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = input();
        let a = analyze(&input);
        let mut ps = problems(&a, &input);
        let r = report(&dir, &stamp(&input.sources));
        apply(&r, &a, &input.sources, &mut ps).unwrap();
        let summary: Vec<_> = ps
            .iter()
            .map(|p| (p.kind.name(), p.files[0].as_str(), p.build.len()))
            .collect();
        // W5 attached to advise's missing-import; W4 on a file advise has
        // no problem with, reported as is; W25 a stale entry; W22 ignored.
        assert_eq!(
            summary,
            [
                ("missing-import", "a.pb", 1),
                ("build", "b.proto", 0),
                ("stale-entry", "old.proto: new.proto", 0),
            ]
        );

        // A report built with fixups from other sources is refused.
        let r = report(&dir, "# computed from:\n#   other\n");
        assert!(apply(&r, &a, &input.sources, &mut ps)
            .unwrap_err()
            .contains("wasn't computed from the current sources"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `old.proto` (2025) defines `p.M` and `p.L`, `new.proto` (2026) `p.M`
    /// and `p.N`: moved forward. `user.proto` imports `old.proto` and uses
    /// `p.L`, which the move costs it; `child.proto` imports `user.proto`,
    /// `grand.proto` imports `child.proto`; `x.proto` and `y.proto` have no
    /// problem `advise` sees.
    fn skew() -> Input {
        let file = |path: &str, text: &str| SnapshotFile {
            path: path.into(),
            fdp: parse_descriptor(text, false).unwrap(),
            written_at: 0,
            written_by: String::new(),
        };
        let source = |id: &str, t: i64, paths: &[&str]| Source {
            identity: id.into(),
            build_time: t,
            order_hint: None,
            group: None,
            paths: paths.iter().map(|p| p.to_string()).collect(),
            metadata: vec![],
        };
        Input {
            files: vec![
                file(
                    "old.pb",
                    r#"name: "old.proto" package: "p" message_type { name: "M" } message_type { name: "L" }"#,
                ),
                file(
                    "new.pb",
                    r#"name: "new.proto" package: "p" message_type { name: "M" } message_type { name: "N" }"#,
                ),
                file(
                    "user.pb",
                    r#"name: "user.proto" package: "u" dependency: "old.proto"
                    message_type { name: "U" field { name: "l" number: 1 label: LABEL_OPTIONAL
                      type: TYPE_MESSAGE type_name: ".p.L" } }"#,
                ),
                file(
                    "child.pb",
                    r#"name: "child.proto" dependency: "user.proto""#,
                ),
                file(
                    "grand.pb",
                    r#"name: "grand.proto" dependency: "child.proto""#,
                ),
                file("x.pb", r#"name: "x.proto""#),
                file("y.pb", r#"name: "y.proto" dependency: "x.proto""#),
            ],
            sources: vec![
                source("n", 1_780_000_000, &["new.pb"]),
                source(
                    "o",
                    1_750_000_000,
                    &["old.pb", "user.pb", "child.pb", "grand.pb", "x.pb", "y.pb"],
                ),
            ],
        }
    }

    /// The report of a build with `input`'s fixups: `warnings`, and
    /// `idle` directives.
    fn build_report(dir: &Path, input: &Input, warnings: Value, idle: Value) -> Value {
        std::fs::write(dir.join("fixups.yaml"), stamp(&input.sources)).unwrap();
        json!({
            "run": { "argv": ["protoback", "-C", "fixups.yaml", "."], "cwd": dir },
            "warnings": warnings,
            "idle_directives": idle,
        })
    }

    fn scratch(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("advise-report-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn w19(file: &str, cause: Value) -> Value {
        let mut fields = json!({ "file": file });
        let message = if let Some(i) = cause["import"].as_str() {
            fields["import"] = i.into();
            format!("'{file}' : the descriptor pool refused its import '{i}'")
        } else {
            let r = cause["reason"].as_str().unwrap();
            fields["reason"] = r.into();
            format!("'{file}' : the descriptor pool refused it: {r}")
        };
        json!({ "code": "W19", "message": message, "fields": fields })
    }

    fn summary(ps: &[Problem]) -> Vec<(&str, &str, usize)> {
        ps.iter()
            .map(|p| (p.kind.name(), p.files[0].as_str(), p.build.len()))
            .collect()
    }

    #[test]
    fn w24_attaches_to_its_move() {
        let dir = scratch("w24");
        let input = skew();
        let a = analyze(&input);
        let mut ps = problems(&a, &input);
        assert!(
            ps.iter().any(|p| p.kind == ProblemKind::StaleImporter
                && p.detail == "imports old.proto (moved; re-pointed to new.proto)"),
            "{ps:?}"
        );
        let w24 = |importer: &str, old: &str| {
            json!({ "code": "W24",
                "message": format!("'{importer}': type '.p.L' is defined by no file it sees; lost to move '{old}'"),
                "fields": { "importer": importer, "definition": ".p.L", "move": [old] } })
        };
        let r = build_report(
            &dir,
            &input,
            json!([
                w24("user.proto", "old.proto"),
                w24("user.proto", "other.proto")
            ]),
            json!([]),
        );
        apply(&r, &a, &input.sources, &mut ps).unwrap();
        let stale = ps
            .iter()
            .find(|p| p.kind == ProblemKind::StaleImporter)
            .unwrap();
        assert_eq!(stale.build.len(), 1, "{stale:?}");
        assert!(stale.build[0].starts_with("W24: "), "{stale:?}");
        // The W24 of a move advise didn't decide: a loss it didn't see.
        let unforeseen: Vec<_> = ps.iter().filter(|p| p.kind == ProblemKind::Build).collect();
        assert_eq!(unforeseen.len(), 1, "{ps:?}");
        assert!(unforeseen[0].detail.contains("other.proto"), "{ps:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn knock_on_refusals_fold_under_the_root() {
        let dir = scratch("knock-on");
        let input = skew();
        let a = analyze(&input);
        let w4 = json!({ "code": "W4", "message": "unresolvable type .u.U",
            "fields": { "file": "grand.proto", "type": ".u.U" } });
        let root = w19("user.proto", json!({ "reason": "\".p.L\" is not defined" }));
        let child = w19("child.proto", json!({ "import": "user.proto" }));
        let grand = w19("grand.proto", json!({ "import": "child.proto" }));
        // The root refused by advise too; in either report order.
        for warnings in [
            json!([root, child, grand, w4]),
            json!([w4, grand, child, root]),
        ] {
            let mut ps = problems(&a, &input);
            let r = build_report(&dir, &input, warnings, json!([]));
            apply(&r, &a, &input.sources, &mut ps).unwrap();
            assert_eq!(
                summary(&ps),
                [("stale-importer", "user.pb", 0), ("refused", "user.pb", 4)],
                "{ps:?}"
            );
        }
        // A root advise didn't refuse: one build problem holding the chain.
        let mut ps = problems(&a, &input);
        let r = build_report(
            &dir,
            &input,
            json!([
                w19("y.proto", json!({ "import": "x.proto" })),
                w19("x.proto", json!({ "reason": "something" })),
            ]),
            json!([]),
        );
        apply(&r, &a, &input.sources, &mut ps).unwrap();
        let build: Vec<_> = ps.iter().filter(|p| p.kind == ProblemKind::Build).collect();
        assert_eq!(build.len(), 1, "{ps:?}");
        assert_eq!(build[0].files, ["x.pb"]);
        assert_eq!(build[0].build.len(), 1, "{ps:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
