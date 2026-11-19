// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 1: the seed files, the files they import, the fallbacks, then
//! the files given as text, read in import order (reproto's
//! `_phase1_load_files` and `_read_text_files`).

use std::path::{Path, PathBuf};

use prototext_upb::descriptor_access::FileDescriptorSet;
use prototext_upb::input::{read_input, Input, Read};
use prototext_upb::{DynamicMessage, Pool};

use crate::load::{load_from_path, name_and_imports, LoadCtx, QualFile};
use crate::run::Run;
use crate::topology::Topology;

/// Whether the plugin failed: reproto's `PluginError` ends the phase at
/// once, so nothing after it is read or reported (spec 0431 S4).
fn plugin_failed(p: Option<&crate::plugin::Plugin>) -> bool {
    p.is_some_and(|p| p.failure().is_some())
}

pub fn run(r: &mut Run, topo: &mut Topology) {
    // A fallback's file missing from -I is expected: no W1 for it.
    for f in r
        .fallback_protos
        .iter()
        .chain(&r.fill_in_protos)
        .cloned()
        .collect::<Vec<_>>()
    {
        r.events.register_fallback_file(&f);
    }
    let roots = r.roots.clone();
    let seeds = r.pb_files.clone();
    let mut ctx = LoadCtx {
        pruned_paths: &r.pruned_paths,
        path_seeds: &r.path_seeds,
        events: &mut r.events,
        blobs: &mut r.blobs,
        path_seed_fqdns: &mut r.path_seed_fqdns,
        plugin: r.plugin.as_ref(),
        importing: false,
    };
    let mut qual_files = Vec::new();
    for path in &seeds {
        qual_files.extend(load_from_path(&mut ctx, &roots, path));
        if plugin_failed(ctx.plugin) {
            return;
        }
    }
    for mut qf in qual_files {
        qf.imports = r.moves.rewrite_names(qf.imports);
        let i = topo.load(qf);
        topo.files[i].is_seed = true;
    }
    ctx.importing = true;
    // Import discovery. reproto's check `file not in known_files` compares
    // a file with names, so it always holds: every round retries every
    // file still missing, and warns W1 again. Kept, the dump records it.
    while topo.has_new() {
        topo.merge();
        for i in 0..topo.merged {
            if !topo.files[i].is_ref() {
                continue;
            }
            let name = topo.files[i].name.clone();
            let mut qfiles = load_from_path(&mut ctx, &roots, Path::new(&name));
            if plugin_failed(ctx.plugin) {
                return;
            }
            if qfiles.len() == 1 {
                let mut qf = qfiles.remove(0);
                qf.imports = r.moves.rewrite_names(qf.imports);
                topo.load(qf);
                if r.debug {
                    ctx.events.info(&format!("  Loading: {name}"));
                }
            }
        }
    }
    topo.merge();

    // The fallbacks replace what the input has under their names: a loaded
    // file keeps its imports as read, and takes the fallback's qfile.
    for proto in r.fallback_protos.clone() {
        r.pruned_fqdns.remove(&format!("file:{proto}"));
        let qf = embedded_fallback(r, &proto);
        if plugin_failed(r.plugin.as_ref()) {
            return;
        }
        if let Some(qf) = &qf {
            let mut qf = qf.clone();
            qf.imports = r.moves.rewrite_names(qf.imports);
            topo.load(qf);
            if !r.quiet {
                r.events
                    .attention(&format!("Using embedded fallback: {proto}"));
            }
        }
        topo.merge();
        if let (Some(qf), Some(i)) = (qf, topo.find(&proto)) {
            topo.files[i].qfile = Some(qf);
        }
    }
    fill_in(r, topo, false);
    // The descriptor known before the texts are read: their option names
    // are read against its options messages (spec 0447 S1).
    if r.infer_descriptor {
        infer_descriptor(r, topo);
    }
    fill_in(r, topo, true);
    read_text_files(r, topo);
    // Spec 0445 S7: W25 for an idle move, W26 for a target not loaded.
    r.moves.check_loaded(
        |name| topo.find(name).is_some_and(|i| !topo.files[i].is_ref()),
        &mut r.events,
    );
}

