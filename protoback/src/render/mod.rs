// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Rendering, step 5 (spec 0429): reproto's phase 7, each summoned file's
//! `.proto` text. Stage 1, structure: no options, no comments from source
//! info, no binary output yet (spec 0429 S1).

mod comments;
mod file;
mod message;
mod names;
mod options;
mod pyfmt;
mod text;

use std::path::PathBuf;

use prototext_upb::descriptor_access::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    MethodDescriptorProto, ServiceDescriptorProto,
};

use std::collections::BTreeSet;

use crate::db::Database;
use crate::graph::{Graph, Kind};
use crate::phase2::EditionDefaults;
use crate::topology::{File, Topology};
use crate::variant::Variant;

pub use file::options_fqns;
pub use options::{pool_loads, OptionsPool, PoolLoads};

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// What every file's render reads, never written while rendering: phases
/// 1 to 6's graph and pool, and the run's options (spec 0429 S4).
pub struct Shared<'a> {
    pub graph: &'a Graph,
    pub db: &'a Database,
    pub topo: &'a Topology,
    pub variant: &'a Variant,
    pub edition_defaults: &'a EditionDefaults,
    /// The `*Options` a proto3 file may extend (`options_fqns`).
    pub options_fqns: BTreeSet<String>,
    /// `--keep-descriptor-path`.
    pub keep_variant_descriptor: bool,
    /// `--force-proto2-output`: every file written as proto2.
    pub force_proto2_output: bool,
    /// `--force-proto2-for-editions` (`--prost-workaround`): editions
    /// files written as proto2.
    pub force_proto2_for_editions: bool,
    pub redact_comments: bool,
    pub redact_orphans: bool,
    /// reproto's options pool: the variant's descriptor.proto.
    pub options: Option<OptionsPool>,
    /// What reproto's `ctx.pool` holds as rendering goes (`pool_loads`).
    pub loads: PoolLoads,
}

