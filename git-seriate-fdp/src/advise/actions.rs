// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;

use super::problems::{Problem, ProblemKind};
use super::Source;

/// A group of sources (what to extract again to refresh them) that may fix some problems.
#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    pub group: String,
    /// Its sources holding a file to refresh, newest first.
    pub sources: Vec<&'a Source>,
    pub problems: Vec<&'a Problem>,
}

impl Candidate<'_> {
    pub fn newest(&self) -> &Source {
        self.sources[0]
    }
}

/// The candidates, in the order to try them, and the problems no source
/// can address.
pub fn actions<'a>(
    problems: &'a [Problem],
    sources: &'a [Source],
) -> (Vec<Candidate<'a>>, Vec<&'a Problem>) {
    let mut by_group: BTreeMap<String, Candidate<'a>> = BTreeMap::new();
    let mut no_source = Vec::new();
    for p in problems {
        // The file to refresh: the importer, or the file refused.
        let file = match p.kind {
            ProblemKind::Refused | ProblemKind::StaleImporter | ProblemKind::Build => {
                p.files.first()
            }
            _ => None,
        };
        let holders: Vec<&Source> = match file {
            Some(f) => sources.iter().filter(|s| s.paths.contains(f)).collect(),
            None => Vec::new(),
        };
        if holders.is_empty() {
            no_source.push(p);
            continue;
        }
        let mut groups: Vec<String> = holders.iter().map(|s| group_of(s)).collect();
        groups.sort();
        groups.dedup();
        for g in groups {
            let c = by_group.entry(g.clone()).or_insert_with(|| Candidate {
                group: g.clone(),
                sources: Vec::new(),
                problems: Vec::new(),
            });
            for s in holders.iter().filter(|s| group_of(s) == g) {
                if !c.sources.iter().any(|x| x.identity == s.identity) {
                    c.sources.push(s);
                }
            }
            c.problems.push(p);
        }
    }
    let mut candidates: Vec<Candidate> = by_group.into_values().collect();
    for c in &mut candidates {
        c.sources.sort_by(|a, b| {
            b.build_time
                .cmp(&a.build_time)
                .then(a.identity.cmp(&b.identity))
        });
    }
    // Most problems first; then the one whose newest source is oldest (more
    // likely to be stale); then by name.
    candidates.sort_by(|a, b| {
        b.problems
            .len()
            .cmp(&a.problems.len())
            .then(a.newest().build_time.cmp(&b.newest().build_time))
            .then(a.group.cmp(&b.group))
    });
    (candidates, no_source)
}

fn group_of(s: &Source) -> String {
    s.group.clone().unwrap_or_else(|| s.identity.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, group: &str, time: i64, paths: &[&str]) -> Source {
        Source {
            identity: id.into(),
            build_time: time,
            group: Some(group.into()),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            metadata: vec![],
        }
    }

    fn problem(kind: ProblemKind, file: &str) -> Problem {
        Problem {
            kind,
            files: vec![file.into()],
            detail: String::new(),
            build: Vec::new(),
        }
    }

    #[test]
    fn candidates_are_ordered() {
        let sources = vec![
            source("a1", "A", 300, &["x.pb", "y.pb"]),
            source("b1", "B", 100, &["z.pb"]),
            source("c1", "C", 200, &["w.pb"]),
        ];
        let problems = vec![
            problem(ProblemKind::Refused, "x.pb"),
            problem(ProblemKind::StaleImporter, "y.pb"),
            problem(ProblemKind::Refused, "z.pb"),
            problem(ProblemKind::Refused, "w.pb"),
            problem(ProblemKind::MissingImport, "x.pb"),
            problem(ProblemKind::Manual, "m.pb"),
        ];
        let (c, none) = actions(&problems, &sources);
        // A: 2 problems; then B and C with 1 each, B's newest build older.
        assert_eq!(
            c.iter()
                .map(|c| (c.group.as_str(), c.problems.len()))
                .collect::<Vec<_>>(),
            [("A", 2), ("B", 1), ("C", 1)]
        );
        assert_eq!(
            none.iter().map(|p| p.kind).collect::<Vec<_>>(),
            [ProblemKind::MissingImport, ProblemKind::Manual]
        );
    }
}
