// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `settle`: the history rebuilt in sorted order (spec 0452 S5).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use gix::bstr::ByteSlice;
use gix::object::tree::EntryKind;
use gix::refs::transaction::PreviousValue;
use gix::ObjectId;

use crate::format::Block;
use crate::layout::{lineage_branch, record_ref, snapshot_ref, DEFAULT_LINEAGE};
use crate::plugins::{Canonicalizer, ItemPaths, Renderer};
pub(crate) use crate::sign::Signer;
use crate::store::{self, branch_ref, tip};
use crate::Error;

/// The trailer recording a manual commit's anchor (spec 0452 S5 step 3).
pub const ANCHOR_TRAILER: &str = "Snapshot-Anchor";

/// Where the re-sorted part of the history starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cutoff {
    /// The oldest new snapshot's build time, less the settling window.
    Auto,
    /// This time, in seconds since the epoch.
    At(i64),
    /// None: the whole walk is re-sorted.
    Off,
}

/// The plugins `settle` uses (spec 0452 S2).
pub struct Plugins<'a> {
    pub renderer: &'a dyn Renderer,
    pub canonicalizer: &'a dyn Canonicalizer,
    pub item_paths: &'a dyn ItemPaths,
    pub comparator: &'a dyn seriation::Comparator,
    /// Reads older blocks; `None` for none.
    pub legacy: Option<&'a dyn crate::plugins::LegacyReader>,
}

/// What to settle, and how.
pub struct Options<'a> {
    /// The history branch; the current branch when `None`.
    pub into: Option<String>,
    pub cutoff: Cutoff,
    pub settling_days: u32,
    pub dry_run: bool,
    /// The candidates; every snapshot not yet in the history when
    /// empty.
    pub branches: Vec<String>,
    pub sort: seriation::Settings,
    pub plugins: Plugins<'a>,
    /// Canonicalize every item, trusting no history commit, and report each
    /// one a commit's marker wrongly claims canonical (spec 0464 S4).
    pub verify_canon: bool,
}

/// What `settle` did, or would do with `dry_run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub branch: String,
    /// Under `--verify-canon`: each item a history commit's marker claims
    /// canonical, and isn't (spec 0464 S4).
    pub false_markers: Vec<String>,
    /// Commits kept as they were, above the re-sorted part's base.
    pub kept: usize,
    /// The snapshots written, oldest first.
    pub written: Vec<String>,
    /// Manual commits replayed, with where.
    pub replayed: Vec<String>,
    /// Whether the branch moved (always false with `dry_run`).
    pub moved: bool,
    pub backup: Option<String>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
    /// What each phase of the sort did; its moves' snapshots index
    /// `sorted`. Empty when nothing was sorted.
    pub phases: Vec<seriation::PhaseReport>,
    /// The snapshots the sort was given: identity and group.
    pub sorted: Vec<(String, Option<String>)>,
}

// ── The walk ──────────────────────────────────────────────────────────────────

/// A commit of the history, as the walk reads it.
enum Entry {
    Snapshot(SnapCommit),
    Manual(ManualCommit),
}

struct SnapCommit {
    id: ObjectId,
    tree: ObjectId,
    block: Block,
}

struct ManualCommit {
    id: ObjectId,
    changes: Vec<Change>,
    /// Recorded in a trailer, or inferred from the nearest snapshot below.
    anchor: Option<String>,
    inferred: bool,
    /// The nearest snapshot commit below it: where it goes when its
    /// recorded anchor is gone (spec 0452 S5 step 5).
    below: Option<String>,
}

/// A path whose entry differs between two trees: its new entry, or `None`
/// when removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub new: Option<(EntryKind, ObjectId)>,
}

/// One lineage's branch settled (spec 0452 S5), with `candidates`, the new
/// snapshots to sort in, and the snapshots of `forget` left out of it
/// (spec 0497): with any, no new snapshot is settled, and each must be in
/// the re-sorted region.
fn settle_with(
    repo: &gix::Repository,
    branch: &str,
    options: &Options,
    candidate_ids: &[String],
    forget: &HashSet<String>,
    signer: &Signer,
) -> Result<Report, Error> {
    let branch = branch.to_string();
    let ref_name = branch_ref(&branch)?;
    let old_tip = tip(repo, &ref_name)?;
    let mut report = Report {
        branch: branch.clone(),
        ..Report::default()
    };

    // Every snapshot identity the history holds, for the candidates.
    let legacy = options.plugins.legacy;
    let in_history = history_identities(repo, old_tip, legacy)?;

    // 1. The candidates.
    let listed = store::list_with(repo, legacy)?;
    let by_branch: HashMap<&str, &store::Listed> =
        listed.iter().map(|l| (l.identity.as_str(), l)).collect();
    let mut candidates: Vec<&store::Listed> = Vec::new();
    for b in candidate_ids {
        match by_branch.get(b.as_str()) {
            Some(l) if l.block.contents.is_some() => {
                if !in_history.contains(b) {
                    candidates.push(l);
                }
            }
            _ => return Err(Error::Refused(format!("{b:?} is not a snapshot"))),
        }
    }

    if !forget.is_empty() {
        candidates.clear();
    }

    // 2. The cutoff.
    let settling = i64::from(options.settling_days) * 86_400;
    let cutoff = match options.cutoff {
        Cutoff::At(t) => Some(t),
        Cutoff::Off => None,
        Cutoff::Auto => match candidates.iter().map(|l| l.block.source.build_time).min() {
            Some(oldest) => Some(oldest - settling),
            None => {
                report.notes.push("nothing new to settle".into());
                return Ok(report);
            }
        },
    };

    // 3. The walk.
    let walk = walk(
        repo,
        old_tip,
        cutoff,
        settling,
        options.plugins.item_paths,
        legacy,
        &mut report,
    )?;
    let region = &walk.entries[..walk.region_len];
    let old_chain: Vec<&Entry> = region.iter().rev().collect();
    refuse_repeated(repo, &old_chain)?;
    let in_region: HashSet<&str> = old_chain
        .iter()
        .filter_map(|e| match e {
            Entry::Snapshot(s) => Some(s.block.source.identity.as_str()),
            Entry::Manual(_) => None,
        })
        .collect();
    // A snapshot to forget below where the walk stopped (a manual commit
    // changing a snapshot file): refused, saying why, as `settle` warns.
    let mut out_of_reach: Vec<&str> = forget
        .iter()
        .filter(|f| !in_region.contains(f.as_str()))
        .map(String::as_str)
        .collect();
    if !out_of_reach.is_empty() {
        out_of_reach.sort_unstable();
        let why = report
            .warnings
            .iter()
            .find(|w| w.starts_with("settle stopped at"))
            .map_or_else(
                || "the history can't be rewritten that far down".to_string(),
                |w| w.replacen("settle stopped at", "the walk stops at", 1),
            );
        // A glob can reach thousands: a count, then the first few.
        const SHOWN: usize = 5;
        let named = out_of_reach[..out_of_reach.len().min(SHOWN)].join(", ");
        let more = out_of_reach.len().saturating_sub(SHOWN);
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        return Err(Error::Refused(format!(
            "cannot forget {} snapshot(s), below where the history can be rewritten \
             ({named}{more}): {why}",
            out_of_reach.len()
        )));
    }

    // 4. The sort. A history commit written with today's rule sets and
    // renderer is canonical: its items are taken from its tree as they are
    // (spec 0464 S3).
    let rules = options.plugins.canonicalizer.rule_sets();
    let renderer = options.plugins.renderer.version();
    let trust = Trust {
        rules: &rules,
        renderer: &renderer,
        verify: options.verify_canon,
    };
    let target = if options.dry_run {
        repo.clone().with_object_memory()
    } else {
        repo.clone()
    };
    let mut inputs: Vec<SortInput> = Vec::new();
    // Each tree decoded once for every history commit's lookups (spec 0470
    // S1): consecutive commits share nearly all their subtrees.
    let mut trees = Trees::default();
    for entry in &old_chain {
        if let Entry::Snapshot(s) = entry {
            if forget.contains(&s.block.source.identity) {
                continue;
            }
            inputs.push(sort_input_from_history(
                &target,
                s,
                &by_branch,
                &trust,
                &mut trees,
                options.dry_run,
                &mut report,
            )?);
        }
    }
    for l in &candidates {
        inputs.push(sort_input_from_branch(repo, l)?);
    }
    let mut versions = Versions::new(&target, &options.plugins);
    let snapshots: Vec<seriation::Snapshot> = inputs
        .iter()
        .map(|input| versions.snapshot(input))
        .collect::<Result<_, _>>()?;
    if options.verify_canon {
        for input in &inputs {
            for (path, claimed) in &input.claimed {
                let canonical = versions.canonical(path, input_item(input, path))?;
                if canonical != *claimed {
                    report.false_markers.push(format!(
                        "{}: {path}: {} is not canonical, {} is",
                        input.block.source.identity,
                        claimed.to_hex_with_len(8),
                        canonical.to_hex_with_len(8)
                    ));
                }
            }
        }
    }
    let sorted = seriation::sort(
        &snapshots,
        &mut versions,
        options.plugins.comparator,
        &options.sort,
    )
    .map_err(|e| Error::Refused(format!("sorting: {e}")))?;
    report.phases = sorted.phases.clone();
    report.sorted = inputs
        .iter()
        .map(|i| {
            (
                i.block.source.identity.clone(),
                i.block.source.group.clone(),
            )
        })
        .collect();

    // 5. The plan.
    let plan = plan(
        &old_chain,
        &inputs,
        &sorted.order,
        &trust,
        walk.base,
        &walk.below_identities,
        &mut report,
    );
    report.kept = plan.kept;

    // 6. The commits.
    let committer = committer(repo, options.dry_run)?;
    let author_identity = author_identity(repo).unwrap_or_else(|| committer.clone());
    let mut parent = plan.base;
    let mut parent_tree = match parent {
        Some(id) => commit_tree(repo, id)?,
        None => ObjectId::empty_tree(repo.object_hash()),
    };
    for step in &plan.steps {
        let (commit, label) = match step {
            Step::Snapshot(i) => {
                let input = &inputs[*i];
                let mut block = input.block.clone();
                block.canon_rules = rules.clone();
                block.renderer = renderer.clone();
                let tree = overlay(&target, parent_tree, &versions.stored_items(input)?)?;
                report.written.push(block.source.identity.clone());
                let commit = gix::objs::Commit {
                    tree,
                    parents: parent.into_iter().collect(),
                    author: authored(&input.author, &author_identity),
                    committer: with_time(&committer, block.source.build_time),
                    encoding: None,
                    message: block.message().into(),
                    extra_headers: Vec::new(),
                };
                (commit, None)
            }
            Step::Manual { entry, after } => {
                let m = match old_chain[*entry] {
                    Entry::Manual(m) => m,
                    Entry::Snapshot(_) => unreachable!("a manual step is a manual commit"),
                };
                let original = repo.find_commit(m.id).map_err(Error::git)?;
                let decoded = original.decode().map_err(Error::git)?;
                let mut message = decoded.message.to_str_lossy().into_owned();
                if m.inferred {
                    message = with_anchor_trailer(&message, m.anchor.as_deref());
                }
                let tree = apply(&target, parent_tree, &m.changes)?;
                let author = decoded
                    .author()
                    .map_err(Error::git)?
                    .to_owned()
                    .map_err(Error::git)?;
                let time = decoded.committer().map_err(Error::git)?.seconds();
                let subject = message.lines().next().unwrap_or("").to_string();
                let commit = gix::objs::Commit {
                    tree,
                    parents: parent.into_iter().collect(),
                    author: authored(&author, &author_identity),
                    committer: with_time(&committer, time),
                    encoding: None,
                    message: message.into(),
                    extra_headers: Vec::new(),
                };
                let short = m.id.to_hex_with_len(8).to_string();
                (commit, Some(format!("{short} ('{subject}'), {after}")))
            }
        };
        let commit = signer.sign(commit)?;
        let id = target.write_object(&commit).map_err(Error::git)?.detach();
        if let Some(label) = label {
            report.replayed.push(label);
        }
        parent_tree = commit.tree;
        parent = Some(id);
    }

    // 7. The ref.
    let new_tip = parent;
    if new_tip == old_tip || options.dry_run {
        return Ok(report);
    }
    let new_tip = new_tip.expect("a moved branch has a tip");
    if let Some(old) = old_tip {
        let backup = (1..)
            .map(|n| format!("{branch}-{n}"))
            .find(|b| matches!(branch_ref(b).and_then(|r| tip(repo, &r)), Ok(None)))
            .unwrap();
        store::set_ref(
            repo,
            &branch_ref(&backup)?,
            PreviousValue::MustNotExist,
            old,
        )?;
        report.backup = Some(backup);
    }
    let expected = match old_tip {
        Some(old) => PreviousValue::MustExistAndMatch(gix::refs::Target::Object(old)),
        None => PreviousValue::MustNotExist,
    };
    store::set_ref(repo, &ref_name, expected, new_tip)?;
    report.moved = true;
    if checked_out(repo, &ref_name)? {
        // HEAD already names the new tip, so `git reset --keep` would see
        // nothing to do: the merge goes from the previous tip, saved.
        let command = match &report.backup {
            Some(backup) => format!("git read-tree -m -u {backup} {branch}"),
            None => format!("git read-tree -m -u {branch}"),
        };
        report.notes.push(format!(
            "{branch} is checked out: run '{command}' to bring the index and work tree \
             to it, keeping local changes"
        ));
    }
    Ok(report)
}

