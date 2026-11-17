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

use crate::hash::HashMap;
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
    /// Each node's definition, found once (`find_defs`, spec 0442 S3).
    pub defs: &'a [Option<Def<'a>>],
}

/// A node's definition, read from the pool's file it is in.
#[derive(Clone)]
pub enum Def<'a> {
    File(FileDescriptorProto<'a>),
    Message(DescriptorProto<'a>),
    Field(FieldDescriptorProto<'a>),
    Enum(EnumDescriptorProto<'a>),
    Service(ServiceDescriptorProto<'a>),
    Method(MethodDescriptorProto<'a>),
}

/// Each node's definition (spec 0442 S3): its `DefRef`'s path walked down
/// from its file's root once (file, then field number and index, as
/// message 3, nested message 1, field 2), each step straight to the one
/// child it names (a reader's `message_type()` would build every sibling's,
/// which made a lookup quadratic); `Shared::def` walked it at every call.
pub fn find_defs<'a>(graph: &Graph, db: &'a Database) -> Vec<Option<Def<'a>>> {
    fn step<'m>(m: MessageRef<'m>, number: u32, index: usize) -> Option<MessageRef<'m>> {
        match m.get_by_number(number)? {
            prototext_upb::Value::List(list) if index < list.len() => match list.get(index) {
                prototext_upb::Value::Message(Some(r)) => Some(r),
                _ => None,
            },
            _ => None,
        }
    }
    let find = |i: usize| -> Option<Def<'a>> {
        let r = graph.nodes[i].def.as_ref()?;
        let mut cur = Def::File(FileDescriptorProto(db.files.get(r.file)?.as_ref()));
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
    };
    (0..graph.nodes.len()).map(find).collect()
}

impl<'a> Shared<'a> {
    /// A rendered file's descriptor is built (`ctx.out_desc`): for
    /// `--emit-binary` or the schema database.
    pub fn wants_descriptor(&self) -> bool {
        self.emit_binary || self.schema_db
    }

