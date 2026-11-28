// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A snapshot met again (spec 0500): a raw snapshot meeting the history
//! commit of its identity, decided once, here, for `add`, `import` and
//! `settle`.
//!
//! The history determines its snapshots up to canonicalization. A raw
//! snapshot R *is* the snapshot of the history commit H of its identity
//! when their sources agree (identity, build time, order hint, group;
//! labels don't count) and R canonicalizes to H's items: compared directly
//! when H was written under today's rule sets and renderer, else both
//! canonicalized today. Rule sets need not commute, nor C1 then C2 equal
//! C2, so a difference under another stamp is *undecided*, for the user to
//! decide (`accept`), never guessed.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use gix::bstr::ByteSlice;
use gix::ObjectId;

use crate::format::Block;
use crate::plugins::{Canonicalizer, LegacyReader, Renderer};
use crate::Error;

/// The decisions' record (S3a).
pub const ACCEPTED: &str = "refs/seriate/accepted";

/// A history commit holding a snapshot.
#[derive(Debug, Clone)]
struct Held {
    branch: String,
    commit: ObjectId,
    tree: ObjectId,
    block: Block,
}

/// What a meeting decided (S1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The history's snapshot: accepted.
    Is,
    /// Another snapshot under the identity: why.
    IsNot(String),
    /// A difference under another stamp, the paths that differ.
    Undecided {
        paths: Vec<String>,
        stamp: Vec<(String, u32)>,
    },
}

/// One snapshot met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meeting {
    pub identity: String,
    /// R's commit.
    pub commit: ObjectId,
    pub verdict: Verdict,
    /// The history commit it met.
    pub met: String,
}

/// How R's labels differ from H's (S1a): keys added, removed, changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Labels {
    pub added: BTreeSet<String>,
    pub removed: BTreeSet<String>,
    pub changed: BTreeSet<String>,
}

impl Labels {
    fn between(theirs: &[(String, String)], ours: &[(String, String)]) -> Self {
        let a: BTreeMap<&str, &str> = theirs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let b: BTreeMap<&str, &str> = ours.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mut out = Labels::default();
        for (k, v) in &b {
            match a.get(k) {
                None => {
                    out.added.insert(k.to_string());
                }
                Some(w) if w != v => {
                    out.changed.insert(k.to_string());
                }
                _ => {}
            }
        }
        for k in a.keys().filter(|k| !b.contains_key(*k)) {
            out.removed.insert(k.to_string());
        }
        out
    }

    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// The meetings of one command, decided before it refuses anything (S2).
pub struct Meeter<'a> {
    renderer: &'a dyn Renderer,
    canonicalizer: &'a dyn Canonicalizer,
    held: HashMap<String, Vec<Held>>,
    accepted: HashSet<(String, ObjectId)>,
    /// The meetings refused so far.
    pub refused: RefCell<Vec<Meeting>>,
    /// The labels that differed, by identity.
    pub labels: RefCell<Vec<(String, Labels)>>,
}

impl<'a> Meeter<'a> {
    /// Every snapshot the repository's histories hold, and the decisions
    /// accepted so far.
    pub fn new(
        repo: &gix::Repository,
        renderer: &'a dyn Renderer,
        canonicalizer: &'a dyn Canonicalizer,
        legacy: Option<&dyn LegacyReader>,
    ) -> Result<Self, Error> {
        Ok(Meeter {
            renderer,
            canonicalizer,
            held: held(repo, legacy)?,
            accepted: accepted(repo)?
                .into_iter()
                .map(|a| (a.identity, a.commit))
                .collect(),
            refused: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
        })
    }

    /// Whether a history holds `identity`.
    pub fn holds(&self, identity: &str) -> bool {
        self.held.contains_key(identity)
    }

