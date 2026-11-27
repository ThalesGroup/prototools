// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Lineage statements (spec 0499 S1, S1b): which producer's line each
//! snapshot belongs to, stated by whoever knows the producer, after
//! extraction; kept beside the snapshots, never in them, as a record of
//! their own, `refs/seriate/lineages`: a commit per command, its tree one
//! file, `lineages`, the whole mapping in force, its parent the previous
//! record. A snapshot no statement names is in `default`.

use std::collections::{BTreeMap, BTreeSet};

use gix::refs::transaction::PreviousValue;
use gix::ObjectId;

use crate::layout::{check_lineage_name, lineages_ref, DEFAULT_LINEAGE};
use crate::Error;

/// The file of the record's tree that holds the mapping.
const FILE: &str = "lineages";

/// The statements in force, and the record they were read from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Statements {
    /// The record's commit; `None` before any statement.
    pub record: Option<ObjectId>,
    /// Identity → lineage, `default` left out.
    pub lineages: BTreeMap<String, String>,
}

impl Statements {
    /// The lineage of `identity`: its statement's, else `default`.
    pub fn of(&self, identity: &str) -> &str {
        self.lineages
            .get(identity)
            .map_or(DEFAULT_LINEAGE, String::as_str)
    }
}

/// The statements in force (S1b).
pub fn read(repo: &gix::Repository) -> Result<Statements, Error> {
    let Some(record) = crate::store::tip(repo, &lineages_ref())? else {
        return Ok(Statements::default());
    };
    let tree = crate::history::commit_tree(repo, record)?;
    let mut lineages = BTreeMap::new();
    for (path, blob) in crate::history::files(repo, tree, "")? {
        if path != FILE {
            continue;
        }
        let data = repo.find_blob(blob).map_err(Error::git)?;
        for line in String::from_utf8_lossy(&data.data).lines() {
            let Some((identity, lineage)) = line.rsplit_once(' ') else {
                return Err(Error::Refused(format!(
                    "{}: {record}: not `<identity> <lineage>`: {line:?}",
                    crate::layout::LINEAGES
                )));
            };
            lineages.insert(identity.to_string(), lineage.to_string());
        }
    }
    Ok(Statements {
        record: Some(record),
        lineages,
    })
}

/// What [`state`] did, or would do with `dry_run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stated {
    /// Each identity whose lineage changes: from, to.
    pub changed: Vec<(String, String, String)>,
    /// Each identity already stated so.
    pub unchanged: Vec<String>,
    /// The new record, when one was written.
    pub record: Option<ObjectId>,
}

/// State lineages (`lineage set`, `lineage unset`, S1): each identity to
/// its lineage, `default` for `unset`. An identity that is no snapshot of
/// the repository, in the store or in a history, is refused, naming it,
/// with nothing written: a statement cannot silently miss. One record
/// commit for the whole command, none when nothing changes.
pub fn state(
    repo: &gix::Repository,
    statements: &[(String, String)],
    command: &str,
    dry_run: bool,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Stated, Error> {
    for (_, lineage) in statements {
        if lineage != DEFAULT_LINEAGE {
            check_lineage_name(lineage)?;
        }
    }
    let known = known_identities(repo, legacy)?;
    let unknown: Vec<&str> = statements
        .iter()
        .map(|(i, _)| i.as_str())
        .filter(|i| !known.contains(*i))
        .collect();
    if let Some(first) = unknown.first() {
        return Err(Error::Refused(format!(
            "{} identit{} no snapshot of this repository ({first}{}): nothing stated",
            unknown.len(),
            if unknown.len() == 1 {
                "y is"
            } else {
                "ies are"
            },
            if unknown.len() > 1 { ", …" } else { "" }
        )));
    }
    let current = read(repo)?;
    let mut lineages = current.lineages.clone();
    let mut out = Stated::default();
    for (identity, lineage) in statements {
        let from = current.of(identity).to_string();
        if from == *lineage {
            out.unchanged.push(identity.clone());
            continue;
        }
        if lineage == DEFAULT_LINEAGE {
            lineages.remove(identity);
        } else {
            lineages.insert(identity.clone(), lineage.clone());
        }
        out.changed.push((identity.clone(), from, lineage.clone()));
    }
    if out.changed.is_empty() || dry_run {
        return Ok(out);
    }
    let text: String = lineages
        .iter()
        .map(|(identity, lineage)| format!("{identity} {lineage}\n"))
        .collect();
    let tree = crate::history::one_file_tree(repo, FILE, &text)?;
    let body: String = out
        .changed
        .iter()
        .map(|(identity, from, to)| format!("{identity}: {from} -> {to}\n"))
        .collect();
    let message = format!(
        "Lineages: {command}, {} snapshot(s)\n\n{body}",
        out.changed.len()
    );
    let id =
        crate::history::write_record(repo, tree, current.record.into_iter().collect(), message)?;
    let expected = match current.record {
        Some(old) => PreviousValue::MustExistAndMatch(gix::refs::Target::Object(old)),
        None => PreviousValue::MustNotExist,
    };
    crate::store::set_ref(repo, &lineages_ref(), expected, id)?;
    out.record = Some(id);
    Ok(out)
}

/// `--from FILE`'s statements: one `<identity> <lineage>` per line, blank
/// lines skipped.
pub fn parse_statements(text: &str) -> Result<Vec<(String, String)>, Error> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            [identity, lineage] => out.push((identity.to_string(), lineage.to_string())),
            _ => {
                return Err(Error::Refused(format!(
                    "line {}: expected `<identity> <lineage>`: {line:?}",
                    n + 1
                )))
            }
        }
    }
    Ok(out)
}

/// Every snapshot identity of the repository: in the store, and in every
/// branch's history (a snapshot pruned from the store stays in its
/// history, spec 0497 S4).
fn known_identities(
    repo: &gix::Repository,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<BTreeSet<String>, Error> {
    let mut known: BTreeSet<String> = crate::store::snapshot_refs(repo)?
        .into_iter()
        .map(|(identity, _)| identity)
        .collect();
    let refs = repo.references().map_err(Error::git)?;
    for r in refs.local_branches().map_err(Error::git)? {
        let mut r = r.map_err(Error::git)?;
        let tip = r.peel_to_id().map_err(Error::git)?.detach();
        known.extend(crate::history::history_identities(repo, Some(tip), legacy)?);
    }
    Ok(known)
}
