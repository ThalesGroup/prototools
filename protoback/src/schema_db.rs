// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `--schema-db-out DB.desc` (spec 0430 S4–S7): reproto's
//! `_phase_build_schema_db`.
//!
//! The descriptors phase 7 collected, then the WKT dependencies phase 6
//! set aside, rendered with their subtrees promoted; their scoring graphs,
//! merged and minimized by `prototext-graph` without YAML; `DB.desc`, the
//! descriptors in dependency order; `DB/hopcroft.rkyv`; `DB/index.rkyv`.

use crate::hash::{HashMap, HashSet};
use std::path::Path;

use prototext_graph::build_scoring_graph::{build_from_files, ScoringFile};
use prototext_graph::fds_index::{canonical_map, to_bytes as index_bytes, FdsIndex};
use prototext_upb::descriptor_access::{DescriptorProto, FileDescriptorProto};
use prototext_upb::{DynamicMessage, Pool};

use crate::console;
use crate::events::Events;
use crate::graph::Graph;
use crate::render::{self, Failure, Output, Shared};

/// A descriptor collected for the database: the input file it was
/// rendered from, and its bytes.
pub type Collected = Vec<(String, Vec<u8>)>;

/// Node `i` and everything it contains, summoned and reachable
/// (`_promote_subtree`).
fn promote(graph: &mut Graph, i: usize) {
    let mut stack = vec![i];
    while let Some(n) = stack.pop() {
        let node = &mut graph.nodes[n];
        node.is_summoned = true;
        node.is_reachable = true;
        stack.extend(node.contains.iter().copied());
    }
}

fn name(graph: &Graph, i: usize) -> String {
    graph.nodes[i]
        .fqdn
        .strip_prefix("file:")
        .unwrap_or_default()
        .to_string()
}

/// The merged, minimized graph: its rkyv bytes, and with `html` the
/// minimized and raw graphs' YAML; or the error's message.
type Compiled = Result<(Vec<u8>, Option<String>, Option<String>), String>;

/// Steps 2 and 3 of `build`, run ahead (spec 0443 P6) for the files the
/// database will hold if every render succeeds: the summoned files, then
/// the WKT dependencies. Step 2 first, on every thread (`speculate`); then
/// step 3, the merge on one thread, beside rendering (`compile`): the
/// longest chain started first, rendering filling the other threads. Used
/// when the files collected are those (`build`). Prints nothing: the
/// phases' labels and the warnings come from `build`, in their order.
pub struct Speculation {
    files: Vec<String>,
    graphs: Graphs,
    compiled: Option<Compiled>,
}

/// Step 2's result: whether each file got its graph (none for a file the
/// pool refused), and the graphs it got, in order.
struct Graphs {
    present: Vec<bool>,
    kept: Vec<ScoringFile>,
}

impl Speculation {
    /// Step 2's graphs for `files`, taken from the speculation's when it
    /// holds every one (a render failed: the files collected are the ones
    /// it expected, less those), so that only step 3 runs again; `None`
    /// when a file is not among them.
    fn regraph(self, files: &[&str]) -> Option<Graphs> {
        let at: HashMap<&str, usize> = self
            .files
            .iter()
            .enumerate()
            .map(|(j, f)| (f.as_str(), j))
            .collect();
        let positions: Vec<usize> = files
            .iter()
            .map(|f| at.get(f).copied())
            .collect::<Option<_>>()?;
        // Each expected file's graph by position, none where it had none.
        let mut kept = self.graphs.kept.into_iter();
        let mut by_position: Vec<Option<ScoringFile>> = self
            .graphs
            .present
            .iter()
            .map(|&p| if p { kept.next() } else { None })
            .collect();
        let graphs: Vec<Option<ScoringFile>> =
            positions.iter().map(|&j| by_position[j].take()).collect();
        Some(Graphs {
            present: graphs.iter().map(Option::is_some).collect(),
            kept: graphs.into_iter().flatten().collect(),
        })
    }
}

pub fn speculate(shared: &Shared<'_>, output: &Output) -> Speculation {
    let mut seen = HashSet::default();
    let files: Vec<String> = render::summoned_files(shared.graph)
        .into_iter()
        .chain(output.extra_nodes.iter().map(|&x| name(shared.graph, x)))
        .filter(|f| seen.insert(f.clone()))
        .collect();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let graphs = collect_graphs(shared, &refs);
    Speculation {
        files,
        graphs,
        compiled: None,
    }
}

