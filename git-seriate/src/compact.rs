// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `compact`: the history before a cutoff garbage-collected (spec 0452 S6,
//! spec 0450 S4).
//!
//! Three levels, each including the ones below:
//!
//! 1. lossless: manual commits that change nothing, and the second copy of
//!    a snapshot stored twice (same identity, same block), which changes
//!    nothing, are dropped. A snapshot commit that changes nothing but is
//!    its source's only record stays: `advise` dates files by it;
//! 2. also the commits that own no path, their sources' records with them:
//!    a path's owner is the last commit before the cutoff holding it, by a
//!    change or as an item of its snapshot (spec 0501). An owner is kept,
//!    and rewritten to make the changes the rebuilt tree lacks. The tree at
//!    the cutoff is unchanged; what is lost is when a version appeared, and
//!    which sources held it;
//! 3. from the snapshot commits kept, the paths they hold but don't own are
//!    removed, and the commit says so (`reduced: true`).
//!
//! Commits at and after the cutoff are kept, their trees unchanged; only
//! their parents change.

use std::collections::HashMap;

use gix::bstr::ByteSlice;
use gix::object::tree::EntryKind;
use gix::refs::transaction::PreviousValue;
use gix::ObjectId;

use crate::format::Block;
use crate::history::{
    apply, author_identity, authored, canonical_form, commit_tree, committer, diff, files,
    with_time, Change, Signer,
};
use crate::plugins::{Canonicalizer, LegacyReader, Renderer};
use crate::store::{self, branch_ref, tip};
use crate::Error;

/// What to compact.
#[derive(Debug, Clone)]
pub struct Options {
    /// The history; the current branch's when `None`.
    pub into: Option<String>,
    /// Commits built before this, in seconds since the epoch, are compacted.
    pub cutoff: i64,
    /// 1, 2 or 3.
    pub level: u8,
    pub dry_run: bool,
    /// Every commit named in the listing, not the first two (spec 0501 S6).
    pub verbose: bool,
}

/// The plugins `compact` uses.
pub struct Plugins<'a> {
    /// Reads older blocks; `None` for none.
    pub legacy: Option<&'a dyn LegacyReader>,
    /// To check a commit written before spec 0464 before it is marked.
    pub renderer: &'a dyn Renderer,
    pub canonicalizer: &'a dyn Canonicalizer,
}

/// What `compact` did, or would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub branch: String,
    /// One line per commit dropped or reduced, oldest first.
    pub actions: Vec<String>,
    pub dropped: usize,
    pub reduced: usize,
    /// Commits of an older format written in `format: 1`.
    pub converted: usize,
    /// Commits written before spec 0464, their items checked canonical,
    /// given the renderer's version (spec 0464 S3).
    pub marked: usize,
    pub moved: bool,
    pub backup: Option<String>,
    pub warnings: Vec<String>,
    /// The snapshots deleted (or, with `dry_run`, to delete): those
    /// of the identities whose last commit is dropped, which `settle`
    /// would otherwise take for new snapshots.
    pub snapshots_deleted: Vec<String>,
}

/// One commit of the history, oldest first.
struct Commit {
    id: ObjectId,
    tree: ObjectId,
    /// `None` for a manual commit.
    block: Option<Block>,
    time: i64,
    changes: Vec<Change>,
    subject: String,
    /// Its block was read by the legacy reader.
    legacy: bool,
    /// It has no renderer's version, and its items are canonical under
    /// today's rule sets and renderer: it is given that version.
    mark: bool,
}

