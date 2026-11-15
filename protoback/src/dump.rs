// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The graph dump (spec 0428 S1–S6), as reproto's `dump_graph.py` writes
//! it, line for line.

use std::fmt::Write as _;

use prototext_upb::descriptor_access::FileDescriptorProto;
use sha2::{Digest, Sha256};

use crate::pathmatch::posix_str;
use crate::phase2::EditionDefault;
use crate::run::Run;
use crate::topology::Topology;

fn list<I: IntoIterator<Item = S>, S: AsRef<str>>(items: I) -> String {
    let v: Vec<String> = items.into_iter().map(|s| s.as_ref().to_string()).collect();
    format!("[{}]", v.join(", "))
}

fn files(topo: &Topology, out: &mut String) {
    for f in &topo.files {
        let Some(qf) = &f.qfile else {
            let _ = writeln!(out, "{} | ref", f.name);
            continue;
        };
        let mut imports: Vec<&str> = f
            .targets
            .iter()
            .map(|&t| topo.files[t].name.as_str())
            .collect();
        imports.sort();
        let _ = writeln!(
            out,
            "{} | file | {}/{} | {} | {} | {} | imports {} | stripped imports {} | stripped public {}",
            f.name,
            posix_str(&qf.root.to_string_lossy()),
            posix_str(&qf.rel_path.to_string_lossy()),
            if qf.from_text { "text" } else { "binary" },
            if f.is_seed { "seed" } else { "-" },
            if f.is_pruned { "pruned" } else { "-" },
            list(imports),
            list(&f.stripped_dependencies),
            list(&f.stripped_public_dependencies),
        );
        for (owner, records) in &f.stripped_field_types {
            for rec in records {
                let _ = writeln!(out, "  stripped field | {owner} | {}", rec.join(" | "));
            }
        }
        for (owner, records) in &f.stripped_method_types {
            for rec in records {
                let _ = writeln!(out, "  stripped method | {owner} | {}", rec.join(" | "));
            }
        }
    }
}

fn pool(r: &Run, out: &mut String) {
    for fdp in &r.db.files {
        let data = fdp.encode().unwrap_or_default();
        let name = String::from_utf8_lossy(FileDescriptorProto(fdp.as_ref()).name()).into_owned();
        let _ = writeln!(out, "{name} | {} | {:x}", data.len(), Sha256::digest(&data));
    }
}

fn context(r: &Run, out: &mut String) {
    let _ = writeln!(out, "pruned file names | {}", list(&r.pruned_file_names));
    let _ = writeln!(out, "pruned fqdns | {}", list(&r.pruned_fqdns));
    let _ = writeln!(out, "path seed fqdns | {}", list(&r.path_seed_fqdns));
    for (key, value) in &r.edition_defaults {
        match value {
            EditionDefault::Numbers(numbers) => {
                for (name, n) in numbers {
                    let _ = writeln!(out, "edition enum | {key} | {name} | {n}");
                }
            }
            EditionDefault::Defaults(defaults) => {
                for (edition, default) in defaults {
                    let _ = writeln!(out, "edition default | {key} | {edition} | {default}");
                }
            }
        }
    }
    let _ = writeln!(out, "schema db extra nodes | []");
}

/// The dump of what the phases ran so far leave.
pub fn write(r: &Run, topo: &Topology) -> String {
    let mut out = String::new();
    out.push_str("[files]\n");
    files(topo, &mut out);
    out.push_str("[pool]\n");
    pool(r, &mut out);
    out.push_str("[context]\n");
    context(r, &mut out);
    out.push_str("[nodes]\n[seeds]\n[warnings]\n");
    for line in &r.events.lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}
