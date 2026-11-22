// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{TimeZone, Utc};

use super::fdp_info::{self, FdpInfo};
use super::Input;

/// A file's age, and how it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Age {
    pub time: i64,
    /// Built at `time` (a source lists it), or ingested no later than it.
    pub built: bool,
}

/// A time as its UTC date, `YYYY-MM-DD`.
pub fn date(time: i64) -> String {
    Utc.timestamp_opt(time, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "?".into())
}

impl Age {
    pub fn describe(self) -> String {
        let date = date(self.time);
        if self.built {
            format!("built {date}")
        } else {
            format!("ingested no later than {date}")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// One file embedded twice, once without `json_name`.
    Copies,
    /// Two versions of one file.
    Versions,
    /// One extension number on one message, given to two extensions.
    Numbers,
    /// The same symbols under a new path: the older file moves forward.
    Moves,
    /// A newer file holding part of an older one, identically: it moves
    /// back onto the older one.
    Backward,
}

/// One entry of `fixups.yaml` (oss-prototools spec 0445).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Directive {
    /// `path:P`: a `~N` copy, which shares its name with the file kept.
    PrunePath(String),
    /// `file:N`: pruned however it is reached, through an importer too.
    PruneFile(String),
    /// `OLD: NEW`: OLD pruned, its imports re-pointed to NEW.
    Move { old: String, new: String },
}

impl Directive {
    pub fn render(&self) -> String {
        match self {
            Directive::PrunePath(p) => format!("path:{p}"),
            Directive::PruneFile(n) => format!("file:{n}"),
            Directive::Move { old, new } => format!("{old}: {new}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub directive: Directive,
    pub kind: Kind,
    pub comment: String,
}

#[derive(Debug, Clone)]
pub struct FileState {
    pub info: FdpInfo,
    pub age: Age,
    /// The `FileDescriptorProto`, serialized, as the input has it.
    pub fdp: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Analysis {
    /// Every readable file, by path.
    pub files: BTreeMap<String, FileState>,
    /// Paths whose descriptor doesn't decode.
    pub unreadable: Vec<String>,
    pub decisions: Vec<Decision>,
    /// Conflicting files equally new: no decision (spec 0454 S3).
    pub ties: Vec<Vec<String>>,
    /// The paths the decisions remove.
    pub pruned: BTreeSet<String>,
}

impl Analysis {
    fn age(&self, p: &str) -> Age {
        self.files[p].age
    }

    fn info(&self, p: &str) -> &FdpInfo {
        &self.files[p].info
    }
}

/// Read the files, date them, and decide.
pub fn analyze(input: &Input) -> Analysis {
    let mut built: HashMap<&str, i64> = HashMap::new();
    for s in &input.sources {
        for p in &s.paths {
            let t = built.entry(p.as_str()).or_insert(s.build_time);
            *t = (*t).max(s.build_time);
        }
    }

    let mut a = Analysis::default();
    for f in &input.files {
        let Some(info) = fdp_info::read(&f.fdp) else {
            a.unreadable.push(f.path.clone());
            continue;
        };
        let age = match built.get(f.path.as_str()) {
            Some(&t) => Age {
                time: t,
                built: true,
            },
            None => Age {
                time: f.written_at,
                built: false,
            },
        };
        a.files.insert(
            f.path.clone(),
            FileState {
                info,
                age,
                fdp: f.fdp.clone(),
            },
        );
    }
    decide(&mut a);
    a
}

fn decide(a: &mut Analysis) {
    let mut decisions = Vec::new();
    let mut pruned: BTreeSet<String> = BTreeSet::new();

    // 1. One descriptor name, several files: `~N` copies or versions. Keep
    //    the newest, then the shortest path (the plain name).
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (p, f) in &a.files {
        by_name.entry(f.info.name.as_str()).or_default().push(p);
    }
    for (name, mut ps) in by_name {
        if ps.len() < 2 {
            continue;
        }
        ps.sort_by(|x, y| {
            (a.age(y).time, std::cmp::Reverse(y.len()))
                .cmp(&(a.age(x).time, std::cmp::Reverse(x.len())))
        });
        let keep = ps[0];
        for q in &ps[1..] {
            pruned.insert(q.to_string());
            if a.info(q).canonical == a.info(keep).canonical {
                decisions.push(Decision {
                    directive: Directive::PrunePath(q.to_string()),
                    kind: Kind::Copies,
                    comment: format!(
                        "same file as {keep}, without json_name (one binary embeds it twice)"
                    ),
                });
            } else {
                decisions.push(Decision {
                    directive: Directive::PrunePath(q.to_string()),
                    kind: Kind::Versions,
                    comment: format!(
                        "older version of {name} ({}); kept: {keep} ({})",
                        a.age(q).describe(),
                        a.age(keep).describe()
                    ),
                });
            }
        }
    }

    // 2. Files sharing symbols, or one extension number on one message.
    let mut sym_owner: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for (p, f) in &a.files {
        for s in &f.info.symbols {
            sym_owner.entry(s.as_str()).or_default().insert(p);
        }
    }
    let mut groups: BTreeMap<Vec<&str>, bool> = BTreeMap::new(); // paths → shares a symbol
    {
        let mut by_sym: HashMap<&str, BTreeSet<&str>> = HashMap::new();
        let mut by_ext: HashMap<(&str, i32), BTreeMap<&str, BTreeSet<&str>>> = HashMap::new();
        for (p, f) in &a.files {
            if pruned.contains(p.as_str()) {
                continue;
            }
            for s in &f.info.symbols {
                by_sym.entry(s.as_str()).or_default().insert(p);
            }
            for x in &f.info.extensions {
                by_ext
                    .entry((x.extendee.as_str(), x.number))
                    .or_default()
                    .entry(x.full_name.as_str())
                    .or_default()
                    .insert(p);
            }
        }
        for ps in by_sym.into_values() {
            if ps.len() > 1 {
                groups.insert(ps.into_iter().collect(), true);
            }
        }
        for by_full in by_ext.into_values() {
            if by_full.len() < 2 {
                continue; // one extension, declared once or several times
            }
            let ps: BTreeSet<&str> = by_full.into_values().flatten().collect();
            if ps.len() > 1 {
                groups.entry(ps.into_iter().collect()).or_insert(false);
            }
        }
    }

    let mut ties = Vec::new();
    for (group, shares_symbol) in groups {
        let mut ps: Vec<&str> = group.into_iter().filter(|p| !pruned.contains(*p)).collect();
        if ps.len() < 2 {
            continue;
        }
        ps.sort_by_key(|p| std::cmp::Reverse(a.age(p).time)); // stable: path order on ties
        if a.age(ps[0]).time == a.age(ps[1]).time {
            ties.push(ps.iter().map(|p| p.to_string()).collect());
            continue;
        }
        let newest = ps[0];
        for &old in &ps[1..] {
            if pruned.contains(old) || pruned.contains(newest) {
                continue;
            }
            let o: BTreeSet<&str> = a.info(old).symbols.iter().map(String::as_str).collect();
            let n: BTreeSet<&str> = a.info(newest).symbols.iter().map(String::as_str).collect();
            let only_old: Vec<&str> = o
                .difference(&n)
                .copied()
                .filter(|x| sym_owner[x].iter().all(|p| *p == old || *p == newest))
                .collect();
            let name_old = a.info(old).name.clone();
            let name_new = a.info(newest).name.clone();
            if !shares_symbol {
                pruned.insert(old.to_string());
                decisions.push(Decision {
                    directive: Directive::PruneFile(name_old),
                    kind: Kind::Numbers,
                    comment: format!(
                        "extension number reused: {name_new} ({}) now declares it; this one is {}",
                        a.age(newest).describe(),
                        a.age(old).describe()
                    ),
                });
            } else if !only_old.is_empty() && n.is_subset(&o) && identical(a, old, newest) {
                pruned.insert(newest.to_string());
                decisions.push(Decision {
                    directive: Directive::Move {
                        old: name_new.clone(),
                        new: name_old.clone(),
                    },
                    kind: Kind::Backward,
                    comment: format!(
                        "part of {name_old} split off ({}): all {} of its symbols are in {name_old} \
                         ({}), identically, and {} more are defined nowhere else",
                        a.age(newest).describe(),
                        n.len(),
                        a.age(old).describe(),
                        only_old.len()
                    ),
                });
            } else {
                pruned.insert(old.to_string());
                let lost = if only_old.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; {} defined nowhere else are lost to its importers",
                        only_old.len()
                    )
                };
                decisions.push(Decision {
                    directive: Directive::Move {
                        old: name_old,
                        new: name_new.clone(),
                    },
                    kind: Kind::Moves,
                    comment: format!(
                        "moved or renamed to {name_new} ({}): {} of its {} symbols are there; \
                         this one is {}{lost}",
                        a.age(newest).describe(),
                        o.intersection(&n).count(),
                        o.len(),
                        a.age(old).describe()
                    ),
                });
            }
        }
    }

    a.decisions = decisions;
    a.ties = ties;
    a.pruned = pruned;
}

/// Every top-level definition of `new` is in `old`, identically.
fn identical(a: &Analysis, old: &str, new: &str) -> bool {
    let o = &a.info(old).definitions;
    a.info(new)
        .definitions
        .iter()
        .all(|(k, v)| o.get(k) == Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advise::{SnapshotFile, Source};
    use prototext_upb::text_format::parse_descriptor;

    const OLD: i64 = 1_600_000_000;
    const NEW: i64 = 1_700_000_000;

    fn file(path: &str, text: &str) -> SnapshotFile {
        SnapshotFile {
            path: path.into(),
            fdp: parse_descriptor(text, false).unwrap(),
            written_at: NEW + 1,
            written_by: "manual".into(),
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

    fn rendered(a: &Analysis) -> Vec<(Kind, String)> {
        let mut v: Vec<_> = a
            .decisions
            .iter()
            .map(|d| (d.kind, d.directive.render()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn copies_and_versions() {
        let a = analyze(&Input {
            files: vec![
                file(
                    "a.pb",
                    r#"name: "a.proto" message_type { name: "M" field { name: "f_g" number: 1 json_name: "fG" } }"#,
                ),
                file(
                    "a~2.pb",
                    r#"name: "a.proto" message_type { name: "M" field { name: "f_g" number: 1 } }"#,
                ),
                file(
                    "b.pb",
                    r#"name: "b.proto" message_type { name: "B" field { name: "x" number: 2 } }"#,
                ),
                file(
                    "b~2.pb",
                    r#"name: "b.proto" message_type { name: "B" field { name: "x" number: 1 } }"#,
                ),
            ],
            sources: vec![
                source("old", OLD, &["b~2.pb"]),
                source("new", NEW, &["a.pb", "a~2.pb", "b.pb"]),
            ],
        });
        assert_eq!(
            rendered(&a),
            [
                (Kind::Copies, "path:a~2.pb".into()),
                (Kind::Versions, "path:b~2.pb".into())
            ]
        );
    }

    #[test]
    fn rename_moves_forward() {
        let a = analyze(&Input {
            files: vec![
                file(
                    "old.pb",
                    r#"name: "old.proto" package: "p" message_type { name: "M" }"#,
                ),
                file(
                    "new.pb",
                    r#"name: "new.proto" package: "p" message_type { name: "M" }"#,
                ),
            ],
            sources: vec![source("o", OLD, &["old.pb"]), source("n", NEW, &["new.pb"])],
        });
        assert_eq!(rendered(&a), [(Kind::Moves, "old.proto: new.proto".into())]);
    }

    #[test]
    fn identical_split_moves_backward() {
        let a = analyze(&Input {
            files: vec![
                file(
                    "whole.pb",
                    r#"name: "whole.proto" package: "p" message_type { name: "A" } message_type { name: "B" }"#,
                ),
                file(
                    "part.pb",
                    r#"name: "part.proto" package: "p" message_type { name: "B" }"#,
                ),
            ],
            sources: vec![
                source("o", OLD, &["whole.pb"]),
                source("n", NEW, &["part.pb"]),
            ],
        });
        assert_eq!(
            rendered(&a),
            [(Kind::Backward, "part.proto: whole.proto".into())]
        );
    }

    #[test]
    fn differing_split_moves_forward() {
        let a = analyze(&Input {
            files: vec![
                file(
                    "whole.pb",
                    r#"name: "whole.proto" package: "p" message_type { name: "A" } message_type { name: "B" field { name: "x" number: 1 } }"#,
                ),
                file(
                    "part.pb",
                    r#"name: "part.proto" package: "p" message_type { name: "B" field { name: "x" number: 2 } }"#,
                ),
            ],
            sources: vec![
                source("o", OLD, &["whole.pb"]),
                source("n", NEW, &["part.pb"]),
            ],
        });
        assert_eq!(
            rendered(&a),
            [(Kind::Moves, "whole.proto: part.proto".into())]
        );
    }

    #[test]
    fn reused_extension_number() {
        let a = analyze(&Input {
            files: vec![
                file(
                    "x1.pb",
                    r#"name: "x1.proto" package: "p" extension { name: "old_ext" number: 50 extendee: ".q.O" }"#,
                ),
                file(
                    "x2.pb",
                    r#"name: "x2.proto" package: "p" extension { name: "new_ext" number: 50 extendee: ".q.O" }"#,
                ),
            ],
            sources: vec![source("o", OLD, &["x1.pb"]), source("n", NEW, &["x2.pb"])],
        });
        assert_eq!(rendered(&a), [(Kind::Numbers, "file:x1.proto".into())]);
    }

    #[test]
    fn tie_has_no_decision() {
        let a = analyze(&Input {
            files: vec![
                file("a.pb", r#"name: "a.proto" message_type { name: "M" }"#),
                file("b.pb", r#"name: "b.proto" message_type { name: "M" }"#),
            ],
            sources: vec![source("s", NEW, &["a.pb", "b.pb"])],
        });
        assert!(a.decisions.is_empty());
        assert_eq!(a.ties, vec![vec!["a.pb".to_string(), "b.pb".to_string()]]);
    }

    #[test]
    fn unlisted_file_is_dated_by_its_commit() {
        let a = analyze(&Input {
            files: vec![file("m.pb", r#"name: "m.proto""#)],
            sources: vec![],
        });
        assert_eq!(
            a.files["m.pb"].age,
            Age {
                time: NEW + 1,
                built: false
            }
        );
    }
}
