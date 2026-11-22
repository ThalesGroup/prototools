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
}

/// What `settle` did, or would do with `dry_run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub branch: String,
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

/// Settle a history branch (spec 0452 S5).
pub fn settle(repo: &gix::Repository, options: &Options) -> Result<Report, Error> {
    let branch = match &options.into {
        Some(b) => b.clone(),
        None => repo
            .head_name()
            .map_err(Error::git)?
            .map(|n| n.shorten().to_string())
            .ok_or_else(|| Error::Refused("HEAD is detached: give --into".into()))?,
    };
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

    // 4. The sort.
    let target = if options.dry_run {
        repo.clone().with_object_memory()
    } else {
        repo.clone()
    };
    let mut inputs: Vec<SortInput> = Vec::new();
    for entry in &old_chain {
        if let Entry::Snapshot(s) = entry {
            inputs.push(sort_input_from_history(
                &target,
                s,
                &by_branch,
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
    let rules = options.plugins.canonicalizer.rule_sets();
    let plan = plan(
        &old_chain,
        &inputs,
        &sorted.order,
        &rules,
        walk.base,
        &walk.below_identities,
        &mut report,
    );
    report.kept = plan.kept;

    // 6. The commits.
    let committer = committer(repo, options.dry_run)?;
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
                let tree = overlay(&target, parent_tree, &versions.stored_items(input)?)?;
                report.written.push(block.source.identity.clone());
                let commit = gix::objs::Commit {
                    tree,
                    parents: parent.into_iter().collect(),
                    author: input.author.clone(),
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
                    author,
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
    })
}

/// A snapshot commit of the history: its snapshot branch's raw items when
/// the branch is there, else its own, read from the history's tree at it;
/// the branch is then recreated from them.
fn sort_input_from_history(
    target: &gix::Repository,
    s: &SnapCommit,
    by_branch: &HashMap<&str, &store::Listed>,
    dry_run: bool,
    report: &mut Report,
) -> Result<SortInput, Error> {
    let identity = &s.block.source.identity;
    if let Some(l) = by_branch.get(identity.as_str()) {
        return sort_input_from_branch(target, l);
    }
    let all: HashMap<String, ObjectId> = files(target, s.tree, "")?.into_iter().collect();
    let paths = s.block.contents.clone().unwrap_or_default();
    let mut items = Vec::new();
    for path in paths {
        let oid = *all.get(&path).ok_or_else(|| {
            Error::Refused(format!(
                "history commit {} lists {path}, which its tree lacks",
                s.id
            ))
        })?;
        items.push((path, oid));
    }
    let commit = target.find_commit(s.id).map_err(Error::git)?;
    let decoded = commit.decode().map_err(Error::git)?;
    let author = decoded
        .author()
        .map_err(Error::git)?
        .to_owned()
        .map_err(Error::git)?;
    if !dry_run {
        let mut snapshot_items = Vec::new();
        for (path, oid) in &items {
            let blob = target.find_blob(*oid).map_err(Error::git)?;
            snapshot_items.push(store::Item {
                path: path.clone(),
                bytes: blob.data.clone(),
            });
        }
        let mut block = s.block.clone();
        block.canon_rules.clear();
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
        let raw = p
            .renderer
            .parse(path, &stored)
            .map_err(|e| Error::Refused(format!("{path}: {e}")))?;
        let canonical = p.canonicalizer.canon(path, &raw);
        let rendered = p
            .renderer
            .render(path, &canonical)
            .map_err(|e| Error::Refused(format!("{path}: {e}")))?;
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
            let c = self.canonical(path, *oid)?;
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
            .map(|(path, oid)| Ok((path.clone(), self.canonical(path, *oid)?)))
            .collect()
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
    rules: &[(String, u32)],
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
                    && s.block.canon_rules == rules
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
fn files(
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

/// Signs commits with SSH when git is configured to (spec 0452 S5 step 6).
pub(crate) struct Signer {
    key: Option<std::path::PathBuf>,
}

impl Signer {
    pub(crate) fn from_config(repo: &gix::Repository) -> Result<Self, Error> {
        let config = repo.config_snapshot();
        let sign = config.boolean("commit.gpgSign").unwrap_or(false);
        let ssh = config
            .string("gpg.format")
            .is_some_and(|f| f.eq_ignore_ascii_case(b"ssh"));
        if !(sign && ssh) {
            return Ok(Signer { key: None });
        }
        let key = config
            .string("user.signingKey")
            .ok_or_else(|| Error::Refused("commit.gpgSign is set, but not user.signingKey".into()))?
            .to_string();
        let key = match key.strip_prefix("~/") {
            Some(rest) => {
                std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest)
            }
            None => std::path::PathBuf::from(key),
        };
        Ok(Signer { key: Some(key) })
    }

    pub(crate) fn sign(&self, mut commit: gix::objs::Commit) -> Result<gix::objs::Commit, Error> {
        use gix::objs::WriteTo;
        use std::io::Write;
        let Some(key) = &self.key else {
            return Ok(commit);
        };
        let mut payload = Vec::new();
        commit.write_to(&mut payload).map_err(Error::git)?;
        let mut child = std::process::Command::new("ssh-keygen")
            .args(["-Y", "sign", "-n", "git", "-f"])
            .arg(key)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Refused(format!("running ssh-keygen: {e}")))?;
        child.stdin.take().unwrap().write_all(&payload)?;
        let out = child.wait_with_output()?;
        if !out.status.success() || out.stdout.is_empty() {
            return Err(Error::Refused(format!(
                "signing with {}: {}",
                key.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        commit
            .extra_headers
            .push(("gpgsig".into(), out.stdout.into()));
        Ok(commit)
    }
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