    /// The definition of node `i` (`node.this`), as `find_defs` found it
    /// (spec 0442 S3). `None` for a stub.
    pub fn def(&self, i: usize) -> Option<Def<'a>> {
        self.defs.get(i)?.clone()
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

/// `'a', 'b' and 'c'`.
fn quoted_list(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Node `i` and everything it contains out of the output.
fn unsummon(graph: &mut Graph, i: usize) {
    graph.nodes[i].is_summoned = false;
    let children: Vec<usize> = graph.nodes[i].contains.iter().copied().collect();
    for c in children {
        unsummon(graph, c);
    }
}

/// `_resolve_rewrite_collisions` (spec 0437): the summoned files the import
/// rewrites give one output path. A group holding the variant's
/// descriptor.proto keeps it, the others taken out of the output, each
/// with its warning (returned, in order); any other group is an error
/// line, and the run stops before anything is rendered.
pub fn resolve_rewrite_collisions(
    graph: &mut Graph,
    variant: &crate::variant::Variant,
    keep_descriptor: bool,
) -> Result<Vec<String>, Vec<String>> {
    let mut by_path: Vec<(String, Vec<(usize, String)>)> = Vec::new();
    for i in summoned(graph) {
        let name = graph.nodes[i]
            .fqdn
            .strip_prefix("file:")
            .unwrap_or_default()
            .to_string();
        let path = variant.canonize_dependency(&name, keep_descriptor);
        match by_path.iter_mut().find(|(p, _)| *p == path) {
            Some((_, files)) => files.push((i, name)),
            None => by_path.push((path, vec![(i, name)])),
        }
    }
    let descriptor = &variant.descriptor_proto;
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    for (path, files) in by_path {
        if files.len() < 2 {
            continue;
        }
        if !files.iter().any(|(_, n)| n == descriptor) {
            let names: Vec<&str> = files.iter().map(|(_, n)| n.as_str()).collect();
            errors.push(format!(
                "{} are both written as '{path}'; prune one (-p)",
                quoted_list(&names)
            ));
            continue;
        }
        for (i, name) in files {
            if name != *descriptor {
                warnings.push(format!(
                    "'{name}' and '{descriptor}' are both written as '{path}': \
                     '{descriptor}', the variant's descriptor.proto, is kept"
                ));
                unsummon(graph, i);
            }
        }
    }
    // No namespace rule renaming: no symbol can collide (spec 0443 P1).
    if variant.renames_symbols(keep_descriptor) {
        errors.extend(symbol_collisions(graph, variant, keep_descriptor));
    }
    if errors.is_empty() {
        Ok(warnings)
    } else {
        Err(errors)
    }
}

/// `_symbol_collisions` (spec 0440 S2): two symbols of the summoned files,
/// of two input names, with one output name, one line per name, the
/// outermost only (two colliding messages, not each of their fields).
fn symbol_collisions(
    graph: &Graph,
    variant: &crate::variant::Variant,
    keep_descriptor: bool,
) -> Vec<String> {
    const KINDS: [&str; 5] = ["desc", "enum", "serv", "meth", "fdsc"];
    // Output name -> (input name, file), in node order.
    // Sized up front: grown as it went, rehashing was most of its cost.
    let n = graph.nodes.len();
    let mut by_name: Vec<(String, Vec<(String, String)>)> = Vec::with_capacity(n);
    let mut index: HashMap<String, usize> =
        HashMap::with_capacity_and_hasher(n, Default::default());
    for node in &graph.nodes {
        let Some((kind, name)) = node.fqdn.split_once(':') else {
            continue;
        };
        if !KINDS.contains(&kind) || !node.present || !node.is_summoned {
            continue;
        }
        let name = name.trim_start_matches('.').to_string();
        let mut f = node;
        while let Some(p) = f.parent {
            f = &graph.nodes[p];
        }
        if !f.is_summoned {
            continue; // a copy 0437 dropped
        }
        let file = f.fqdn.strip_prefix("file:").unwrap_or_default().to_string();
        let out = variant.output_symbol(&name, keep_descriptor);
        let at = *index.entry(out.clone()).or_insert_with(|| {
            by_name.push((out, Vec::new()));
            by_name.len() - 1
        });
        let defs = &mut by_name[at].1;
        if !defs.iter().any(|(n, _)| *n == name) {
            defs.push((name, file));
        }
    }
    let colliding = |out: &str| index.get(out).is_some_and(|&i| by_name[i].1.len() > 1);
    let mut lines = Vec::new();
    for (out, defs) in &by_name {
        if defs.len() < 2 {
            continue;
        }
        let mut parent = out.rsplit_once('.').map_or("", |(p, _)| p);
        while !parent.is_empty() && !index.contains_key(parent) {
            parent = parent.rsplit_once('.').map_or("", |(p, _)| p);
        }
        if !parent.is_empty() && colliding(parent) {
            continue;
        }
        let files: Vec<&str> = defs.iter().map(|(_, f)| f.as_str()).collect();
        lines.push(format!(
            "{} both define '{out}' once rewritten; prune one (-p)",
            quoted_list(&files)
        ));
    }
    lines
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
        // With a database, its scoring graphs collected first, on every
        // thread; then their merge, on this thread, beside rendering on the
        // others (spec 0443 P6): the longest chain started first.
        let (collected, speculation) = match output.schema_db {
            Some(_) => {
                let mut speculation = crate::schema_db::speculate(shared, output);
                let html = output.scoring_html.is_some();
                let ((), collected) = rayon::join(
                    || crate::affinity::on_fast(|| speculation.compile(html)),
                    || write_files(shared, output, dry_run, events),
                );
                (collected?, Some(speculation))
            }
            None => (write_files(shared, output, dry_run, events)?, None),
        };
        if let (true, Some(out_repo)) = (shared.emit_scoring_yaml, &output.out_repo) {
            // Every summoned file loaded, rendered or not (spec 0430 S1).
            write_scoring_yaml(shared, out_repo, events)?;
        }
        if let Some(db) = &output.schema_db {
            crate::schema_db::build(shared, output, collected, db, events, speculation)?;
        }
        Ok(())
    })
}

/// A render's reports into the collector, in order (spec 0433 S3): its
/// warnings printed, its W6 and patched files counted.
pub fn replay(events: &mut Events, reports: Vec<file::RenderEvent>) {
    for r in reports {
        match r {
            file::RenderEvent::Warning(code, m) => events.warn(code, &m),
            file::RenderEvent::Squash(code, m) => events.squash(code, &m, None, None),
            // A file the pool refuses is W19; anything else not rendered, W6.
            file::RenderEvent::W6 {
                file,
                field,
                reason,
            } => {
                let code = if reason == POOL_REFUSED { "W19" } else { "W6" };
                events.w6(code, &file, &field, &reason)
            }
            file::RenderEvent::Prost(name) => events.w_prost(&name),
        }
    }
}

/// The fixed reason of a file the pool refused (spec 0429 S16).
pub const POOL_REFUSED: &str = "the descriptor pool refused this file or one of its imports";

