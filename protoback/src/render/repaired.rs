// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0473 S2: the descriptors `--repair-skew` repaired, as their text
//! has them, for the pool and the schema database: the imports added; the
//! fields, methods and extensions commented out, removed, a oneof left with
//! no field removed with them.

use std::collections::{BTreeMap, BTreeSet};

use prototext_upb::descriptor_access::{
    DescriptorProtoMut, FileDescriptorProto, FileDescriptorProtoMut, ServiceDescriptorProtoMut,
};
use prototext_upb::DynamicMessage;

use crate::db::Database;
use crate::graph::Kind;

use super::repair::Repairs;
use super::Shared;

/// What to remove from one file: by owner (a path of message names, or a
/// service's name), the names of its members to drop.
#[derive(Default)]
struct Edits {
    imports: Vec<String>,
    /// (message path, field or extension name).
    message_members: BTreeMap<Vec<String>, BTreeSet<String>>,
    /// (message path, map entry name).
    message_nested: BTreeMap<Vec<String>, BTreeSet<String>>,
    /// Top-level extensions.
    file_extensions: BTreeSet<String>,
    /// (service, method).
    methods: BTreeMap<String, BTreeSet<String>>,
}

/// `db`, the files `repairs` touch patched (S2); `None` when it touches
/// none.
pub fn repaired_db(shared: &Shared<'_>, repairs: &Repairs, db: &Database) -> Option<Database> {
    let mut edits: BTreeMap<String, Edits> = BTreeMap::new();
    for (file, deps) in &repairs.imports {
        edits
            .entry(file.clone())
            .or_default()
            .imports
            .extend(deps.keys().cloned());
    }
    // The nodes out: the orphans, and the map entries of map fields out.
    let mut out_nodes: BTreeSet<usize> = repairs.orphans.keys().copied().collect();
    for &node in repairs.orphans.keys() {
        if let Some(super::Def::Field(f)) = shared.def(node) {
            let entry = String::from_utf8_lossy(f.type_name()).into_owned();
            if let Some(m) = shared
                .graph
                .find(&format!("{}:{entry}", Kind::Message.prefix()))
            {
                if repairs.is_out(shared, m) {
                    out_nodes.insert(m);
                }
            }
        }
    }
    for &node in &out_nodes {
        let graph = &shared.graph.nodes;
        let name = last(&graph[node].prefix);
        // The owners, from the file down.
        let mut owners = Vec::new();
        let mut at = graph[node].parent;
        while let Some(o) = at {
            if graph[o].kind == Kind::File {
                break;
            }
            owners.push(o);
            at = graph[o].parent;
        }
        owners.reverse();
        let Some(file) = at else { continue };
        let file = graph[file]
            .fqdn
            .strip_prefix("file:")
            .unwrap_or_default()
            .to_string();
        let e = edits.entry(file).or_default();
        match owners.last().map(|&o| graph[o].kind) {
            None => {
                e.file_extensions.insert(name);
            }
            Some(Kind::Service) => {
                e.methods
                    .entry(last(&graph[owners[0]].prefix))
                    .or_default()
                    .insert(name);
            }
            Some(_) if graph[node].kind == Kind::Message => {
                let path = owners.iter().map(|&o| last(&graph[o].prefix)).collect();
                e.message_nested.entry(path).or_default().insert(name);
            }
            Some(_) => {
                let path = owners.iter().map(|&o| last(&graph[o].prefix)).collect();
                e.message_members.entry(path).or_default().insert(name);
            }
        }
    }
    if edits.is_empty() {
        return None;
    }
    let mut out = Database::default();
    for fdp in &db.files {
        let f = FileDescriptorProto(fdp.as_ref());
        let name = String::from_utf8_lossy(f.name()).into_owned();
        match edits.get(&name) {
            Some(e) => out.add(patched(fdp, e).unwrap_or_else(|| fdp.clone())),
            None => out.add(fdp.clone()),
        }
    }
    Some(out)
}

