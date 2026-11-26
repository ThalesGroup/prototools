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

use crate::db::{extension_keys, file_symbols};
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
                 or use --builtin to load the built-in copy"
            )),
            _ => None,
        }
    }

    /// The CLI's message.
    pub fn message(&self) -> String {
        match self {
            DescriptorProtoError::Missing(_) => "descriptor.proto not found in the input set; \
                add it to your -I tree or use --builtin descriptor"
                .to_string(),
            DescriptorProtoError::Unresolved => {
                "descriptor.proto is referenced but could not be loaded; \
                add it to your -I tree or use --builtin descriptor"
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
    let phase = crate::console::phase(
        "Loading descriptors",
        crate::console::Label::Always,
        if r.quiet { 0 } else { topo.files.len() as u64 },
    );
    // Without a plugin, each file decoded and its symbols listed on its
    // own, in parallel, before the loop takes them in its order (spec 0443
    // P8); a plugin's calls keep their order.
    let mut prepared: Vec<Option<Prepared>> = if r.plugin.is_none() {
        use rayon::prelude::*;
        let def = Pool::vendored()
            .get_message_by_name("google.protobuf.FileDescriptorProto")
            .expect("the vendored descriptor.proto declares it");
        topo.files
            .par_iter()
            .map(|f| {
                let qf = f.qfile.as_ref()?;
                let fdp = DynamicMessage::decode(&def, &qf.contents).ok();
                let symbols = fdp
                    .as_ref()
                    .map(|m| file_symbols(&FileDescriptorProto(m.as_ref())));
                Some(Prepared { fdp, symbols })
            })
            .collect()
    } else {
        Vec::new()
    };
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
            let ready = prepared.get_mut(i).and_then(Option::take);
            merge(r, topo, i, ready);
            phase.advance(1);
        }
    }
    phase.done();
    for i in waiting {
        r.events.warn(
            "W10",
            &format!("Circular dependency detected: {}", topo.files[i].name),
        );
        topo.files[i].is_pruned = true;
    }
    r.edition_defaults =
        r.db.file_by_name(&descriptor)
            .map(|f| edition_defaults(&FileDescriptorProto(f.as_ref())))
            .unwrap_or_default();
    r.edition_names =
        r.db.file_by_name(&descriptor)
            .map(|f| edition_names(&FileDescriptorProto(f.as_ref())))
            .unwrap_or_default();
    r.visibility_defaults =
        r.db.file_by_name(&descriptor)
            .map(|f| visibility_defaults(&FileDescriptorProto(f.as_ref())))
            .unwrap_or_default();
    Ok(())
}

/// A file decoded ahead, and the symbols its duplicate check reads (none
/// when it does not decode).
struct Prepared {
    fdp: Option<DynamicMessage>,
    symbols: Option<Vec<String>>,
}

/// One file into the database, mended, unless pruned; `ready`, its decoding
/// and symbols when prepared ahead.
fn merge(r: &mut Run, topo: &mut Topology, i: usize, ready: Option<Prepared>) {
    let Some(qf) = topo.files[i].qfile.as_ref() else {
        return;
    };
    let name = topo.files[i].name.clone();
    // A moved file is pruned, however it was reached (spec 0445 S5).
    let pattern = r
        .prunings
        .iter()
        .find(|p| fqdn_match(p, &format!("file:{name}")));
    if r.moves.is_old(&name) || pattern.is_some() {
        let cause = match pattern {
            Some(p) => serde_json::json!({ "pattern": p }),
            None => serde_json::json!({ "move": r.moves.targets(&name) }),
        };
        crate::report::pruned_file(&name, cause);
        topo.files[i].is_pruned = true;
        r.pruned_file_names.insert(name);
        return;
    }
    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .expect("the vendored descriptor.proto declares it");
    let (decoded, symbols) = match ready {
        Some(p) => (p.fdp, p.symbols),
        None => (DynamicMessage::decode(&def, &qf.contents).ok(), None),
    };
    let Some(mut fdp) = decoded else {
        r.events
            .warn("W9", &format!("Skipping unparseable file: {name}"));
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
    // Spec 0445 S4: the imports of moved files, re-pointed.
    r.moves.rewrite_fdp(&mut fdp);
    patch_go_package(r, &mut fdp);
    strip_self_dependency(r, &mut fdp);
    if !r.keep_duplicates && prune_if_duplicate(r, topo, i, &fdp, symbols) {
        return;
    }
    strip_unresolvable_dependencies(r, topo, i, &mut fdp);
    // Spec 0445 S6: what a re-pointed importer lost, before its fields of
    // types that do not resolve are stripped.
    let descriptor = r.variant.descriptor_proto.clone();
    r.moves
        .check_importer(&fdp, &r.db, &descriptor, &mut r.events);
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
        r.events.warn(
            "W16",
            &format!(
                "Stripped self-dependency from '{}' (malformed descriptor)",
                text(&name)
            ),
        );
    }
}