impl Speculation {
    /// Step 3 on the speculation's graphs.
    pub fn compile(&mut self, html: bool) {
        if !self.graphs.kept.is_empty() {
            self.compiled = Some(compile(&self.graphs.kept, html, |_, _| {}));
        }
    }
}

/// Step 2: each file's scoring graph, names renamed as `.desc` renames
/// them (spec 0430 S6); none for a file the pool refused.
fn collect_graphs(shared: &Shared<'_>, files: &[&str]) -> Graphs {
    use rayon::prelude::*;
    let rename = |full: &str| -> String {
        shared
            .variant
            .output_symbol(full, shared.keep_variant_descriptor)
    };
    let graphs: Vec<Option<ScoringFile>> = files
        .par_iter()
        .map(|origin| {
            let c = crate::scoring::Collect {
                graph: shared.graph,
                loads: shared.loads,
                extension_ranges: shared.emit_extension_ranges,
                rename: &rename,
            };
            crate::scoring::file(&c, origin)
        })
        .collect();
    Graphs {
        present: graphs.iter().map(Option::is_some).collect(),
        kept: graphs.into_iter().flatten().collect(),
    }
}

/// Step 3: the graphs merged and minimized.
fn compile(graphs: &[ScoringFile], html: bool, progress: impl FnMut(u64, u64)) -> Compiled {
    build_from_files(graphs, html, html, progress).map_err(|e| e.to_string())
}