fn last(prefix: &str) -> String {
    prefix.rsplit('.').next().unwrap_or_default().to_string()
}

/// A copy of `fdp` with `e` applied.
fn patched(fdp: &DynamicMessage, e: &Edits) -> Option<DynamicMessage> {
    let bytes = fdp.encode().ok()?;
    let mut copy = DynamicMessage::decode(fdp.as_ref().descriptor(), &bytes).ok()?;
    {
        let mut w = FileDescriptorProtoMut(copy.as_mut());
        for dep in &e.imports {
            w.push_dependency(dep.as_bytes()).ok()?;
        }
        if !e.file_extensions.is_empty() {
            let kept = encoded_without(
                w.as_ref()
                    .extension()
                    .iter()
                    .map(|x| (x.name().to_vec(), x.0.encode().ok())),
                &e.file_extensions,
            )?;
            w.clear_extension();
            for b in &kept {
                w.push_extension().ok()?.0.merge_from(b).ok()?;
            }
        }
        for (path, members) in &e.message_members {
            let (first, rest) = path.split_first()?;
            let i = w
                .as_ref()
                .message_type()
                .iter()
                .position(|m| m.name() == first.as_bytes())?;
            let mut m = w.message_type_mut(i).ok()?;
            remove_at(&mut m, rest, members)?;
        }
        for (path, nested) in &e.message_nested {
            let (first, rest) = path.split_first()?;
            let i = w
                .as_ref()
                .message_type()
                .iter()
                .position(|m| m.name() == first.as_bytes())?;
            let mut m = w.message_type_mut(i).ok()?;
            remove_nested_at(&mut m, rest, nested)?;
        }
        // Every message of the file: a oneof left empty goes (S2).
        let count = w.as_ref().message_type().len();
        for i in 0..count {
            let mut m = w.message_type_mut(i).ok()?;
            drop_empty_oneofs(&mut m)?;
        }
        for (service, methods) in &e.methods {
            let count = w.as_ref().service().len();
            for i in 0..count {
                let mut s = w.service_mut(i).ok()?;
                if s.as_ref().name() != service.as_bytes() {
                    continue;
                }
                remove_methods(&mut s, methods)?;
            }
        }
    }
    Some(copy)
}

/// The encoded elements, each `(name, bytes)`, whose name isn't in `drop`.
fn encoded_without(
    items: impl Iterator<Item = (Vec<u8>, Option<Vec<u8>>)>,
    drop: &BTreeSet<String>,
) -> Option<Vec<Vec<u8>>> {
    items
        .filter(|(name, _)| !drop.contains(String::from_utf8_lossy(name).as_ref()))
        .map(|(_, bytes)| bytes)
        .collect()
}

/// `members` removed from the message at `path` below `m` (names, `m`'s
/// own when empty).
fn remove_at(
    m: &mut DescriptorProtoMut<'_>,
    path: &[String],
    members: &BTreeSet<String>,
) -> Option<()> {
    let Some((first, rest)) = path.split_first() else {
        return remove_members(m, members);
    };
    let j = m
        .as_ref()
        .nested_type()
        .iter()
        .position(|n| n.name() == first.as_bytes())?;
    let mut nested = m.nested_type_mut(j).ok()?;
    remove_at(&mut nested, rest, members)
}

/// The map entries `nested` removed from the message at `path` below `m`.
fn remove_nested_at(
    m: &mut DescriptorProtoMut<'_>,
    path: &[String],
    nested: &BTreeSet<String>,
) -> Option<()> {
    if let Some((first, rest)) = path.split_first() {
        let j = m
            .as_ref()
            .nested_type()
            .iter()
            .position(|n| n.name() == first.as_bytes())?;
        let mut inner = m.nested_type_mut(j).ok()?;
        return remove_nested_at(&mut inner, rest, nested);
    }
    let kept = encoded_without(
        m.as_ref()
            .nested_type()
            .iter()
            .map(|x| (x.name().to_vec(), x.0.encode().ok())),
        nested,
    )?;
    m.clear_nested_type();
    for b in &kept {
        m.push_nested_type().ok()?.0.merge_from(b).ok()?;
    }
    Some(())
}