/// `_prune_if_duplicate`: a file declaring a symbol the database has, or an
/// extension number another file uses on the same extendee (which the
/// descriptor pool would refuse), is pruned, with a W3 naming the files it
/// collides with (spec 0438 S1).
fn prune_if_duplicate(
    r: &mut Run,
    topo: &mut Topology,
    i: usize,
    fdp: &DynamicMessage,
    symbols: Option<Vec<String>>,
) -> bool {
    let mut conflicts: BTreeMap<String, String> = BTreeMap::new();
    // Listed ahead when prepared: nothing before this check changes the
    // file's types (`patch_go_package`, `strip_self_dependency`).
    let symbols = symbols.unwrap_or_else(|| file_symbols(&FileDescriptorProto(fdp.as_ref())));
    for sym in symbols {
        if let Some(f) = r.db.file_containing_symbol(&sym) {
            conflicts.insert(sym, r.db.name(f));
        }
    }
    // The file each taken (extendee, number) is held by, per extendee.
    let mut taken: BTreeMap<String, BTreeMap<String, Vec<i32>>> = BTreeMap::new();
    for (extendee, number) in extension_keys(&FileDescriptorProto(fdp.as_ref())) {
        if let Some(f) = r.db.file_extending(&extendee, number) {
            let owner = taken.entry(r.db.name(f)).or_default();
            owner.entry(extendee).or_default().push(number);
        }
    }
    if conflicts.is_empty() && taken.is_empty() {
        return false;
    }
    // The report's copy (spec 0446 S3), each extendee's numbers sorted.
    let mut taken_report = taken.clone();
    for nums in taken_report.values_mut().flat_map(BTreeMap::values_mut) {
        nums.sort_unstable();
    }
    let name = topo.files[i].name.clone();
    let mut parts = Vec::new();
    if !conflicts.is_empty() {
        let mut by_file: BTreeMap<String, usize> = BTreeMap::new();
        for f in conflicts.values() {
            *by_file.entry(f.clone()).or_default() += 1;
        }
        parts.push(format!("file:{name} pruned — duplicate symbols with:"));
        for (f, count) in by_file {
            let noun = if count == 1 { "symbol" } else { "symbols" };
            parts.push(format!("    file:{f} ({count} {noun})"));
        }
    }
    if !taken.is_empty() {
        let head = "duplicate extension numbers with:";
        parts.push(if parts.is_empty() {
            format!("file:{name} pruned — {head}")
        } else {
            format!("  and {head}")
        });
        for (f, by_extendee) in taken {
            let numbers: Vec<String> = by_extendee
                .into_iter()
                .map(|(extendee, mut nums)| {
                    nums.sort_unstable();
                    let nums: Vec<String> = nums.iter().map(i32::to_string).collect();
                    format!("{extendee} {}", nums.join(", "))
                })
                .collect();
            parts.push(format!("    file:{f} ({})", numbers.join("; ")));
        }
    }
    if crate::report::active() {
        let mut with: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for f in conflicts.values() {
            let e = with
                .entry(f.clone())
                .or_insert_with(|| serde_json::json!({ "file": f, "symbols": 0 }));
            e["symbols"] = (e["symbols"].as_u64().unwrap_or(0) + 1).into();
        }
        for (f, by_extendee) in &taken_report {
            let e = with
                .entry(f.clone())
                .or_insert_with(|| serde_json::json!({ "file": f, "symbols": 0 }));
            e["extensions"] = by_extendee
                .iter()
                .map(|(extendee, numbers)| {
                    serde_json::json!({ "extendee": extendee, "numbers": numbers })
                })
                .collect();
        }
        let with: Vec<serde_json::Value> = with.into_values().collect();
        r.events
            .with_fields(serde_json::json!({ "pruned": name, "with": with }));
        crate::report::pruned_file(&name, serde_json::json!({ "duplicate_of": with }));
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
        r.events
            .with_fields(serde_json::json!({ "importer": file_name, "import": dep }));
        if r.pruned_file_names.contains(&dep) {
            r.events.w23(&format!(
                "stripped pruned dependency \"{dep}\" from {file_name}"
            ));
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
    let file_name = topo.files[i].name.clone();
    // W4, with its file in the report (spec 0446 S3), which -q, silencing
    // it here, does not shorten.
    let w4 = |r: &mut Run, type_name: &str| {
        let fields = serde_json::json!({ "file": file_name, "type": type_name });
        if r.quiet {
            crate::report::warning(
                "W4",
                &format!("unresolvable type {type_name}"),
                crate::report::fields(fields),
            );
        } else {
            r.events.with_fields(fields).w4(type_name);
        }
    };
    let file = &mut topo.files[i];
    for (owner, record, type_name) in fields {
        file.stripped_field_types
            .entry(owner)
            .or_default()
            .push(record);
        w4(r, &type_name);
    }
    let file = &mut topo.files[i];
    for (owner, record, types) in methods {
        file.stripped_method_types
            .entry(owner)
            .or_default()
            .push(record);
        for t in types {
            w4(r, &t);
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

/// The `Edition` enum of the input's descriptor.proto: each value's
/// number and name (spec 0457 S1).
pub type EditionNames = BTreeMap<i32, String>;

fn edition_names(descriptor: &FileDescriptorProto<'_>) -> EditionNames {
    descriptor
        .enum_type()
        .into_iter()
        .find(|e| e.name() == b"Edition")
        .map(|e| {
            e.value()
                .iter()
                .map(|v| (v.number(), text(v.name())))
                .collect()
        })
        .unwrap_or_default()
}

/// `FeatureSet.default_symbol_visibility`'s defaults in the input's
/// descriptor.proto: (edition, `DefaultSymbolVisibility` number), sorted
/// by edition (spec 0458 S2). Apart from [`EditionDefaults`], which holds
/// the six features reproto resolves, and is dumped as reproto dumps it.
pub type VisibilityDefaults = Vec<(i32, i32)>;

fn visibility_defaults(descriptor: &FileDescriptorProto<'_>) -> VisibilityDefaults {
    let Some(feature_set) = descriptor
        .message_type()
        .into_iter()
        .find(|m| m.name() == b"FeatureSet")
    else {
        return Vec::new();
    };
    let numbers: BTreeMap<String, i32> = feature_set
        .nested_type()
        .into_iter()
        .filter(|m| m.name() == b"VisibilityFeature")
        .flat_map(|m| m.enum_type())
        .flat_map(|e| e.value())
        .map(|v| (text(v.name()), v.number()))
        .collect();
    let mut out: VisibilityDefaults = feature_set
        .field()
        .into_iter()
        .filter(|f| f.name() == b"default_symbol_visibility")
        .filter_map(|f| f.options())
        .flat_map(|o| o.edition_defaults())
        .filter_map(|d| Some((d.edition(), *numbers.get(&text(d.value()))?)))
        .collect();
    out.sort_by_key(|(edition, _)| *edition);
    out
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

/// The unit tests of `test_feature_resolution.py` (spec 0488 S2), T1 to T7,
/// on the functions that port reproto's `build_edition_defaults`
/// (`edition_defaults`, here) and `resolve_features` (`features::resolve`,
/// one feature at a time: a `ResolvedFeatures` is the six of
/// `Feature::ALL`); names lower-cased (`test_T1_` is `test_t1_`). "The
/// built-in descriptor.proto", Python protobuf's in the original, is the
/// one protoback reads its features from, the release's
/// (`prototext_upb::DESCRIPTOR_SET`). A `FeatureSet` the original built is
/// its bytes here; T6's and T7's descriptors, built with Python's protobuf,
/// are `#@prototext` fixtures made once by its code
/// (`protoback/tests/fixtures/gen/test_feature_resolution.py`).
#[cfg(test)]
mod test_feature_resolution {
    use super::*;
    use crate::features::{
        resolve, Feature, FIELD_PRESENCE_IMPLICIT, REPEATED_FIELD_ENCODING_PACKED,
    };
    use crate::suite_fixtures::encoded;
    use prototext_upb::descriptor_access::{FeatureSet, FileDescriptorSet};
    use prototext_upb::{DynamicMessage, Pool};

    /// `FieldPresence.UNKNOWN`, `EXPLICIT`; `RepeatedFieldEncoding.EXPANDED`.
    const FIELD_PRESENCE_UNKNOWN: i32 = 0;
    const FIELD_PRESENCE_EXPLICIT: i32 = 1;
    const REPEATED_FIELD_ENCODING_EXPANDED: i32 = 2;

    // Edition numbers (google.protobuf.Edition).
    const EDITION_LEGACY: i32 = 900;
    const EDITION_PROTO3: i32 = 999;
    const EDITION_2023: i32 = 1000;

    fn decode(type_name: &str, bytes: &[u8]) -> DynamicMessage {
        let def = Pool::vendored().get_message_by_name(type_name).unwrap();
        DynamicMessage::decode(&def, bytes).unwrap()
    }

    /// The built-in descriptor.proto's table.
    fn builtin_table() -> EditionDefaults {
        let set = decode(
            "google.protobuf.FileDescriptorSet",
            prototext_upb::DESCRIPTOR_SET,
        );
        let file = FileDescriptorSet(set.as_ref())
            .file()
            .into_iter()
            .find(|f| f.name() == b"google/protobuf/descriptor.proto")
            .unwrap();
        edition_defaults(&file)
    }

    /// A fixture's bare FileDescriptorProto's table.
    fn fixture_table(name: &str, sha256: &str) -> EditionDefaults {
        let msg = decode(
            "google.protobuf.FileDescriptorProto",
            &encoded(name, sha256),
        );
        edition_defaults(&FileDescriptorProto(msg.as_ref()))
    }

    /// A FeatureSet setting field_presence (field 1) to `value`.
    fn presence(value: u8) -> DynamicMessage {
        decode("google.protobuf.FeatureSet", &[0x08, value])
    }

    fn resolved(
        table: &EditionDefaults,
        feature: Feature,
        edition: i32,
        sets: &[Option<&DynamicMessage>],
    ) -> i32 {
        let sets: Vec<Option<FeatureSet<'_>>> = sets
            .iter()
            .map(|m| m.map(|m| FeatureSet(m.as_ref())))
            .collect();
        resolve(table, feature, edition, &sets)
    }

    fn defaults(table: &EditionDefaults, name: &str) -> Vec<(i32, String)> {
        match table.get(name) {
            Some(EditionDefault::Defaults(d)) => d.clone(),
            _ => panic!("{name}: no defaults"),
        }
    }

    /// The built-in descriptor's table, not empty, with field_presence's and
    /// repeated_field_encoding's defaults, LEGACY's and 2023's among them.
    #[test]
    fn test_t1_real_descriptor_builds_table() {
        let table = builtin_table();
        assert!(
            !table.is_empty(),
            "edition_defaults table must not be empty"
        );
        assert!(table.contains_key("field_presence"));
        assert!(table.contains_key("repeated_field_encoding"));
        let editions: Vec<i32> = defaults(&table, "field_presence")
            .iter()
            .map(|(e, _)| *e)
            .collect();
        assert!(
            editions.contains(&EDITION_LEGACY),
            "field_presence must have EDITION_LEGACY default"
        );
        assert!(
            editions.contains(&EDITION_2023),
            "field_presence must have EDITION_2023 default"
        );
    }

    /// No override: the edition defaults.
    #[test]
    fn test_t2_default_lookup() {
        let table = builtin_table();
        let fp = |e| resolved(&table, Feature::FieldPresence, e, &[]);
        let rfe = |e| resolved(&table, Feature::RepeatedFieldEncoding, e, &[]);
        assert_eq!(fp(EDITION_LEGACY), FIELD_PRESENCE_EXPLICIT);
        assert_eq!(fp(EDITION_PROTO3), FIELD_PRESENCE_IMPLICIT);
        assert_eq!(fp(EDITION_2023), FIELD_PRESENCE_EXPLICIT);
        assert_eq!(rfe(EDITION_LEGACY), REPEATED_FIELD_ENCODING_EXPANDED);
        assert_eq!(rfe(EDITION_2023), REPEATED_FIELD_ENCODING_PACKED);
        // Below all defaults (edition 0): unknown.
        assert_eq!(fp(0), FIELD_PRESENCE_UNKNOWN);
    }

    /// An explicit field_presence = IMPLICIT overrides the 2023 default.
    #[test]
    fn test_t3_explicit_override_wins() {
        let table = builtin_table();
        let fs = presence(2);
        assert_eq!(
            resolved(&table, Feature::FieldPresence, EDITION_2023, &[Some(&fs)]),
            FIELD_PRESENCE_IMPLICIT
        );
    }

    /// A field's FeatureSet overrides its file's.
    #[test]
    fn test_t4_finer_wins_over_coarser() {
        let table = builtin_table();
        let (file_fs, field_fs) = (presence(2), presence(1));
        let fp = |sets: &[Option<&DynamicMessage>]| {
            resolved(&table, Feature::FieldPresence, EDITION_2023, sets)
        };
        assert_eq!(
            fp(&[Some(&file_fs), Some(&field_fs)]),
            FIELD_PRESENCE_EXPLICIT
        );
        assert_eq!(fp(&[Some(&file_fs)]), FIELD_PRESENCE_IMPLICIT);
    }

    /// Levels without a FeatureSet are skipped: all six features as bare.
    #[test]
    fn test_t5_none_levels_skipped() {
        let table = builtin_table();
        for f in Feature::ALL {
            assert_eq!(
                resolved(&table, f, EDITION_2023, &[None, None, None]),
                resolved(&table, f, EDITION_2023, &[]),
                "{}",
                f.name()
            );
        }
    }

    /// A descriptor without FeatureSet: an empty table.
    #[test]
    fn test_t6_empty_variant_returns_empty_table() {
        let table = fixture_table(
            "feature_resolution/no_featureset.pb",
            "75aabac3c5c27a961e04c734ba1a3b701daf2c0d80fd76af5f1ba52c757d6faf",
        );
        assert!(table.is_empty());
    }

    /// A synthetic descriptor: field_presence's enum FIRST = 1, SECOND = 2;
    /// edition 900 defaults to FIRST, 1000 to SECOND.
    #[test]
    fn test_t7_synthetic_descriptor() {
        let table = fixture_table(
            "feature_resolution/synthetic.pb",
            "f1b36fc1d4b09cdc65f93055bc2a2a76d7e4e47d3d2204c447c39e9fd534fa45",
        );
        assert_eq!(
            defaults(&table, "field_presence"),
            [
                (EDITION_LEGACY, "FIRST".to_string()),
                (EDITION_2023, "SECOND".to_string())
            ]
        );
        let Some(EditionDefault::Numbers(numbers)) = table.get("_enum_field_presence") else {
            panic!("no _enum_field_presence");
        };
        assert_eq!(numbers.get("FIRST"), Some(&1));
        assert_eq!(numbers.get("SECOND"), Some(&2));
        let fp = |e| resolved(&table, Feature::FieldPresence, e, &[]);
        assert_eq!(fp(900), 1);
        assert_eq!(fp(1000), 2);
        assert_eq!(fp(0), 0);
    }
}
