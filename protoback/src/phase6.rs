// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 6, reproto's `_phase6_summoning`: what is reachable, and all that
//! encloses it, summoned; then the files on the import path between two
//! summoned files, one naming a type of the other (spec 0046); then, under
//! `--schema-db-out`, the files summoned files depend on, kept aside for
//! the schema database.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use prototext_upb::descriptor_access::FileDescriptorProto;

use crate::graph::{Graph, Kind};
use crate::run::Run;

fn is_file(graph: &Graph, i: usize) -> bool {
    graph.nodes[i].kind == Kind::File
}

/// Sub-pass 1: each reachable node, and the nodes enclosing it, summoned.
fn summon_enclosing(graph: &mut Graph) {
    for i in 0..graph.nodes.len() {
        if !graph.nodes[i].is_reachable {
            continue;
        }
        let mut cur = i;
        loop {
            graph.nodes[cur].is_summoned = true;
            match graph.nodes[cur].parent {
                Some(p) if !graph.nodes[p].is_summoned => cur = p,
                _ => break,
            }
        }
    }
}

/// `_shortest_lex_path`: a breadth-first search from `start`, each file's
/// imports taken in name order; the path to `end`, both included, or
/// none.
fn shortest_lex_path(start: usize, end: usize, imports: &HashMap<usize, Vec<usize>>) -> Vec<usize> {
    let mut prev = HashMap::from([(start, start)]);
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        for &next in imports.get(&node).into_iter().flatten() {
            if prev.contains_key(&next) {
                continue;
            }
            prev.insert(next, node);
            if next == end {
                let mut path = vec![end];
                let mut cur = end;
                while cur != start {
                    cur = prev[&cur];
                    path.push(cur);
                }
                path.reverse();
                return path;
            }
            queue.push_back(next);
        }
    }
    Vec::new()
}

/// Sub-pass 2: import bridging, until nothing changes. The outcome does
/// not depend on the order files and targets are taken in: each file
/// summoned is on a path that depends on the imports only.
fn bridge_imports(graph: &mut Graph) {
    let live = |g: &Graph, i: usize| g.nodes[i].present && !g.nodes[i].is_pruned;
    // Each present file not pruned: its imports, present and not pruned,
    // by name.
    let mut imports: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..graph.nodes.len() {
        if !is_file(graph, i) || !live(graph, i) {
            continue;
        }
        let mut targets: Vec<usize> = graph.nodes[i]
            .targets
            .iter()
            .copied()
            .filter(|&t| is_file(graph, t) && live(graph, t))
            .collect();
        targets.sort_by(|&a, &b| graph.nodes[a].fqdn.cmp(&graph.nodes[b].fqdn));
        imports.insert(i, targets);
    }
    let host_file = |g: &Graph, mut cur: usize| -> Option<usize> {
        while let Some(p) = g.nodes[cur].parent {
            cur = p;
        }
        is_file(g, cur).then_some(cur)
    };
    // The present targets other than files, of a file and all it contains.
    let type_targets = |g: &Graph, file: usize| -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        let mut stack = vec![file];
        let mut visited = HashSet::new();
        while let Some(n) = stack.pop() {
            if !visited.insert(n) {
                continue;
            }
            let node = &g.nodes[n];
            out.extend(
                node.targets
                    .iter()
                    .filter(|&&t| !is_file(g, t) && g.nodes[t].present),
            );
            stack.extend(&node.contains);
        }
        out
    };
    let mut paths: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        let summoned: Vec<usize> = (0..graph.nodes.len())
            .filter(|&i| is_file(graph, i) && graph.nodes[i].is_summoned)
            .collect();
        for a in summoned {
            for t in type_targets(graph, a) {
                let Some(c) = host_file(graph, t) else {
                    continue;
                };
                if !graph.nodes[c].is_summoned || c == a {
                    continue;
                }
                let path = paths
                    .entry((a, c))
                    .or_insert_with(|| shortest_lex_path(a, c, &imports));
                for &between in path.iter().skip(1).take(path.len().saturating_sub(2)) {
                    if !graph.nodes[between].is_summoned {
                        graph.nodes[between].is_summoned = true;
                        changed = true;
                    }
                }
            }
        }
    }
}

/// Sub-pass 3 (`--schema-db-out`): the files the summoned files depend
/// on, transitively, present, not pruned and not summoned, in the order
/// reproto's stack finds them.
fn schema_db_extra_nodes(run: &Run) -> Vec<usize> {
    let graph = &run.graph;
    let summoned: Vec<usize> = (0..graph.nodes.len())
        .filter(|&i| is_file(graph, i) && graph.nodes[i].is_summoned)
        .collect();
    let name = |i: usize| graph.nodes[i].fqdn.strip_prefix("file:").unwrap_or("");
    let mut seen: HashSet<String> = summoned.iter().map(|&i| name(i).to_string()).collect();
    let mut extra = Vec::new();
    let mut work = summoned;
    while let Some(f) = work.pop() {
        let Some(def) = &graph.nodes[f].def else {
            continue;
        };
        let fdp = FileDescriptorProto(run.db.files[def.file].as_ref());
        for dep in fdp.dependency() {
            let dep = String::from_utf8_lossy(dep).into_owned();
            if !seen.insert(dep.clone()) {
                continue;
            }
            let Some(d) = graph.find(&format!("file:{dep}")) else {
                continue;
            };
            let node = &graph.nodes[d];
            if node.kind != Kind::File || !node.present || node.is_pruned {
                continue;
            }
            extra.push(d);
            work.push(d);
        }
    }
    extra
}

pub fn run(run: &mut Run) {
    summon_enclosing(&mut run.graph);
    bridge_imports(&mut run.graph);
    crate::console::phase_done("Marking summoned nodes");
    if run.build_schema_db {
        run.schema_db_extra_nodes = schema_db_extra_nodes(run);
    }
}