/// Spec 0447 S2: a built-in copy fills in a file imported but missing,
/// silently; a filled-in file's own imports, filled in in turn. The
/// descriptor apart (`descriptor`), once known (S1), even when nothing
/// imports it: every run reads it.
fn fill_in(r: &mut Run, topo: &mut Topology, descriptor: bool) {
    let mut filled = true;
    while filled {
        filled = false;
        for name in r.fill_in_protos.clone() {
            if (name == r.variant.descriptor_proto) != descriptor {
                continue;
            }
            let existing = topo.find(&name);
            if existing.is_none() && !descriptor {
                continue; // nothing imports it
            }
            if existing.is_some_and(|i| !topo.files[i].is_ref()) {
                continue; // the input has it
            }
            let Some(mut qf) = embedded_fallback(r, &name) else {
                continue;
            };
            qf.imports = r.moves.rewrite_names(qf.imports);
            let i = topo.load(qf.clone());
            topo.merge();
            if topo.files[i].qfile.is_none() {
                topo.files[i].qfile = Some(qf);
            }
            crate::report::filled_in(&name);
            filled = true;
        }
    }
}

/// The descriptor messages custom options and features extend.
const OPTION_CARRIERS: &[&str] = &[
    "FileOptions",
    "MessageOptions",
    "FieldOptions",
    "OneofOptions",
    "ExtensionRangeOptions",
    "EnumOptions",
    "EnumValueOptions",
    "ServiceOptions",
    "MethodOptions",
    "FeatureSet",
];