// ── Histories of lineages (spec 0499) ────────────────────────────────────────

/// A history as laid out since spec 0499 (S2): a branch per lineage,
/// `<history>/<lineage>`, and its record, `refs/seriate/histories/<history>`,
/// whose tree lists the lineages and their tips as of the last command that
/// wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub name: String,
    /// The record's commit; `None` for a history not yet written.
    pub record: Option<ObjectId>,
    /// Lineage → its branch's tip, now.
    pub tips: BTreeMap<String, ObjectId>,
    /// Lineage → its tip as the record has it.
    pub recorded: BTreeMap<String, ObjectId>,
    /// The statements' record the record routed by.
    pub statements: Option<ObjectId>,
}

impl History {
    /// The branch of `lineage`.
    pub fn branch(&self, lineage: &str) -> String {
        lineage_branch(&self.name, lineage)
    }
}

/// The history `into` names, else the current branch's: `H`, or a lineage
/// branch `H/L` of a history `H`. A history of before spec 0499, a branch
/// of its own name, is refused, naming `migrate`.
pub fn resolve_history(repo: &gix::Repository, into: Option<&str>) -> Result<String, Error> {
    let name = match into {
        Some(n) => n.to_string(),
        None => repo
            .head_name()
            .map_err(Error::git)?
            .map(|n| n.shorten().to_string())
            .ok_or_else(|| Error::Refused("HEAD is detached: give --into".into()))?,
    };
    let has_record = |h: &str| -> Result<bool, Error> { Ok(tip(repo, &record_ref(h)?)?.is_some()) };
    // A lineage branch `H/L` names `H`, its record looked up first: under a
    // ref file, git can't look a deeper name up.
    let name = match name.rsplit_once('/') {
        Some((h, _)) if has_record(h)? => h.to_string(),
        _ => name,
    };
    if !has_record(&name)? && tip(repo, &branch_ref(&name)?)?.is_some() {
        return Err(Error::Refused(format!(
            "{name} is a history of before spec 0499, a branch of its own: run `git seriate \
             migrate {name}` (or `migrate {name} --into NEW` to leave it as it is)"
        )));
    }
    Ok(name)
}

/// The history `name` (S2): its lineages from its record, their tips from
/// their branches. A lineage whose branch is gone is refused, naming it.
pub fn read_history(repo: &gix::Repository, name: &str) -> Result<History, Error> {
    let mut history = History {
        name: name.to_string(),
        ..History::default()
    };
    let Some(record) = tip(repo, &record_ref(name)?)? else {
        return Ok(history);
    };
    history.record = Some(record);
    let tree = commit_tree(repo, record)?;
    for (path, blob) in files(repo, tree, "")? {
        let data = repo.find_blob(blob).map_err(Error::git)?;
        let text = String::from_utf8_lossy(&data.data).into_owned();
        let bad = || {
            Error::Refused(format!(
                "{}: {record}: {path}: unreadable",
                record_ref(name).map_or_else(|_| name.to_string(), |r| r.as_bstr().to_string())
            ))
        };
        match path.as_str() {
            "lineages" => {
                for line in text.lines() {
                    let (lineage, id) = line.split_once(' ').ok_or_else(bad)?;
                    let id = ObjectId::from_hex(id.as_bytes()).map_err(|_| bad())?;
                    history.recorded.insert(lineage.to_string(), id);
                }
            }
            "statements" => {
                history.statements =
                    Some(ObjectId::from_hex(text.trim().as_bytes()).map_err(|_| bad())?);
            }
            _ => {}
        }
    }
    for lineage in history.recorded.keys() {
        let branch = history.branch(lineage);
        let Some(t) = tip(repo, &branch_ref(&branch)?)? else {
            return Err(Error::Refused(format!(
                "{branch}, a lineage of {name}'s record, is no branch any more: restore it, \
                 or settle again from a copy of the history"
            )));
        };
        history.tips.insert(lineage.clone(), t);
    }
    Ok(history)
}