/// The database, from the descriptors phase 7 collected; `extras`,
/// phase 6's `schema_db_extra_nodes`; `speculation`, steps 2 and 3 run
/// beside rendering.
pub fn build(
    shared: &Shared<'_>,
    output: &Output,
    mut collected: Collected,
    db_path: &Path,
    events: &mut Events,
    speculation: Option<Speculation>,
) -> Result<(), Failure> {
    let extras = &output.extra_nodes;

    // 1. The WKT dependencies, rendered after phase 7, one by one: each
    // sees the promotions before it (spec 0430 S4). A failed render is
    // left out, silently.
    let wkt = console::phase(
        "Rendering WKT proto files",
        console::Label::OnTerminal,
        extras.len() as u64,
    );
    let mut graph = shared.graph.clone();
    for &x in extras {
        promote(&mut graph, x);
        let promoted = Shared {
            graph: &graph,
            ..shared.clone()
        };
        let (bytes, reports) = render::descriptor(&promoted, x);
        render::replay(events, reports);
        wkt.advance(1);
        if let Some(bytes) = bytes {
            collected.push((name(&graph, x), bytes));
        }
    }

    // 2. Each file's scoring graph, once, in collection order, names
    // renamed as `.desc` renames them (spec 0430 S6). A file the pool
    // refused gets none (W6, step 9).
    wkt.done();
    let mut seen = HashSet::default();
    let scoring_files: Vec<&str> = collected
        .iter()
        .map(|(origin, _)| origin.as_str())
        .filter(|o| seen.insert(*o))
        .collect();
    let collecting = console::phase(
        "Collecting scoring data",
        console::Label::Always,
        if output.quiet {
            0
        } else {
            scoring_files.len() as u64
        },
    );
    // The speculation's, when it was for these files (spec 0443 P6).
    // The speculation's (spec 0443 P6): all of it when it was for these
    // files; else its graphs of these files, step 3 to run again; else
    // step 2 too.
    let (Graphs { present, kept }, compiled) = match speculation {
        Some(s)
            if s.files
                .iter()
                .map(String::as_str)
                .eq(scoring_files.iter().copied()) =>
        {
            (s.graphs, s.compiled)
        }
        Some(s) => match s.regraph(&scoring_files) {
            Some(graphs) => (graphs, None),
            None => (collect_graphs(shared, &scoring_files), None),
        },
        None => (collect_graphs(shared, &scoring_files), None),
    };
    for (origin, _) in scoring_files.iter().zip(&present).filter(|(_, &p)| !p) {
        // A file the pool refused: W6, its fixed reason (spec 0429 S16).
        events.w6("W20", origin, "schema db", render::POOL_REFUSED);
    }
    let graphs = kept;
    collecting.done();
    if graphs.is_empty() {
        events.w6(
            "W20",
            "--build-schema-db",
            "schema db",
            "no scoring graphs generated; skipping",
        );
        return Ok(());
    }

    // 3. The merged, minimized graph; with --scoring-html-out, the raw and
    // minimized graphs' YAML the pages are drawn from.
    let html = output.scoring_html.is_some();
    let compiling = console::phase("Compiling scoring graph", console::Label::OnTerminal, 0);
    let built = match compiled {
        Some(compiled) => compiled,
        None => compile(&graphs, html, |current, total| {
            compiling.set(current, total);
        }),
    };
    compiling.done();
    let (rkyv, minimized_yaml, raw_yaml) =
        built.map_err(|e| Failure::Fatal(format!("Error: building the scoring graph: {e}")))?;

    // 4. The descriptors: a name given two contents stops the run; the
    // rest in reproto's dependency order (Kahn's rounds), duplicates kept.
    let fdp_type = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .ok_or_else(|| Failure::Fatal("Error: no FileDescriptorProto type".into()))?;
    let decoded: Vec<DynamicMessage> = collected
        .iter()
        .map(|(_, bytes)| {
            DynamicMessage::decode(&fdp_type, bytes)
                .map_err(|e| Failure::Fatal(format!("Error: a rendered descriptor: {e}")))
        })
        .collect::<Result<_, _>>()?;
    let names: Vec<String> = decoded
        .iter()
        .map(|d| text(FileDescriptorProto(d.as_ref()).name()))
        .collect();
    let mut first: HashMap<&str, usize> = HashMap::default();
    for (j, name) in names.iter().enumerate() {
        match first.get(name.as_str()) {
            Some(&p) if collected[p].1 != collected[j].1 => {
                return Err(Failure::Fatal(format!(
                    "error: schema-db name collision: '{}' and '{}' both canonize to '{name}' \
                     with different content \u{2014} cannot build a valid FileDescriptorSet. \
                     Use -p/--prune to remove one of the colliding sources.",
                    collected[p].0, collected[j].0
                )));
            }
            Some(_) => {}
            None => {
                first.insert(name, j);
            }
        }
    }
    let deps: Vec<Vec<String>> = decoded
        .iter()
        .map(|d| {
            FileDescriptorProto(d.as_ref())
                .dependency()
                .iter()
                .map(|x| text(x))
                .collect()
        })
        .collect();
    let mut placed: HashSet<&str> = HashSet::default();
    let mut order = Vec::with_capacity(collected.len());
    let mut remaining: Vec<usize> = (0..collected.len()).collect();
    for _ in 0..=collected.len() {
        if remaining.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for &j in &remaining {
            let ready = deps[j]
                .iter()
                .all(|d| !first.contains_key(d.as_str()) || placed.contains(d.as_str()));
            if ready {
                order.push(j);
                placed.insert(names[j].as_str());
            } else {
                next.push(j);
            }
        }
        remaining = next;
    }
    order.extend(remaining);

    // 5. `DB.desc`: a FileDescriptorSet, each file a field 1 record.
    let mut raw = Vec::new();
    let mut spans = Vec::with_capacity(order.len());
    for &j in &order {
        let bytes = &collected[j].1;
        raw.push(0x0a);
        varint(&mut raw, bytes.len() as u64);
        spans.push((raw.len() as u64, (raw.len() + bytes.len()) as u64));
        raw.extend_from_slice(bytes);
    }
    // Four steps: the set, its bytes written, the graph, the index.
    let writing = console::phase("Writing schema DB", console::Label::OnTerminal, 4);
    let index = fds_index(&order, &decoded, &spans)
        .map_err(|e| Failure::Fatal(format!("Error: building the index: {e}")))?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    writing.advance(1);
    std::fs::write(db_path, &raw)?;
    writing.advance(1);
    let dir = db_path.with_extension("");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("hopcroft.rkyv"), rkyv)?;
    writing.advance(1);
    std::fs::write(dir.join("index.rkyv"), index)?;
    writing.advance(1);
    writing.done();
    console::note(&format!("  descriptor: {}", db_path.display()));
    console::note(&format!(
        "  graph:      {}",
        dir.join("hopcroft.rkyv").display()
    ));
    console::note(&format!(
        "  index:      {}",
        dir.join("index.rkyv").display()
    ));

    // 7. The pages (spec 0430 S8).
    if let (Some(path), Some(raw), Some(minimized)) =
        (&output.scoring_html, raw_yaml, minimized_yaml)
    {
        let (raw_page, minimized_page) = crate::scoring::html::write(
            path,
            &raw,
            &minimized,
            output.with_leaf_nodes,
            &output.hide,
        )
        .map_err(|e| Failure::Fatal(format!("Error: --scoring-html-out: {e}")))?;
        console::note(&format!("  pyvis raw:       {raw_page}"));
        console::note(&format!("  pyvis hopcroft:  {minimized_page}"));
    }
    Ok(())
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// `index.rkyv` (`build_index.py`): each file's span in `DB.desc`, its
/// types, imports and extensions; a later file of the same name replaces
/// an earlier one, as Python's dicts do.
fn fds_index(
    order: &[usize],
    decoded: &[DynamicMessage],
    spans: &[(u64, u64)],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut type_to_file = HashMap::default();
    let mut file_to_span = HashMap::default();
    let mut dep_graph = HashMap::default();
    let mut ext_to_file = HashMap::default();
    for (&j, &span) in order.iter().zip(spans) {
        let f = FileDescriptorProto(decoded[j].as_ref());
        let name = text(f.name());
        file_to_span.insert(name.clone(), span);
        dep_graph.insert(
            name.clone(),
            f.dependency().iter().map(|d| text(d)).collect(),
        );
        let package = text(f.package());
        let prefix = if package.is_empty() {
            String::new()
        } else {
            format!("{package}.")
        };
        for m in f.message_type() {
            let fqdn = format!("{prefix}{}", text(m.name()));
            type_to_file.insert(fqdn.clone(), name.clone());
            nested_types(&format!("{fqdn}."), &m, &name, &mut type_to_file);
        }
        for e in f.enum_type() {
            type_to_file.insert(format!("{prefix}{}", text(e.name())), name.clone());
        }
        for x in f.extension() {
            let extendee = text(x.extendee());
            ext_to_file.insert(
                format!("{}/{}", extendee.trim_start_matches('.'), x.number()),
                name.clone(),
            );
        }
        for m in f.message_type() {
            nested_extensions(&m, &name, &mut ext_to_file);
        }
    }
    index_bytes(&FdsIndex {
        type_to_file: canonical_map(type_to_file),
        file_to_span: canonical_map(file_to_span),
        dep_graph: canonical_map(dep_graph),
        ext_to_file: canonical_map(ext_to_file),
    })
}

