// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What the contents suggest of producers (spec 0499 S6): reported, with
//! the remedy, never acted on. A lineage is stated by whoever knows the
//! producer; `advise` only points at what looks like another producer's
//! output, or like none:
//!
//! - a snapshot's copy straddled in its lineage (another version of the
//!   file before it and after it) by a copy it equals once options and
//!   `json_name`s are set aside: possibly another producer's, which drops
//!   them;
//! - a snapshot of a stated lineage every copy of which is one of
//!   `default`'s: possibly no other producer's.

use std::collections::{BTreeMap, HashMap, HashSet};

use git_seriate::cli::Kit;
use git_seriate::format::Block;
use git_seriate::layout::DEFAULT_LINEAGE;
use git_seriate::Error;
use gix::bstr::ByteSlice;
use gix::ObjectId;
use prototext_upb::{DynamicMessage, Kind, MessageMut, Pool, Value};

/// A snapshot the contents suggest is in the wrong lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suspect {
    pub identity: String,
    /// Why, in a few words.
    pub reason: String,
    /// The command that would act on it, if the user agrees.
    pub remedy: String,
    pub paths: Vec<String>,
}

/// A snapshot commit of a lineage: its identity, its tree, its paths.
struct Snap {
    identity: String,
    tree: ObjectId,
    paths: Vec<String>,
}

/// The snapshot commits of `branch`, oldest first.
fn snapshots(repo: &gix::Repository, kit: &Kit, branch: &str) -> Result<Vec<Snap>, Error> {
    let Some(tip) = git_seriate::store::tip(repo, &git_seriate::store::branch_ref(branch)?)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut current = Some(tip);
    while let Some(id) = current {
        let commit = repo
            .find_commit(id)
            .map_err(|e| Error::Git(e.to_string()))?;
        let decoded = commit.decode().map_err(|e| Error::Git(e.to_string()))?;
        if let Ok(Some(block)) =
            Block::read_with(&decoded.message.to_str_lossy(), kit.legacy.as_deref())
        {
            if let Some(paths) = block.contents {
                out.push(Snap {
                    identity: block.source.identity,
                    tree: decoded.tree(),
                    paths,
                });
            }
        }
        current = decoded.parents().next();
    }
    out.reverse();
    Ok(out)
}

/// The blob at `path` in `tree`, if any.
fn blob_at(repo: &gix::Repository, tree: ObjectId, path: &str) -> Result<Option<ObjectId>, Error> {
    let tree = repo
        .find_tree(tree)
        .map_err(|e| Error::Git(e.to_string()))?;
    let entry = tree
        .lookup_entry_by_path(path)
        .map_err(|e| Error::Git(e.to_string()))?;
    Ok(entry.filter(|e| e.mode().is_blob()).map(|e| e.object_id()))
}

/// A snapshot's own copies.
struct Copies {
    identity: String,
    /// (path, blob), in the snapshot's order.
    own: Vec<(String, ObjectId)>,
}

/// Each snapshot's copies, in order.
fn copies(repo: &gix::Repository, snaps: &[Snap]) -> Result<Vec<Copies>, Error> {
    let mut out = Vec::with_capacity(snaps.len());
    for s in snaps {
        let mut own = Vec::new();
        for p in &s.paths {
            if let Some(b) = blob_at(repo, s.tree, p)? {
                own.push((p.clone(), b));
            }
        }
        out.push(Copies {
            identity: s.identity.clone(),
            own,
        });
    }
    Ok(out)
}

/// `fdp` with every `options` and `json_name` cleared, at any depth, encoded
/// deterministically; `None` if it doesn't decode.
fn without_options(fdp: &[u8]) -> Option<Vec<u8>> {
    let def = Pool::vendored().get_message_by_name("google.protobuf.FileDescriptorProto")?;
    let mut msg = DynamicMessage::decode(&def, fdp).ok()?;
    strip(&mut msg.as_mut());
    msg.as_ref().encode_deterministic().ok()
}