/// The lineage branch `into` names, for a command that reads one line of
/// versions (`advise`): `H/L`, or `H` when it has one lineage.
pub fn resolve_lineage_branch(repo: &gix::Repository, into: Option<&str>) -> Result<String, Error> {
    let name = resolve_history(repo, into)?;
    let history = read_history(repo, &name)?;
    if let Some(into) = into {
        if let Some((h, l)) = into.rsplit_once('/') {
            if h == name && history.tips.contains_key(l) {
                return Ok(into.to_string());
            }
        }
    }
    let lineages: Vec<&String> = history.tips.keys().collect();
    match lineages[..] {
        [] => Err(Error::Refused(format!("{name} has no history"))),
        [one] => Ok(history.branch(one)),
        _ => Err(Error::Refused(format!(
            "{name} has {} lineages: give --into {name}/<lineage>, one of {}",
            lineages.len(),
            lineages
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// The history's record written (S2, S5a) when its lineages' tips, or the
/// statements routed by, differ from what it records: its tree lists each
/// lineage and its tip (`lineages`) and the statements' record
/// (`statements`); its parents, the previous record, every tip and the
/// statements' record. Returns the new record, if one was written.
fn write_history_record(
    repo: &gix::Repository,
    signer: &Signer,
    previous: &History,
    tips: &BTreeMap<String, ObjectId>,
    statements: Option<ObjectId>,
    what: &str,
) -> Result<Option<ObjectId>, Error> {
    if previous.record.is_some() && previous.recorded == *tips && previous.statements == statements
    {
        return Ok(None);
    }
    if tips.is_empty() {
        return Ok(None);
    }
    let lineages: String = tips.iter().map(|(l, id)| format!("{l} {id}\n")).collect();
    let mut entries = vec![(
        "lineages",
        repo.write_blob(lineages.as_bytes())
            .map_err(Error::git)?
            .detach(),
    )];
    if let Some(s) = statements {
        entries.push((
            "statements",
            repo.write_blob(format!("{s}\n").as_bytes())
                .map_err(Error::git)?
                .detach(),
        ));
    }
    let tree = repo
        .write_object(&gix::objs::Tree {
            entries: entries
                .into_iter()
                .map(|(name, oid)| gix::objs::tree::Entry {
                    mode: EntryKind::Blob.into(),
                    filename: name.into(),
                    oid,
                })
                .collect(),
        })
        .map_err(Error::git)?
        .detach();
    let mut parents: Vec<ObjectId> = previous.record.into_iter().collect();
    for id in tips.values().copied().chain(statements) {
        if !parents.contains(&id) {
            parents.push(id);
        }
    }
    let body: String = tips
        .iter()
        .map(|(l, id)| format!("{}: {}\n", lineage_branch(&previous.name, l), id))
        .collect();
    let message = format!(
        "History {}: {what}, {} lineage(s)\n\n{body}",
        previous.name,
        tips.len()
    );
    let id = write_record(repo, signer, tree, parents, message)?;
    let expected = match previous.record {
        Some(old) => PreviousValue::MustExistAndMatch(gix::refs::Target::Object(old)),
        None => PreviousValue::MustNotExist,
    };
    store::set_ref(repo, &record_ref(&previous.name)?, expected, id)?;
    Ok(Some(id))
}

/// The history's record written after a command that moved its lineages'
/// branches, from their tips now (`compact`'s).
pub(crate) fn record_history(
    repo: &gix::Repository,
    signer: &Signer,
    history: &History,
    what: &str,
) -> Result<Option<ObjectId>, Error> {
    let mut tips = BTreeMap::new();
    for lineage in history.tips.keys() {
        if let Some(t) = tip(repo, &branch_ref(&history.branch(lineage))?)? {
            tips.insert(lineage.clone(), t);
        }
    }
    let statements = crate::lineage::read(repo)?.record;
    write_history_record(repo, signer, history, &tips, statements, what)
}

/// The snapshot commits of the history from `tip`, along first parents,
/// newest first: identity, commit, build time.
fn snapshot_commits(
    repo: &gix::Repository,
    tip: ObjectId,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Vec<(String, ObjectId, i64)>, Error> {
    let mut out = Vec::new();
    let mut current = Some(tip);
    while let Some(id) = current {
        let commit = repo.find_commit(id).map_err(Error::git)?;
        let decoded = commit.decode().map_err(Error::git)?;
        if let Ok(Some(block)) = Block::read_with(&decoded.message.to_str_lossy(), legacy) {
            if block.contents.is_some() {
                out.push((block.source.identity, id, block.source.build_time));
            }
        }
        current = decoded.parents().next();
    }
    Ok(out)
}

/// What [`settle`] did, or would do with `dry_run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settled {
    pub history: String,
    /// Each lineage settled, by branch, in order.
    pub lineages: Vec<Report>,
    /// Each snapshot moved to another lineage (S3a): identity, from, to.
    pub moved: Vec<(String, String, String)>,
    /// Each lineage every snapshot of which moved out: its branch, deleted,
    /// and the backup keeping its tip.
    pub emptied: Vec<(String, String)>,
    /// The history's new record, when one was written.
    pub record: Option<ObjectId>,
    pub notes: Vec<String>,
}

/// Settle a history (spec 0452 S5, 0499 S3): each candidate to the branch
/// of its lineage as the statements stand, each lineage sorted on its own;
/// a snapshot whose lineage changed since moved (S3a), its former lineage
/// rewritten without it as `forget` rewrites, its new one sorting it in;
/// the history's record then written.
pub fn settle(repo: &gix::Repository, options: &Options) -> Result<Settled, Error> {
    let legacy = options.plugins.legacy;
    let name = resolve_history(repo, options.into.as_deref())?;
    let history = read_history(repo, &name)?;
    let statements = crate::lineage::read(repo)?;
    // One signer for the command: an encrypted key's passphrase asked once.
    let signer = Signer::from_config(repo)?;
    let mut out = Settled {
        history: name.clone(),
        ..Settled::default()
    };

    // Each lineage's snapshots; those whose statement now names another.
    let mut members: BTreeMap<String, Vec<(String, ObjectId, i64)>> = BTreeMap::new();
    let mut in_any: HashSet<String> = HashSet::new();
    for (lineage, &t) in &history.tips {
        let commits = snapshot_commits(repo, t, legacy)?;
        in_any.extend(commits.iter().map(|(i, _, _)| i.clone()));
        members.insert(lineage.clone(), commits);
    }
    let mut leaving: BTreeMap<String, Vec<(String, ObjectId, i64)>> = BTreeMap::new();
    let mut arriving: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (lineage, commits) in &members {
        let mut seen = HashSet::new();
        for (identity, commit, time) in commits {
            let to = statements.of(identity);
            if to != lineage && seen.insert(identity.clone()) {
                out.moved.push((
                    identity.clone(),
                    history.branch(lineage),
                    history.branch(to),
                ));
                leaving.entry(lineage.clone()).or_default().push((
                    identity.clone(),
                    *commit,
                    *time,
                ));
                arriving
                    .entry(to.to_string())
                    .or_default()
                    .push(identity.clone());
            }
        }
    }

    // The new snapshots, routed.
    let listed = store::list_with(repo, legacy)?;
    let new: Vec<String> = if options.branches.is_empty() {
        listed
            .iter()
            .filter(|l| l.block.contents.is_some() && !in_any.contains(&l.identity))
            .map(|l| l.identity.clone())
            .collect()
    } else {
        let known: HashSet<&str> = listed
            .iter()
            .filter(|l| l.block.contents.is_some())
            .map(|l| l.identity.as_str())
            .collect();
        for b in &options.branches {
            if !known.contains(b.as_str()) {
                return Err(Error::Refused(format!("{b:?} is not a snapshot")));
            }
        }
        options
            .branches
            .iter()
            .filter(|b| !in_any.contains(*b))
            .cloned()
            .collect()
    };
    for identity in new {
        arriving
            .entry(statements.of(&identity).to_string())
            .or_default()
            .push(identity);
    }

    // The lineages left: rewritten without what moved out, or emptied.
    let no_forget = HashSet::new();
    let mut tips = history.tips.clone();
    for (lineage, gone) in &leaving {
        let branch = history.branch(lineage);
        let set: HashSet<String> = gone.iter().map(|(i, _, _)| i.clone()).collect();
        // Moved snapshots the store no longer holds: rebuilt from their
        // history commits first, to be sorted into their new lineage.
        let present: HashSet<String> = listed.iter().map(|l| l.identity.clone()).collect();
        for (identity, commit, _) in gone {
            if !present.contains(identity) {
                if options.dry_run {
                    out.notes
                        .push(format!("would recreate snapshot {identity}"));
                } else {
                    recreate_snapshot(repo, *commit, legacy)?;
                    out.notes.push(format!("recreated snapshot {identity}"));
                }
            }
        }
        let all: HashSet<&str> = members[lineage]
            .iter()
            .map(|(i, _, _)| i.as_str())
            .collect();
        if all.iter().all(|i| set.contains(*i)) {
            let old = history.tips[lineage];
            let backup = backup_name(repo, &branch)?;
            if !options.dry_run {
                store::set_ref(
                    repo,
                    &branch_ref(&backup)?,
                    PreviousValue::MustNotExist,
                    old,
                )?;
                store::delete_ref(repo, branch_ref(&branch)?, old)?;
            }
            tips.remove(lineage);
            out.emptied.push((branch, backup));
            continue;
        }
        let oldest = gone.iter().map(|(_, _, t)| *t).min().expect("one moved");
        let report = settle_with(
            repo,
            &branch,
            &Options {
                into: None,
                cutoff: Cutoff::At(oldest),
                settling_days: 0,
                dry_run: options.dry_run,
                branches: Vec::new(),
                sort: options.sort.clone(),
                plugins: copy_plugins(&options.plugins),
                verify_canon: false,
            },
            &[],
            &set,
            &signer,
        )?;
        out.lineages.push(report);
    }

    // Each lineage with something to sort in, or every one under an explicit
    // cutoff.
    let mut targets: BTreeSet<String> = arriving.keys().cloned().collect();
    if options.cutoff != Cutoff::Auto {
        targets.extend(tips.keys().cloned());
    }
    for lineage in targets {
        let branch = history.branch(&lineage);
        if !options.dry_run {
            if let Some(note) = point_unborn_head(repo, &name, &branch)? {
                out.notes.push(note);
            }
        }
        let mut candidates = arriving.get(&lineage).cloned().unwrap_or_default();
        if options.dry_run {
            // A moved snapshot the store lacks is only recreated for real.
            let present: HashSet<&str> = listed.iter().map(|l| l.identity.as_str()).collect();
            candidates.retain(|c| present.contains(c.as_str()));
        }
        if candidates.is_empty() && options.cutoff == Cutoff::Auto {
            continue;
        }
        let report = settle_with(repo, &branch, options, &candidates, &no_forget, &signer)?;
        out.lineages.push(report);
    }
    if out.lineages.is_empty() && out.emptied.is_empty() {
        out.notes.push("nothing new to settle".into());
    }
    if options.dry_run {
        return Ok(out);
    }

    // The record.
    let mut now_tips = BTreeMap::new();
    for lineage in tips.keys().chain(arriving.keys()) {
        if let Some(t) = tip(repo, &branch_ref(&history.branch(lineage))?)? {
            now_tips.insert(lineage.clone(), t);
        }
    }
    out.record = write_history_record(
        repo,
        &signer,
        &history,
        &now_tips,
        statements.record,
        "settle",
    )?;
    Ok(out)
}

/// HEAD, unborn and naming the history `history` itself, pointed at its
/// lineage branch `branch`: git holds no `H` beside `H/L`, so HEAD would
/// name a branch that can never be; nothing is checked out yet, so nothing
/// is lost. A note when it moves.
fn point_unborn_head(
    repo: &gix::Repository,
    history: &str,
    branch: &str,
) -> Result<Option<String>, Error> {
    use gix::refs::transaction::{Change, LogChange, RefEdit, RefLog};
    let Some(head) = repo.head_name().map_err(Error::git)? else {
        return Ok(None);
    };
    if head.shorten() != history || tip(repo, &head)?.is_some() {
        return Ok(None);
    }
    repo.edit_reference(RefEdit {
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: "git-seriate: HEAD to the history's lineage".into(),
            },
            expected: PreviousValue::Any,
            new: gix::refs::Target::Symbolic(branch_ref(branch)?),
        },
        name: "HEAD".try_into().expect("a valid ref name"),
        deref: false,
    })
    .map_err(Error::git)?;
    Ok(Some(format!(
        "HEAD named {history}, unborn: it names {branch} now, a lineage of {history}"
    )))
}

/// The lowest free `<branch>-N`, where a branch's previous tip is saved.
fn backup_name(repo: &gix::Repository, branch: &str) -> Result<String, Error> {
    for n in 1.. {
        let candidate = format!("{branch}-{n}");
        if tip(repo, &branch_ref(&candidate)?)?.is_none() {
            return Ok(candidate);
        }
    }
    unreachable!("a free name")
}

/// The same plugins, for another call.
fn copy_plugins<'a>(p: &Plugins<'a>) -> Plugins<'a> {
    Plugins {
        renderer: p.renderer,
        canonicalizer: p.canonicalizer,
        item_paths: p.item_paths,
        comparator: p.comparator,
        legacy: p.legacy,
    }
}

/// The snapshot of the history commit `commit` written back to the store,
/// from the items the commit lists (spec 0452 S5): the original snapshot's
/// commit.
fn recreate_snapshot(
    repo: &gix::Repository,
    commit: ObjectId,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<(), Error> {
    let c = repo.find_commit(commit).map_err(Error::git)?;
    let decoded = c.decode().map_err(Error::git)?;
    let block = Block::read_with(&decoded.message.to_str_lossy(), legacy)
        .map_err(|e| Error::Refused(format!("commit {commit}: {e}")))?
        .ok_or_else(|| Error::Bug(format!("commit {commit}: no block")))?;
    let s = SnapCommit {
        id: commit,
        tree: decoded.tree(),
        block,
    };
    let items = history_items(repo, &s, &mut Trees::default())?;
    write_back(repo, &s, &items)
}

/// `items` of history commit `s` added to the store as its snapshot, its
/// origins back from the block (spec 0494 S1), so that the rebuilt
/// snapshot is the same commit.
fn write_back(
    repo: &gix::Repository,
    s: &SnapCommit,
    items: &[(String, ObjectId)],
) -> Result<(), Error> {
    let mut snapshot_items = Vec::new();
    for (path, oid) in items {
        let blob = repo.find_blob(*oid).map_err(Error::git)?;
        snapshot_items.push(store::Item {
            path: path.clone(),
            bytes: blob.data.clone(),
            record: Vec::new(),
            origin: s.block.evidence.get(path).cloned(),
        });
    }
    let mut block = s.block.clone();
    block.canon_rules.clear();
    block.renderer.clear();
    store::add(
        repo,
        store::NewSnapshot {
            source: block.source,
            items: snapshot_items,
            rejected: block.rejected,
            record: block.record,
        },
    )?;
    Ok(())
}

/// What to forget, and from which history (spec 0497).
pub struct ForgetOptions<'a> {
    /// The history; the current branch's when `None`.
    pub into: Option<String>,
    /// Snapshot identities, or globs of them.
    pub patterns: Vec<String>,
    /// Why, kept with each forgotten snapshot.
    pub reason: String,
    pub dry_run: bool,
    pub sort: seriation::Settings,
    pub plugins: Plugins<'a>,
}

/// What [`forget`] did, or would do with `dry_run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forgotten {
    /// Each lineage's rewrite, as `settle` reports one.
    pub settles: Vec<Report>,
    /// The snapshots forgotten, in the history's order, newest first.
    pub forgotten: Vec<String>,
    /// The decision's record, `refs/seriate/forgotten/<n>`, which keeps them.
    pub record: String,
    /// The snapshots deleted from the store.
    pub deleted: Vec<String>,
    /// Each lineage every snapshot of which was forgotten: its branch,
    /// deleted, and the backup keeping its tip.
    pub emptied: Vec<(String, String)>,
}

