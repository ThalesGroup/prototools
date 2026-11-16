// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 2: the files into the database rank by rank, imports first, each
//! mended before it goes in (reproto's `_phase2_build_pool` and its
//! helpers). Each change is made on a upb message and the message
//! re-encoded, as Python's protobuf, upb's too, re-encodes the one reproto
//! changes: the bytes agree.

use std::collections::{BTreeMap, BTreeSet};

use prototext_upb::descriptor_access::{
    DescriptorProtoMut, FileDescriptorProto, FileDescriptorProtoMut, ServiceDescriptorProtoMut,
};
use prototext_upb::{DynamicMessage, Pool};
use regex::Regex;

use crate::db::file_symbols;
use crate::pathmatch::fqdn_match;
use crate::run::Run;
use crate::topology::{Stripped, Topology};

/// Why phase 2 stops the run (reproto's `DescriptorProto*Error`s), with
/// reproto's messages.
pub enum DescriptorProtoError {
    Missing(String),
    Unresolved,
    HasTargets,
}

impl DescriptorProtoError {
    /// What reproto logs before raising, when it does (`cli_error`).
    pub fn detail(&self) -> Option<String> {
        match self {
            DescriptorProtoError::Missing(path) => Some(format!(
                "descriptor.proto not found in input files ({path}); add it to your input set \
                 or use --use-variant to load the embedded fallback"
            )),
            _ => None,
        }
    }

