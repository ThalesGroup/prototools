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
/// (W4), an import no file has (W5).
const FILE_CODES: [&str; 4] = ["W19", "W6", "W4", "W5"];

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
        let path = path_of.get(name).copied().unwrap_or(name).to_string();
        let message = w["message"].as_str().unwrap_or("").to_string();
        let what = fields["reason"]
            .as_str()
            .or(fields["type"].as_str())
            .or(fields["import"].as_str())
            .unwrap_or("")
            .to_string();
        if seen.contains(&(path.clone(), what.clone())) {
            continue;
        }
        seen.push((path.clone(), what));
        let note = format!("{code}: {message}");
        // A missing import goes to the problem naming that import; anything
        // else to the first problem of the file.
        let import = fields["import"].as_str().filter(|_| code == "W5");
        let target = problems
            .iter()
            .position(|p| {
                p.files.first() == Some(&path)
                    && import.is_some_and(|i| {
                        p.kind == ProblemKind::MissingImport
                            && p.detail.contains(&format!("imports {i},"))
                    })
            })
            .or_else(|| {
                problems
                    .iter()
                    .position(|p| p.files.first() == Some(&path) && p.kind != ProblemKind::Manual)
            });
        match target.map(|i| &mut problems[i]) {
            Some(p) => p.build.push(note),
            None => problems.push(Problem {
                kind: ProblemKind::Build,
                files: vec![path],
                detail: note,
                build: Vec::new(),
            }),
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
            build: Vec::new(),
        });
    }
    problems.sort();
    Ok(())
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
}