fn strip(msg: &mut MessageMut) {
    /// What to do with a set field, read before anything is changed.
    enum Act {
        Clear,
        List(usize),
        One,
    }
    let acts: Vec<(prototext_upb::Field, Act)> = msg
        .as_ref()
        .set_fields()
        .into_iter()
        .filter_map(|(field, value)| {
            if field.name() == "options" || field.name() == "json_name" {
                return Some((field, Act::Clear));
            }
            if !matches!(field.kind(), Kind::Message(_)) {
                return None;
            }
            match value {
                Value::List(list) => Some((field, Act::List(list.len()))),
                Value::Message(Some(_)) => Some((field, Act::One)),
                _ => None,
            }
        })
        .collect();
    for (field, act) in acts {
        match act {
            Act::Clear => msg.clear(&field),
            Act::List(n) => {
                for i in 0..n {
                    if let Ok(mut child) = msg.list_message_mut(&field, i) {
                        strip(&mut child);
                    }
                }
            }
            Act::One => {
                if let Ok(mut child) = msg.message_mut(&field) {
                    strip(&mut child);
                }
            }
        }
    }
}

/// The suspects of the lineage branch `branch` of the history `history`
/// (S6).
pub fn suspects(
    repo: &gix::Repository,
    kit: &Kit,
    history: &str,
    branch: &str,
) -> Result<Vec<Suspect>, Error> {
    let lineage = branch
        .strip_prefix(&format!("{history}/"))
        .unwrap_or(DEFAULT_LINEAGE);
    let snaps = snapshots(repo, kit, branch)?;
    let copies = copies(repo, &snaps)?;
    let mut out = Vec::new();

    // Straddled copies equal to their straddler but for the options.
    let mut seq: HashMap<&str, Vec<(usize, ObjectId)>> = HashMap::new();
    for (i, c) in copies.iter().enumerate() {
        for (p, b) in &c.own {
            seq.entry(p.as_str()).or_default().push((i, *b));
        }
    }
    let mut stripped: HashMap<ObjectId, Option<Vec<u8>>> = HashMap::new();
    let mut strip_of = |path: &str, blob: ObjectId| -> Result<Option<Vec<u8>>, Error> {
        if let Some(s) = stripped.get(&blob) {
            return Ok(s.clone());
        }
        let data = repo
            .find_blob(blob)
            .map_err(|e| Error::Git(e.to_string()))?
            .data
            .clone();
        let s = kit
            .renderer
            .parse(path, &data)
            .ok()
            .and_then(|fdp| without_options(&fdp));
        stripped.insert(blob, s.clone());
        Ok(s)
    };
    for (i, Copies { identity, own }) in copies.iter().enumerate() {
        let mut paths = Vec::new();
        for (p, b) in own {
            let versions = &seq[p.as_str()];
            let mut span: BTreeMap<ObjectId, (usize, usize)> = BTreeMap::new();
            for &(j, v) in versions {
                let e = span.entry(v).or_insert((j, j));
                e.0 = e.0.min(j);
                e.1 = e.1.max(j);
            }
            let straddlers: Vec<ObjectId> = span
                .iter()
                .filter(|(v, (first, last))| *v != b && *first < i && i < *last)
                .map(|(v, _)| *v)
                .collect();
            for a in straddlers {
                let mine = strip_of(p, *b)?;
                if mine.is_some() && mine == strip_of(p, a)? {
                    paths.push(p.clone());
                    break;
                }
            }
        }
        if !paths.is_empty() {
            out.push(Suspect {
                identity: identity.clone(),
                reason: format!(
                    "straddled in {branch} by copies it equals but for options and json_names: \
                     possibly another producer's"
                ),
                remedy: format!(
                    "git seriate lineage set NAME '{identity}', then settle, if a producer of \
                     its own made it"
                ),
                paths,
            });
        }
    }

    // A stated lineage's snapshots whose every copy is one of default's.
    if lineage != DEFAULT_LINEAGE {
        let default_branch = git_seriate::layout::lineage_branch(history, DEFAULT_LINEAGE);
        let theirs = copies_of(repo, kit, &default_branch)?;
        for Copies { identity, own } in &copies {
            if !own.is_empty() && own.iter().all(|(p, b)| theirs.contains(&(p.clone(), *b))) {
                out.push(Suspect {
                    identity: identity.clone(),
                    reason: format!(
                        "in {branch}, every copy one of {default_branch}'s: possibly no other \
                         producer's"
                    ),
                    remedy: format!("git seriate lineage unset '{identity}', then settle"),
                    paths: own.iter().map(|(p, _)| p.clone()).collect(),
                });
            }
        }
    }
    Ok(out)
}

/// Every (path, blob) the snapshots of `branch` hold.
fn copies_of(
    repo: &gix::Repository,
    kit: &Kit,
    branch: &str,
) -> Result<HashSet<(String, ObjectId)>, Error> {
    let snaps = snapshots(repo, kit, branch)?;
    Ok(copies(repo, &snaps)?
        .into_iter()
        .flat_map(|c| c.own)
        .collect())
}
