// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 1: the seed files, the files they import, the fallbacks, then
//! the files given as text, read in import order (reproto's
//! `_phase1_load_files` and `_read_text_files`).

use std::path::{Path, PathBuf};

use prototext_upb::descriptor_access::FileDescriptorSet;
use prototext_upb::input::{read_input, Input, Read};
use prototext_upb::DynamicMessage;

use crate::load::{descriptor_pool, load_from_path, name_and_imports, LoadCtx, QualFile};
use crate::run::Run;
use crate::topology::Topology;

pub fn run(r: &mut Run, topo: &mut Topology) {
    let roots = r.roots.clone();
    let seeds = r.pb_files.clone();
    let mut ctx = LoadCtx {
        pruned_paths: &r.pruned_paths,
        path_seeds: &r.path_seeds,
        events: &mut r.events,
        blobs: &mut r.blobs,
        path_seed_fqdns: &mut r.path_seed_fqdns,
    };
    let mut qual_files = Vec::new();
    for path in &seeds {
        qual_files.extend(load_from_path(&mut ctx, &roots, path));
    }
    for qf in qual_files {
        let i = topo.load(qf);
        topo.files[i].is_seed = true;
    }
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
            if qfiles.len() == 1 {
                topo.load(qfiles.remove(0));
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
        if let Some(qf) = &qf {
            topo.load(qf.clone());
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
    read_text_files(r, topo);
}

/// The variant's `.pb` for `proto_name`, its first file re-serialized.
fn embedded_fallback(r: &Run, proto_name: &str) -> Option<QualFile> {
    let pb_name = format!(
        "{}.pb",
        proto_name.strip_suffix(".proto").unwrap_or(proto_name)
    );
    let data = r.variant.resource(&pb_name)?;
    let def = descriptor_pool().get_message_by_name("google.protobuf.FileDescriptorSet")?;
    let set = DynamicMessage::decode(&def, &data).ok()?;
    let first = FileDescriptorSet(set.as_ref()).file().into_iter().next()?;
    let bytes = first.0.encode().ok()?;
    let (name, imports) = name_and_imports(&bytes)?;
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
                for w in warnings {
                    r.events.warning(&format!("{name}: {w}"));
                }
                let qf = topo.files[i].qfile.as_mut().expect("loaded");
                qf.text = None;
                if let Some((_, imports)) = name_and_imports(&bytes) {
                    qf.imports = imports;
                }
                qf.contents = bytes;
            }
            Read::Error(e) => {
                r.events.warning(&format!("Cannot parse '{name}': {e}"));
                topo.files[i].qfile = None;
            }
            Read::Binary => unreachable!("a text input"),
        }
    }
}
