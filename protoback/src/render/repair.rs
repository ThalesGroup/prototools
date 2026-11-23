// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `--repair-skew` (spec 0469): what a written file must change for protoc
//! to compile it despite version skew between the builds its descriptors
//! come from. An import added where exactly one written file defines a
//! type it uses (S2); a field or method commented out where no written
//! file defines its type (S3), an extension where its number is in no
//! extension range of its extendee (S4); the ambiguous left, reported (S5).

use std::collections::{BTreeMap, BTreeSet};

use crate::graph::Kind;
use crate::hash::{HashMap, HashSet};

use super::{Def, Shared};

/// A field, method or extension commented out, and why.
#[derive(Debug, Clone)]
pub struct Orphan {
    pub code: &'static str,
    /// The comment before it, after `WARNING[repair-skew]: `.
    pub comment: String,
    /// The warning's message, after the file's name.
    pub message: String,
    pub fields: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct Repairs {
    /// By file: each import added, with the symbols the file uses from it.
    pub imports: HashMap<String, BTreeMap<String, BTreeSet<String>>>,
    /// By graph node: the fields, methods and extensions commented out.
    pub orphans: HashMap<usize, Orphan>,
    /// By file: each type several written files define, with them.
    pub ambiguous: HashMap<String, BTreeMap<String, Vec<String>>>,
}

impl Repairs {
    /// Whether `node` is out of the descriptors written (spec 0473 S2): a
    /// field, method or extension commented out, or a map entry whose map
    /// field is (the text writes no map entry of its own).
    pub fn is_out(&self, shared: &Shared<'_>, node: usize) -> bool {
        if self.orphans.contains_key(&node) || extends_nothing(shared, node) {
            return true;
        }
        let n = &shared.graph.nodes[node];
        let is_map_entry = n.kind == Kind::Message
            && shared
                .message_def(node)
                .and_then(|d| d.options())
                .is_some_and(|o| o.map_entry());
        let Some(parent) = n.parent.filter(|_| is_map_entry) else {
            return false;
        };
        let type_name = &n.prefix;
        let mut users = shared.message_def(parent).into_iter().flat_map(|m| {
            m.field()
                .iter()
                .filter(|f| f.type_name() == type_name.as_bytes())
                .filter_map(|f| shared.child(Kind::Field, parent, f.name()))
                .collect::<Vec<_>>()
        });
        let first = users.next();
        first.is_some_and(|f| self.orphans.contains_key(&f))
            && users.all(|f| self.orphans.contains_key(&f))
    }

