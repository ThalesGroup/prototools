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
pub mod names;
mod options;
pub(crate) mod pyfmt;
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
use crate::events::Events;
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
#[derive(Clone)]
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
    /// reproto's options pool: the variant's descriptor.proto (a run whose
    /// descriptor.proto the pool refuses stops before rendering).
    pub options: &'a OptionsPool,
    /// What reproto's `ctx.pool` holds as rendering goes (`pool_loads`).
    pub loads: &'a PoolLoads,
    /// `--schema-db-out`: each rendered file's descriptor is collected,
    /// written or not (spec 0430 S4).
    pub schema_db: bool,
    /// `--debug`: the reasons behind fixed ones.
    pub debug: bool,
    /// Every loaded file's symbols, for the type names written (spec 0434).
    pub names: &'a names::SymbolTable,
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
    /// A rendered file's descriptor is built (`ctx.out_desc`): for
    /// `--emit-binary` or the schema database.
    pub fn wants_descriptor(&self) -> bool {
        self.emit_binary || self.schema_db
    }

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
    /// `--quiet`: reproto's bars count nothing then.
    pub quiet: bool,
    /// `--schema-db-out`: the database's path.
    pub schema_db: Option<PathBuf>,
    /// Phase 6's `schema_db_extra_nodes`: the WKT dependencies the
    /// database adds.
    pub extra_nodes: Vec<usize>,
    /// `--scoring-html-out`: the database's graph drawn, raw and minimized.
    pub scoring_html: Option<PathBuf>,
    /// `--with-leaf-nodes`: the leaves drawn too.
    pub with_leaf_nodes: bool,
    /// `--hide`: the states a root of which matches one of these globs.
    pub hide: Vec<String>,
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
/// Why a run stops: an output it cannot write, or reproto's own fatal
/// errors (a schema-database name collision), each exit status 1.
pub enum Failure {
    Io(std::io::Error),
    Fatal(String),
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Failure {
        Failure::Io(e)
    }
}

pub fn run(shared: &Shared<'_>, output: &Output, events: &mut Events) -> Result<(), Failure> {
    if output.out_repo.is_none() && output.schema_db.is_none() {
        return Ok(());
    }
    on_pool(output.jobs, || {
        // Without -O, reproto renders for the database alone, as a dry run.
        let dry_run = output.dry_run || output.out_repo.is_none();
        let collected = write_files(shared, output, dry_run, events)?;
        if let (true, Some(out_repo)) = (shared.emit_scoring_yaml, &output.out_repo) {
            // reproto's pool holds every summoned file once phase 7 has
            // rendered; when it renders nothing, each lookup below loads
            // its file (spec 0430 S1).
            let rendered = !dry_run || shared.wants_descriptor();
            write_scoring_yaml(shared, out_repo, rendered, events)?;
        }
        if let Some(db) = &output.schema_db {
            crate::schema_db::build(shared, output, collected, db, events)?;
        }
        Ok(())
    })
}

/// A render's reports into the collector, in order (spec 0433 S3): its
/// warnings printed, its W6 and patched files counted.
pub fn replay(events: &mut Events, reports: Vec<file::RenderEvent>) {
    for r in reports {
        match r {
            file::RenderEvent::Warning(m) => crate::console::warning(&m),
            file::RenderEvent::W6 {
                file,
                field,
                reason,
            } => events.w6(&file, &field, &reason),
            file::RenderEvent::Prost(name) => events.w_prost(&name),
        }
    }
}

/// The fixed reason of a file the pool refused (spec 0429 S16).
pub const POOL_REFUSED: &str = "the descriptor pool refused this file or one of its imports";

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

/// File `i`'s descriptor, rendered as the `k`-th file (a WKT dependency
/// of the schema database); `None` when its render fails.
pub fn descriptor(
    shared: &Shared<'_>,
    i: usize,
    k: usize,
) -> (Option<Vec<u8>>, Vec<file::RenderEvent>) {
    let r = file::render(shared, i, k);
    (r.output.and_then(|o| o.1), r.events)
}

/// One summoned file, as phase 7 takes it.
struct Planned {
    k: usize,
    i: usize,
    name: String,
    /// Where it is written: none in a dry run, or for the variant's
    /// descriptor.proto under `--no-emit-descriptor` (spec 0150).
    path: Option<PathBuf>,
}