/// Forget snapshots (spec 0497): out of the history, each lineage holding
/// one rewritten from the oldest of them as `settle` rewrites it, the
/// previous tip saved; out of the store, so that `settle` doesn't put them
/// back; kept, with the reason, by one record per decision,
/// `refs/seriate/forgotten/<n>`: a commit whose parents are the forgotten
/// snapshots, whose message gives the reason, and whose tree lists them.
pub fn forget(repo: &gix::Repository, options: &ForgetOptions) -> Result<Forgotten, Error> {
    let legacy = options.plugins.legacy;
    let name = resolve_history(repo, options.into.as_deref())?;
    let history = read_history(repo, &name)?;
    if history.tips.is_empty() {
        return Err(Error::Refused(format!("{name} has no history")));
    }
    let signer = Signer::from_config(repo)?;
    let mut members: BTreeMap<String, Vec<(String, ObjectId, i64)>> = BTreeMap::new();
    for (lineage, &t) in &history.tips {
        members.insert(lineage.clone(), snapshot_commits(repo, t, legacy)?);
    }
    let matches = |identity: &str| {
        options
            .patterns
            .iter()
            .any(|p| p == identity || crate::plugins::glob_matches(p, identity))
    };
    for p in &options.patterns {
        if !members
            .values()
            .flatten()
            .any(|(i, _, _)| i == p || crate::plugins::glob_matches(p, i))
        {
            return Err(Error::Refused(format!(
                "no snapshot of {name} matches {p:?}"
            )));
        }
    }
    if members.values().flatten().all(|(i, _, _)| matches(i)) {
        return Err(Error::Refused(format!(
            "forgetting every snapshot of {name} would leave it empty: delete its branches \
             instead"
        )));
    }
    let mut out = Forgotten::default();
    let mut chosen_all: Vec<(String, ObjectId)> = Vec::new();
    for (lineage, snapshots) in &members {
        let chosen: Vec<&(String, ObjectId, i64)> =
            snapshots.iter().filter(|(i, _, _)| matches(i)).collect();
        if chosen.is_empty() {
            continue;
        }
        let set: HashSet<String> = chosen.iter().map(|(i, _, _)| i.clone()).collect();
        let branch = history.branch(lineage);
        chosen_all.extend(chosen.iter().map(|(i, c, _)| (i.clone(), *c)));
        // A lineage every snapshot of which goes: emptied, its tip saved.
        if snapshots.iter().all(|(i, _, _)| set.contains(i)) {
            let old = history.tips[lineage];
            let backup = backup_name(repo, &branch)?;
            if !options.dry_run {
                store::set_ref(
                    repo,
                    &branch_ref(&backup)?,
                    PreviousValue::MustNotExist,
                    old,
                )?;
                store::delete_ref(repo, branch_ref(&branch)?, old)?;
            }
            out.emptied.push((branch, backup));
            continue;
        }
        let oldest = chosen
            .iter()
            .map(|(_, _, t)| *t)
            .min()
            .expect("one matched");
        let settle = settle_with(
            repo,
            &branch,
            &Options {
                into: None,
                cutoff: Cutoff::At(oldest),
                settling_days: 0,
                dry_run: options.dry_run,
                branches: Vec::new(),
                sort: options.sort.clone(),
                plugins: copy_plugins(&options.plugins),
                verify_canon: false,
            },
            &[],
            &set,
            &signer,
        )?;
        out.settles.push(settle);
    }
    let record = (1..)
        .map(|n| format!("refs/seriate/forgotten/{n}"))
        .find(|n| {
            gix::refs::FullName::try_from(n.as_str())
                .ok()
                .is_some_and(|f| matches!(tip(repo, &f), Ok(None)))
        })
        .expect("a free name");
    out.record = record.clone();
    // Each snapshot itself where the store keeps it, else its history
    // commit, which keeps it as well.
    let mut seen = HashSet::new();
    let mut kept: Vec<(String, ObjectId, Option<ObjectId>)> = Vec::new();
    for (identity, history_commit) in chosen_all {
        if !seen.insert(identity.clone()) {
            continue;
        }
        let stored = tip(repo, &snapshot_ref(&identity)?)?;
        kept.push((identity.clone(), stored.unwrap_or(history_commit), stored));
        out.forgotten.push(identity.clone());
        if stored.is_some() {
            out.deleted.push(identity);
        }
    }
    if options.dry_run {
        return Ok(out);
    }
    let list: String = kept
        .iter()
        .map(|(identity, commit, _)| format!("{identity}\t{commit}\n"))
        .collect();
    let tree = one_file_tree(repo, "forgotten", &list)?;
    let signature = with_time(&committer(repo, false)?, now());
    let marker = gix::objs::Commit {
        tree,
        parents: kept.iter().map(|(_, c, _)| *c).collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: format!(
            "Forgotten from {name}: {} snapshot(s)\n\nReason: {}\nHistory: {name}\n",
            kept.len(),
            options.reason
        )
        .into(),
        extra_headers: Vec::new(),
    };
    let id = repo.write_object(&marker).map_err(Error::git)?.detach();
    let full: gix::refs::FullName = record
        .as_str()
        .try_into()
        .map_err(|e| Error::Refused(format!("{record:?} is not a valid ref name: {e}")))?;
    store::set_ref(repo, &full, PreviousValue::MustNotExist, id)?;
    for (identity, _, stored) in &kept {
        if let Some(t) = stored {
            store::delete_snapshot(repo, identity, *t)?;
        }
    }
    record_history(repo, &signer, &history, "forget")?;
    Ok(out)
}

/// What [`migrate_history`] did, or would do with `dry_run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigratedHistory {
    /// The new lineage branch, `<new>/default`.
    pub branch: String,
    /// Where the former branch is kept, when migrated in place.
    pub kept: Option<String>,
}