/// `members` (fields or extensions) removed from `m`; a oneof left with no
/// field removed, the others' indices kept consistent.
fn remove_members(m: &mut DescriptorProtoMut<'_>, members: &BTreeSet<String>) -> Option<()> {
    let r = m.as_ref();
    let before: Vec<Option<i32>> = r
        .field()
        .iter()
        .map(|f| f.has_oneof_index().then(|| f.oneof_index()))
        .collect();
    let keep: Vec<bool> = r
        .field()
        .iter()
        .map(|f| !members.contains(String::from_utf8_lossy(f.name()).as_ref()))
        .collect();
    // A oneof left with no field goes, as the text has none (phase 2 may
    // have emptied it already, stripping a field of a type in no file);
    // the others renumbered.
    let oneofs = r.oneof_decl().len();
    let has: BTreeSet<i32> = before
        .iter()
        .zip(&keep)
        .filter(|(_, &k)| k)
        .filter_map(|(o, _)| *o)
        .collect();
    let gone: BTreeSet<i32> = (0..oneofs as i32).filter(|o| !has.contains(o)).collect();
    let remap = |o: i32| o - gone.iter().filter(|&&g| g < o).count() as i32;
    let fields: Vec<(Vec<u8>, Option<i32>)> = r
        .field()
        .iter()
        .zip(&keep)
        .filter(|(_, &k)| k)
        .map(|(f, _)| {
            Some((
                f.0.encode().ok()?,
                f.has_oneof_index().then(|| f.oneof_index()),
            ))
        })
        .collect::<Option<_>>()?;
    let decls: Vec<Vec<u8>> = (0..oneofs as i32)
        .filter(|o| !gone.contains(o))
        .map(|o| r.oneof_decl()[o as usize].0.encode().ok())
        .collect::<Option<_>>()?;
    let extensions = encoded_without(
        r.extension()
            .iter()
            .map(|x| (x.name().to_vec(), x.0.encode().ok())),
        members,
    )?;
    m.clear_field();
    for (b, o) in &fields {
        let mut f = m.push_field().ok()?;
        f.0.merge_from(b).ok()?;
        if let Some(o) = o {
            f.set_oneof_index(remap(*o)).ok()?;
        }
    }
    if !gone.is_empty() {
        m.clear_oneof_decl();
        for b in &decls {
            m.push_oneof_decl().ok()?.0.merge_from(b).ok()?;
        }
    }
    m.clear_extension();
    for b in &extensions {
        m.push_extension().ok()?.0.merge_from(b).ok()?;
    }
    Some(())
}

/// `m`'s oneofs left with no field removed, and its nested messages'.
pub(super) fn drop_empty_oneofs(m: &mut DescriptorProtoMut<'_>) -> Option<()> {
    let r = m.as_ref();
    let used: BTreeSet<i32> = r
        .field()
        .iter()
        .filter(|f| f.has_oneof_index())
        .map(|f| f.oneof_index())
        .collect();
    if (0..r.oneof_decl().len() as i32).any(|o| !used.contains(&o)) {
        remove_members(m, &BTreeSet::new())?;
    }
    let count = m.as_ref().nested_type().len();
    for j in 0..count {
        let mut nested = m.nested_type_mut(j).ok()?;
        drop_empty_oneofs(&mut nested)?;
    }
    Some(())
}

fn remove_methods(s: &mut ServiceDescriptorProtoMut<'_>, methods: &BTreeSet<String>) -> Option<()> {
    let kept = encoded_without(
        s.as_ref()
            .method()
            .iter()
            .map(|x| (x.name().to_vec(), x.0.encode().ok())),
        methods,
    )?;
    s.clear_method();
    for b in &kept {
        s.push_method().ok()?.0.merge_from(b).ok()?;
    }
    Some(())
}
