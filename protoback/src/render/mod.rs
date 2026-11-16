// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Rendering, step 5 (spec 0429): reproto's phase 7, each summoned file's
//! `.proto` text. Stage 1, structure: no options, no comments from source
//! info, no binary output yet (spec 0429 S1).

mod binary;
mod comments;
mod file;
mod message;
mod names;
mod options;
mod pyfmt;
mod sci;
mod text;

use std::path::{Path, PathBuf};

use prototext_upb::descriptor_access::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    MethodDescriptorProto, ServiceDescriptorProto,
};
use prototext_upb::MessageRef;

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
    /// `--emit-binary`: each file's descriptor written beside its text.
    pub emit_binary: bool,
    /// `--emit-scoring-yaml`: each file's scoring graph beside its text.
    pub emit_scoring_yaml: bool,
    /// `--emit-extension-ranges`: each message's in its scoring graph.
    pub emit_extension_ranges: bool,
    /// Not `--no-source-info`: the descriptor's SourceCodeInfo synthesized
    /// from the text.
    pub source_info: bool,
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
    /// from its file, each step straight to the one child it names (a
    /// reader's `message_type()` would build every sibling's, which made a
    /// lookup quadratic). `None` for a stub.
    pub fn def(&self, i: usize) -> Option<Def<'a>> {
        fn step<'m>(m: MessageRef<'m>, number: u32, index: usize) -> Option<MessageRef<'m>> {
            match m.get_by_number(number)? {
                prototext_upb::Value::List(list) if index < list.len() => match list.get(index) {
                    prototext_upb::Value::Message(Some(r)) => Some(r),
                    _ => None,
                },
                _ => None,
            }
        }
        let r = self.graph.nodes[i].def.as_ref()?;
        let mut cur = Def::File(FileDescriptorProto(self.db.files.get(r.file)?.as_ref()));
        for &(number, index) in &r.path {
            cur = match (cur, number) {
                (Def::File(f), 4) => Def::Message(DescriptorProto(step(f.0, 4, index)?)),
                (Def::File(f), 5) => Def::Enum(EnumDescriptorProto(step(f.0, 5, index)?)),
                (Def::File(f), 6) => Def::Service(ServiceDescriptorProto(step(f.0, 6, index)?)),
                (Def::File(f), 7) => Def::Field(FieldDescriptorProto(step(f.0, 7, index)?)),
                (Def::Message(m), 2) => Def::Field(FieldDescriptorProto(step(m.0, 2, index)?)),
                (Def::Message(m), 3) => Def::Message(DescriptorProto(step(m.0, 3, index)?)),
                (Def::Message(m), 4) => Def::Enum(EnumDescriptorProto(step(m.0, 4, index)?)),
                (Def::Message(m), 6) => Def::Field(FieldDescriptorProto(step(m.0, 6, index)?)),
                (Def::Service(s), 2) => Def::Method(MethodDescriptorProto(step(s.0, 2, index)?)),
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
    /// `--jobs`: the threads files render on (spec 0429 S4).
    pub jobs: usize,
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
    on_pool(output.jobs, || {
        if !output.dry_run {
            write_files(shared, output, out_repo)?;
        }
        if shared.emit_scoring_yaml {
            // reproto's pool holds every summoned file once phase 7 has
            // rendered; under --dry-run without --emit-binary it renders
            // nothing, and each lookup below loads its file (spec 0430 S1).
            let rendered = !output.dry_run || shared.emit_binary;
            write_scoring_yaml(shared, out_repo, rendered)?;
        }
        Ok(())
    })
}

/// `f` on a pool of `jobs` threads (spec 0429 S4); on this thread when no
/// pool can be had.
fn on_pool<T: Send>(jobs: usize, f: impl FnOnce() -> T + Send) -> T {
    match rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .build()
    {
        Ok(pool) => pool.install(f),
        Err(_) => f(),
    }
}

/// The `.proto` files (and `.pb` under `--emit-binary`), then `buf.yaml`.
fn write_files(shared: &Shared<'_>, output: &Output, out_repo: &Path) -> std::io::Result<()> {
    let graph = shared.graph;
    // The files written, in reproto's order: (k, node, path).
    let mut plan: Vec<(usize, usize, PathBuf)> = Vec::new();
    for (k, i) in summoned(graph).into_iter().enumerate() {
        let name = graph.nodes[i]
            .fqdn
            .strip_prefix("file:")
            .unwrap_or_default();
        if name == shared.variant.descriptor_proto && !output.write_variant_descriptor {
            continue;
        }
        let canonical = shared
            .variant
            .canonize_dependency(name, shared.keep_variant_descriptor);
        plan.push((k, i, out_repo.join(&canonical)));
    }
    let rendered = render_all(shared, &plan);
    for ((_, _, path), r) in plan.iter().zip(rendered) {
        // reproto makes the directory first, a file it fails to render
        // (an edition Python protobuf does not name) included.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let Some((text, descriptor)) = r else {
            continue;
        };
        // reproto writes the descriptor first (`with_suffix(".pb")`).
        if let Some(bytes) = descriptor {
            std::fs::write(path.with_extension("pb"), bytes)?;
        }
        std::fs::write(path, text)?;
    }
    let buf = out_repo.join("buf.yaml");
    if !buf.exists() {
        std::fs::write(buf, BUF_YAML)?;
    }
    Ok(())
}

