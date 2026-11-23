// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Snapshot branches: add, list, drop, import (spec 0452 S3).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gix::bstr::{BStr, ByteSlice};
use gix::object::tree::EntryKind;
use gix::prelude::Write as _;
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::refs::Target;
use gix::ObjectId;

use crate::format::{Block, Rejection, Source};
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
    /// A new branch.
    Written {
        branch: String,
        rejected: Vec<Rejection>,
    },
    /// The branch already holds this very commit.
    Unchanged { branch: String },
    /// No item and no record: nothing to keep.
    Nothing,
}

/// A snapshot or record-only branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub branch: String,
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

    fn tip(&self, branch: &str, ref_name: &gix::refs::FullName) -> Result<Option<ObjectId>, Error> {
        match self {
            Sink::Repo(repo) => tip(repo, ref_name),
            Sink::Archive(archive) => Ok(archive.branch(branch)),
        }
    }

    fn set_branch(
        &mut self,
        branch: &str,
        ref_name: &gix::refs::FullName,
        id: ObjectId,
    ) -> Result<(), Error> {
        match self {
            Sink::Repo(repo) => set_ref(repo, ref_name, PreviousValue::MustNotExist, id),
            Sink::Archive(archive) => {
                archive.set_branch(branch, id);
                Ok(())
            }
        }
    }
}

/// Add a snapshot as its branch (spec 0452 S3).
pub fn add(repo: &gix::Repository, snapshot: NewSnapshot) -> Result<Added, Error> {
    add_to(&mut Sink::Repo(repo), snapshot)
}

/// [`add`], into a repository or an archive.
pub fn add_to(sink: &mut Sink, snapshot: NewSnapshot) -> Result<Added, Error> {
    let branch = snapshot.source.identity.clone();
    let ref_name = branch_ref(&branch)?;
    let mut rejected = snapshot.rejected;
    let placed = place_items(snapshot.items, &mut rejected);
    if placed.is_empty() && snapshot.record.is_empty() {
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
        rejected: rejected.clone(),
        record: snapshot.record,
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
    if let Some(existing) = sink.tip(&branch, &ref_name)? {
        return if existing == id {
            Ok(Added::Unchanged { branch })
        } else {
            Err(Error::Refused(format!(
                "branch {branch:?} exists with another commit ({existing}); this snapshot \
                 would be {id}"
            )))
        };
    }
    let written = sink.write(&commit)?;
    debug_assert_eq!(written, id);
    sink.set_branch(&branch, &ref_name, written)?;
    Ok(Added::Written { branch, rejected })
}

/// Every snapshot and record-only branch, by name (spec 0452 S3): a
/// parentless tip with a block, named as the block's identity.
pub fn list(repo: &gix::Repository) -> Result<Vec<Listed>, Error> {
    list_with(repo, None)
}

/// [`list`], reading older blocks with `legacy`.
pub fn list_with(
    repo: &gix::Repository,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Vec<Listed>, Error> {
    let mut out = Vec::new();
    let refs = repo.references().map_err(Error::git)?;
    for r in refs.local_branches().map_err(Error::git)? {
        let mut r = r.map_err(Error::git)?;
        let name = r.name().shorten().to_string();
        let commit_id = r.peel_to_id().map_err(Error::git)?.detach();
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
                branch: name,
                commit: commit_id,
                block,
            });
        }
    }
    out.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(out)
}

/// Delete snapshot branches matching `glob` (all when `None`), or the
/// record-only ones with `record_only`. Returns the branches deleted.
pub fn drop(
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
        if glob.is_some_and(|g| !crate::plugins::glob_matches(g, &listed.branch)) {
            continue;
        }
        delete_branch(repo, &listed.branch, listed.commit)?;
        deleted.push(listed.branch);
    }
    Ok(deleted)
}

/// Delete the branch `name`, which must point at `commit`.
pub(crate) fn delete_branch(
    repo: &gix::Repository,
    name: &str,
    commit: ObjectId,
) -> Result<(), Error> {
    repo.edit_references_as(
        [RefEdit {
            change: Change::Delete {
                expected: PreviousValue::MustExistAndMatch(Target::Object(commit)),
                log: RefLog::AndReference,
            },
            name: branch_ref(name)?,
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

/// Copy the snapshot and record-only branches of the repository at `from`
/// (spec 0452 S3). A branch here with another commit is refused, before
/// anything is copied.
pub fn import(
    repo: &gix::Repository,
    from: &Path,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Imported, Error> {
    let source = gix::open(from).map_err(Error::git)?;
    let theirs = list_with(&source, legacy)?;
    let mut report = Imported::default();
    let mut todo = Vec::new();
    for listed in theirs {
        let ref_name = branch_ref(&listed.branch)?;
        match tip(repo, &ref_name)? {
            Some(ours) if ours == listed.commit => report.present.push(listed.branch),
            Some(ours) => {
                return Err(Error::Refused(format!(
                    "branch {:?} exists here as {ours}, there as {}",
                    listed.branch, listed.commit
                )))
            }
            None => todo.push((listed, ref_name)),
        }
    }
    for (listed, ref_name) in todo {
        copy_object(&source, repo, listed.commit)?;
        set_ref(repo, &ref_name, PreviousValue::MustNotExist, listed.commit)?;
        report.copied.push(listed.branch);
    }
    Ok(report)
}

// ── Path rules ────────────────────────────────────────────────────────────────

/// The items' final paths and bytes, by path (spec 0452 S3 steps 1–2).
fn place_items(items: Vec<Item>, rejected: &mut Vec<Rejection>) -> Vec<(String, Vec<u8>)> {
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
        if seen.iter().any(|p| files[p] == item.bytes) {
            continue; // the same item again
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
        files.insert(path, item.bytes);
    }
    let mut placed: Vec<(String, Vec<u8>)> = files.into_iter().collect();
    placed.sort_by(|a, b| a.0.cmp(&b.0));
    placed
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
            })
            .collect()
    }

    fn placed(list: &[(&str, &str)]) -> (Vec<String>, Vec<(String, String)>) {
        let mut rejected = Vec::new();
        let paths = place_items(items(list), &mut rejected)
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