/// Phase 7: the `.proto` files (and `.pb` under `--emit-binary`), then
/// `buf.yaml`; each descriptor collected for the schema database, written
/// or not. Renders only when something needs it, as reproto does.
fn write_files(
    shared: &Shared<'_>,
    output: &Output,
    dry_run: bool,
    events: &mut Events,
) -> std::io::Result<crate::schema_db::Collected> {
    let mut collected = Vec::new();
    let summoned_count = summoned(shared.graph).len() as u64;
    let phase = crate::console::phase(
        "Rendering proto files",
        crate::console::Label::Always,
        if output.quiet { 0 } else { summoned_count },
    );
    if dry_run && !shared.wants_descriptor() {
        phase.done();
        return Ok(collected);
    }
    let graph = shared.graph;
    let plan: Vec<Planned> = summoned(graph)
        .into_iter()
        .enumerate()
        .map(|(k, i)| {
            let name = graph.nodes[i]
                .fqdn
                .strip_prefix("file:")
                .unwrap_or_default()
                .to_string();
            let suppress =
                name == shared.variant.descriptor_proto && !output.write_variant_descriptor;
            let path = match (&output.out_repo, dry_run || suppress) {
                (Some(out_repo), false) => Some(
                    out_repo.join(
                        shared
                            .variant
                            .canonize_dependency(&name, shared.keep_variant_descriptor),
                    ),
                ),
                _ => None,
            };
            Planned { k, i, name, path }
        })
        .collect();
    let rendered = render_all(shared, &plan);
    for (p, (r, reports, failed)) in plan.iter().zip(rendered) {
        // reproto makes the directory first, a file it fails to render
        // (an edition Python protobuf does not name) included.
        if let Some(parent) = p.path.as_ref().and_then(|p| p.parent()) {
            std::fs::create_dir_all(parent)?;
        }
        replay(events, reports);
        phase.advance(1);
        if failed {
            // reproto's phase 7 catches the failure: W6, internal error.
            events.w6(&p.name, "", "internal error");
        }
        let Some((text, descriptor)) = r else {
            continue;
        };
        if let Some(bytes) = descriptor {
            // reproto writes the descriptor first (`with_suffix(".pb")`).
            if let (true, Some(path)) = (shared.emit_binary, &p.path) {
                std::fs::write(path.with_extension("pb"), &bytes)?;
            }
            if shared.schema_db {
                collected.push((p.name.clone(), bytes));
            }
        }
        if let Some(path) = &p.path {
            std::fs::write(path, text)?;
        }
    }
    phase.done();
    if let (false, Some(out_repo)) = (dry_run, &output.out_repo) {
        let buf = out_repo.join("buf.yaml");
        if !buf.exists() {
            std::fs::write(buf, BUF_YAML)?;
        }
    }
    Ok(collected)
}

/// A file rendered: its text, and its descriptor under `--emit-binary`;
/// none for a file reproto does not write.
type Rendered = (
    Option<(String, Option<Vec<u8>>)>,
    Vec<file::RenderEvent>,
    bool,
);

/// The planned files rendered on the current pool (spec 0429 S4), each
/// with a context of its own, over the shared state rendering never
/// writes; collected in the plan's order, so the output is the same bytes
/// whatever the threads.
fn render_all(shared: &Shared<'_>, plan: &[Planned]) -> Vec<Rendered> {
    use rayon::prelude::*;
    plan.par_iter()
        .map(|&Planned { k, i, .. }| -> Rendered {
            let r = file::render(shared, i, k);
            let output = r.output.map(|(block, descriptor)| {
                (
                    block.flush(shared.redact_comments, shared.redact_orphans),
                    descriptor,
                )
            });
            (output, r.events, r.failed)
        })
        .collect()
}

/// `--emit-scoring-yaml` (`_phase_emit_scoring_graphs`, spec 0430 S3):
/// each summoned file's scoring graph, `NAME.yaml` beside its `NAME.proto`
/// (the variant's descriptor.proto included), FQDNs as the input has them.
/// A file the pool refused gets none. Collected on the current pool,
/// written in reproto's order.
fn write_scoring_yaml(
    shared: &Shared<'_>,
    out_repo: &Path,
    rendered: bool,
    events: &mut Events,
) -> std::io::Result<()> {
    use rayon::prelude::*;
    let graph = shared.graph;
    let files = summoned(graph);
    let n = files.len();
    let identity = |name: &str| name.to_string();
    let docs: Vec<(String, PathBuf, Option<String>)> = files
        .par_iter()
        .enumerate()
        .map(|(j, &i)| {
            let name = graph.nodes[i]
                .fqdn
                .strip_prefix("file:")
                .unwrap_or_default();
            let c = crate::scoring::Collect {
                graph,
                loads: shared.loads,
                upto: if rendered { n - 1 } else { j },
                extension_ranges: shared.emit_extension_ranges,
                rename: &identity,
            };
            let canonical = shared
                .variant
                .canonize_dependency(name, shared.keep_variant_descriptor);
            let path = out_repo.join(canonical).with_extension("yaml");
            let doc = crate::scoring::file(&c, name).map(|f| crate::scoring::pyyaml::dump(&f));
            (name.to_string(), path, doc)
        })
        .collect();
    for (name, path, doc) in docs {
        let Some(doc) = doc else {
            // A file the pool refused: W6, its fixed reason (spec 0429 S16).
            events.w6(&name, "scoring graph", POOL_REFUSED);
            continue;
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, doc)?;
    }
    Ok(())
}
