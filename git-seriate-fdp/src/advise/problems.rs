// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, BTreeSet, HashMap};

use prototext_upb::descriptor_access::FileDescriptorProtoMut;
use prototext_upb::{DynamicMessage, Pool};

use super::decide::{Analysis, Directive};
use super::{Input, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProblemKind {
    /// A kept file importing a file the fixups move or prune.
    StaleImporter,
    /// A kept file the descriptor pool refuses (version skew, mostly).
    Refused,
    /// A kept file importing a name no file of the snapshot has.
    MissingImport,
    /// Files no source lists.
    Manual,
    /// Conflicting files equally new: no decision.
    Tie,
    /// An extraction applied after a newer one sharing files with it.
    OutOfOrder,
    /// What a protoback build reported that advise didn't find (`--report`).
    Build,
    /// A fixups.yaml entry the build reports as idle (W25, `--report`).
    StaleEntry,
}

impl ProblemKind {
    pub fn name(self) -> &'static str {
        match self {
            ProblemKind::StaleImporter => "stale-importer",
            ProblemKind::Refused => "refused",
            ProblemKind::MissingImport => "missing-import",
            ProblemKind::Manual => "manual",
            ProblemKind::Tie => "tie",
            ProblemKind::OutOfOrder => "out-of-order",
            ProblemKind::Build => "build",
            ProblemKind::StaleEntry => "stale-entry",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Problem {
    pub kind: ProblemKind,
    /// The files involved, by path (for `out-of-order`, the sources).
    pub files: Vec<String>,
    pub detail: String,
    /// For `manual`, the subject of the commit that wrote the files.
    pub subject: Option<String>,
    /// What a protoback build said of the same files (`--report`).
    pub build: Vec<String>,
}

/// Every problem, sorted by kind, then files. `input.sources` must be in
/// history order, newest application first (as the reader returns them).
pub fn problems(a: &Analysis, input: &Input) -> Vec<Problem> {
    problems_and_dependents(a, input).0
}

/// [`problems`], and each file the pool refused only because an import was
/// refused, by path, with its root's path: the refused file the chain of
/// imports starts from (spec 0468 S1).
pub fn problems_and_dependents(
    a: &Analysis,
    input: &Input,
) -> (Vec<Problem>, HashMap<String, String>) {
    let mut out = Vec::new();

    // The kept files, by descriptor name (copies pruned, so names are unique).
    let kept: BTreeMap<&str, &str> = a
        .files
        .iter()
        .filter(|(p, _)| !a.pruned.contains(*p))
        .map(|(p, f)| (f.info.name.as_str(), p.as_str()))
        .collect();

    // What becomes of each name the fixups remove entirely.
    let mut moved: HashMap<&str, &str> = HashMap::new();
    let mut dropped: HashMap<&str, &'static str> = HashMap::new();
    for d in &a.decisions {
        match &d.directive {
            Directive::Move { old, new } => {
                moved.insert(old, new);
            }
            Directive::PruneFile(name) => {
                dropped.insert(name, "pruned: a reused extension number");
            }
            Directive::PrunePath(_) => {} // a copy: its name stays
        }
    }

    // stale-importer and missing-import, from each kept file's imports.
    for (&name, &path) in &kept {
        for dep in &a.files[path].info.dependencies {
            let dep = dep.as_str();
            if kept.contains_key(dep) {
                continue;
            }
            let detail = if let Some(new) = moved.get(dep) {
                format!("imports {dep} (moved; re-pointed to {new})")
            } else if let Some(why) = dropped.get(dep) {
                format!("imports {dep} ({why}; the import is dropped)")
            } else {
                out.push(Problem {
                    kind: ProblemKind::MissingImport,
                    files: vec![path.to_string()],
                    detail: format!("{name} imports {dep}, which no file of the snapshot has"),
                    subject: None,
                    build: Vec::new(),
                });
                continue;
            };
            out.push(Problem {
                kind: ProblemKind::StaleImporter,
                files: vec![path.to_string()],
                detail,
                subject: None,
                build: Vec::new(),
            });
        }
    }

    // refused: the kept files in a pool, with the moves applied.
    let (refused, skipped) = refusals(a, &kept, &moved);
    let path_of = |name: &str| kept.get(name).copied().unwrap_or(name).to_string();
    let mut dependents = HashMap::new();
    for name in skipped.keys() {
        // Followed to a file refused on its own; a loop has no root.
        let mut chain = vec![name.as_str()];
        let mut next = skipped.get(name);
        while let Some(import) = next {
            if chain.contains(&import.as_str()) {
                break;
            }
            if !skipped.contains_key(import) {
                if refused.iter().any(|(n, _)| n == import) {
                    dependents.insert(path_of(name), path_of(import));
                }
                break;
            }
            chain.push(import);
            next = skipped.get(import);
        }
    }
    for (name, reason) in refused {
        let path = kept.get(name.as_str()).copied().unwrap_or(name.as_str());
        out.push(Problem {
            kind: ProblemKind::Refused,
            files: vec![path.to_string()],
            detail: reason,
            subject: None,
            build: Vec::new(),
        });
    }

    // manual: files no source lists, grouped by the commit that wrote them.
    let mut manual: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (p, f) in &a.files {
        if !f.age.built {
            let subject = input
                .files
                .iter()
                .find(|x| &x.path == p)
                .map(|x| x.written_by.as_str())
                .unwrap_or("");
            manual.entry(subject).or_default().push(p.clone());
        }
    }
    for (subject, files) in manual {
        out.push(Problem {
            kind: ProblemKind::Manual,
            detail: format!("{} files, from the commit \"{subject}\"", files.len()),
            subject: Some(subject.to_string()),
            files,
            build: Vec::new(),
        });
    }

    for t in &a.ties {
        out.push(Problem {
            kind: ProblemKind::Tie,
            files: t.clone(),
            detail: format!("equally new: {}", a.files[&t[0]].age.describe()),
            subject: None,
            build: Vec::new(),
        });
    }

    out.extend(out_of_order(&input.sources));
    out.sort();
    (out, dependents)
}

/// The kept files the pool refuses, each with its reason; and apart, each
/// file refused only because an import was refused, with that import: its
/// cause is the problem. A file with a missing import is neither: that is
/// `missing-import`.
fn refusals(
    a: &Analysis,
    kept: &BTreeMap<&str, &str>,
    moved: &HashMap<&str, &str>,
) -> (Vec<(String, String)>, HashMap<String, String>) {
    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .expect("the vendored descriptor.proto declares it");
    let mut set = Vec::new();
    for (&name, &path) in kept {
        let Some(bytes) = input_bytes(a, path) else {
            continue;
        };
        let Ok(mut msg) = DynamicMessage::decode(&def, bytes) else {
            continue;
        };
        repoint(&mut msg, name, moved);
        let Ok(encoded) = msg.encode() else {
            continue;
        };
        // FileDescriptorSet.file: field 1, length-delimited.
        set.push(0x0a);
        let mut n = encoded.len();
        while n >= 0x80 {
            set.push((n as u8) | 0x80);
            n >>= 7;
        }
        set.push(n as u8);
        set.extend_from_slice(&encoded);
    }
    let report = Pool::new().load_set(&set);
    let mut refused = Vec::new();
    let mut skipped = HashMap::new();
    for (name, reason) in report.skipped {
        // `import 'DEP' was skipped`, as prototext-upb words it.
        if let Some(dep) = reason
            .strip_prefix("import '")
            .and_then(|r| r.strip_suffix("' was skipped"))
        {
            skipped.insert(name, dep.to_string());
        } else if !reason.ends_with("is missing") {
            refused.push((name, reason));
        }
    }
    (refused, skipped)
}

fn input_bytes<'a>(a: &'a Analysis, path: &str) -> Option<&'a [u8]> {
    a.files.get(path).map(|f| f.fdp.as_slice())
}

/// Point each import of a moved file at its new name, as protoback does
/// (spec 0445): no duplicate import, never the file itself, and the public
/// and weak import indices following.
fn repoint(msg: &mut DynamicMessage, name: &str, moved: &HashMap<&str, &str>) {
    let mut f = FileDescriptorProtoMut(msg.as_mut());
    let view = f.as_ref();
    let old: Vec<String> = view
        .dependency()
        .into_iter()
        .map(|d| String::from_utf8_lossy(d).into_owned())
        .collect();
    if !old.iter().any(|d| moved.contains_key(d.as_str())) {
        return;
    }
    let public = view.public_dependency();
    let weak = view.weak_dependency();

    let mut new: Vec<String> = Vec::new();
    let mut index: Vec<Option<usize>> = Vec::new(); // old position → new position
    for d in &old {
        let target = moved
            .get(d.as_str())
            .map(|s| s.to_string())
            .unwrap_or(d.clone());
        if target == name {
            index.push(None);
        } else if let Some(i) = new.iter().position(|x| *x == target) {
            index.push(Some(i));
        } else {
            new.push(target);
            index.push(Some(new.len() - 1));
        }
    }
    let remap = |list: Vec<i32>| -> BTreeSet<i32> {
        list.into_iter()
            .filter_map(|i| index.get(i as usize).copied().flatten().map(|j| j as i32))
            .collect()
    };
    let (public, weak) = (remap(public), remap(weak));

    f.clear_dependency();
    for d in &new {
        let _ = f.push_dependency(d.as_bytes());
    }
    f.clear_public_dependency();
    for i in public {
        let _ = f.push_public_dependency(i);
    }
    f.clear_weak_dependency();
    for i in weak {
        let _ = f.push_weak_dependency(i);
    }
}

/// Extractions applied after an extraction of a newer source that shares
/// files with them: their older versions replaced newer ones (spec 0452).
fn out_of_order(sources: &[Source]) -> Vec<Problem> {
    let applied: Vec<&Source> = sources.iter().rev().collect(); // oldest application first
    let mut out = Vec::new();
    for (i, s) in applied.iter().enumerate() {
        for earlier in &applied[..i] {
            if earlier.build_time > s.build_time {
                let shared = earlier.paths.intersection(&s.paths).count();
                if shared > 0 {
                    out.push(Problem {
                        kind: ProblemKind::OutOfOrder,
                        files: vec![s.identity.clone(), earlier.identity.clone()],
                        detail: format!(
                            "{} applied after {}, built later: {shared} shared files",
                            s.identity, earlier.identity
                        ),
                        subject: None,
                        build: Vec::new(),
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advise::decide::analyze;
    use crate::advise::SnapshotFile;
    use prototext_upb::text_format::parse_descriptor;

    const OLD: i64 = 1_600_000_000;
    const NEW: i64 = 1_700_000_000;

    fn file(path: &str, text: &str) -> SnapshotFile {
        SnapshotFile {
            path: path.into(),
            fdp: parse_descriptor(text, false).unwrap(),
            written_at: NEW + 1,
            written_by: "Add by hand".into(),
        }
    }

    fn source(id: &str, time: i64, paths: &[&str]) -> Source {
        Source {
            identity: id.into(),
            build_time: time,
            order_hint: None,
            group: None,
            paths: paths.iter().map(|p| p.to_string()).collect(),
            metadata: vec![],
        }
    }

    fn kinds(ps: &[Problem]) -> Vec<(&'static str, Vec<String>)> {
        ps.iter()
            .map(|p| (p.kind.name(), p.files.clone()))
            .collect()
    }

    #[test]
    fn version_skew_is_refused_with_the_pools_reason() {
        // user.pb, from an older source, uses dep.E, which the newer dep.pb
        // no longer has.
        let input = Input {
            files: vec![
                file(
                    "dep.pb",
                    r#"name: "dep.proto" package: "d" message_type { name: "Other" }"#,
                ),
                file(
                    "user.pb",
                    r#"name: "user.proto" package: "u" dependency: "dep.proto"
                    message_type { name: "U" field { name: "e" number: 1 label: LABEL_OPTIONAL
                      type: TYPE_MESSAGE type_name: ".d.E" } }"#,
                ),
                file(
                    "child.pb",
                    r#"name: "child.proto" dependency: "user.proto""#,
                ),
            ],
            sources: vec![
                source("new", NEW, &["dep.pb"]),
                source("old", OLD, &["user.pb", "child.pb"]),
            ],
        };
        let a = analyze(&input);
        let ps = problems(&a, &input);
        // child.pb is refused only because user.pb is: not listed.
        assert_eq!(kinds(&ps), [("refused", vec!["user.pb".to_string()])]);
        assert!(ps[0].detail.contains(".d.E"), "{}", ps[0].detail);
    }

    #[test]
    fn moved_import_is_repointed_and_reported() {
        let input = Input {
            files: vec![
                file(
                    "old.pb",
                    r#"name: "old.proto" package: "p" message_type { name: "M" }"#,
                ),
                file(
                    "new.pb",
                    r#"name: "new.proto" package: "p" message_type { name: "M" }"#,
                ),
                file(
                    "user.pb",
                    r#"name: "user.proto" dependency: "old.proto"
                    message_type { name: "U" field { name: "m" number: 1 label: LABEL_OPTIONAL
                      type: TYPE_MESSAGE type_name: ".p.M" } }"#,
                ),
            ],
            sources: vec![
                source("n", NEW, &["new.pb"]),
                source("o", OLD, &["old.pb", "user.pb"]),
            ],
        };
        let a = analyze(&input);
        let ps = problems(&a, &input);
        // Re-pointed, the importer loads: no refusal.
        assert_eq!(
            kinds(&ps),
            [("stale-importer", vec!["user.pb".to_string()])]
        );
        assert_eq!(
            ps[0].detail,
            "imports old.proto (moved; re-pointed to new.proto)"
        );
    }

    #[test]
    fn missing_import_manual_and_out_of_order() {
        let input = Input {
            files: vec![
                file("a.pb", r#"name: "a.proto" dependency: "gone.proto""#),
                file("m.pb", r#"name: "m.proto""#),
            ],
            // History, newest application first: the newer source was
            // applied first, then the older one, sharing a.pb.
            sources: vec![
                source("older", OLD, &["a.pb"]),
                source("newer", NEW, &["a.pb"]),
            ],
        };
        let a = analyze(&input);
        let ps = problems(&a, &input);
        assert_eq!(
            kinds(&ps),
            [
                ("missing-import", vec!["a.pb".to_string()]),
                ("manual", vec!["m.pb".to_string()]),
                (
                    "out-of-order",
                    vec!["older".to_string(), "newer".to_string()]
                ),
            ]
        );
        assert_eq!(ps[1].detail, "1 files, from the commit \"Add by hand\"");
    }
}