/// A history of before spec 0499, the branch `from`, laid out as one (S4):
/// `NEW/default` with its commits and `NEW`'s record; `from` left as it is
/// when `into` names `NEW`, else moved in place, its former branch kept
/// under `refs/seriate/migrated/`.
pub fn migrate_history(
    repo: &gix::Repository,
    from: &str,
    into: Option<&str>,
    dry_run: bool,
) -> Result<MigratedHistory, Error> {
    let Some(old) = tip(repo, &branch_ref(from)?)? else {
        return Err(Error::Refused(format!("no branch {from:?}")));
    };
    if tip(repo, &record_ref(from)?)?.is_some() {
        return Err(Error::Refused(format!(
            "{from} is a history laid out already"
        )));
    }
    let new = into.unwrap_or(from);
    let branch = lineage_branch(new, DEFAULT_LINEAGE);
    if into.is_some() {
        if tip(repo, &record_ref(new)?)?.is_some() || tip(repo, &branch_ref(new)?)?.is_some() {
            return Err(Error::Refused(format!("{new} exists: give another name")));
        }
        let refs = repo.references().map_err(Error::git)?;
        let prefix = format!("refs/heads/{new}/");
        if refs
            .prefixed(prefix.as_str())
            .map_err(Error::git)?
            .next()
            .is_some()
        {
            return Err(Error::Refused(format!(
                "branches under {new}/ exist: give another name"
            )));
        }
    }
    let mut out = MigratedHistory {
        branch: branch.clone(),
        kept: None,
    };
    if into.is_none() {
        out.kept = Some(format!("{}{from}", crate::layout::MIGRATED));
    }
    if dry_run {
        return Ok(out);
    }
    if let Some(kept) = &out.kept {
        let kept = crate::layout::full_name(kept, "migrated history")?;
        store::set_ref(repo, &kept, PreviousValue::MustNotExist, old)?;
        // Git holds no `NAME` beside `NAME/default`: the branch goes first.
        store::delete_ref(repo, branch_ref(from)?, old)?;
    }
    store::set_ref(
        repo,
        &branch_ref(&branch)?,
        PreviousValue::MustNotExist,
        old,
    )?;
    let previous = History {
        name: new.to_string(),
        ..History::default()
    };
    let tips = BTreeMap::from([(DEFAULT_LINEAGE.to_string(), old)]);
    let statements = crate::lineage::read(repo)?.record;
    write_history_record(
        repo,
        &Signer::from_config(repo)?,
        &previous,
        &tips,
        statements,
        &format!("migrate {from}"),
    )?;
    Ok(out)
}

/// The walk's result.
struct Walk {
    /// Newest first: the re-sorted region, then what lies below it.
    entries: Vec<Entry>,
    /// How many of `entries` are the region (above its base).
    region_len: usize,
    /// The base: the commit the region is rebuilt on.
    base: Option<ObjectId>,
    /// Identities of the snapshot commits below the region.
    below_identities: HashSet<String>,
}

fn walk(
    repo: &gix::Repository,
    tip: Option<ObjectId>,
    cutoff: Option<i64>,
    settling: i64,
    item_paths: &dyn ItemPaths,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
    report: &mut Report,
) -> Result<Walk, Error> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut pending: Vec<usize> = Vec::new();
    let mut stop: Option<ObjectId> = None;
    let mut current = tip;
    while let Some(id) = current {
        let commit = repo.find_commit(id).map_err(Error::git)?;
        let decoded = commit.decode().map_err(Error::git)?;
        let tree = decoded.tree();
        let parent = decoded.parents().next();
        let message = decoded.message.to_str_lossy().into_owned();
        match Block::read_with(&message, legacy) {
            Err(e) => return Err(Error::Refused(format!("commit {id}: {e}"))),
            Ok(Some(block)) => {
                for i in pending.drain(..) {
                    if let Entry::Manual(m) = &mut entries[i] {
                        m.below = Some(block.source.identity.clone());
                        if m.inferred {
                            m.anchor = m.below.clone();
                        }
                    }
                }
                let build_time = block.source.build_time;
                entries.push(Entry::Snapshot(SnapCommit { id, tree, block }));
                if cutoff.is_some_and(|c| build_time < c - settling) {
                    break;
                }
            }
            Ok(None) => {
                let parent_tree = match parent {
                    Some(p) => commit_tree(repo, p)?,
                    None => ObjectId::empty_tree(repo.object_hash()),
                };
                let changes = diff(repo, parent_tree, tree)?;
                if let Some(item) = changes.iter().find(|c| item_paths.is_item(&c.path)) {
                    let subject = message.lines().next().unwrap_or("");
                    let mut w = format!(
                        "settle stopped at {} ('{subject}'), which changes {}",
                        id.to_hex_with_len(8),
                        item.path
                    );
                    if item_paths.is_default() {
                        w.push_str(
                            ":\nno seriate.itemPattern is set, so every file counts as a \
                             snapshot file.\nSet one (git config seriate.itemPattern '*.txt') \
                             to let the history's rewrites pass over hand-made commits.",
                        );
                    }
                    report.warnings.push(w);
                    stop = Some(id);
                    break;
                }
                let (anchor, inferred) = match recorded_anchor(&message) {
                    Some(anchor) => (anchor, false),
                    None => (None, true),
                };
                pending.push(entries.len());
                entries.push(Entry::Manual(ManualCommit {
                    id,
                    changes,
                    anchor,
                    inferred,
                    below: None,
                }));
            }
        }
        current = parent;
    }
    // Anchors still pending: the nearest snapshot commit below, past every
    // limit of the walk.
    if !pending.is_empty() {
        let below = match stop {
            Some(s) => first_parent(repo, s)?,
            None => match entries.last() {
                Some(Entry::Snapshot(s)) => first_parent(repo, s.id)?,
                Some(Entry::Manual(m)) => first_parent(repo, m.id)?,
                None => None,
            },
        };
        let identity = nearest_identity(repo, below, legacy)?;
        for i in pending {
            if let Entry::Manual(m) = &mut entries[i] {
                m.below = identity.clone();
                if m.inferred {
                    m.anchor = identity.clone();
                }
            }
        }
    }

    // The tail: down to the deepest snapshot built at or after the cutoff.
    let snapshot_at: Vec<usize> = (0..entries.len())
        .filter(|&i| matches!(entries[i], Entry::Snapshot(_)))
        .collect();
    let deepest = match cutoff {
        None => snapshot_at.last().copied(),
        Some(c) => snapshot_at
            .iter()
            .copied()
            .filter(
                |&i| matches!(&entries[i], Entry::Snapshot(s) if s.block.source.build_time >= c),
            )
            .max(),
    };
    // The base: the first snapshot commit below the tail, or what stopped
    // the walk, or the root.
    let base_at = match deepest {
        Some(d) => snapshot_at.iter().copied().find(|&i| i > d),
        None => snapshot_at.first().copied(),
    };
    let (region_len, base) = match base_at {
        Some(i) => match &entries[i] {
            Entry::Snapshot(s) => (i, Some(s.id)),
            Entry::Manual(_) => unreachable!(),
        },
        None => (entries.len(), stop),
    };
    let mut below_identities = HashSet::new();
    if let Some(b) = base {
        below_identities = history_identities(repo, Some(b), legacy)?;
    }
    Ok(Walk {
        entries,
        region_len,
        base,
        below_identities,
    })
}

// ── The sort's input ──────────────────────────────────────────────────────────

/// A snapshot to sort: its block, its items as stored, the author to keep.
struct SortInput {
    block: Block,
    items: Vec<(String, ObjectId)>,
    author: gix::actor::Signature,
    /// Its items are canonical, from a history commit trusted (spec 0464
    /// S3): taken as they are.
    trusted: bool,
    /// Under `--verify-canon`, the items a trusted commit holds, to compare
    /// with the canonical forms of `items`.
    claimed: Vec<(String, ObjectId)>,
}

/// Which history commits are canonical as they stand (spec 0464 S3).
struct Trust<'a> {
    rules: &'a [(String, u32)],
    renderer: &'a [(String, u32)],
    /// `--verify-canon`: none, but each is checked.
    verify: bool,
}

impl Trust<'_> {
    /// Written with today's rule sets and renderer.
    fn current(&self, block: &Block) -> bool {
        block.canon_rules == self.rules && block.renderer == self.renderer
    }

    /// Kept by the plan as it is: written with today's rule sets, and
    /// today's renderer or none recorded (before spec 0464; `compact` adds
    /// it).
    fn keeps(&self, block: &Block) -> bool {
        block.canon_rules == self.rules
            && (block.renderer == self.renderer || block.renderer.is_empty())
    }
}

/// `stored`'s canonical stored form: parsed, canonicalized, rendered.
pub(crate) fn canonical_form(
    renderer: &dyn Renderer,
    canonicalizer: &dyn Canonicalizer,
    path: &str,
    stored: &[u8],
) -> Result<Vec<u8>, Error> {
    let raw = renderer
        .parse(path, stored)
        .map_err(|e| Error::Refused(format!("{path}: {e}")))?;
    let canonical = canonicalizer.canon(path, &raw);
    renderer
        .render(path, &canonical)
        .map_err(|e| Error::Refused(format!("{path}: {e}")))
}

/// The item of `input` at `path`.
fn input_item(input: &SortInput, path: &str) -> ObjectId {
    input
        .items
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, oid)| *oid)
        .expect("a claimed item is one of the input's")
}