/// A file rendered: its text, and its descriptor under `--emit-binary`;
/// none for a file reproto does not write.
type Rendered = Option<(String, Option<Vec<u8>>)>;

/// The planned files rendered on the current pool (spec 0429 S4), each
/// with a context of its own, over the shared state rendering never
/// writes; collected in the plan's order, so the output is the same bytes
/// whatever the threads.
fn render_all(shared: &Shared<'_>, plan: &[(usize, usize, PathBuf)]) -> Vec<Rendered> {
    use rayon::prelude::*;
    plan.par_iter()
        .map(|&(k, i, _)| -> Rendered {
            let (block, descriptor) = file::render(shared, i, k)?;
            Some((
                block.flush(shared.redact_comments, shared.redact_orphans),
                descriptor,
            ))
        })
        .collect()
}

/// `--emit-scoring-yaml` (`_phase_emit_scoring_graphs`, spec 0430 S3):
/// each summoned file's scoring graph, `NAME.yaml` beside its `NAME.proto`
/// (the variant's descriptor.proto included), FQDNs as the input has them.
/// A file the pool refused gets none. Collected on the current pool,
/// written in reproto's order.
fn write_scoring_yaml(shared: &Shared<'_>, out_repo: &Path, rendered: bool) -> std::io::Result<()> {
    use rayon::prelude::*;
    let graph = shared.graph;
    let files = summoned(graph);
    let n = files.len();
    let identity = |name: &str| name.to_string();
    let docs: Vec<(PathBuf, Option<String>)> = files
        .par_iter()
        .enumerate()
        .map(|(j, &i)| {
            let name = graph.nodes[i]
                .fqdn
                .strip_prefix("file:")
                .unwrap_or_default();
            let c = crate::scoring::Collect {
                graph,
                loads: &shared.loads,
                upto: if rendered { n - 1 } else { j },
                extension_ranges: shared.emit_extension_ranges,
                rename: &identity,
            };
            let canonical = shared
                .variant
                .canonize_dependency(name, shared.keep_variant_descriptor);
            let path = out_repo.join(canonical).with_extension("yaml");
            let doc = crate::scoring::file(&c, name).map(|f| crate::scoring::pyyaml::dump(&f));
            (path, doc)
        })
        .collect();
    for (path, doc) in docs {
        let Some(doc) = doc else {
            continue;
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, doc)?;
    }
    Ok(())
}