/// Compact a history branch (spec 0452 S6).
///
/// With `plugins.legacy`, commits of an older format are read, and written
/// in `format: 1`; commits with no renderer's version whose items are
/// canonical are given it (spec 0464). A run that drops nothing still
/// converts and marks them.
pub fn compact(
    repo: &gix::Repository,
    options: &Options,
    plugins: &Plugins,
) -> Result<Vec<Report>, Error> {
    let name = crate::history::resolve_history(repo, options.into.as_deref())?;
    let history = crate::history::read_history(repo, &name)?;
    if history.tips.is_empty() {
        return Err(Error::Refused(format!("{name} has no history")));
    }
    // One signer for the command: an encrypted key's passphrase asked once.
    let signer = Signer::from_config(repo)?;
    let mut reports = Vec::new();
    for lineage in history.tips.keys() {
        reports.push(compact_branch(
            repo,
            &history.branch(lineage),
            options,
            plugins,
            &signer,
        )?);
    }
    if !options.dry_run {
        crate::history::record_history(repo, &signer, &history, "compact")?;
    }
    Ok(reports)
}

/// One lineage's branch compacted.
fn compact_branch(
    repo: &gix::Repository,
    branch: &str,
    options: &Options,
    plugins: &Plugins,
    signer: &Signer,
) -> Result<Report, Error> {
    let legacy = plugins.legacy;
    let rules = plugins.canonicalizer.rule_sets();
    let renderer = plugins.renderer.version();
    if !(1..=3).contains(&options.level) {
        return Err(Error::Refused(format!(
            "--level {}: 1, 2 or 3",
            options.level
        )));
    }
    let branch = branch.to_string();
    let ref_name = branch_ref(&branch)?;
    let old_tip =
        tip(repo, &ref_name)?.ok_or_else(|| Error::Refused(format!("no branch {branch:?}")))?;
    let mut report = Report {
        branch: branch.clone(),
        ..Report::default()
    };

    // The whole first-parent chain, oldest first.
    let mut chain = Vec::new();
    let mut current = Some(old_tip);
    while let Some(id) = current {
        let commit = repo.find_commit(id).map_err(Error::git)?;
        let decoded = commit.decode().map_err(Error::git)?;
        let parent = decoded.parents().next();
        let message = decoded.message.to_str_lossy().into_owned();
        let own = Block::read(&message).map_err(|e| Error::Refused(format!("commit {id}: {e}")))?;
        let is_legacy = own.is_none() && legacy.is_some_and(|l| l.read(&message).is_some());
        let block = own.or_else(|| legacy.and_then(|l| l.read(&message)));
        let time = match &block {
            Some(b) => b.source.build_time,
            None => decoded.committer().map_err(Error::git)?.seconds(),
        };
        let parent_tree = match parent {
            Some(p) => commit_tree(repo, p)?,
            None => ObjectId::empty_tree(repo.object_hash()),
        };
        let changes = diff(repo, parent_tree, decoded.tree())?;
        let mark = match &block {
            Some(b) => {
                unmarked_and_canonical(repo, plugins, &rules, b, decoded.tree(), &mut report)?
            }
            None => false,
        };
        chain.push(Commit {
            id,
            tree: decoded.tree(),
            block,
            time,
            changes,
            subject: message.lines().next().unwrap_or("").to_string(),
            legacy: is_legacy,
            mark,
        });
        current = parent;
    }
    chain.reverse();

    // Before the cutoff: below the deepest commit built at or after it.
    let first_kept = chain
        .iter()
        .position(|c| c.time >= options.cutoff)
        .unwrap_or(chain.len());
    let (before, after) = chain.split_at(first_kept);
    // Nothing before the cutoff: nothing to drop, but the commits after it
    // may still be converted or marked.
    let cutoff_tree = match before.last() {
        Some(c) => commit_tree(repo, c.id)?,
        None => ObjectId::empty_tree(repo.object_hash()),
    };

    // A first pass: the second copies, refused when they contradict their
    // first, and what level 1 drops (a manual commit or a second copy that
    // changes nothing).
    let mut first: HashMap<&str, &Commit> = HashMap::new();
    let mut copy_of: Vec<Option<&Commit>> = Vec::with_capacity(before.len());
    for c in before {
        let copy = match &c.block {
            Some(b) => match first.get(b.source.identity.as_str()) {
                Some(f) if f.block.as_ref() == Some(b) => Some(*f),
                Some(f) => {
                    return Err(Error::Refused(format!(
                        "the history holds {:?} twice, at {} and {}, with different blocks",
                        b.source.identity,
                        f.id.to_hex_with_len(8),
                        c.id.to_hex_with_len(8)
                    )))
                }
                None => {
                    first.insert(&b.source.identity, c);
                    None
                }
            },
            None => None,
        };
        copy_of.push(copy);
    }
    let level_1_drops = |i: usize| {
        let c = &before[i];
        c.changes.is_empty() && (c.block.is_none() || copy_of[i].is_some())
    };

    // Each path's owner (spec 0501 S1, S2): the last commit before the
    // cutoff holding it, by a change, a deletion's included, or as an item
    // of its snapshot, among those level 1 keeps.
    let mut owner: HashMap<&str, usize> = HashMap::new();
    for (i, c) in before.iter().enumerate() {
        if !level_1_drops(i) {
            for path in holds(c) {
                owner.insert(path, i);
            }
        }
    }

    // What each commit keeps: its changes, as the rebuild replays them, and
    // the items a reduction removes from its block.
    struct Kept<'a> {
        commit: &'a Commit,
        changes: Vec<Change>,
        removed: Vec<String>,
        reduced: bool,
    }
    let mut kept: Vec<Kept> = Vec::new();
    // The commits kept, by position, and the drop lines naming later
    // holders, written once the pass knows which of them it keeps: a line
    // names only kept commits (S6). Each: the line's index in the actions,
    // its head, the paths it speaks of, their owners.
    let mut kept_at: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut pending: Vec<(usize, String, &str, Vec<usize>)> = Vec::new();
    // The rebuilt tree, path by path, as the kept commits leave it.
    let mut rebuilt: HashMap<String, (EntryKind, ObjectId)> = HashMap::new();
    for (i, c) in before.iter().enumerate() {
        let short = c.id.to_hex_with_len(8);
        if level_1_drops(i) {
            report.actions.push(match copy_of[i] {
                Some(f) => format!(
                    "drop {short} ('{}'): changes nothing, a second copy of {}",
                    c.subject,
                    f.id.to_hex_with_len(8)
                ),
                None => format!("drop {short} ('{}'): changes nothing", c.subject),
            });
            continue;
        }
        if options.level == 1 {
            // Lossless: a snapshot changing nothing is its source's only
            // record, and stays.
            kept_at.insert(i);
            kept.push(Kept {
                commit: c,
                changes: c.changes.clone(),
                removed: Vec::new(),
                reduced: false,
            });
            continue;
        }
        // A deletion with nothing left to delete, its path's writers
        // dropped, owns nothing (S4): decided here, before the commit is
        // kept for it, so that one pass is final.
        let idle = |p: &str| {
            c.changes.iter().any(|ch| ch.path == p && ch.new.is_none()) && !rebuilt.contains_key(p)
        };
        let owned: Vec<&str> = holds(c).filter(|p| owner[p] == i && !idle(p)).collect();
        if owned.is_empty() {
            let mut owners: Vec<usize> = holds(c)
                .filter(|p| owner[p] != i)
                .map(|p| owner[p])
                .collect();
            owners.sort_unstable();
            owners.dedup();
            let deletions = holds(c).any(|p| owner[p] == i);
            match &c.block {
                Some(b) if c.changes.is_empty() => report.actions.push(format!(
                    "drop {short} ('{}'): changes nothing; the only record of {:?} goes with it",
                    c.subject, b.source.identity
                )),
                _ if deletions && owners.is_empty() => report.actions.push(format!(
                    "drop {short} ('{}'): its deletions have nothing left to delete",
                    c.subject
                )),
                _ => {
                    let (head, noun) = if deletions {
                        (
                            format!(
                                "drop {short} ('{}'): its deletions have nothing left to delete, and ",
                                c.subject
                            ),
                            "every other path it holds",
                        )
                    } else {
                        (
                            format!("drop {short} ('{}'): ", c.subject),
                            "every path it holds",
                        )
                    };
                    pending.push((report.actions.len(), head, noun, owners));
                    report.actions.push(String::new());
                }
            }
            continue;
        }
        // Its changes, those it doesn't own left out at level 3, and a
        // deletion only where the rebuilt tree holds the path (S4, S5).
        let reducing = options.level >= 3 && c.block.is_some();
        let mut changes: Vec<Change> = Vec::new();
        for ch in &c.changes {
            if reducing && owner[ch.path.as_str()] != i {
                continue;
            }
            if ch.new.is_none() && !rebuilt.contains_key(&ch.path) {
                continue;
            }
            changes.push(ch.clone());
        }
        let removed: Vec<String> = if reducing {
            holds(c)
                .filter(|p| owner[p] != i)
                .map(str::to_string)
                .collect()
        } else {
            Vec::new()
        };
        let mut state = rebuilt.clone();
        for ch in &changes {
            match ch.new {
                Some(entry) => state.insert(ch.path.clone(), entry),
                None => state.remove(&ch.path),
            };
        }
        // Each path it owns as it held it, added where the rebuilt tree has
        // something else (S4): an item becomes a change of its owner.
        let mut taken = 0;
        for path in &owned {
            let held = match c.changes.iter().find(|ch| ch.path == *path) {
                Some(ch) => ch.new,
                None => entry_at(repo, c.tree, path)?,
            };
            if state.get(*path).copied() != held {
                changes.push(Change {
                    path: path.to_string(),
                    new: held,
                });
                match held {
                    Some(entry) => state.insert(path.to_string(), entry),
                    None => state.remove(*path),
                };
                taken += 1;
            }
        }
        if taken > 0 {
            report.actions.push(format!(
                "own {short} ('{}'): {taken} items it held become its changes",
                c.subject
            ));
        }
        let reduced = !removed.is_empty();
        if reduced {
            report.actions.push(format!(
                "reduce {short} ('{}'): {} of the {} paths it holds are held later",
                c.subject,
                removed.len(),
                holds(c).count()
            ));
        }
        rebuilt = state;
        kept_at.insert(i);
        kept.push(Kept {
            commit: c,
            changes,
            removed,
            reduced,
        });
    }
    // The drop lines naming later holders: the kept ones; a holder dropped
    // too can only be a deletion with nothing left to delete (S4), so its
    // paths are deleted later.
    for (at, head, noun, owners) in pending {
        let (held, deleted): (Vec<usize>, Vec<usize>) =
            owners.iter().partition(|o| kept_at.contains(o));
        let ids: Vec<ObjectId> = held.iter().map(|&o| before[o].id).collect();
        report.actions[at] = match (held.is_empty(), deleted.is_empty()) {
            (false, true) => format!(
                "{head}{noun} is held later, by {}",
                named(&ids, options.verbose)
            ),
            (false, false) => format!(
                "{head}{noun} is held later, by {}, or deleted later",
                named(&ids, options.verbose)
            ),
            (true, _) => format!("{head}{noun} is deleted later"),
        };
    }
    report.dropped = before.len() - kept.len();
    report.reduced = kept.iter().filter(|k| k.reduced).count();

    // Anchors on snapshots no kept commit holds any more: settle will look
    // for them. A dropped copy of a snapshot another commit still holds
    // leaves its anchors resolving.
    let kept_identities: std::collections::HashSet<&str> = kept
        .iter()
        .map(|k| k.commit)
        .chain(after)
        .filter_map(|c| c.block.as_ref().map(|b| b.source.identity.as_str()))
        .collect();
    let mut dropped_identities: Vec<&str> = before
        .iter()
        .filter(|c| !kept.iter().any(|k| k.commit.id == c.id))
        .filter_map(|c| c.block.as_ref().map(|b| b.source.identity.as_str()))
        .filter(|id| !kept_identities.contains(id))
        .collect();
    dropped_identities.sort_unstable();
    dropped_identities.dedup();

    // Their snapshots go too: left, `settle` would take them for
    // new snapshots and put back what this drops.
    let branches: HashMap<String, ObjectId> = store::list_with(repo, legacy)?
        .into_iter()
        .filter(|l| l.block.contents.is_some())
        .map(|l| (l.identity, l.commit))
        .collect();
    let mut to_delete: Vec<(&str, ObjectId)> = Vec::new();
    for id in &dropped_identities {
        if let Some(&commit) = branches.get(*id) {
            report
                .actions
                .push(format!("delete snapshot {id}: its last commit is dropped"));
            report.snapshots_deleted.push(id.to_string());
            to_delete.push((id, commit));
        }
    }
    for c in &chain {
        if c.block.is_some() {
            continue;
        }
        let commit = repo.find_commit(c.id).map_err(Error::git)?;
        let message = commit
            .decode()
            .map_err(Error::git)?
            .message
            .to_str_lossy()
            .into_owned();
        for id in &dropped_identities {
            if message.contains(&format!("{}: {id}", crate::history::ANCHOR_TRAILER)) {
                report.warnings.push(format!(
                    "{} ('{}') is anchored to {id}, which compact drops",
                    c.id.to_hex_with_len(8),
                    c.subject
                ));
            }
        }
    }

    let kept_ids: std::collections::HashSet<ObjectId> = kept.iter().map(|k| k.commit.id).collect();
    report.converted = before
        .iter()
        .filter(|c| c.legacy && kept_ids.contains(&c.id))
        .count()
        + after.iter().filter(|c| c.legacy).count();
    report.marked = before
        .iter()
        .filter(|c| c.mark && kept_ids.contains(&c.id))
        .count()
        + after.iter().filter(|c| c.mark).count();
    if report.dropped == 0 && report.reduced == 0 && report.converted == 0 && report.marked == 0 {
        return Ok(report);
    }
    let target = if options.dry_run {
        repo.clone().with_object_memory()
    } else {
        repo.clone()
    };
    let user = committer(repo, options.dry_run)?;
    let author_identity = author_identity(repo).unwrap_or_else(|| user.clone());
    let mut parent: Option<ObjectId> = None;
    let mut tree = ObjectId::empty_tree(repo.object_hash());
    let rewrite = |parent: Option<ObjectId>,
                   tree: ObjectId,
                   original: ObjectId,
                   message: Option<String>|
     -> Result<ObjectId, Error> {
        let commit = repo.find_commit(original).map_err(Error::git)?;
        let decoded = commit.decode().map_err(Error::git)?;
        let author = decoded
            .author()
            .map_err(Error::git)?
            .to_owned()
            .map_err(Error::git)?;
        let time = decoded.committer().map_err(Error::git)?.seconds();
        let message = message.unwrap_or_else(|| decoded.message.to_str_lossy().into_owned());
        let commit = signer.sign(gix::objs::Commit {
            tree,
            parents: parent.into_iter().collect(),
            // An author it has kept; none, the user's (spec 0471 S3).
            author: authored(&author, &author_identity),
            committer: with_time(&user, time),
            encoding: None,
            message: message.into(),
            extra_headers: Vec::new(),
        })?;
        Ok(target.write_object(&commit).map_err(Error::git)?.detach())
    };
    for k in &kept {
        tree = apply(&target, tree, &k.changes)?;
        let message = match (
            &k.commit.block,
            k.reduced || k.commit.legacy || k.commit.mark,
        ) {
            (Some(block), true) => {
                let mut block = block.clone();
                if k.commit.mark {
                    block.renderer = renderer.clone();
                }
                if let Some(contents) = &mut block.contents {
                    contents.retain(|p| !k.removed.contains(p));
                }
                block.evidence.retain(|p, _| !k.removed.contains(p));
                block.reduced |= k.reduced;
                Some(block.message())
            }
            _ => None,
        };
        parent = Some(rewrite(parent, tree, k.commit.id, message)?);
    }
    if tree != cutoff_tree {
        return Err(Error::Bug(format!(
            "compacting would change the tree at the cutoff ({cutoff_tree} to {tree}); nothing written"
        )));
    }
    // The commits at and after the cutoff, on the new base, trees as they
    // were.
    for c in after {
        let tree = commit_tree(repo, c.id)?;
        let message = c.block.as_ref().filter(|_| c.legacy || c.mark).map(|b| {
            let mut b = b.clone();
            if c.mark {
                b.renderer = renderer.clone();
            }
            b.message()
        });
        parent = Some(rewrite(parent, tree, c.id, message)?);
    }
    if options.dry_run {
        return Ok(report);
    }
    let new_tip = parent.expect("a compacted history has a tip");
    let backup = (1..)
        .map(|n| format!("{branch}-{n}"))
        .find(|b| matches!(branch_ref(b).and_then(|r| tip(repo, &r)), Ok(None)))
        .unwrap();
    store::set_ref(
        repo,
        &branch_ref(&backup)?,
        PreviousValue::MustNotExist,
        old_tip,
    )?;
    report.backup = Some(backup);
    store::set_ref(
        repo,
        &ref_name,
        PreviousValue::MustExistAndMatch(gix::refs::Target::Object(old_tip)),
        new_tip,
    )?;
    report.moved = true;
    for (name, commit) in to_delete {
        store::delete_snapshot(repo, name, commit)?;
    }
    Ok(report)
}

