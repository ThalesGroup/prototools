// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Snapshots: add, list, prune, import (spec 0452 S3), each a parentless
//! commit under `refs/seriate/snapshots/` (spec 0499 S2).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use gix::bstr::{BStr, ByteSlice};
use gix::object::tree::EntryKind;
use gix::prelude::Write as _;
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::refs::Target;
use gix::ObjectId;

use crate::format::{Block, Rejection, Source};
use crate::layout::{snapshot_ref, SNAPSHOTS};
use crate::Error;

/// The identity snapshot commits are written as, author and committer: a
/// snapshot's commit depends on the snapshot alone, so the same snapshot
/// added anywhere is the same commit.
const TOOL: &str = "git-seriate";

/// One item of a snapshot, as the extractor found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub bytes: Vec<u8>,
    /// Record entries about this item (spec 0480 S8): each is written as
    /// `<key> <stored path>`, the path the store places it under.
    pub record: Vec<(String, String)>,
    /// Where the extractor found it, its origin (spec 0494 S1): written as
    /// its path's leaf in `contents`, which makes the block format 2.
    pub origin: Option<String>,
}

/// A snapshot to add.
#[derive(Debug, Clone)]
pub struct NewSnapshot {
    pub source: Source,
    pub items: Vec<Item>,
    /// The extractor's refusals; the store appends its own.
    pub rejected: Vec<Rejection>,
    /// The extractor's record, shown, never interpreted.
    pub record: Vec<(String, String)>,
}

/// What [`add`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    /// A new snapshot.
    Written {
        identity: String,
        rejected: Vec<Rejection>,
    },
    /// The store already holds this very commit.
    Unchanged { identity: String },
    /// No item and no record: nothing to keep.
    Nothing,
}

/// A snapshot, or a record-only one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub identity: String,
    pub commit: ObjectId,
    pub block: Block,
}

/// Where [`add_to`] writes a snapshot: a repository, or an archive being
/// built (spec 0466 S1).
pub enum Sink<'a> {
    Repo(&'a gix::Repository),
    Archive(&'a mut crate::archive::Archive),
}

impl Sink<'_> {
    fn write(&mut self, object: &impl gix::objs::WriteTo) -> Result<ObjectId, Error> {
        match self {
            Sink::Repo(repo) => Ok(repo.write_object(object).map_err(Error::git)?.detach()),
            Sink::Archive(archive) => {
                let mut data = Vec::new();
                object.write_to(&mut data).map_err(Error::git)?;
                archive.write(object.kind(), &data)
            }
        }
    }

    /// A snapshot that `identity` can't sit beside: one whose identity is a
    /// directory of `identity`, or under it. Git keeps a ref and a
    /// directory of refs apart (`a` and `a/b` can't both be refs).
    fn clash(&self, identity: &str) -> Result<Option<String>, Error> {
        let clashes = |other: &str| {
            other != identity
                && (identity.starts_with(&format!("{other}/"))
                    || other.starts_with(&format!("{identity}/")))
        };
        match self {
            Sink::Repo(repo) => Ok(snapshot_refs(repo)?
                .into_iter()
                .map(|(name, _)| name)
                .find(|n| clashes(n))),
            Sink::Archive(archive) => Ok(archive
                .snapshot_identities()
                .find(|n| clashes(n))
                .map(str::to_string)),
        }
    }

    fn tip(
        &self,
        identity: &str,
        ref_name: &gix::refs::FullName,
    ) -> Result<Option<ObjectId>, Error> {
        match self {
            Sink::Repo(repo) => tip(repo, ref_name),
            Sink::Archive(archive) => Ok(archive.snapshot(identity)),
        }
    }

    fn set_snapshot(
        &mut self,
        identity: &str,
        ref_name: &gix::refs::FullName,
        id: ObjectId,
    ) -> Result<(), Error> {
        match self {
            Sink::Repo(repo) => set_ref(repo, ref_name, PreviousValue::MustNotExist, id),
            Sink::Archive(archive) => {
                archive.set_snapshot(identity, id);
                Ok(())
            }
        }
    }
}

/// Add a snapshot, at `refs/seriate/snapshots/<identity>` (spec 0452 S3,
/// 0499 S2).
pub fn add(repo: &gix::Repository, snapshot: NewSnapshot) -> Result<Added, Error> {
    add_to(&mut Sink::Repo(repo), snapshot)
}