/// The sort places each snapshot once, so a history that holds one twice
/// is refused. When the later copy changes nothing, as a snapshot stored
/// again right after itself does, the refusal names the way out:
/// `compact --level 1` drops such a commit.
fn refuse_repeated(repo: &gix::Repository, chain: &[&Entry]) -> Result<(), Error> {
    let mut seen: HashMap<&str, ObjectId> = HashMap::new();
    for entry in chain {
        let Entry::Snapshot(s) = entry else {
            continue;
        };
        let identity = s.block.source.identity.as_str();
        let Some(first) = seen.insert(identity, s.id) else {
            continue;
        };
        let parent_tree = match first_parent(repo, s.id)? {
            Some(p) => Some(commit_tree(repo, p)?),
            None => None,
        };
        let mut message = format!(
            "the history holds {identity:?} twice, at {} and {}",
            first.to_hex_with_len(8),
            s.id.to_hex_with_len(8)
        );
        if parent_tree == Some(s.tree) {
            message.push_str(
                "; the second copy changes nothing, and `compact --level 1` drops it: \
                 run it first",
            );
        }
        return Err(Error::Refused(message));
    }
    Ok(())
}

fn sort_input_from_branch(repo: &gix::Repository, l: &store::Listed) -> Result<SortInput, Error> {
    let commit = repo.find_commit(l.commit).map_err(Error::git)?;
    let decoded = commit.decode().map_err(Error::git)?;
    let author = decoded
        .author()
        .map_err(Error::git)?
        .to_owned()
        .map_err(Error::git)?;
    Ok(SortInput {
        block: l.block.clone(),
        items: files(repo, decoded.tree(), "")?,
        author,
        trusted: false,
        claimed: Vec::new(),
    })
}

/// Decoded trees by id, for one command: a tree several history commits
/// share is decoded once (spec 0470 S1).
#[derive(Default)]
pub(crate) struct Trees {
    /// Each entry: its name, whether it is a tree, its id.
    decoded: HashMap<ObjectId, Vec<(Vec<u8>, bool, ObjectId)>>,
}

impl Trees {
    /// The blob at `path` in the tree `root`, if it has one; looked up
    /// component by component, no path built (S2).
    pub(crate) fn lookup(
        &mut self,
        repo: &gix::Repository,
        root: ObjectId,
        path: &str,
    ) -> Result<Option<ObjectId>, Error> {
        let mut tree = root;
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            let entries = match self.decoded.entry(tree) {
                std::collections::hash_map::Entry::Occupied(known) => known.into_mut(),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    let object = repo.find_tree(tree).map_err(Error::git)?;
                    let entries = object
                        .decode()
                        .map_err(Error::git)?
                        .entries
                        .iter()
                        .map(|e| (e.filename.to_vec(), e.mode.is_tree(), e.oid.to_owned()))
                        .collect();
                    slot.insert(entries)
                }
            };
            let found = entries
                .iter()
                .find(|(name, _, _)| name.as_slice() == component.as_bytes())
                .map(|&(_, is_tree, oid)| (is_tree, oid));
            match (found, components.peek().is_some()) {
                (Some((false, oid)), false) => return Ok(Some(oid)),
                (Some((true, oid)), true) => tree = oid,
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// How many trees were decoded.
    #[cfg(test)]
    pub(crate) fn decoded(&self) -> usize {
        self.decoded.len()
    }
}

/// The items a history commit lists, looked up in the history's tree at it
/// (spec 0470 S1): its snapshot's paths, not the whole tree.
fn history_items(
    target: &gix::Repository,
    s: &SnapCommit,
    trees: &mut Trees,
) -> Result<Vec<(String, ObjectId)>, Error> {
    let paths = s.block.contents.clone().unwrap_or_default();
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let oid = trees.lookup(target, s.tree, &path)?.ok_or_else(|| {
            Error::Refused(format!(
                "history commit {} lists {path}, which its tree lacks",
                s.id
            ))
        })?;
        items.push((path, oid));
    }
    Ok(items)
}

/// A snapshot commit of the history: its own items when it is trusted
/// (spec 0464 S3); else its snapshot's raw items when the store holds it,
/// else its own, the snapshot then recreated from them.
fn sort_input_from_history(
    target: &gix::Repository,
    s: &SnapCommit,
    by_branch: &HashMap<&str, &store::Listed>,
    trust: &Trust,
    trees: &mut Trees,
    dry_run: bool,
    report: &mut Report,
) -> Result<SortInput, Error> {
    let identity = &s.block.source.identity;
    let current = trust.current(&s.block);
    let branch = by_branch.get(identity.as_str());
    if let Some(l) = branch.filter(|_| !current || trust.verify) {
        let mut input = sort_input_from_branch(target, l)?;
        if current {
            input.claimed = history_items(target, s, trees)?;
        }
        // The history commit's author, not the branch's: a rewrite keeps it
        // (spec 0471 S3).
        let commit = target.find_commit(s.id).map_err(Error::git)?;
        input.author = commit
            .decode()
            .map_err(Error::git)?
            .author()
            .map_err(Error::git)?
            .to_owned()
            .map_err(Error::git)?;
        return Ok(input);
    }
    let items = history_items(target, s, trees)?;
    let commit = target.find_commit(s.id).map_err(Error::git)?;
    let decoded = commit.decode().map_err(Error::git)?;
    let author = decoded
        .author()
        .map_err(Error::git)?
        .to_owned()
        .map_err(Error::git)?;
    let trusted = current && !trust.verify;
    // Under --verify-canon, with no raw items to start from: the commit's
    // own, canonicalized, against themselves.
    let claimed = if current && trust.verify {
        items.clone()
    } else {
        Vec::new()
    };
    if branch.is_some() {
        return Ok(SortInput {
            block: s.block.clone(),
            items,
            author,
            trusted,
            claimed,
        });
    }
    if !dry_run {
        write_back(target, s, &items)?;
    }
    let verb = if dry_run {
        "would recreate"
    } else {
        "recreated"
    };
    report.notes.push(format!("{verb} snapshot {identity}"));
    Ok(SortInput {
        block: s.block.clone(),
        items,
        author,
        trusted,
        claimed,
    })
}

/// Versions as the sort sees them: canonical stored forms, one
/// `VersionId` per distinct one, written to the target repository.
struct Versions<'a, 'r> {
    target: &'r gix::Repository,
    plugins: &'a Plugins<'a>,
    files: HashMap<String, seriation::FileId>,
    /// Stored blob → canonical stored blob.
    canon: HashMap<ObjectId, ObjectId>,
    ids: HashMap<ObjectId, seriation::VersionId>,
    by_id: Vec<ObjectId>,
}

impl<'a, 'r> Versions<'a, 'r> {
    fn new(target: &'r gix::Repository, plugins: &'a Plugins<'a>) -> Self {
        Versions {
            target,
            plugins,
            files: HashMap::new(),
            canon: HashMap::new(),
            ids: HashMap::new(),
            by_id: Vec::new(),
        }
    }

    /// The canonical stored blob of `oid` at `path`.
    fn canonical(&mut self, path: &str, oid: ObjectId) -> Result<ObjectId, Error> {
        if let Some(&c) = self.canon.get(&oid) {
            return Ok(c);
        }
        let stored = self.target.find_blob(oid).map_err(Error::git)?.data.clone();
        let p = &self.plugins;
        let rendered = canonical_form(p.renderer, p.canonicalizer, path, &stored)?;
        let c = if rendered == stored {
            oid
        } else {
            self.target
                .write_blob(&rendered)
                .map_err(Error::git)?
                .detach()
        };
        self.canon.insert(oid, c);
        Ok(c)
    }

    fn snapshot(&mut self, input: &SortInput) -> Result<seriation::Snapshot, Error> {
        let mut items = Vec::new();
        for (path, oid) in &input.items {
            let next = self.files.len() as seriation::FileId;
            let file = *self.files.entry(path.clone()).or_insert(next);
            let c = self.stored(input, path, *oid)?;
            let next = self.by_id.len() as seriation::VersionId;
            let version = *self.ids.entry(c).or_insert(next);
            if version == next {
                self.by_id.push(c);
            }
            items.push((file, version));
        }
        let s = &input.block.source;
        Ok(seriation::Snapshot {
            identity: s.identity.clone(),
            build_time: s.build_time,
            order_hint: s.order_hint,
            items,
        })
    }

    /// `input`'s items as their canonical stored blobs.
    fn stored_items(&mut self, input: &SortInput) -> Result<Vec<(String, ObjectId)>, Error> {
        input
            .items
            .iter()
            .map(|(path, oid)| Ok((path.clone(), self.stored(input, path, *oid)?)))
            .collect()
    }

    /// An item of `input` as its canonical stored blob: itself when the
    /// input is trusted (spec 0464 S3).
    fn stored(&mut self, input: &SortInput, path: &str, oid: ObjectId) -> Result<ObjectId, Error> {
        if input.trusted {
            return Ok(oid);
        }
        self.canonical(path, oid)
    }
}

impl seriation::Contents for Versions<'_, '_> {
    fn fetch(
        &mut self,
        _file: seriation::FileId,
        version: seriation::VersionId,
    ) -> Result<Vec<u8>, seriation::BoxError> {
        let oid = self.by_id[version as usize];
        Ok(self.target.find_blob(oid)?.data.clone())
    }
}

// ── The plan ──────────────────────────────────────────────────────────────────

enum Step {
    /// An index into the sort's inputs.
    Snapshot(usize),
    /// An index into the old chain, and where it is replayed.
    Manual { entry: usize, after: String },
}

struct Plan {
    /// The commit the rewrite starts from: the last one kept, or the base.
    base: Option<ObjectId>,
    kept: usize,
    steps: Vec<Step>,
}