/// `_collect_nested`.
fn nested_types(
    prefix: &str,
    m: &DescriptorProto<'_>,
    file: &str,
    out: &mut HashMap<String, String>,
) {
    for n in m.nested_type() {
        let fqdn = format!("{prefix}{}", text(n.name()));
        out.insert(fqdn.clone(), file.to_string());
        nested_types(&format!("{fqdn}."), &n, file, out);
    }
    for e in m.enum_type() {
        out.insert(format!("{prefix}{}", text(e.name())), file.to_string());
    }
}

/// `_collect_extensions`.
fn nested_extensions(m: &DescriptorProto<'_>, file: &str, out: &mut HashMap<String, String>) {
    for x in m.extension() {
        let extendee = text(x.extendee());
        out.insert(
            format!("{}/{}", extendee.trim_start_matches('.'), x.number()),
            file.to_string(),
        );
    }
    for n in m.nested_type() {
        nested_extensions(&n, file, out);
    }
}

#[cfg(test)]
mod tests {
    use super::{Graphs, Speculation};
    use prototext_graph::build_scoring_graph::ScoringFile;

    /// A graph told apart by its one entry.
    fn graph(name: &str) -> ScoringFile {
        ScoringFile {
            entries: vec![name.to_string()],
            ..ScoringFile::default()
        }
    }

    fn speculation() -> Speculation {
        // b.proto's graph is none: a file the pool refused.
        Speculation {
            files: ["a.proto", "b.proto", "c.proto", "d.proto"]
                .map(String::from)
                .to_vec(),
            graphs: Graphs {
                present: vec![true, false, true, true],
                kept: vec![graph("a"), graph("c"), graph("d")],
            },
            compiled: None,
        }
    }

    fn entries(g: &Graphs) -> Vec<&str> {
        g.kept.iter().map(|f| f.entries[0].as_str()).collect()
    }

    #[test]
    fn a_failed_file_dropped_the_others_graphs_are_reused_in_order() {
        let g = speculation()
            .regraph(&["a.proto", "b.proto", "d.proto"])
            .unwrap();
        assert_eq!(g.present, vec![true, false, true]);
        assert_eq!(entries(&g), vec!["a", "d"]);
    }

    #[test]
    fn a_file_the_speculation_did_not_expect_is_collected_again() {
        assert!(speculation().regraph(&["a.proto", "e.proto"]).is_none());
    }
}