/// [`add`], into a repository or an archive.
pub fn add_to(sink: &mut Sink, snapshot: NewSnapshot) -> Result<Added, Error> {
    let identity = snapshot.source.identity.clone();
    let ref_name = snapshot_ref(&identity)?;
    let mut rejected = snapshot.rejected;
    let Placed {
        files: placed,
        record: item_record,
        evidence,
    } = place_items(snapshot.items, &mut rejected);
    let mut record = snapshot.record;
    record.extend(item_record);
    if placed.is_empty() && record.is_empty() {
        return Ok(Added::Nothing);
    }

    let mut blobs = Vec::with_capacity(placed.len());
    for (path, bytes) in &placed {
        let blob = sink.write(&gix::objs::BlobRef { data: bytes })?;
        blobs.push((path.as_str(), blob));
    }
    let tree = write_tree(sink, &blobs)?;

    let block = Block {
        source: snapshot.source,
        contents: (!placed.is_empty()).then(|| placed.iter().map(|(p, _)| p.clone()).collect()),
        evidence,
        rejected: rejected.clone(),
        record,
        canon_rules: Vec::new(),
        renderer: Vec::new(),
        reduced: false,
    };
    let commit = gix::objs::Commit {
        tree,
        parents: Default::default(),
        author: signature(block.source.order_hint.map_or(0, |h| h as i64)),
        committer: signature(block.source.build_time),
        encoding: None,
        message: block.message().into(),
        extra_headers: Vec::new(),
    };
    let id = commit_id(&commit)?;
    // Before reading the snapshot's ref, which git can't even look up under a
    // name that is a ref already.
    if let Some(other) = sink.clash(&identity)? {
        return Err(Error::Refused(format!(
            "snapshot {identity:?} can't sit beside snapshot {other:?}: git keeps a ref and a \
             directory of refs apart; give one of them another identity"
        )));
    }
    if let Some(existing) = sink.tip(&identity, &ref_name)? {
        return if existing == id {
            Ok(Added::Unchanged { identity })
        } else {
            Err(Error::Refused(format!(
                "snapshot {identity:?} exists with another commit ({existing}); this snapshot \
                 would be {id}"
            )))
        };
    }
    let written = sink.write(&commit)?;
    debug_assert_eq!(written, id);
    sink.set_snapshot(&identity, &ref_name, written)?;
    Ok(Added::Written { identity, rejected })
}

/// Every snapshot and record-only one, by identity (spec 0452 S3): a
/// parentless commit with a block under `refs/seriate/snapshots/`, at the
/// block's identity (spec 0499 S2).
pub fn list(repo: &gix::Repository) -> Result<Vec<Listed>, Error> {
    list_with(repo, None)
}