    /// The CLI's message.
    pub fn message(&self) -> String {
        match self {
            DescriptorProtoError::Missing(_) => "descriptor.proto not found in the input set; \
                add it to your -I tree or use --use-variant descriptor"
                .to_string(),
            DescriptorProtoError::Unresolved => {
                "descriptor.proto is referenced but could not be loaded; \
                add it to your -I tree or use --use-variant descriptor"
                    .to_string()
            }
            DescriptorProtoError::HasTargets => {
                "descriptor.proto appears corrupted: it has dependencies of its own".to_string()
            }
        }
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `_LABEL_NAMES`, `optional` for anything else.
fn label_name(label: i32) -> &'static str {
    match label {
        2 => "required",
        3 => "repeated",
        _ => "optional",
    }
}

pub fn run(r: &mut Run, topo: &mut Topology) -> Result<(), DescriptorProtoError> {
    if r.debug_fqdn {
        debug_reachability(r, topo);
    }
    let descriptor = r.variant.descriptor_proto.clone();
    let Some(d) = topo.find(&descriptor) else {
        return Err(DescriptorProtoError::Missing(descriptor));
    };
    if topo.files[d].is_ref() {
        return Err(DescriptorProtoError::Unresolved);
    }
    if !topo.files[d].targets.is_empty() {
        return Err(DescriptorProtoError::HasTargets);
    }

    // Rank by rank: the files none of whose imports wait, in topology order
    // (reproto since spec 0428), then the files they freed.
    let mut waiting: BTreeSet<usize> = (0..topo.files.len()).collect();
    for rank in 1.. {
        let leaves: Vec<usize> = waiting
            .iter()
            .copied()
            .filter(|&i| topo.files[i].targets.iter().all(|t| !waiting.contains(t)))
            .collect();
        if leaves.is_empty() {
            break;
        }
        if r.debug {
            r.events
                .info(&format!("  Rank {rank}: processing {} files", leaves.len()));
        }
        for &i in &leaves {
            waiting.remove(&i);
            if r.debug {
                r.events.info(&format!("  Merging: {}", topo.files[i].name));
            }
            merge(r, topo, i);
        }
    }
    crate::console::phase_done("Loading descriptors");
    for i in waiting {
        r.events.warning(&format!(
            "Circular dependency detected: {}",
            topo.files[i].name
        ));
        topo.files[i].is_pruned = true;
    }
    r.edition_defaults =
        r.db.file_by_name(&descriptor)
            .map(|f| edition_defaults(&FileDescriptorProto(f.as_ref())))
            .unwrap_or_default();
    Ok(())
}

/// One file into the database, mended, unless pruned.
fn merge(r: &mut Run, topo: &mut Topology, i: usize) {
    let Some(qf) = topo.files[i].qfile.as_ref() else {
        return;
    };
    let name = topo.files[i].name.clone();
    if r.prunings
        .iter()
        .any(|p| fqdn_match(p, &format!("file:{name}")))
    {
        topo.files[i].is_pruned = true;
        r.pruned_file_names.insert(name);
        return;
    }
    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .expect("the vendored descriptor.proto declares it");
    let Ok(mut fdp) = DynamicMessage::decode(&def, &qf.contents) else {
        r.events
            .warning(&format!("Skipping unparseable file: {name}"));
        topo.files[i].is_pruned = true;
        return;
    };
    // The plugin, on the file as parsed, before anything reads it (spec
    // 0431 S1); its answer decodes, the plugin checked it.
    if let Some(p) = &r.plugin {
        let answer = p.apply(&name, &qf.contents);
        if let Ok(patched) = DynamicMessage::decode(&def, &answer) {
            fdp = patched;
        }
    }
    patch_go_package(r, &mut fdp);
    strip_self_dependency(r, &mut fdp);
    if !r.keep_duplicates && prune_if_duplicate(r, topo, i, &fdp) {
        return;
    }
    strip_unresolvable_dependencies(r, topo, i, &mut fdp);
    strip_unresolvable_field_types(r, topo, i, &mut fdp);
    r.db.add(fdp);
}

/// `patch_go_package`: with `--go-root`, `go_package` set to the file's path
/// under it, an `internal` segment written `intern4l`.
fn patch_go_package(r: &Run, fdp: &mut DynamicMessage) {
    let Some(root) = &r.go_root else { return };
    let name = text(FileDescriptorProto(fdp.as_ref()).name());
    let re = Regex::new(r"(^|/)(internal)(/|$)").expect("a valid regex");
    let go_package = re
        .replace_all(&format!("{root}/{name}"), "${1}intern4l${3}")
        .into_owned();
    let mut w = FileDescriptorProtoMut(fdp.as_mut());
    if let Ok(mut options) = w.options_mut() {
        let _ = options.set_go_package(go_package.as_bytes());
    }
}

/// `_strip_self_dependency`: every import of the file by itself, each
/// warned.
fn strip_self_dependency(r: &mut Run, fdp: &mut DynamicMessage) {
    let name = FileDescriptorProto(fdp.as_ref()).name().to_vec();
    loop {
        let deps = FileDescriptorProto(fdp.as_ref()).dependency();
        let Some(at) = deps.iter().position(|d| *d == name.as_slice()) else {
            break;
        };
        let _ = FileDescriptorProtoMut(fdp.as_mut()).remove_dependency(at);
        r.events.warning(&format!(
            "Stripped self-dependency from '{}' (malformed descriptor)",
            text(&name)
        ));
    }
}

/// `_prune_if_duplicate`: a file declaring a symbol the database has is
/// pruned, with a W3 naming the files it collides with.
fn prune_if_duplicate(r: &mut Run, topo: &mut Topology, i: usize, fdp: &DynamicMessage) -> bool {
    let mut conflicts: BTreeMap<String, String> = BTreeMap::new();
    for sym in file_symbols(&FileDescriptorProto(fdp.as_ref())) {
        if let Some(f) = r.db.file_containing_symbol(&sym) {
            conflicts.insert(sym, r.db.name(f));
        }
    }
    if conflicts.is_empty() {
        return false;
    }
    let mut by_file: BTreeMap<String, usize> = BTreeMap::new();
    for f in conflicts.values() {
        *by_file.entry(f.clone()).or_default() += 1;
    }
    let name = topo.files[i].name.clone();
    let mut parts = vec![format!(
        "Warning: file:{name} pruned — duplicate symbols with:"
    )];
    for (f, count) in by_file {
        let noun = if count == 1 { "symbol" } else { "symbols" };
        parts.push(format!("    file:{f} ({count} {noun})"));
    }
    r.events.w3(&parts.join("\n"));
    topo.files[i].is_pruned = true;
    r.pruned_file_names.insert(name);
    true
}

/// `_strip_unresolvable_dependencies`: the imports of pruned files and of
/// files the input lacks, `public_dependency` and `weak_dependency`
/// renumbered, each reported.
fn strip_unresolvable_dependencies(
    r: &mut Run,
    topo: &mut Topology,
    i: usize,
    fdp: &mut DynamicMessage,
) {
    let f = FileDescriptorProto(fdp.as_ref());
    let deps: Vec<String> = f.dependency().iter().map(|d| text(d)).collect();
    let public_before: Vec<i32> = f.public_dependency();
    let weak_before: Vec<i32> = f.weak_dependency();
    let file_name = text(f.name());
    let to_strip: Vec<(usize, String)> = deps
        .iter()
        .enumerate()
        .filter(|(_, dep)| {
            r.pruned_file_names.contains(*dep)
                || topo.find(dep).is_some_and(|t| topo.files[t].is_ref())
        })
        .map(|(n, dep)| (n, dep.clone()))
        .collect();
    if to_strip.is_empty() {
        return;
    }
    let stripped: BTreeSet<usize> = to_strip.iter().map(|(n, _)| *n).collect();
    let new_index: BTreeMap<usize, i32> = (0..deps.len())
        .filter(|n| !stripped.contains(n))
        .enumerate()
        .map(|(new, old)| (old, new as i32))
        .collect();
    let renumber = |v: &[i32]| -> Vec<i32> {
        v.iter()
            .filter_map(|&old| {
                usize::try_from(old)
                    .ok()
                    .and_then(|o| new_index.get(&o).copied())
            })
            .collect()
    };
    let (public, weak) = (renumber(&public_before), renumber(&weak_before));
    {
        let mut w = FileDescriptorProtoMut(fdp.as_mut());
        for &n in stripped.iter().rev() {
            let _ = w.remove_dependency(n);
        }
        w.clear_public_dependency();
        for p in public {
            let _ = w.push_public_dependency(p);
        }
        w.clear_weak_dependency();
        for p in weak {
            let _ = w.push_weak_dependency(p);
        }
    }
    let public_indices: BTreeSet<i32> = public_before.into_iter().collect();
    for (n, dep) in to_strip.into_iter().rev() {
        let file = &mut topo.files[i];
        if public_indices.contains(&(n as i32)) {
            file.stripped_public_dependencies.push(dep.clone());
        } else {
            file.stripped_dependencies.push(dep.clone());
        }
        if r.pruned_file_names.contains(&dep) {
            if !r.quiet {
                r.events.w3(&format!(
                    "Warning: stripped pruned dependency \"{dep}\" from {file_name}"
                ));
            }
        } else {
            r.events.w1(&dep);
        }
    }
}

/// `_strip_unresolvable_field_types`: the fields and methods whose types
/// neither the file nor the database declares, recorded on the file, each
/// type W4.
fn strip_unresolvable_field_types(
    r: &mut Run,
    topo: &mut Topology,
    i: usize,
    fdp: &mut DynamicMessage,
) {
    let f = FileDescriptorProto(fdp.as_ref());
    let own: BTreeSet<String> = file_symbols(&f).into_iter().collect();
    let package = text(f.package());
    let prefix = if package.is_empty() {
        String::new()
    } else {
        format!(".{package}")
    };
    let unresolvable = |type_name: &[u8]| -> bool {
        if type_name.is_empty() {
            return false;
        }
        let name = text(type_name);
        let name = name.trim_start_matches('.');
        !own.contains(name) && r.db.file_containing_symbol(name).is_none()
    };
    let mut fields: Vec<(String, Stripped, String)> = Vec::new();
    let mut methods: Vec<(String, Stripped, Vec<String>)> = Vec::new();
    {
        let mut w = FileDescriptorProtoMut(fdp.as_mut());
        let count = w.as_ref().message_type().len();
        for m in 0..count {
            if let Ok(mut msg) = w.message_type_mut(m) {
                walk_messages(&mut msg, &prefix, &unresolvable, &mut fields);
            }
        }
        let count = w.as_ref().service().len();
        for s in 0..count {
            if let Ok(mut svc) = w.service_mut(s) {
                strip_methods(&mut svc, &prefix, &unresolvable, &mut methods);
            }
        }
    }
    let file = &mut topo.files[i];
    for (owner, record, type_name) in fields {
        file.stripped_field_types
            .entry(owner)
            .or_default()
            .push(record);
        if !r.quiet {
            r.events.w4(&type_name);
        }
    }
    for (owner, record, types) in methods {
        file.stripped_method_types
            .entry(owner)
            .or_default()
            .push(record);
        if !r.quiet {
            for t in types {
                r.events.w4(&t);
            }
        }
    }
}

/// A message's unresolvable fields, removed and returned in their order
/// (owner, record, type), then its nested messages', as reproto walks them.
fn walk_messages(
    msg: &mut DescriptorProtoMut<'_>,
    parent: &str,
    unresolvable: &dyn Fn(&[u8]) -> bool,
    out: &mut Vec<(String, Stripped, String)>,
) {
    let m = msg.as_ref();
    let fqdn = format!("{parent}.{}", text(m.name()));
    let mut gone = Vec::new();
    for (n, field) in m.field().iter().enumerate() {
        if unresolvable(field.type_name()) {
            let record = vec![
                field.number().to_string(),
                text(field.name()),
                text(field.type_name()),
                label_name(field.label()).to_string(),
            ];
            out.push((fqdn.clone(), record, text(field.type_name())));
            gone.push(n);
        }
    }
    for n in gone.into_iter().rev() {
        let _ = msg.remove_field(n);
    }
    let count = msg.as_ref().nested_type().len();
    for n in 0..count {
        if let Ok(mut nested) = msg.nested_type_mut(n) {
            walk_messages(&mut nested, &fqdn, unresolvable, out);
        }
    }
}

/// A service's methods with an unresolvable input or output type, removed
/// and returned (owner, record, the unresolvable types).
fn strip_methods(
    svc: &mut ServiceDescriptorProtoMut<'_>,
    prefix: &str,
    unresolvable: &dyn Fn(&[u8]) -> bool,
    out: &mut Vec<(String, Stripped, Vec<String>)>,
) {
    let s = svc.as_ref();
    let fqdn = format!("{prefix}.{}", text(s.name()));
    let mut gone = Vec::new();
    for (n, method) in s.method().iter().enumerate() {
        let types: Vec<String> = [method.input_type(), method.output_type()]
            .into_iter()
            .filter(|t| unresolvable(t))
            .map(text)
            .collect();
        if !types.is_empty() {
            let record = vec![
                text(method.name()),
                text(method.input_type()),
                text(method.output_type()),
            ];
            out.push((fqdn.clone(), record, types));
            gone.push(n);
        }
    }
    for n in gone.into_iter().rev() {
        let _ = svc.remove_method(n);
    }
}

/// `--debug-fqdn`'s report of phase 2: each file, seed, present, reachable.
fn debug_reachability(r: &mut Run, topo: &Topology) {
    let mut reachable: BTreeSet<usize> = BTreeSet::new();
    let mut frontier: Vec<usize> = (0..topo.files.len())
        .filter(|&i| topo.files[i].is_seed && !topo.files[i].is_ref())
        .collect();
    while let Some(i) = frontier.pop() {
        if !reachable.insert(i) {
            continue;
        }
        frontier.extend(
            topo.files[i]
                .targets
                .iter()
                .copied()
                .filter(|&t| !topo.files[t].is_ref()),
        );
    }
    for (i, f) in topo.files.iter().enumerate() {
        r.events.info(&format!(
            "FILE {} {} {} {}",
            if f.is_seed { "S" } else { "-" },
            if f.is_ref() { "-" } else { "P" },
            if f.is_ref() || !reachable.contains(&i) {
                "-"
            } else {
                "R"
            },
            f.name
        ));
    }
}

/// `build_edition_defaults`: for each feature reproto resolves, its
/// defaults by edition, and its values' numbers.
pub type EditionDefaults = BTreeMap<String, EditionDefault>;

pub enum EditionDefault {
    /// (edition, value), sorted by edition.
    Defaults(Vec<(i32, String)>),
    /// value name → number.
    Numbers(BTreeMap<String, i32>),
}

const RESOLVED_FIELDS: &[&str] = &[
    "field_presence",
    "enum_type",
    "repeated_field_encoding",
    "utf8_validation",
    "message_encoding",
    "json_format",
];

fn edition_defaults(descriptor: &FileDescriptorProto<'_>) -> EditionDefaults {
    let mut table = EditionDefaults::new();
    let Some(feature_set) = descriptor
        .message_type()
        .into_iter()
        .find(|m| m.name() == b"FeatureSet")
    else {
        return table;
    };
    let mut enum_by_name: BTreeMap<String, BTreeMap<String, i32>> = BTreeMap::new();
    let nested_enums = feature_set.enum_type().into_iter().chain(
        feature_set
            .nested_type()
            .into_iter()
            .flat_map(|m| m.enum_type()),
    );
    for e in nested_enums {
        enum_by_name.insert(
            text(e.name()),
            e.value()
                .iter()
                .map(|v| (text(v.name()), v.number()))
                .collect(),
        );
    }
    for field in feature_set.field() {
        let name = text(field.name());
        if !RESOLVED_FIELDS.contains(&name.as_str()) {
            continue;
        }
        let type_name = text(field.type_name());
        let enum_name = type_name.rsplit('.').next().unwrap_or_default();
        let numbers = enum_by_name.get(enum_name).cloned().unwrap_or_default();
        let mut entries: Vec<(i32, String)> = field
            .options()
            .map(|o| {
                o.edition_defaults()
                    .iter()
                    .map(|d| (d.edition(), text(d.value())))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by_key(|(edition, _)| *edition);
        table.insert(name.clone(), EditionDefault::Defaults(entries));
        table.insert(format!("_enum_{name}"), EditionDefault::Numbers(numbers));
    }
    table
}