    /// `names` with the imports added, so that type names in a repaired file
    /// resolve as protoc will see them.
    pub fn apply_to(&self, names: &mut super::names::SymbolTable) {
        for (file, deps) in &self.imports {
            for dep in deps.keys() {
                names.add_import(file, dep);
            }
        }
    }
}

/// The repairs of every file to write.
pub fn repairs(shared: &Shared<'_>) -> Repairs {
    let graph = shared.graph;
    let written: HashSet<String> = super::summoned(graph)
        .into_iter()
        .map(|i| file_name(shared, i))
        .collect();
    let mut out = Repairs::default();
    for i in 0..graph.nodes.len() {
        let node = &graph.nodes[i];
        // An orphan already (unsummoned, or pruned): protoback comments it
        // out without a repair.
        if !matches!(node.kind, Kind::Field | Kind::Method) || !node.present || !node.is_summoned {
            continue;
        }
        let Some(file) = file_of(shared, i) else {
            continue;
        };
        let file = file_name(shared, file);
        if !written.contains(&file) {
            continue;
        }
        let visible = shared.names.visible(&file);
        // The written files defining `t`, none of them seen from `file`: a
        // repair to make, or not.
        let resolve = |t: &[u8], out: &mut Repairs| -> Option<()> {
            let t = String::from_utf8_lossy(t);
            let t = t.trim_start_matches('.');
            if t.is_empty() {
                return Some(());
            }
            let defining: Vec<String> = shared
                .names
                .type_files(t)
                .into_iter()
                .filter(|f| written.contains(*f))
                .map(str::to_string)
                .collect();
            if defining.iter().any(|d| visible.sees(d)) {
                return Some(());
            }
            match defining.as_slice() {
                [] => None,
                [one] => {
                    out.imports
                        .entry(file.clone())
                        .or_default()
                        .entry(one.clone())
                        .or_default()
                        .insert(t.to_string());
                    Some(())
                }
                several => {
                    out.ambiguous
                        .entry(file.clone())
                        .or_default()
                        .insert(t.to_string(), several.to_vec());
                    Some(())
                }
            }
        };
        match shared.def(i) {
            Some(Def::Field(f)) => {
                let name = String::from_utf8_lossy(f.name()).into_owned();
                // A map whose value phase 2 stripped (its type in no file,
                // a W4): the entry is malformed, written as a message the
                // output lacks.
                if let Some(t) = stripped_map_value(shared, f.type_name()) {
                    out.orphans.insert(
                        i,
                        Orphan {
                            code: "W34",
                            comment: format!(
                                "commented out: its map value {t} is defined in no file"
                            ),
                            message: format!(
                                "field {name}: commented out, its map value {t} is defined in no \
                                 file (--repair-skew)"
                            ),
                            fields: serde_json::json!({ "file": file, "field": name, "type": t }),
                        },
                    );
                    continue;
                }
                if resolve(f.type_name(), &mut out).is_none() {
                    let t = String::from_utf8_lossy(f.type_name()).into_owned();
                    // A map's value: its map field.
                    let target = map_field_of(shared, i).unwrap_or(i);
                    let target_name = match shared.def(target) {
                        Some(Def::Field(m)) => String::from_utf8_lossy(m.name()).into_owned(),
                        _ => name.clone(),
                    };
                    out.orphans.insert(
                        target,
                        Orphan {
                            code: "W34",
                            comment: format!("commented out: {t} is defined in no written file"),
                            message: format!(
                                "field {target_name}: commented out, {t} is defined in no \
                                 written file (--repair-skew)"
                            ),
                            fields: serde_json::json!({
                                "file": file, "field": target_name, "type": t,
                            }),
                        },
                    );
                }
                if !f.has_extendee() {
                    continue;
                }
                resolve(f.extendee(), &mut out);
                let extendee = String::from_utf8_lossy(f.extendee()).into_owned();
                let number = f.number();
                let Some(e) = graph.find(&format!("desc:{extendee}")) else {
                    continue;
                };
                if !written.contains(
                    &file_of(shared, e)
                        .map(|x| file_name(shared, x))
                        .unwrap_or_default(),
                ) {
                    continue;
                }
                let Some(m) = shared.message_def(e) else {
                    continue;
                };
                let declared = m
                    .extension_range()
                    .iter()
                    .any(|r| r.start() <= number && number < r.end());
                if !declared {
                    let short = extendee.trim_start_matches('.');
                    out.orphans.insert(
                        i,
                        Orphan {
                            code: "W35",
                            comment: format!(
                                "commented out: {number} is in no extension range of {short}"
                            ),
                            message: format!(
                                "extension {name}: commented out, {number} is in no extension \
                                 range of {short} (--repair-skew)"
                            ),
                            fields: serde_json::json!({
                                "file": file, "extension": name, "number": number,
                                "extendee": short,
                            }),
                        },
                    );
                }
            }
            Some(Def::Method(m)) => {
                let name = String::from_utf8_lossy(m.name()).into_owned();
                for t in [m.input_type(), m.output_type()] {
                    if resolve(t, &mut out).is_none() {
                        let t = String::from_utf8_lossy(t).into_owned();
                        out.orphans.insert(
                            i,
                            Orphan {
                                code: "W34",
                                comment: format!(
                                    "commented out: {t} is defined in no written file"
                                ),
                                message: format!(
                                    "method {name}: commented out, {t} is defined in no written \
                                     file (--repair-skew)"
                                ),
                                fields: serde_json::json!({
                                    "file": file, "method": name, "type": t,
                                }),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn file_name(shared: &Shared<'_>, file: usize) -> String {
    shared.graph.nodes[file]
        .fqdn
        .strip_prefix("file:")
        .unwrap_or_default()
        .to_string()
}

/// Whether `node` is an extension whose extendee no file of the run
/// defines (its file a missing dependency): the text writes it commented
/// out, and the pool would refuse its file for it (spec 0476 S1).
pub(super) fn extends_nothing(shared: &Shared<'_>, node: usize) -> bool {
    let Some(super::Def::Field(f)) = shared.def(node) else {
        return false;
    };
    if f.extendee().is_empty() {
        return false;
    }
    let extendee = String::from_utf8_lossy(f.extendee());
    shared
        .graph
        .find(&format!("{}:{extendee}", Kind::Message.prefix()))
        .and_then(|m| shared.message_def(m))
        .is_none()
}

/// The file node `node` is in.
pub(super) fn file_of_node(shared: &Shared<'_>, node: usize) -> Option<usize> {
    file_of(shared, node)
}

/// The file node `node` is in.
fn file_of(shared: &Shared<'_>, mut node: usize) -> Option<usize> {
    loop {
        let n = &shared.graph.nodes[node];
        if n.kind == Kind::File {
            return Some(node);
        }
        node = n.parent?;
    }
}

/// When `field` is a map entry's, the map field the entry is the type of.
fn map_field_of(shared: &Shared<'_>, field: usize) -> Option<usize> {
    let entry = shared.graph.nodes[field].parent?;
    let is_entry = shared
        .message_def(entry)
        .and_then(|m| m.options())
        .is_some_and(|o| o.map_entry());
    if !is_entry {
        return None;
    }
    let owner = shared.graph.nodes[entry].parent?;
    let entry_name = &shared.graph.nodes[entry].prefix;
    let owner_def = shared.message_def(owner)?;
    owner_def.field().iter().find_map(|f| {
        let t = String::from_utf8_lossy(f.type_name());
        (t == entry_name.as_str())
            .then(|| shared.child(Kind::Field, owner, f.name()))
            .flatten()
    })
}

/// When `type_name` is a map entry whose value field phase 2 stripped, that
/// value's type.
fn stripped_map_value(shared: &Shared<'_>, type_name: &[u8]) -> Option<String> {
    let t = String::from_utf8_lossy(type_name);
    let entry = shared.graph.find(&format!("desc:{t}"))?;
    let is_entry = shared
        .message_def(entry)
        .and_then(|m| m.options())
        .is_some_and(|o| o.map_entry());
    if !is_entry {
        return None;
    }
    // (number, name, type_name, label), as phase 2 records them.
    shared.graph.nodes[entry]
        .stripped_fields
        .iter()
        .find(|sf| sf.first().map(String::as_str) == Some("2"))
        .and_then(|sf| sf.get(2).cloned())
}