fn plan(
    old_chain: &[&Entry],
    inputs: &[SortInput],
    order: &[usize],
    trust: &Trust,
    walk_base: Option<ObjectId>,
    below: &HashSet<String>,
    report: &mut Report,
) -> Plan {
    let identity = |i: usize| inputs[i].block.source.identity.as_str();
    // The unchanged prefix: snapshots in the same order and rules, and the
    // manual commits that a replay would put back where they are.
    let mut kept = 0;
    let mut matched = 0;
    let mut last_kept_snapshot: Option<&str> = None;
    let mut base = walk_base;
    for entry in old_chain {
        let keep = match entry {
            Entry::Snapshot(s) => {
                matched < order.len()
                    && identity(order[matched]) == s.block.source.identity
                    && trust.keeps(&s.block)
            }
            Entry::Manual(m) => match &m.anchor {
                Some(a) if Some(a.as_str()) == last_kept_snapshot => true,
                Some(a) => last_kept_snapshot.is_none() && below.contains(a),
                None => last_kept_snapshot.is_none(),
            },
        };
        if !keep {
            break;
        }
        if let Entry::Snapshot(s) = entry {
            matched += 1;
            last_kept_snapshot = Some(&s.block.source.identity);
            base = Some(s.id);
        } else if let Entry::Manual(m) = entry {
            base = Some(m.id);
        }
        kept += 1;
    }
    let rest_snapshots: Vec<usize> = order[matched..].to_vec();
    let placed: HashSet<&str> = rest_snapshots.iter().map(|&i| identity(i)).collect();
    let kept_identities: HashSet<&str> = order[..matched].iter().map(|&i| identity(i)).collect();

    // Where each remaining manual commit goes: `None` at the bottom, or
    // after a remaining snapshot.
    let mut bottom: Vec<(usize, String)> = Vec::new();
    let mut after: HashMap<&str, Vec<(usize, String)>> = HashMap::new();
    let mut original_order: Vec<usize> = Vec::new();
    for (index, entry) in old_chain.iter().enumerate().skip(kept) {
        let Entry::Manual(m) = entry else { continue };
        original_order.push(index);
        let anchor = m.anchor.as_deref();
        match anchor {
            Some(a) if placed.contains(a) => after
                .entry(a)
                .or_default()
                .push((index, format!("after '{a}'"))),
            Some(a) if kept_identities.contains(a) || below.contains(a) => {
                bottom.push((index, "at the bottom".into()))
            }
            None => bottom.push((index, "at the bottom".into())),
            Some(a) => {
                // The nearest snapshot below it that the history still
                // holds (spec 0497): the one directly below may be gone
                // too, as when a manual commit's anchor is forgotten.
                // Replayed after it, or at the bottom when it is in the kept
                // prefix, the manual commit keeps its place among the
                // others, and the tip their versions.
                let nearest = old_chain[..index].iter().rev().find_map(|e| match e {
                    Entry::Snapshot(s) => {
                        let i = s.block.source.identity.as_str();
                        (placed.contains(i) || kept_identities.contains(i)).then_some(i)
                    }
                    Entry::Manual(_) => None,
                });
                let fallback = nearest
                    .or(m.below.as_deref())
                    .filter(|b| placed.contains(b));
                match fallback {
                    Some(b) => {
                        report.notes.push(format!(
                            "anchor '{a}' of {} is gone; replayed after '{b}'",
                            m.id.to_hex_with_len(8)
                        ));
                        after
                            .entry(b)
                            .or_default()
                            .push((index, format!("after '{b}'")));
                    }
                    None => {
                        report.notes.push(format!(
                            "anchor '{a}' of {} is gone; replayed at the bottom",
                            m.id.to_hex_with_len(8)
                        ));
                        bottom.push((index, "at the bottom".into()));
                    }
                }
            }
        }
    }
    let mut steps: Vec<Step> = bottom
        .into_iter()
        .map(|(entry, after)| Step::Manual { entry, after })
        .collect();
    // Manual commits go after the last snapshot of their anchor's identity.
    let last_of: HashMap<&str, usize> = rest_snapshots
        .iter()
        .enumerate()
        .map(|(k, &i)| (identity(i), k))
        .collect();
    for (k, &i) in rest_snapshots.iter().enumerate() {
        steps.push(Step::Snapshot(i));
        if last_of[identity(i)] == k {
            if let Some(list) = after.remove(identity(i)) {
                steps.extend(
                    list.into_iter()
                        .map(|(entry, after)| Step::Manual { entry, after }),
                );
            }
        }
    }

    // Two manual commits replayed in the opposite order, on a common path.
    let replay_rank: HashMap<usize, usize> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Manual { entry, .. } => Some(*entry),
            Step::Snapshot(_) => None,
        })
        .enumerate()
        .map(|(rank, entry)| (entry, rank))
        .collect();
    for (a, &ea) in original_order.iter().enumerate() {
        for &eb in &original_order[a + 1..] {
            if replay_rank[&ea] < replay_rank[&eb] {
                continue;
            }
            let (Entry::Manual(ma), Entry::Manual(mb)) = (old_chain[ea], old_chain[eb]) else {
                continue;
            };
            let pa: HashSet<&str> = ma.changes.iter().map(|c| c.path.as_str()).collect();
            let mut common: Vec<&str> = mb
                .changes
                .iter()
                .map(|c| c.path.as_str())
                .filter(|p| pa.contains(p))
                .collect();
            common.sort();
            if let Some(first) = common.first() {
                report.warnings.push(format!(
                    "{} is now replayed before {}, its anchor having moved; both change {first}{}: \
                     the tip has the older version",
                    mb.id.to_hex_with_len(8),
                    ma.id.to_hex_with_len(8),
                    if common.len() > 1 {
                        format!(" and {} more", common.len() - 1)
                    } else {
                        String::new()
                    }
                ));
            }
        }
    }
    Plan { base, kept, steps }
}

// ── Trees and commits ─────────────────────────────────────────────────────────

pub fn commit_tree(repo: &gix::Repository, id: ObjectId) -> Result<ObjectId, Error> {
    let commit = repo.find_commit(id).map_err(Error::git)?;
    let tree = commit.decode().map_err(Error::git)?.tree();
    Ok(tree)
}

pub(crate) fn first_parent(
    repo: &gix::Repository,
    id: ObjectId,
) -> Result<Option<ObjectId>, Error> {
    let commit = repo.find_commit(id).map_err(Error::git)?;
    let parent = commit.decode().map_err(Error::git)?.parents().next();
    Ok(parent)
}