/// `f` on a pool of `jobs` threads (spec 0429 S4); on this thread when no
/// pool can be had. Each pool thread first takes every CPU the process
/// inherited, the main thread's confinement to the fast ones being
/// inherited too (spec 0444 S3).
fn on_pool<T: Send>(jobs: usize, f: impl FnOnce() -> T + Send) -> T {
    match rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .start_handler(|_| crate::affinity::widen())
        .build()
    {
        Ok(pool) => pool.install(f),
        Err(_) => f(),
    }
}

/// File `i`'s descriptor (a WKT dependency of the schema database);
/// `None` when its render fails.
pub fn descriptor(shared: &Shared<'_>, i: usize) -> (Option<Vec<u8>>, Vec<file::RenderEvent>) {
    let r = file::render(shared, i);
    (r.output.and_then(|o| o.1), r.events)
}

/// One summoned file, as phase 7 takes it.
struct Planned {
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
        .map(|i| {
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
            Planned { i, name, path }
        })
        .collect();
    // Each file written by the thread that rendered it (spec 0443 P4);
    // here, in file order, what needs it: the warnings replayed, the
    // descriptors collected, the first I/O error raised where the
    // sequential writes raised it.
    let rendered = render_all(shared, &plan);
    for (p, r) in plan.iter().zip(rendered) {
        if let Err(Written::Directory(e)) = r.written {
            return Err(e);
        }
        replay(events, r.reports);
        phase.advance(1);
        if r.failed {
            // reproto's phase 7 catches the failure: W6, internal error.
            events.w6("W6", &p.name, "", "internal error");
        }
        if let Err(Written::File(e)) = r.written {
            return Err(e);
        }
        if let (true, Some(bytes)) = (shared.schema_db, r.descriptor) {
            collected.push((p.name.clone(), bytes));
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

/// A file rendered and written: its descriptor, for the schema database;
/// what its render reports; whether the render failed; and whether its
/// writes succeeded.
struct Rendered {
    descriptor: Option<Vec<u8>>,
    reports: Vec<file::RenderEvent>,
    failed: bool,
    written: Result<(), Written>,
}

/// Where a file's writes failed: its directory, which reproto makes before
/// anything else, or its files.
enum Written {
    Directory(std::io::Error),
    File(std::io::Error),
}

/// File `p`'s outputs written, in reproto's order: its directory (made
/// even for a file that fails to render: an edition Python protobuf does
/// not name), its descriptor (`with_suffix(".pb")`), its text.
fn write_rendered(
    shared: &Shared<'_>,
    p: &Planned,
    output: Option<&(String, Option<Vec<u8>>)>,
) -> Result<(), Written> {
    let Some(path) = &p.path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(Written::Directory)?;
    }
    let Some((text, descriptor)) = output else {
        return Ok(());
    };
    if let (true, Some(bytes)) = (shared.emit_binary, descriptor) {
        std::fs::write(path.with_extension("pb"), bytes).map_err(Written::File)?;
    }
    std::fs::write(path, text).map_err(Written::File)
}

/// The planned files rendered on the current pool (spec 0429 S4), each
/// with a context of its own, over the shared state rendering never
/// writes; collected in the plan's order, so the output is the same bytes
/// whatever the threads.
fn render_all(shared: &Shared<'_>, plan: &[Planned]) -> Vec<Rendered> {
    use rayon::prelude::*;
    plan.par_iter()
        .map(|p| -> Rendered {
            let r = file::render(shared, p.i);
            let output = r.output.map(|(block, descriptor)| {
                (
                    block.flush(shared.redact_comments, shared.redact_orphans),
                    descriptor,
                )
            });
            let written = write_rendered(shared, p, output.as_ref());
            Rendered {
                descriptor: output.and_then(|(_, descriptor)| descriptor),
                reports: r.events,
                failed: r.failed,
                written,
            }
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
    events: &mut Events,
) -> std::io::Result<()> {
    use rayon::prelude::*;
    let graph = shared.graph;
    let files = summoned(graph);
    let identity = |name: &str| name.to_string();
    let docs: Vec<(String, PathBuf, Option<String>)> = files
        .par_iter()
        .map(|&i| {
            let name = graph.nodes[i]
                .fqdn
                .strip_prefix("file:")
                .unwrap_or_default();
            let c = crate::scoring::Collect {
                graph,
                loads: shared.loads,
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
            events.w6("W21", &name, "scoring graph", POOL_REFUSED);
            continue;
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, doc)?;
    }
    Ok(())
}
