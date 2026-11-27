// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `settle`: the history rebuilt in sorted order (spec 0452 S5).

use std::collections::{HashMap, HashSet};

use gix::bstr::ByteSlice;
use gix::object::tree::EntryKind;
use gix::refs::transaction::PreviousValue;
use gix::ObjectId;

use crate::format::Block;
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
    /// The candidates; every snapshot branch not yet in the history when
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

/// The history branch `into` names, or the current one.
fn history_branch(repo: &gix::Repository, into: Option<&String>) -> Result<String, Error> {
    match into {
        Some(b) => Ok(b.clone()),
        None => repo
            .head_name()
            .map_err(Error::git)?
            .map(|n| n.shorten().to_string())
            .ok_or_else(|| Error::Refused("HEAD is detached: give --into".into())),
    }
}

/// Settle a history branch (spec 0452 S5).
pub fn settle(repo: &gix::Repository, options: &Options) -> Result<Report, Error> {
    settle_with(repo, options, &HashSet::new())
}

/// [`settle`], the snapshots of `forget` left out of the history (spec
/// 0497): with any, no new snapshot is settled, and each must be in the
/// re-sorted region.
fn settle_with(
    repo: &gix::Repository,
    options: &Options,
    forget: &HashSet<String>,
) -> Result<Report, Error> {
    let branch = history_branch(repo, options.into.as_ref())?;
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
        listed.iter().map(|l| (l.branch.as_str(), l)).collect();
    let mut candidates: Vec<&store::Listed> = Vec::new();
    if options.branches.is_empty() {
        candidates.extend(
            listed
                .iter()
                .filter(|l| l.block.contents.is_some() && !in_history.contains(&l.branch)),
        );
    } else {
        for b in &options.branches {
            match by_branch.get(b.as_str()) {
                Some(l) if l.block.contents.is_some() => {
                    if !in_history.contains(b) {
                        candidates.push(l);
                    }
                }
                _ => return Err(Error::Refused(format!("{b:?} is not a snapshot branch"))),
            }
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
    if let Some(missing) = forget.iter().find(|f| !in_region.contains(f.as_str())) {
        return Err(Error::Bug(format!(
            "{missing} to forget is not in the re-sorted region"
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
    let signer = Signer::from_config(repo)?;
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

/// What to forget, and from which history (spec 0497).
pub struct ForgetOptions<'a> {
    /// The history branch; the current branch when `None`.
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
    /// The history's rewrite, as `settle` reports one.
    pub settle: Report,
    /// The snapshots forgotten, in the history's order, newest first.
    pub forgotten: Vec<String>,
    /// The decision's record, `refs/seriate/forgotten/<n>`, which keeps them.
    pub record: String,
    /// The snapshot branches deleted.
    pub deleted: Vec<String>,
}

/// Forget snapshots (spec 0497): out of the history, which is rewritten
/// from the oldest of them as `settle` rewrites it, the previous tip saved;
/// out of the store, their branches deleted, so that `settle` doesn't put
/// them back; kept, with the reason, by one record per decision,
/// `refs/seriate/forgotten/<n>`: a commit whose parents are the forgotten
/// snapshots, whose message gives the reason, and whose tree lists them.
pub fn forget(repo: &gix::Repository, options: &ForgetOptions) -> Result<Forgotten, Error> {
    let branch = history_branch(repo, options.into.as_ref())?;
    let Some(old_tip) = tip(repo, &branch_ref(&branch)?)? else {
        return Err(Error::Refused(format!("{branch} has no history")));
    };
    // The history's snapshot commits, along first parents, newest first.
    let mut snapshots: Vec<(String, ObjectId, i64)> = Vec::new();
    let mut current = Some(old_tip);
    while let Some(id) = current {
        let commit = repo.find_commit(id).map_err(Error::git)?;
        let decoded = commit.decode().map_err(Error::git)?;
        if let Ok(Some(block)) =
            Block::read_with(&decoded.message.to_str_lossy(), options.plugins.legacy)
        {
            if block.contents.is_some() {
                snapshots.push((block.source.identity, id, block.source.build_time));
            }
        }
        current = decoded.parents().next();
    }
    let matches = |identity: &str| {
        options
            .patterns
            .iter()
            .any(|p| p == identity || crate::plugins::glob_matches(p, identity))
    };
    for p in &options.patterns {
        if !snapshots
            .iter()
            .any(|(i, _, _)| i == p || crate::plugins::glob_matches(p, i))
        {
            return Err(Error::Refused(format!(
                "no snapshot of {branch} matches {p:?}"
            )));
        }
    }
    let chosen: Vec<&(String, ObjectId, i64)> =
        snapshots.iter().filter(|(i, _, _)| matches(i)).collect();
    let set: HashSet<String> = chosen.iter().map(|(i, _, _)| i.clone()).collect();
    if snapshots.iter().all(|(i, _, _)| set.contains(i)) {
        return Err(Error::Refused(format!(
            "forgetting every snapshot of {branch} would leave it empty: delete the branch instead"
        )));
    }
    let oldest = chosen
        .iter()
        .map(|(_, _, t)| *t)
        .min()
        .expect("one matched");
    let settle = settle_with(
        repo,
        &Options {
            into: Some(branch.clone()),
            cutoff: Cutoff::At(oldest),
            settling_days: 0,
            dry_run: options.dry_run,
            branches: Vec::new(),
            sort: options.sort.clone(),
            plugins: Plugins {
                renderer: options.plugins.renderer,
                canonicalizer: options.plugins.canonicalizer,
                item_paths: options.plugins.item_paths,
                comparator: options.plugins.comparator,
                legacy: options.plugins.legacy,
            },
            verify_canon: false,
        },
        &set,
    )?;
    let mut out = Forgotten {
        settle,
        ..Forgotten::default()
    };
    let record = (1..)
        .map(|n| format!("refs/seriate/forgotten/{n}"))
        .find(|n| {
            gix::refs::FullName::try_from(n.as_str())
                .ok()
                .is_some_and(|f| matches!(tip(repo, &f), Ok(None)))
        })
        .expect("a free name");
    out.record = record.clone();
    // Each snapshot itself where its branch keeps it, else its history
    // commit, which keeps it as well.
    let mut seen = HashSet::new();
    let mut kept: Vec<(String, ObjectId, Option<ObjectId>)> = Vec::new();
    for (identity, history_commit, _) in chosen {
        if !seen.insert(identity.as_str()) {
            continue;
        }
        let branch_tip = tip(repo, &branch_ref(identity)?)?;
        kept.push((
            identity.clone(),
            branch_tip.unwrap_or(*history_commit),
            branch_tip,
        ));
        out.forgotten.push(identity.clone());
        if branch_tip.is_some() {
            out.deleted.push(identity.clone());
        }
    }
    if options.dry_run {
        return Ok(out);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let list: String = kept
        .iter()
        .map(|(identity, commit, _)| format!("{identity}\t{commit}\n"))
        .collect();
    let blob = repo
        .write_blob(list.as_bytes())
        .map_err(Error::git)?
        .detach();
    let tree = repo
        .write_object(&gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: EntryKind::Blob.into(),
                filename: "forgotten".into(),
                oid: blob,
            }],
        })
        .map_err(Error::git)?
        .detach();
    let signature = with_time(&committer(repo, false)?, now);
    let marker = gix::objs::Commit {
        tree,
        parents: kept.iter().map(|(_, c, _)| *c).collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: format!(
            "Forgotten from {branch}: {} snapshot(s)\n\nReason: {}\nHistory: {branch}\n",
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
    for (identity, _, branch_tip) in &kept {
        if let Some(t) = branch_tip {
            store::delete_branch(repo, identity, *t)?;
        }
    }
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
                             to let settle pass over hand-made commits.",
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
/// (spec 0464 S3); else its snapshot branch's raw items when the branch is
/// there, else its own, the branch then recreated from them.
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
        let mut snapshot_items = Vec::new();
        for (path, oid) in &items {
            let blob = target.find_blob(*oid).map_err(Error::git)?;
            // Its origin back from the block (spec 0494 S1), so that the
            // rebuilt snapshot is the same commit.
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
            target,
            store::NewSnapshot {
                source: block.source,
                items: snapshot_items,
                rejected: block.rejected,
                record: block.record,
            },
        )?;
    }
    report
        .notes
        .push(format!("recreated snapshot branch {identity}"));
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
                let fallback = m.below.as_deref().filter(|b| placed.contains(b));
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
fn history_identities(
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
                .find_reference(format!("refs/heads/{id}").as_str())
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