/// A node's definition, read from the pool's file it is in.
pub enum Def<'a> {
    File(FileDescriptorProto<'a>),
    Message(DescriptorProto<'a>),
    Field(FieldDescriptorProto<'a>),
    Enum(EnumDescriptorProto<'a>),
    Service(ServiceDescriptorProto<'a>),
    Method(MethodDescriptorProto<'a>),
}

impl<'a> Shared<'a> {
    /// The definition of node `i` (`node.this`): its `DefRef`'s path walked
    /// from its file. `None` for a stub.
    pub fn def(&self, i: usize) -> Option<Def<'a>> {
        let r = self.graph.nodes[i].def.as_ref()?;
        let mut cur = Def::File(FileDescriptorProto(self.db.files.get(r.file)?.as_ref()));
        for &(number, index) in &r.path {
            cur = match (cur, number) {
                (Def::File(f), 4) => Def::Message(f.message_type().into_iter().nth(index)?),
                (Def::File(f), 5) => Def::Enum(f.enum_type().into_iter().nth(index)?),
                (Def::File(f), 6) => Def::Service(f.service().into_iter().nth(index)?),
                (Def::File(f), 7) => Def::Field(f.extension().into_iter().nth(index)?),
                (Def::Message(m), 2) => Def::Field(m.field().into_iter().nth(index)?),
                (Def::Message(m), 3) => Def::Message(m.nested_type().into_iter().nth(index)?),
                (Def::Message(m), 4) => Def::Enum(m.enum_type().into_iter().nth(index)?),
                (Def::Message(m), 6) => Def::Field(m.extension().into_iter().nth(index)?),
                (Def::Service(s), 2) => Def::Method(s.method().into_iter().nth(index)?),
                _ => return None,
            };
        }
        Some(cur)
    }

    pub fn file_def(&self, i: usize) -> Option<FileDescriptorProto<'a>> {
        match self.def(i)? {
            Def::File(f) => Some(f),
            _ => None,
        }
    }

    pub fn message_def(&self, i: usize) -> Option<DescriptorProto<'a>> {
        match self.def(i)? {
            Def::Message(m) => Some(m),
            _ => None,
        }
    }

    /// A file node's package (empty for a stub).
    pub fn file_package(&self, file: usize) -> String {
        self.file_def(file)
            .map_or_else(String::new, |f| text(f.package()))
    }

    /// The node of `kind` named `name` under node `parent`'s prefix
    /// (`ReX(ctx, proto, parent=…)`, which finds the node phase 3 made).
    pub fn child(&self, kind: Kind, parent: usize, name: &[u8]) -> Option<usize> {
        let prefix = &self.graph.nodes[parent].prefix;
        self.graph
            .find(&format!("{}:{prefix}.{}", kind.prefix(), text(name)))
    }

    /// The file node a node is in (`get_file_node`).
    pub fn file_of(&self, mut i: usize) -> usize {
        while let Some(p) = self.graph.nodes[i].parent {
            i = p;
        }
        i
    }

    /// The topology's file of that name: what phase 2 stripped from it.
    pub fn topo_file(&self, name: &str) -> Option<&'a File> {
        self.topo.find(name).map(|i| &self.topo.files[i])
    }
}

/// The output's options (reproto's `_phase7_output`).
pub struct Output {
    /// The `-O` directory; none renders nothing (a schema database alone).
    pub out_repo: Option<PathBuf>,
    pub dry_run: bool,
    /// `--emit-descriptor` (default) or `--no-emit-descriptor`: whether
    /// the variant's descriptor.proto is written.
    pub write_variant_descriptor: bool,
}

/// What `buf lsp serve` needs at the output's root (spec 0146).
const BUF_YAML: &str = "# Auto-generated by reproto \u{2014} lets `buf lsp serve` (used by\n\
# protolens's Neovim integration, spec 0145) resolve imports\n\
# rooted at this directory. Safe to edit or delete.\n\
version: v2\n\
modules:\n  - path: .\n";

/// The summoned, present file nodes, in node order: what phase 7
/// renders, in the order it renders them; the order `pool_loads` follows.
pub fn summoned(graph: &Graph) -> Vec<usize> {
    (0..graph.nodes.len())
        .filter(|&i| {
            let n = &graph.nodes[i];
            n.kind == Kind::File && n.present && n.is_summoned
        })
        .collect()
}

/// The same files, by name.
pub fn summoned_files(graph: &Graph) -> Vec<String> {
    summoned(graph)
        .into_iter()
        .map(|i| {
            graph.nodes[i]
                .fqdn
                .strip_prefix("file:")
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Phase 7: each summoned, present file, in node order, rendered and
/// written under its canonical name; then `buf.yaml`.
pub fn run(shared: &Shared<'_>, output: &Output) -> std::io::Result<()> {
    let Some(out_repo) = &output.out_repo else {
        return Ok(());
    };
    let graph = shared.graph;
    for (k, i) in summoned(graph).into_iter().enumerate() {
        let name = graph.nodes[i]
            .fqdn
            .strip_prefix("file:")
            .unwrap_or_default();
        let suppress = name == shared.variant.descriptor_proto && !output.write_variant_descriptor;
        let canonical = shared
            .variant
            .canonize_dependency(name, shared.keep_variant_descriptor);
        let path = out_repo.join(&canonical);
        if output.dry_run {
            continue;
        }
        if suppress {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A file reproto cannot render is not written (an edition Python
        // protobuf does not name).
        let Some(block) = file::render(shared, i, k) else {
            continue;
        };
        std::fs::write(
            &path,
            block.flush(shared.redact_comments, shared.redact_orphans),
        )?;
    }
    if !output.dry_run {
        let buf = out_repo.join("buf.yaml");
        if !buf.exists() {
            std::fs::write(buf, BUF_YAML)?;
        }
    }
    Ok(())
}