/// The identity of the first snapshot commit from `from` down.
fn nearest_identity(
    repo: &gix::Repository,
    from: Option<ObjectId>,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Option<String>, Error> {
    let mut current = from;
    while let Some(id) = current {
        let commit = repo.find_commit(id).map_err(Error::git)?;
        let decoded = commit.decode().map_err(Error::git)?;
        if let Ok(Some(block)) = Block::read_with(&decoded.message.to_str_lossy(), legacy) {
            return Ok(Some(block.source.identity));
        }
        current = decoded.parents().next();
    }
    Ok(None)
}

/// Every snapshot identity from `from` down, along first parents.
pub(crate) fn history_identities(
    repo: &gix::Repository,
    from: Option<ObjectId>,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<HashSet<String>, Error> {
    let mut out = HashSet::new();
    let mut current = from;
    while let Some(id) = current {
        let commit = repo.find_commit(id).map_err(Error::git)?;
        let decoded = commit.decode().map_err(Error::git)?;
        if let Ok(Some(block)) = Block::read_with(&decoded.message.to_str_lossy(), legacy) {
            out.insert(block.source.identity);
        }
        current = decoded.parents().next();
    }
    Ok(out)
}

/// Every blob of `tree`, with its path, in tree order.
pub(crate) fn files(
    repo: &gix::Repository,
    tree: ObjectId,
    prefix: &str,
) -> Result<Vec<(String, ObjectId)>, Error> {
    let mut out = Vec::new();
    let object = repo.find_tree(tree).map_err(Error::git)?;
    let decoded = object.decode().map_err(Error::git)?;
    for entry in &decoded.entries {
        let path = format!("{prefix}{}", entry.filename);
        if entry.mode.is_tree() {
            out.extend(files(repo, entry.oid.to_owned(), &format!("{path}/"))?);
        } else {
            out.push((path, entry.oid.to_owned()));
        }
    }
    Ok(out)
}

/// The paths whose entries differ between trees `a` and `b`.
pub fn diff(repo: &gix::Repository, a: ObjectId, b: ObjectId) -> Result<Vec<Change>, Error> {
    let mut out = Vec::new();
    diff_into(repo, Some(a), Some(b), "", &mut out)?;
    Ok(out)
}

fn diff_into(
    repo: &gix::Repository,
    a: Option<ObjectId>,
    b: Option<ObjectId>,
    prefix: &str,
    out: &mut Vec<Change>,
) -> Result<(), Error> {
    if a == b {
        return Ok(());
    }
    let entries = |t: Option<ObjectId>| -> Result<HashMap<String, (EntryKind, ObjectId)>, Error> {
        let Some(t) = t else {
            return Ok(HashMap::new());
        };
        let object = repo.find_tree(t).map_err(Error::git)?;
        let decoded = object.decode().map_err(Error::git)?;
        Ok(decoded
            .entries
            .iter()
            .map(|e| (e.filename.to_string(), (e.mode.kind(), e.oid.to_owned())))
            .collect())
    };
    let (ea, eb) = (entries(a)?, entries(b)?);
    let mut names: Vec<&String> = ea.keys().chain(eb.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        let path = format!("{prefix}{name}");
        let (x, y) = (ea.get(name).copied(), eb.get(name).copied());
        if x == y {
            continue;
        }
        let tree = |e: Option<(EntryKind, ObjectId)>| {
            e.filter(|(k, _)| *k == EntryKind::Tree).map(|(_, o)| o)
        };
        let blob = |e: Option<(EntryKind, ObjectId)>| e.filter(|(k, _)| *k != EntryKind::Tree);
        if tree(x).is_some() || tree(y).is_some() {
            diff_into(repo, tree(x), tree(y), &format!("{path}/"), out)?;
        }
        if blob(x) != blob(y) {
            out.push(Change { path, new: blob(y) });
        }
    }
    Ok(())
}

/// `base` with `items` written over it.
fn overlay(
    repo: &gix::Repository,
    base: ObjectId,
    items: &[(String, ObjectId)],
) -> Result<ObjectId, Error> {
    let mut editor = repo.edit_tree(base).map_err(Error::git)?;
    for (path, oid) in items {
        editor
            .upsert(path.as_str(), EntryKind::Blob, *oid)
            .map_err(Error::git)?;
    }
    Ok(editor.write().map_err(Error::git)?.detach())
}

/// `base` with `changes` applied.
pub(crate) fn apply(
    repo: &gix::Repository,
    base: ObjectId,
    changes: &[Change],
) -> Result<ObjectId, Error> {
    let mut editor = repo.edit_tree(base).map_err(Error::git)?;
    for change in changes {
        match change.new {
            Some((kind, oid)) => {
                editor
                    .upsert(change.path.as_str(), kind, oid)
                    .map_err(Error::git)?;
            }
            None => {
                editor.remove(change.path.as_str()).map_err(Error::git)?;
            }
        }
    }
    Ok(editor.write().map_err(Error::git)?.detach())
}

/// The user running `settle` (spec 0452 S5 step 6): `GIT_COMMITTER_NAME`
/// and `GIT_COMMITTER_EMAIL`, else `user.name` and `user.email`.
pub(crate) fn committer(
    repo: &gix::Repository,
    dry_run: bool,
) -> Result<gix::actor::Signature, Error> {
    let config = repo.config_snapshot();
    let get = |env: &str, key: &str| -> Option<String> {
        std::env::var(env)
            .ok()
            .or_else(|| config.string(key).map(|v| v.to_string()))
    };
    let (name, email) = (
        get("GIT_COMMITTER_NAME", "user.name"),
        get("GIT_COMMITTER_EMAIL", "user.email"),
    );
    match (name, email) {
        (Some(name), Some(email)) => Ok(gix::actor::Signature {
            name: name.into(),
            email: email.into(),
            time: gix::date::Time::new(0, 0),
        }),
        // A dry run writes nothing, so it needs no committer.
        _ if dry_run => Ok(gix::actor::Signature {
            name: "git-seriate".into(),
            email: "".into(),
            time: gix::date::Time::new(0, 0),
        }),
        _ => Err(Error::Refused(
            "no committer: set user.name and user.email (or GIT_COMMITTER_NAME and \
             GIT_COMMITTER_EMAIL)"
                .into(),
        )),
    }
}

/// The identity a commit is authored with when it has no author (spec 0471
/// S2): `GIT_AUTHOR_NAME` and `GIT_AUTHOR_EMAIL`, as git reads them, else
/// `user.name` and `user.email`. `None` when neither gives both: the caller
/// then uses the identity it commits with.
pub(crate) fn author_identity(repo: &gix::Repository) -> Option<gix::actor::Signature> {
    let config = repo.config_snapshot();
    let get = |env: &str, key: &str| -> Option<String> {
        std::env::var(env)
            .ok()
            .or_else(|| config.string(key).map(|v| v.to_string()))
    };
    let name = get("GIT_AUTHOR_NAME", "user.name")?;
    let email = get("GIT_AUTHOR_EMAIL", "user.email")?;
    Some(gix::actor::Signature {
        name: name.into(),
        email: email.into(),
        time: gix::date::Time::new(0, 0),
    })
}

/// `author` as a history commit is written with it (spec 0471): kept when
/// it has an email; else, having none, `identity` at `author`'s date (S1 to
/// S3).
pub(crate) fn authored(
    author: &gix::actor::Signature,
    identity: &gix::actor::Signature,
) -> gix::actor::Signature {
    if !author.email.is_empty() {
        return author.clone();
    }
    gix::actor::Signature {
        time: author.time,
        ..identity.clone()
    }
}

pub(crate) fn with_time(sig: &gix::actor::Signature, seconds: i64) -> gix::actor::Signature {
    gix::actor::Signature {
        time: gix::date::Time::new(seconds, 0),
        ..sig.clone()
    }
}

/// Now, in seconds since the epoch.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// A tree of one file, `name`, holding `text`.
pub(crate) fn one_file_tree(
    repo: &gix::Repository,
    name: &str,
    text: &str,
) -> Result<ObjectId, Error> {
    let blob = repo
        .write_blob(text.as_bytes())
        .map_err(Error::git)?
        .detach();
    Ok(repo
        .write_object(&gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: EntryKind::Blob.into(),
                filename: name.into(),
                oid: blob,
            }],
        })
        .map_err(Error::git)?
        .detach())
}

/// A record's commit (spec 0499 S5a): committed and signed as history
/// commits are (spec 0452 S5 step 6), authored by the user running the
/// command (spec 0471), now.
pub(crate) fn write_record(
    repo: &gix::Repository,
    signer: &Signer,
    tree: ObjectId,
    parents: Vec<ObjectId>,
    message: String,
) -> Result<ObjectId, Error> {
    let now = now();
    let committer = with_time(&committer(repo, false)?, now);
    let author = with_time(
        &author_identity(repo).unwrap_or_else(|| committer.clone()),
        now,
    );
    let commit = gix::objs::Commit {
        tree,
        parents: parents.into_iter().collect(),
        author,
        committer,
        encoding: None,
        message: message.into(),
        extra_headers: Vec::new(),
    };
    let commit = signer.sign(commit)?;
    Ok(repo.write_object(&commit).map_err(Error::git)?.detach())
}

pub(crate) fn checked_out(
    repo: &gix::Repository,
    name: &gix::refs::FullName,
) -> Result<bool, Error> {
    if repo.is_bare() {
        return Ok(false);
    }
    Ok(repo.head_name().map_err(Error::git)?.as_ref() == Some(name))
}

// ── Trailers ──────────────────────────────────────────────────────────────────

/// The message's last paragraph, if every line of it is a `Key: value`
/// trailer.
fn trailer_block(message: &str) -> Option<&str> {
    let trimmed = message.trim_end();
    let last = trimmed.rsplit("\n\n").next()?;
    let is_trailer = |line: &str| {
        line.split_once(": ").is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    };
    (trimmed.contains("\n\n") && last.lines().all(is_trailer)).then_some(last)
}

/// The anchor a message records: `Some(None)` for `Snapshot-Anchor: none`.
fn recorded_anchor(message: &str) -> Option<Option<String>> {
    let block = trailer_block(message)?;
    block.lines().rev().find_map(|line| {
        let (k, v) = line.split_once(": ")?;
        (k == ANCHOR_TRAILER).then(|| (v != "none").then(|| v.to_string()))
    })
}

fn with_anchor_trailer(message: &str, anchor: Option<&str>) -> String {
    let line = format!("{ANCHOR_TRAILER}: {}", anchor.unwrap_or("none"));
    let trimmed = message.trim_end();
    if trailer_block(message).is_some() {
        format!("{trimmed}\n{line}\n")
    } else {
        format!("{trimmed}\n\n{line}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 0470 S1: items looked up by path are what listing the tree
    /// gave; a missing path is none; a subtree two trees share is decoded
    /// once.
    #[test]
    fn items_by_path_are_the_trees_files() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = gix::init_bare(tmp.path().join("r.git")).unwrap();
        let tree_of = |files: &[(&str, &str)], id: &str| {
            let snapshot = store::NewSnapshot {
                source: crate::format::Source {
                    identity: id.into(),
                    build_time: 1,
                    order_hint: None,
                    group: None,
                    metadata: Vec::new(),
                },
                items: files
                    .iter()
                    .map(|(p, t)| store::Item {
                        path: p.to_string(),
                        bytes: t.as_bytes().to_vec(),
                        record: Vec::new(),
                        origin: None,
                    })
                    .collect(),
                rejected: Vec::new(),
                record: Vec::new(),
            };
            store::add(&repo, snapshot).unwrap();
            let commit = repo
                .find_reference(format!("refs/seriate/snapshots/{id}").as_str())
                .unwrap()
                .peel_to_id()
                .unwrap()
                .detach();
            commit_tree(&repo, commit).unwrap()
        };
        let shared = [("d/e/f.txt", "f\n"), ("d/g.txt", "g\n")];
        let one = tree_of(&[shared[0], shared[1], ("a.txt", "1\n")], "one");
        let two = tree_of(&[shared[0], shared[1], ("a.txt", "2\n")], "two");
        let mut trees = Trees::default();
        for root in [one, two] {
            for (path, oid) in files(&repo, root, "").unwrap() {
                assert_eq!(
                    trees.lookup(&repo, root, &path).unwrap(),
                    Some(oid),
                    "{path}"
                );
            }
            for missing in ["b.txt", "d/e", "d/e/f.txt/x", "d/x/f.txt"] {
                assert_eq!(
                    trees.lookup(&repo, root, missing).unwrap(),
                    None,
                    "{missing}"
                );
            }
        }
        // Two roots, d and d/e shared: four trees, each decoded once.
        assert_eq!(trees.decoded(), 4);
    }

    #[test]
    fn anchors_are_read_and_written_as_trailers() {
        assert_eq!(recorded_anchor("docs: x\n\nbody\n"), None);
        assert_eq!(
            recorded_anchor("docs: x\n\nSnapshot-Anchor: a/b\n"),
            Some(Some("a/b".into()))
        );
        assert_eq!(
            recorded_anchor("docs: x\n\nSnapshot-Anchor: none\n"),
            Some(None)
        );
        assert_eq!(
            with_anchor_trailer("docs: x\n\nbody\n", Some("a")),
            "docs: x\n\nbody\n\nSnapshot-Anchor: a\n"
        );
        assert_eq!(
            with_anchor_trailer("docs: x\n\nSigned-off-by: Me <m@x>\n", None),
            "docs: x\n\nSigned-off-by: Me <m@x>\nSnapshot-Anchor: none\n"
        );
        assert_eq!(
            with_anchor_trailer("docs: x", Some("a")),
            "docs: x\n\nSnapshot-Anchor: a\n"
        );
    }
}