    /// Whether `items` (path, blob) are those of every history commit of
    /// `identity`: a copy rebuilt from the history (S4).
    pub fn is_rebuilt(
        &self,
        repo: &gix::Repository,
        identity: &str,
        items: &[(String, ObjectId)],
    ) -> Result<bool, Error> {
        let Some(held) = self.held.get(identity) else {
            return Ok(false);
        };
        let mine: BTreeMap<&str, ObjectId> = items.iter().map(|(p, b)| (p.as_str(), *b)).collect();
        for h in held {
            for (path, blob) in history_blobs(repo, h)? {
                if mine.get(path.as_str()) != Some(&blob) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Meet R, the snapshot `block` with `items` (path, stored bytes), its
    /// commit `commit`, with every history commit of its identity (S1). A
    /// refusal is kept in `refused`, and labels that differ in `labels`.
    pub fn meet(
        &self,
        repo: &gix::Repository,
        commit: ObjectId,
        block: &Block,
        items: &[(String, Vec<u8>)],
    ) -> Result<Meeting, Error> {
        let identity = block.source.identity.clone();
        let mut meeting = Meeting {
            identity: identity.clone(),
            commit,
            verdict: Verdict::Is,
            met: String::new(),
        };
        let Some(held) = self.held.get(&identity) else {
            return Ok(meeting);
        };
        let rules = self.canonicalizer.rule_sets();
        let renderer = self.renderer.version();
        for h in held {
            meeting.met = format!("{} ({})", h.branch, h.commit.to_hex_with_len(8));
            let verdict = self.decide(repo, h, block, items, &rules, &renderer)?;
            let worse = matches!(
                (&meeting.verdict, &verdict),
                (_, Verdict::IsNot(_)) | (Verdict::Is, Verdict::Undecided { .. })
            );
            if worse {
                meeting.verdict = verdict;
            }
            if matches!(meeting.verdict, Verdict::IsNot(_)) {
                break;
            }
        }
        if matches!(meeting.verdict, Verdict::Undecided { .. })
            && self.accepted.contains(&(identity.clone(), commit))
        {
            meeting.verdict = Verdict::Is;
        }
        match meeting.verdict {
            Verdict::Is => {
                let labels =
                    Labels::between(&held[0].block.source.metadata, &block.source.metadata);
                if !labels.is_empty() {
                    self.labels.borrow_mut().push((identity, labels));
                }
            }
            _ => self.refused.borrow_mut().push(meeting.clone()),
        }
        Ok(meeting)
    }

    fn decide(
        &self,
        repo: &gix::Repository,
        h: &Held,
        block: &Block,
        items: &[(String, Vec<u8>)],
        rules: &[(String, u32)],
        renderer: &[(String, u32)],
    ) -> Result<Verdict, Error> {
        let (a, b) = (&h.block.source, &block.source);
        if a.build_time != b.build_time || a.order_hint != b.order_hint || a.group != b.group {
            return Ok(Verdict::IsNot(format!(
                "its source differs from {}'s (build time, order hint or group)",
                h.branch
            )));
        }
        let theirs = history_blobs(repo, h)?;
        let mine: BTreeMap<&str, &[u8]> = items
            .iter()
            .map(|(p, d)| (p.as_str(), d.as_slice()))
            .collect();
        // A commit `compact` reduced holds some of its snapshot's files only.
        let same_paths = if h.block.reduced {
            theirs.iter().all(|(p, _)| mine.contains_key(p.as_str()))
        } else {
            theirs.len() == mine.len() && theirs.iter().all(|(p, _)| mine.contains_key(p.as_str()))
        };
        if !same_paths {
            return Ok(Verdict::IsNot(
                "its files are not the history commit's".into(),
            ));
        }
        let current = h.block.canon_rules == rules && h.block.renderer == renderer;
        let mut differing = Vec::new();
        for (path, blob) in &theirs {
            let held_bytes = repo.find_blob(*blob).map_err(Error::git)?.data.clone();
            let mine = self.canonical(path, mine[path.as_str()]);
            let theirs = if current {
                Some(held_bytes)
            } else {
                self.canonical(path, &held_bytes)
            };
            if mine.is_none() || mine != theirs {
                differing.push(path.clone());
            }
        }
        Ok(match (differing.is_empty(), current) {
            (true, _) => Verdict::Is,
            (false, true) => Verdict::IsNot(format!(
                "{} file(s) canonicalize otherwise than the history commit's: {}",
                differing.len(),
                first_few(&differing)
            )),
            (false, false) => Verdict::Undecided {
                paths: differing,
                stamp: h.block.canon_rules.clone(),
            },
        })
    }

    /// `stored`'s canonical stored form today, `None` if it doesn't parse.
    fn canonical(&self, path: &str, stored: &[u8]) -> Option<Vec<u8>> {
        crate::history::canonical_form(self.renderer, self.canonicalizer, path, stored).ok()
    }

    /// The refusal for every meeting refused (S2): the list an `accept
    /// --from` takes for the undecided ones, and what to do.
    pub fn refusal(&self) -> Option<Error> {
        let refused = self.refused.borrow();
        if refused.is_empty() {
            return None;
        }
        let mut text = format!(
            "{} snapshot(s) met their history and were refused:\n",
            refused.len()
        );
        let mut undecided = Vec::new();
        for m in refused.iter() {
            match &m.verdict {
                Verdict::IsNot(why) => text.push_str(&format!(
                    "  {}: not the snapshot {} holds: {why}\n",
                    m.identity, m.met
                )),
                Verdict::Undecided { paths, stamp } => {
                    text.push_str(&format!(
                        "  {}: undecided: {} written under {}, today's rule sets {}; {} file(s) \
                         differ: {}\n",
                        m.identity,
                        m.met,
                        stamp_text(stamp),
                        stamp_text(&self.canonicalizer.rule_sets()),
                        paths.len(),
                        first_few(paths)
                    ));
                    undecided.push(format!("{} {}", m.identity, m.commit));
                }
                Verdict::Is => {}
            }
        }
        if refused
            .iter()
            .any(|m| matches!(m.verdict, Verdict::IsNot(_)))
        {
            text.push_str(
                "another snapshot under an identity: forget the history's, then extract again\n",
            );
        }
        if !undecided.is_empty() {
            text.push_str(&format!(
                "undecided ({}): if each is its history commit's snapshot, run `git seriate \
                 accept --reason TEXT --from FILE` with these lines, else forget it:\n",
                undecided.len()
            ));
            for line in undecided {
                text.push_str(&format!("{line}\n"));
            }
        }
        Some(Error::Refused(text.trim_end().to_string()))
    }

    /// The labels that differed, one line per key (S1a), and each
    /// snapshot's with `verbose`.
    pub fn labels_report(&self, verbose: bool) -> Vec<String> {
        let labels = self.labels.borrow();
        if labels.is_empty() {
            return Vec::new();
        }
        let mut keys: BTreeMap<String, usize> = BTreeMap::new();
        for (_, l) in labels.iter() {
            for k in &l.added {
                *keys.entry(format!("+ {k}")).or_default() += 1;
            }
            for k in &l.removed {
                *keys.entry(format!("- {k}")).or_default() += 1;
            }
            for k in &l.changed {
                *keys.entry(format!("~ {k}")).or_default() += 1;
            }
        }
        let mut out = vec![format!(
            "labels differ on {} snapshot(s), not counted (spec 0500 S1a): {}",
            labels.len(),
            keys.iter()
                .map(|(k, n)| format!("{k} ({n})"))
                .collect::<Vec<_>>()
                .join(", ")
        )];
        if verbose {
            for (identity, l) in labels.iter() {
                let mut keys: Vec<String> = Vec::new();
                keys.extend(l.added.iter().map(|k| format!("+ {k}")));
                keys.extend(l.removed.iter().map(|k| format!("- {k}")));
                keys.extend(l.changed.iter().map(|k| format!("~ {k}")));
                out.push(format!("  {identity}: {}", keys.join(", ")));
            }
        }
        out
    }
}

fn first_few(paths: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut s = paths[..paths.len().min(SHOWN)].join(", ");
    if paths.len() > SHOWN {
        s.push_str(&format!(" and {} more", paths.len() - SHOWN));
    }
    s
}

fn stamp_text(stamp: &[(String, u32)]) -> String {
    if stamp.is_empty() {
        return "none".into();
    }
    stamp
        .iter()
        .map(|(n, v)| format!("{n} {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The items a history commit holds: its block's paths, looked up in its
/// tree.
fn history_blobs(repo: &gix::Repository, h: &Held) -> Result<Vec<(String, ObjectId)>, Error> {
    let mut trees = crate::history::Trees::default();
    let mut out = Vec::new();
    for path in h.block.contents.clone().unwrap_or_default() {
        if let Some(blob) = trees.lookup(repo, h.tree, &path)? {
            out.push((path, blob));
        }
    }
    Ok(out)
}

/// Every snapshot commit of every history's lineages, by identity.
fn held(
    repo: &gix::Repository,
    legacy: Option<&dyn LegacyReader>,
) -> Result<HashMap<String, Vec<Held>>, Error> {
    let mut out: HashMap<String, Vec<Held>> = HashMap::new();
    let refs = repo.references().map_err(Error::git)?;
    let mut histories = Vec::new();
    for r in refs
        .prefixed(crate::layout::HISTORIES)
        .map_err(Error::git)?
    {
        let r = r.map_err(Error::git)?;
        let full = r.name().as_bstr().to_str_lossy().into_owned();
        histories.push(full[crate::layout::HISTORIES.len()..].to_string());
    }
    for name in histories {
        let history = crate::history::read_history(repo, &name)?;
        for (lineage, &tip) in &history.tips {
            let branch = history.branch(lineage);
            let mut current = Some(tip);
            while let Some(id) = current {
                let commit = repo.find_commit(id).map_err(Error::git)?;
                let decoded = commit.decode().map_err(Error::git)?;
                if let Ok(Some(block)) = Block::read_with(&decoded.message.to_str_lossy(), legacy) {
                    if block.contents.is_some() {
                        out.entry(block.source.identity.clone())
                            .or_default()
                            .push(Held {
                                branch: branch.clone(),
                                commit: id,
                                tree: decoded.tree(),
                                block,
                            });
                    }
                }
                current = decoded.parents().next();
            }
        }
    }
    Ok(out)
}

/// One decision of the record (S3a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub identity: String,
    pub commit: ObjectId,
}

/// The decisions accepted so far.
pub fn accepted(repo: &gix::Repository) -> Result<Vec<Accepted>, Error> {
    let name = crate::layout::full_name(ACCEPTED, "record")?;
    let Some(record) = crate::store::tip(repo, &name)? else {
        return Ok(Vec::new());
    };
    let tree = crate::history::commit_tree(repo, record)?;
    let mut out = Vec::new();
    for (path, blob) in crate::history::files(repo, tree, "")? {
        if path != "accepted" {
            continue;
        }
        let data = repo.find_blob(blob).map_err(Error::git)?;
        for line in String::from_utf8_lossy(&data.data).lines() {
            let mut fields = line.split('\t');
            let (Some(identity), Some(commit)) = (fields.next(), fields.next()) else {
                continue;
            };
            if let Ok(commit) = ObjectId::from_hex(commit.as_bytes()) {
                out.push(Accepted {
                    identity: identity.to_string(),
                    commit,
                });
            }
        }
    }
    Ok(out)
}

impl Meeter<'_> {
    /// Whether some history commit of `identity` was written under other
    /// rule sets or another renderer than today's: the meetings `settle`
    /// decides before it writes (S6), a store copy otherwise being met
    /// when it enters.
    pub fn stale(&self, identity: &str) -> bool {
        let rules = self.canonicalizer.rule_sets();
        let renderer = self.renderer.version();
        self.held.get(identity).is_some_and(|held| {
            held.iter()
                .any(|h| h.block.canon_rules != rules || h.block.renderer != renderer)
        })
    }

    /// Meet the snapshot commit `commit`, its block `block`, read from
    /// `repo`.
    pub fn meet_commit(
        &self,
        repo: &gix::Repository,
        commit: ObjectId,
        block: &Block,
    ) -> Result<Meeting, Error> {
        let items = snapshot_items(repo, commit)?;
        self.meet(repo, commit, block, &items)
    }
}

/// A snapshot commit's items, (path, bytes).
pub fn snapshot_items(
    repo: &gix::Repository,
    commit: ObjectId,
) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let tree = crate::history::commit_tree(repo, commit)?;
    let mut out = Vec::new();
    for (path, blob) in crate::history::files(repo, tree, "")? {
        out.push((path, repo.find_blob(blob).map_err(Error::git)?.data.clone()));
    }
    Ok(out)
}

/// The store's snapshots whose meeting with their history is undecided
/// now: those `accept`'s patterns choose among (S3).
pub fn undecided_in_store(
    repo: &gix::Repository,
    meeter: &Meeter,
    legacy: Option<&dyn LegacyReader>,
) -> Result<Vec<Meeting>, Error> {
    let mut out = Vec::new();
    for listed in crate::store::list_with(repo, legacy)? {
        if listed.block.contents.is_none() || !meeter.holds(&listed.identity) {
            continue;
        }
        let m = meeter.meet_commit(repo, listed.commit, &listed.block)?;
        if matches!(m.verdict, Verdict::Undecided { .. }) {
            out.push(m);
        }
    }
    Ok(out)
}

/// Record `chosen` as accepted (S3, S3a): each `(identity, R's commit)`,
/// with what its meeting says when R is here. One commit of
/// `refs/seriate/accepted` for the call, its parents the previous record
/// and each R held here. Returns the lines recorded.
pub fn accept(
    repo: &gix::Repository,
    meeter: &Meeter,
    chosen: &[(String, ObjectId)],
    reason: &str,
    dry_run: bool,
    legacy: Option<&dyn LegacyReader>,
) -> Result<Vec<String>, Error> {
    use gix::refs::transaction::PreviousValue;
    let today = stamp_text(&meeter.canonicalizer.rule_sets());
    let mut lines = Vec::new();
    let mut parents = Vec::new();
    let name = crate::layout::full_name(ACCEPTED, "record")?;
    let previous = crate::store::tip(repo, &name)?;
    parents.extend(previous);
    for (identity, commit) in chosen {
        let (stamp, paths) = match repo.find_commit(*commit) {
            Ok(c) => {
                parents.push(*commit);
                let decoded = c.decode().map_err(Error::git)?;
                let block = Block::read_with(&decoded.message.to_str_lossy(), legacy)
                    .ok()
                    .flatten();
                match block
                    .map(|b| meeter.meet_commit(repo, *commit, &b))
                    .transpose()?
                {
                    Some(Meeting {
                        verdict: Verdict::Undecided { paths, stamp },
                        ..
                    }) => (stamp_text(&stamp), paths.join(",")),
                    _ => ("?".into(), "?".into()),
                }
            }
            // Not here yet: an original `import` or `extract` will bring.
            Err(_) => ("?".into(), "?".into()),
        };
        lines.push(format!("{identity}\t{commit}\t{stamp}\t{today}\t{paths}"));
    }
    meeter.refused.borrow_mut().clear();
    if dry_run || lines.is_empty() {
        return Ok(lines);
    }
    let mut text = String::new();
    if let Some(p) = previous {
        let tree = crate::history::commit_tree(repo, p)?;
        for (path, blob) in crate::history::files(repo, tree, "")? {
            if path == "accepted" {
                text.push_str(&String::from_utf8_lossy(
                    &repo.find_blob(blob).map_err(Error::git)?.data,
                ));
            }
        }
    }
    for l in &lines {
        text.push_str(l);
        text.push('\n');
    }
    let tree = crate::history::one_file_tree(repo, "accepted", &text)?;
    let message = format!(
        "Accepted: {} snapshot(s)\n\nReason: {reason}\n",
        lines.len()
    );
    let signer = crate::history::Signer::from_config(repo)?;
    let id = crate::history::write_record(repo, &signer, tree, parents, message)?;
    let expected = match previous {
        Some(old) => PreviousValue::MustExistAndMatch(gix::refs::Target::Object(old)),
        None => PreviousValue::MustNotExist,
    };
    crate::store::set_ref(repo, &name, expected, id)?;
    Ok(lines)
}

/// How many of `listed`, which `prune` would delete, have a raw form their
/// history doesn't hold (S5): an item whose blob isn't the history
/// commit's.
pub fn raw_forms_lost(
    repo: &gix::Repository,
    listed: &[crate::store::Listed],
    legacy: Option<&dyn LegacyReader>,
) -> Result<usize, Error> {
    let held = held(repo, legacy)?;
    let mut lost = 0;
    for l in listed {
        let Some(h) = held.get(&l.identity).and_then(|h| h.first()) else {
            continue;
        };
        let tree = crate::history::commit_tree(repo, l.commit)?;
        let mine: BTreeMap<String, ObjectId> =
            crate::history::files(repo, tree, "")?.into_iter().collect();
        let theirs = history_blobs(repo, h)?;
        if theirs.len() != mine.len() || theirs.iter().any(|(p, b)| mine.get(p) != Some(b)) {
            lost += 1;
        }
    }
    Ok(lost)
}