/// A snapshot commit written before spec 0464 (no renderer's version) with
/// today's rule sets, whose every item is canonical: `compact` gives it the
/// version. One that isn't is said, and left for `settle` to rewrite.
fn unmarked_and_canonical(
    repo: &gix::Repository,
    plugins: &Plugins,
    rules: &[(String, u32)],
    block: &Block,
    tree: ObjectId,
    report: &mut Report,
) -> Result<bool, Error> {
    let Some(contents) = &block.contents else {
        return Ok(false);
    };
    if !block.renderer.is_empty() || block.canon_rules != rules {
        return Ok(false);
    }
    let items: HashMap<String, ObjectId> = files(repo, tree, "")?.into_iter().collect();
    for path in contents {
        let Some(oid) = items.get(path) else {
            return Ok(false);
        };
        let stored = repo.find_blob(*oid).map_err(Error::git)?.data.clone();
        if canonical_form(plugins.renderer, plugins.canonicalizer, path, &stored)? != stored {
            report.warnings.push(format!(
                "{}: {path} is not canonical under its canon_rules: not marked; settle will \
                 rewrite it",
                block.source.identity
            ));
            return Ok(false);
        }
    }
    Ok(true)
}

/// The paths a commit holds (spec 0501 S1): those it changes, a deletion's
/// included, and a snapshot commit's items.
fn holds(c: &Commit) -> impl Iterator<Item = &str> {
    let mut paths: Vec<&str> = c.changes.iter().map(|ch| ch.path.as_str()).collect();
    if let Some(contents) = c.block.as_ref().and_then(|b| b.contents.as_ref()) {
        paths.extend(contents.iter().map(String::as_str));
    }
    paths.sort_unstable();
    paths.dedup();
    paths.into_iter()
}

/// The blob at `path` in `tree`, if any.
fn entry_at(
    repo: &gix::Repository,
    tree: ObjectId,
    path: &str,
) -> Result<Option<(EntryKind, ObjectId)>, Error> {
    let tree = repo.find_tree(tree).map_err(Error::git)?;
    Ok(tree
        .lookup_entry_by_path(path)
        .map_err(Error::git)?
        .map(|e| (e.mode().kind(), e.object_id()))
        .filter(|(kind, _)| *kind != EntryKind::Tree))
}

/// `commits`, counted, the first two named, or all of them with `verbose`
/// (spec 0501 S6).
fn named(commits: &[ObjectId], verbose: bool) -> String {
    let short: Vec<String> = commits
        .iter()
        .map(|c| c.to_hex_with_len(8).to_string())
        .collect();
    let shown = if verbose || short.len() <= 2 {
        short.join(", ")
    } else {
        format!("{}, …", short[..2].join(", "))
    };
    match short.len() {
        1 => format!("1 commit ({shown})"),
        n => format!("{n} commits ({shown})"),
    }
}
