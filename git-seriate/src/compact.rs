// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `compact`: the history before a cutoff garbage-collected (spec 0452 S6,
//! spec 0450 S4).
//!
//! Three levels, each including the ones below:
//!
//! 1. commits that change nothing in their own tree are dropped: lossless;
//! 2. commits whose every change a later commit before the cutoff changes
//!    again are dropped: the tree at the cutoff is unchanged, what is lost
//!    is when a version appeared;
//! 3. from the snapshot commits kept, the items a later commit changes
//!    again are removed, and the commit says so (`reduced: true`).
//!
//! Commits at and after the cutoff are kept, their trees unchanged; only
//! their parents change.

use std::collections::HashMap;

use gix::bstr::ByteSlice;
use gix::refs::transaction::PreviousValue;
use gix::ObjectId;

use crate::format::Block;
use crate::history::{apply, commit_tree, committer, diff, with_time, Change, Signer};
use crate::store::{self, branch_ref, tip};
use crate::Error;

/// What to compact.
#[derive(Debug, Clone)]
pub struct Options {
    /// The history branch; the current branch when `None`.
    pub into: Option<String>,
    /// Commits built before this, in seconds since the epoch, are compacted.
    pub cutoff: i64,
    /// 1, 2 or 3.
    pub level: u8,
    pub dry_run: bool,
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
    pub moved: bool,
    pub backup: Option<String>,
    pub warnings: Vec<String>,
}

/// One commit of the history, oldest first.
struct Commit {
    id: ObjectId,
    /// `None` for a manual commit.
    block: Option<Block>,
    time: i64,
    changes: Vec<Change>,
    subject: String,
    /// Its block was read by the legacy reader.
    legacy: bool,
}

/// Compact a history branch (spec 0452 S6).
///
/// With `legacy`, commits of an older format are read, and written in
/// `format: 1`: a run that drops nothing still converts them.
pub fn compact(
    repo: &gix::Repository,
    options: &Options,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Report, Error> {
    if !(1..=3).contains(&options.level) {
        return Err(Error::Refused(format!(
            "--level {}: 1, 2 or 3",
            options.level
        )));
    }
    let branch = match &options.into {
        Some(b) => b.clone(),
        None => repo
            .head_name()
            .map_err(Error::git)?
            .map(|n| n.shorten().to_string())
            .ok_or_else(|| Error::Refused("HEAD is detached: give --into".into()))?,
    };
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
        chain.push(Commit {
            id,
            block,
            time,
            changes,
            subject: message.lines().next().unwrap_or("").to_string(),
            legacy: is_legacy,
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
    let cutoff_tree = match before.last() {
        Some(c) => commit_tree(repo, c.id)?,
        None => return Ok(report),
    };

    // For each path, the last commit before the cutoff that changes it.
    let mut last_change: HashMap<&str, usize> = HashMap::new();
    for (i, c) in before.iter().enumerate() {
        for change in &c.changes {
            last_change.insert(&change.path, i);
        }
    }
    let overwritten = |i: usize, change: &Change| last_change[change.path.as_str()] > i;

    // What each commit keeps.
    struct Kept<'a> {
        commit: &'a Commit,
        changes: Vec<&'a Change>,
        reduced: bool,
    }
    let mut kept: Vec<Kept> = Vec::new();
    for (i, c) in before.iter().enumerate() {
        let short = c.id.to_hex_with_len(8);
        if c.changes.is_empty() {
            report
                .actions
                .push(format!("drop {short} ('{}'): changes nothing", c.subject));
            continue;
        }
        if options.level >= 2 && c.changes.iter().all(|ch| overwritten(i, ch)) {
            report.actions.push(format!(
                "drop {short} ('{}'): every change it makes is made again later",
                c.subject
            ));
            continue;
        }
        let mut changes: Vec<&Change> = c.changes.iter().collect();
        let mut reduced = false;
        if options.level >= 3 && c.block.is_some() {
            let before_len = changes.len();
            changes.retain(|ch| !overwritten(i, ch));
            if changes.len() < before_len {
                reduced = true;
                report.actions.push(format!(
                    "reduce {short} ('{}'): {} of its {before_len} changes are made again later",
                    c.subject,
                    before_len - changes.len()
                ));
            }
        }
        kept.push(Kept {
            commit: c,
            changes,
            reduced,
        });
    }
    report.dropped = before.len() - kept.len();
    report.reduced = kept.iter().filter(|k| k.reduced).count();

    // Anchors on dropped snapshots: settle will look for them.
    let dropped_identities: Vec<&str> = before
        .iter()
        .filter(|c| !kept.iter().any(|k| k.commit.id == c.id))
        .filter_map(|c| c.block.as_ref().map(|b| b.source.identity.as_str()))
        .collect();
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
    if report.dropped == 0 && report.reduced == 0 && report.converted == 0 {
        return Ok(report);
    }
    let target = if options.dry_run {
        repo.clone().with_object_memory()
    } else {
        repo.clone()
    };
    let user = committer(repo, options.dry_run)?;
    let signer = Signer::from_config(repo)?;
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
            author,
            committer: with_time(&user, time),
            encoding: None,
            message: message.into(),
            extra_headers: Vec::new(),
        })?;
        Ok(target.write_object(&commit).map_err(Error::git)?.detach())
    };
    for k in &kept {
        let changes: Vec<Change> = k.changes.iter().map(|c| (*c).clone()).collect();
        tree = apply(&target, tree, &changes)?;
        let message = match (&k.commit.block, k.reduced || k.commit.legacy) {
            (Some(block), true) => {
                let mut block = block.clone();
                let removed: Vec<&str> = k
                    .commit
                    .changes
                    .iter()
                    .filter(|c| !k.changes.iter().any(|kc| kc.path == c.path))
                    .map(|c| c.path.as_str())
                    .collect();
                if let Some(contents) = &mut block.contents {
                    contents.retain(|p| !removed.contains(&p.as_str()));
                }
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
        let message = c.block.as_ref().filter(|_| c.legacy).map(Block::message);
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
    Ok(report)
}