/// [`list`], reading older blocks with `legacy`.
pub fn list_with(
    repo: &gix::Repository,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Vec<Listed>, Error> {
    listed_under(repo, SNAPSHOTS, legacy)
}

/// The snapshots of the former layout, branches named as their identity
/// (spec 0452 S3, before spec 0499): what `migrate` moves.
pub fn former_snapshots(
    repo: &gix::Repository,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Vec<Listed>, Error> {
    listed_under(repo, "refs/heads/", legacy)
}

/// A repository still holding snapshots as branches, refused, naming
/// `migrate` (spec 0499 S4).
pub fn refuse_former_layout(
    repo: &gix::Repository,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<(), Error> {
    let former = former_snapshots(repo, legacy)?;
    match former.first() {
        None => Ok(()),
        Some(first) => Err(Error::Refused(format!(
            "{} snapshot(s) are still branches, as before spec 0499 ({}{}): run \
             `git seriate migrate` first",
            former.len(),
            first.identity,
            if former.len() > 1 { ", …" } else { "" }
        ))),
    }
}

/// The refs under `prefix`, by their name past it, and their targets.
fn refs_under(repo: &gix::Repository, prefix: &str) -> Result<Vec<(String, ObjectId)>, Error> {
    let mut out = Vec::new();
    let refs = repo.references().map_err(Error::git)?;
    for r in refs.prefixed(prefix).map_err(Error::git)? {
        let mut r = r.map_err(Error::git)?;
        let full = r.name().as_bstr().to_str_lossy().into_owned();
        let name = full.strip_prefix(prefix).unwrap_or(&full).to_string();
        out.push((name, r.peel_to_id().map_err(Error::git)?.detach()));
    }
    Ok(out)
}

/// The snapshot refs, by identity, and their commits.
pub(crate) fn snapshot_refs(repo: &gix::Repository) -> Result<Vec<(String, ObjectId)>, Error> {
    refs_under(repo, SNAPSHOTS)
}

/// The parentless commits with a block under `prefix`, at their block's
/// identity, by identity.
fn listed_under(
    repo: &gix::Repository,
    prefix: &str,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Vec<Listed>, Error> {
    let mut out = Vec::new();
    for (name, commit_id) in refs_under(repo, prefix)? {
        let commit = repo.find_commit(commit_id).map_err(Error::git)?;
        let decoded = commit.decode().map_err(Error::git)?;
        if decoded.parents().next().is_some() {
            continue;
        }
        let message = decoded.message.to_str_lossy();
        let Ok(Some(block)) = Block::read_with(&message, legacy) else {
            continue;
        };
        if block.source.identity == name {
            out.push(Listed {
                identity: name,
                commit: commit_id,
                block,
            });
        }
    }
    out.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(out)
}

/// Delete the snapshots matching `glob` (all when `None`), or the
/// record-only ones with `record_only` (`prune`, spec 0497 S4, formerly
/// `drop`). Returns the identities deleted.
pub fn prune(
    repo: &gix::Repository,
    glob: Option<&str>,
    record_only: bool,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Vec<String>, Error> {
    let mut deleted = Vec::new();
    for listed in list_with(repo, legacy)? {
        if listed.block.contents.is_none() != record_only {
            continue;
        }
        if glob.is_some_and(|g| !crate::plugins::glob_matches(g, &listed.identity)) {
            continue;
        }
        delete_snapshot(repo, &listed.identity, listed.commit)?;
        deleted.push(listed.identity);
    }
    Ok(deleted)
}

/// Delete the snapshot `identity`, whose ref must point at `commit`.
pub(crate) fn delete_snapshot(
    repo: &gix::Repository,
    identity: &str,
    commit: ObjectId,
) -> Result<(), Error> {
    delete_ref(repo, snapshot_ref(identity)?, commit)
}

/// Delete the ref `name`, which must point at `commit`.
pub(crate) fn delete_ref(
    repo: &gix::Repository,
    name: gix::refs::FullName,
    commit: ObjectId,
) -> Result<(), Error> {
    repo.edit_references_as(
        [RefEdit {
            change: Change::Delete {
                expected: PreviousValue::MustExistAndMatch(Target::Object(commit)),
                log: RefLog::AndReference,
            },
            name,
            deref: false,
        }],
        None,
    )
    .map_err(Error::git)?;
    Ok(())
}

/// What [`import`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    pub copied: Vec<String>,
    /// Already here, as the same commit.
    pub present: Vec<String>,
}

/// Copy the snapshots and record-only ones of the repository at `from`
/// (spec 0452 S3), its snapshots under either layout (spec 0499 S4): an
/// archive or a repository of before 0499 imports into this one's. A
/// snapshot here with another commit is refused, before anything is copied.
pub fn import(
    repo: &gix::Repository,
    from: &Path,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Imported, Error> {
    let source = gix::open(from).map_err(Error::git)?;
    let mut theirs = list_with(&source, legacy)?;
    theirs.extend(former_snapshots(&source, legacy)?);
    let mut report = Imported::default();
    let mut todo = Vec::new();
    for listed in theirs {
        let ref_name = snapshot_ref(&listed.identity)?;
        match tip(repo, &ref_name)? {
            Some(ours) if ours == listed.commit => report.present.push(listed.identity),
            Some(ours) => {
                return Err(Error::Refused(format!(
                    "snapshot {:?} exists here as {ours}, there as {}",
                    listed.identity, listed.commit
                )))
            }
            None => todo.push((listed, ref_name)),
        }
    }
    for (listed, ref_name) in todo {
        copy_object(&source, repo, listed.commit)?;
        set_ref(repo, &ref_name, PreviousValue::MustNotExist, listed.commit)?;
        report.copied.push(listed.identity);
    }
    Ok(report)
}

// ── Path rules ────────────────────────────────────────────────────────────────

/// What [`place_items`] decided.
struct Placed {
    /// The items' final paths and bytes, by path (spec 0452 S3 steps 1–2).
    files: Vec<(String, Vec<u8>)>,
    /// Their record entries, keyed by those paths (spec 0480 S8), in item
    /// order.
    record: Vec<(String, String)>,
    /// Their origins, by those paths (spec 0494 S1); the same bytes found
    /// again join theirs to the kept copy's, `; `-separated, in item order.
    evidence: BTreeMap<String, String>,
}

/// Places the items. An item dropped as the same bytes again gives its
/// record entries to the copy kept; two values for one key are joined by
/// `; `, as a record's keys are unique.
fn place_items(items: Vec<Item>, rejected: &mut Vec<Rejection>) -> Placed {
    let mut record: Vec<(String, String)> = Vec::new();
    let mut note = |entries: Vec<(String, String)>, path: &str| {
        for (key, value) in entries {
            let key = format!("{key} {path}");
            match record.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => {
                    v.push_str("; ");
                    v.push_str(&value);
                }
                None => record.push((key, value)),
            }
        }
    };
    let mut evidence: BTreeMap<String, String> = BTreeMap::new();
    let mut found_at = |origin: Option<String>, path: &str| {
        if let Some(origin) = origin {
            evidence
                .entry(path.to_string())
                .and_modify(|e| {
                    e.push_str("; ");
                    e.push_str(&origin);
                })
                .or_insert(origin);
        }
    };
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let mut dirs: HashSet<String> = HashSet::new();
    // Per path as given, the paths its items were written under.
    let mut variants: HashMap<String, Vec<String>> = HashMap::new();
    for item in items {
        if let Err(reason) = check_path(&item.path) {
            rejected.push(Rejection {
                name: item.path,
                reason,
            });
            continue;
        }
        let ancestors: Vec<&str> = item
            .path
            .match_indices('/')
            .map(|(i, _)| &item.path[..i])
            .collect();
        if ancestors.iter().any(|a| files.contains_key(*a)) {
            rejected.push(Rejection {
                name: item.path,
                reason: "under-file".into(),
            });
            continue;
        }
        let seen = variants.entry(item.path.clone()).or_default();
        if let Some(kept) = seen.iter().find(|p| files[*p] == item.bytes) {
            note(item.record, kept); // the same item again
            found_at(item.origin, kept);
            continue;
        }
        let taken = |p: &str| files.contains_key(p) || dirs.contains(p);
        let path = if taken(&item.path) {
            let (stem, ext) = split_extension(&item.path);
            (2..)
                .map(|n| format!("{stem}~{n}{ext}"))
                .find(|p| !taken(p))
                .unwrap()
        } else {
            item.path.clone()
        };
        for a in &ancestors {
            dirs.insert((*a).to_string());
        }
        seen.push(path.clone());
        note(item.record, &path);
        found_at(item.origin, &path);
        files.insert(path, item.bytes);
    }
    let mut placed: Vec<(String, Vec<u8>)> = files.into_iter().collect();
    placed.sort_by(|a, b| a.0.cmp(&b.0));
    Placed {
        files: placed,
        record,
        evidence,
    }
}

/// `("dir/bar~2", ".txt")` for `"dir/bar~2.txt"`: the extension is the last
/// component's, after its last dot, unless that dot starts the name.
fn split_extension(path: &str) -> (&str, &str) {
    let name_start = path.rfind('/').map_or(0, |i| i + 1);
    match path[name_start..].rfind('.') {
        Some(i) if i > 0 => path.split_at(name_start + i),
        _ => (path, ""),
    }
}

/// A path git and every checkout can hold: no empty, `.` or `..`
/// component, and none git's `core.protectHFS` and `core.protectNTFS`
/// refuse (`.git` in any of its forms). Windows' reserved names stay
/// allowed: they are legal in git on other systems.
fn check_path(path: &str) -> Result<(), String> {
    let options = gix::validate::path::component::Options {
        protect_windows: false,
        protect_hfs: true,
        protect_ntfs: true,
    };
    for component in path.split('/') {
        match component {
            "" => return Err("empty-component".into()),
            "." => return Err("dot".into()),
            ".." => return Err("dotdot".into()),
            _ => {}
        }
        if component.contains('\0') {
            return Err("nul".into());
        }
        if let Err(e) = gix::validate::path::component(BStr::new(component), None, options) {
            return Err(format!("invalid: {e}"));
        }
    }
    Ok(())
}

/// The tree of `items` (paths placed, with their blobs), written, every
/// level before its parent. Each level passes what `git fsck --strict`
/// checks of entries: sorted as git sorts, no duplicate. A failure is a bug
/// here, reported, never a written tree.
fn write_tree(sink: &mut Sink, items: &[(&str, ObjectId)]) -> Result<ObjectId, Error> {
    use gix::objs::tree::Entry;
    let mut entries = Vec::new();
    let mut dirs: std::collections::BTreeMap<&str, Vec<(&str, ObjectId)>> = Default::default();
    for &(path, oid) in items {
        match path.split_once('/') {
            None => entries.push(Entry {
                mode: EntryKind::Blob.into(),
                filename: path.into(),
                oid,
            }),
            Some((dir, rest)) => dirs.entry(dir).or_default().push((rest, oid)),
        }
    }
    for (dir, sub) in dirs {
        entries.push(Entry {
            mode: EntryKind::Tree.into(),
            filename: dir.into(),
            oid: write_tree(sink, &sub)?,
        });
    }
    entries.sort();
    for pair in entries.windows(2) {
        if pair[0] >= pair[1] {
            return Err(Error::Bug(format!(
                "a tree: {:?} not before {:?}",
                pair[0].filename, pair[1].filename
            )));
        }
    }
    sink.write(&gix::objs::Tree { entries })
}

// ── Git helpers ───────────────────────────────────────────────────────────────

/// `refs/heads/<branch>`, refused when git would refuse the name.
pub fn branch_ref(branch: &str) -> Result<gix::refs::FullName, Error> {
    format!("refs/heads/{branch}")
        .try_into()
        .map_err(|e| Error::Refused(format!("{branch:?} is not a valid branch name: {e}")))
}

fn signature(seconds: i64) -> gix::actor::Signature {
    gix::actor::Signature {
        name: TOOL.into(),
        email: "".into(),
        time: gix::date::Time::new(seconds, 0),
    }
}

/// The id `commit` will have, without writing it.
fn commit_id(commit: &gix::objs::Commit) -> Result<ObjectId, Error> {
    use gix::objs::WriteTo;
    let mut buf = Vec::new();
    commit.write_to(&mut buf).map_err(Error::git)?;
    gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::object::Kind::Commit, &buf)
        .map_err(Error::git)
}

/// Where `name` points, if it exists.
pub fn tip(repo: &gix::Repository, name: &gix::refs::FullName) -> Result<Option<ObjectId>, Error> {
    match repo.try_find_reference(name.as_ref()).map_err(Error::git)? {
        None => Ok(None),
        Some(mut r) => Ok(Some(r.peel_to_id().map_err(Error::git)?.detach())),
    }
}

/// Point `name` at `id`, if its current value is `expected`; the reflog
/// entry is written as git-seriate, so that no user identity is needed.
pub fn set_ref(
    repo: &gix::Repository,
    name: &gix::refs::FullName,
    expected: PreviousValue,
    id: ObjectId,
) -> Result<(), Error> {
    let mut time = gix::date::parse::TimeBuf::default();
    let actor = signature(0);
    repo.edit_references_as(
        [RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: "git-seriate".into(),
                },
                expected,
                new: Target::Object(id),
            },
            name: name.clone(),
            deref: false,
        }],
        Some(actor.to_ref(&mut time)),
    )
    .map_err(Error::git)?;
    Ok(())
}