/// reproto's `_file_facts`: a loaded file's package, the extendees of its
/// extensions, and whether it defines `FileOptions`: from its descriptor,
/// or, a text not read yet, from a scan of it. In a text FDP, `extendee` and
/// `package` are string fields no other descriptor field shares the name of,
/// so the scan finds exactly them.
fn file_facts(qf: &QualFile) -> Option<(String, Vec<String>, bool)> {
    use prototext_upb::descriptor_access::FileDescriptorProto;
    use regex::Regex;
    use std::sync::LazyLock;
    static EXTENDEE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\bextendee\s*:\s*["']([^"']*)["']"#).expect("valid"));
    static PACKAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\bpackage\s*:\s*["']([^"']*)["']"#).expect("valid"));
    static FILE_OPTIONS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\bname\s*:\s*["']FileOptions["']"#).expect("valid"));
    if let Some(text) = &qf.text {
        let package = PACKAGE
            .captures(text)
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        let extendees = EXTENDEE
            .captures_iter(text)
            .map(|c| c[1].to_string())
            .collect();
        return Some((package, extendees, FILE_OPTIONS.is_match(text)));
    }
    let def = Pool::vendored().get_message_by_name("google.protobuf.FileDescriptorProto")?;
    let fdp = DynamicMessage::decode(&def, &qf.contents).ok()?;
    let f = FileDescriptorProto(fdp.as_ref());
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mut extendees: Vec<String> = f.extension().iter().map(|x| text(x.extendee())).collect();
    let mut todo = f.message_type();
    while let Some(m) = todo.pop() {
        extendees.extend(m.extension().iter().map(|x| text(x.extendee())));
        todo.extend(m.nested_type());
    }
    let defines = f.message_type().iter().any(|m| m.name() == b"FileOptions");
    Some((text(f.package()), extendees, defines))
}

/// reproto's `_infer_descriptor` (spec 0447 S1): the descriptor the input
/// was compiled against, from what its custom options and features extend,
/// before the texts are read. Extendees in protoc's package, or none:
/// protoc's descriptor, as before. In one other package: the loaded file
/// of that package defining `FileOptions`. In several: an error naming a
/// file extending each.
fn infer_descriptor(r: &mut Run, topo: &Topology) {
    let mut facts: Vec<(String, (String, Vec<String>, bool))> = topo
        .files
        .iter()
        .filter(|f| !f.is_ref())
        .filter_map(|f| Some((f.name.clone(), file_facts(f.qfile.as_ref()?)?)))
        .collect();
    facts.sort_by(|a, b| a.0.cmp(&b.0));
    let mut packages: std::collections::BTreeMap<String, String> = Default::default();
    for (name, (_, extendees, _)) in &facts {
        for e in extendees {
            let e = e.trim_start_matches('.');
            let (package, carrier) = e.rsplit_once('.').unwrap_or(("", e));
            if OPTION_CARRIERS.contains(&carrier) && package != "google.protobuf" {
                packages
                    .entry(package.to_string())
                    .or_insert_with(|| name.clone());
            }
        }
    }
    if packages.len() > 1 {
        let named: Vec<String> = packages
            .iter()
            .map(|(p, f)| format!("{p} (as in {f})"))
            .collect();
        r.descriptor_error = Some(format!(
            "the input's custom options extend the options of several descriptors: {}; \
             name the one to use with --descriptor",
            named.join(", ")
        ));
        return;
    }
    let Some(package) = packages.keys().next() else {
        return;
    };
    if let Some((name, _)) = facts
        .iter()
        .find(|(_, (p, _, defines))| p == package && *defines)
    {
        r.variant.descriptor_proto = name.clone();
    }
}

/// The variant's `.pb` for `proto_name`, its first file re-serialized.
fn embedded_fallback(r: &Run, proto_name: &str) -> Option<QualFile> {
    let pb_name = format!(
        "{}.pb",
        proto_name.strip_suffix(".proto").unwrap_or(proto_name)
    );
    let data = r.variant.resource(&pb_name)?;
    let def = Pool::vendored().get_message_by_name("google.protobuf.FileDescriptorSet")?;
    let set = DynamicMessage::decode(&def, &data).ok()?;
    let first = FileDescriptorSet(set.as_ref()).file().into_iter().next()?;
    let bytes = first.0.encode().ok()?;
    let (name, mut imports) = name_and_imports(&bytes)?;
    // The plugin sees the fallback too; the contents stay unpatched
    // (spec 0369 S1, spec 0431 S1).
    if let Some(p) = &r.plugin {
        imports = name_and_imports(&p.apply(&name, &bytes))
            .map(|(_, i)| i)
            .unwrap_or_default();
    }
    Some(QualFile {
        root: PathBuf::from("internal"),
        rel_path: PathBuf::from(proto_name),
        name,
        contents: bytes,
        text: None,
        from_text: false,
        imports,
    })
}

/// `_read_text_files`: every file into one pool, imports first, each text
/// parsed when its turn comes (spec 0427).
fn read_text_files(r: &mut Run, topo: &mut Topology) {
    let loaded: Vec<usize> = (0..topo.files.len())
        .filter(|&i| !topo.files[i].is_ref())
        .collect();
    let text_files: Vec<usize> = loaded
        .iter()
        .copied()
        .filter(|&i| {
            topo.files[i]
                .qfile
                .as_ref()
                .is_some_and(|q| q.text.is_some())
        })
        .collect();
    if text_files.is_empty() {
        return;
    }
    let mut inputs: Vec<Input> = Vec::new();
    for &i in &loaded {
        let qf = topo.files[i].qfile.as_ref().expect("loaded");
        if qf.text.is_none() {
            inputs.push(Input::Binary(&qf.contents));
        }
    }
    let binary_count = inputs.len();
    for &i in &text_files {
        inputs.push(Input::Text(
            topo.files[i]
                .qfile
                .as_ref()
                .and_then(|q| q.text.as_deref())
                .expect("text"),
        ));
    }
    let (_, reads, _) = read_input(&inputs, &r.variant.descriptor_proto);
    let reads: Vec<Read> = reads[binary_count..].to_vec();
    for (&i, read) in text_files.iter().zip(reads) {
        let name = topo.files[i]
            .qfile
            .as_ref()
            .map(|q| q.name.clone())
            .unwrap_or_default();
        match read {
            Read::Parsed {
                bytes, warnings, ..
            } => {
                // T1-T4 (spec 0438 S3): summarized by message, the files
                // listed under --detailed-warnings.
                for w in warnings {
                    let message = w.to_string();
                    let detailed = format!("{name}: {message}");
                    r.events.squash(w.code(), &message, None, Some(&detailed));
                }
                // The plugin sees the file read again; its imports are the
                // answer's, its contents the unpatched read (spec 0431 S1).
                let answer = r.plugin.as_ref().map(|p| p.apply(&name, &bytes));
                let qf = topo.files[i].qfile.as_mut().expect("loaded");
                qf.text = None;
                if let Some((_, imports)) = name_and_imports(answer.as_deref().unwrap_or(&bytes)) {
                    qf.imports = r.moves.rewrite_names(imports);
                }
                qf.contents = bytes;
            }
            Read::Error(e) => {
                r.events.warn("W15", &format!("Cannot parse '{name}': {e}"));
                topo.files[i].qfile = None;
            }
            Read::Binary => unreachable!("a text input"),
        }
    }
}