/// Copy `id` and everything it reaches from `from` into `to`, skipping
/// what `to` already has.
fn copy_object(from: &gix::Repository, to: &gix::Repository, id: ObjectId) -> Result<(), Error> {
    let mut stack = vec![id];
    while let Some(id) = stack.pop() {
        if to.has_object(id) {
            continue;
        }
        let object = from.find_object(id).map_err(Error::git)?;
        match object.kind {
            gix::object::Kind::Commit => {
                let commit = object.clone().into_commit();
                let decoded = commit.decode().map_err(Error::git)?;
                stack.push(decoded.tree());
                stack.extend(decoded.parents());
            }
            gix::object::Kind::Tree => {
                let tree = object.clone().into_tree();
                let decoded = tree.decode().map_err(Error::git)?;
                stack.extend(decoded.entries.iter().map(|e| e.oid.to_owned()));
            }
            _ => {}
        }
        let written = to
            .write_buf(object.kind, &object.data)
            .map_err(Error::git)?;
        debug_assert_eq!(written, id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(list: &[(&str, &str)]) -> Vec<Item> {
        list.iter()
            .map(|(p, b)| Item {
                path: p.to_string(),
                bytes: b.as_bytes().to_vec(),
                record: Vec::new(),
                origin: None,
            })
            .collect()
    }

    fn placed(list: &[(&str, &str)]) -> (Vec<String>, Vec<(String, String)>) {
        let mut rejected = Vec::new();
        let paths = place_items(items(list), &mut rejected)
            .files
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        (
            paths,
            rejected.into_iter().map(|r| (r.name, r.reason)).collect(),
        )
    }

    #[test]
    fn bad_paths_are_refused_and_the_rest_kept() {
        let (paths, rejected) = placed(&[
            ("ok.txt", "a"),
            ("x/../e.txt", "b"),
            ("./d.txt", "c"),
            ("/abs.txt", "d"),
            ("x//y.txt", "e"),
            ("x/.git/f.txt", "f"),
            ("x/.GIT/g.txt", "g"),
            ("x/git~1/h.txt", "h"),
        ]);
        assert_eq!(paths, ["ok.txt"]);
        let reasons: Vec<&str> = rejected.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(
            reasons[..4],
            ["dotdot", "dot", "empty-component", "empty-component"]
        );
        assert!(
            reasons[4..].iter().all(|r| r.starts_with("invalid")),
            "{reasons:?}"
        );
    }

    #[test]
    fn same_path_same_bytes_is_one_item_different_bytes_is_renamed() {
        let (paths, _) = placed(&[
            ("t/bar.txt", "1"),
            ("t/bar.txt", "1"),
            ("t/bar.txt", "2"),
            ("t/bar.txt", "2"),
        ]);
        assert_eq!(paths, ["t/bar.txt", "t/bar~2.txt"]);
    }

    #[test]
    fn item_records_are_keyed_by_the_stored_path() {
        // Spec 0480 S8: a `~2` variant's entry names `~2`, and a dropped
        // copy's entry joins the kept one's.
        let item = |bytes: &str, origin: &str| Item {
            path: "a.pb".into(),
            bytes: bytes.as_bytes().to_vec(),
            record: vec![("origin".into(), origin.into())],
            origin: None,
        };
        let mut rejected = Vec::new();
        let Placed { record, .. } = place_items(
            vec![item("1", "x"), item("2", "y"), item("1", "z")],
            &mut rejected,
        );
        assert_eq!(
            record,
            [
                ("origin a.pb".to_string(), "x; z".to_string()),
                ("origin a~2.pb".to_string(), "y".to_string()),
            ]
        );
    }

    #[test]
    fn origins_are_keyed_by_the_stored_path() {
        // Spec 0494 S1: as record entries are, but as evidence, its path's
        // leaf in `contents`; an item without an origin has none.
        let item = |bytes: &str, origin: Option<&str>| Item {
            path: "a.pb".into(),
            bytes: bytes.as_bytes().to_vec(),
            record: Vec::new(),
            origin: origin.map(str::to_string),
        };
        let mut rejected = Vec::new();
        let Placed {
            evidence, record, ..
        } = place_items(
            vec![
                item("1", Some("0..1")),
                item("2", Some("gzip@9!0..1")),
                item("1", Some("5..6")),
                item("3", None),
            ],
            &mut rejected,
        );
        assert!(record.is_empty());
        assert_eq!(
            evidence.into_iter().collect::<Vec<_>>(),
            [
                ("a.pb".to_string(), "0..1; 5..6".to_string()),
                ("a~2.pb".to_string(), "gzip@9!0..1".to_string()),
            ]
        );
    }

    #[test]
    fn a_genuine_tilde_name_takes_part() {
        // After a generated bar~2.txt: renamed in turn.
        let (paths, _) = placed(&[("bar.txt", "1"), ("bar.txt", "2"), ("bar~2.txt", "3")]);
        assert_eq!(paths, ["bar.txt", "bar~2.txt", "bar~2~2.txt"]);
        // Before it: the generated one skips it.
        let (paths, _) = placed(&[("bar~2.txt", "3"), ("bar.txt", "1"), ("bar.txt", "2")]);
        assert_eq!(paths, ["bar.txt", "bar~2.txt", "bar~3.txt"]);
    }

    #[test]
    fn files_and_directories_of_one_name() {
        let (paths, rejected) = placed(&[("k.txt", "1"), ("k.txt/m.txt", "2")]);
        assert_eq!(paths, ["k.txt"]);
        assert_eq!(
            rejected,
            [("k.txt/m.txt".to_string(), "under-file".to_string())]
        );
        let (paths, _) = placed(&[("k.txt/m.txt", "2"), ("k.txt", "1")]);
        assert_eq!(paths, ["k.txt/m.txt", "k~2.txt"]);
    }

    #[test]
    fn extensions() {
        assert_eq!(split_extension("d/bar~2.txt"), ("d/bar~2", ".txt"));
        assert_eq!(split_extension("d.x/README"), ("d.x/README", ""));
        assert_eq!(split_extension(".hidden"), (".hidden", ""));
    }
}
