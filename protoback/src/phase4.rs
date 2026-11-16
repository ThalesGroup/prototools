// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 4, reproto's `_phase4_pruning`: the nodes the `-p` FQDN patterns
//! name, then all they contain, pruned. A pruned file is no longer a seed.

use crate::graph::Kind;
use crate::run::Run;
use crate::select;
use crate::topology::Topology;

pub fn run(run: &mut Run, topo: &mut Topology) {
    let graph = &mut run.graph;
    let mut current = Vec::new();
    for pattern in &run.prunings {
        let matched = select::matching(graph, pattern);
        if matched.is_empty() {
            // A file phase 2 pruned already has no node: not a miss.
            let already = pattern
                .strip_prefix("file:")
                .is_some_and(|name| run.pruned_file_names.contains(name));
            if !already {
                select::not_found(&mut run.events, graph, "Pruning target", pattern);
            }
            continue;
        }
        for i in matched {
            let n = &mut graph.nodes[i];
            if !n.present {
                continue;
            }
            n.is_pruned = true;
            if n.kind == Kind::File {
                let name = n.fqdn.strip_prefix("file:").unwrap_or(&n.fqdn);
                if let Some(f) = topo.find(name) {
                    topo.files[f].is_seed = false;
                }
            }
            current.push(i);
        }
    }
    // What they contain, present: a node pruned already (named by the user
    // too) is not pruned again, nor walked again.
    while !current.is_empty() {
        let mut next = Vec::new();
        for i in current {
            let children: Vec<usize> = graph.nodes[i].contains.iter().copied().collect();
            for c in children {
                let child = &mut graph.nodes[c];
                if child.present && !child.is_pruned {
                    child.is_pruned = true;
                    next.push(c);
                }
            }
        }
        current = next;
    }
    // Pruned files: no W5 for their importers.
    for f in &topo.files {
        if f.is_pruned {
            run.events.register_pruned_file(&f.name);
        }
    }
    crate::console::phase_done(if run.prunings.is_empty() {
        "Applying pruning rules"
    } else {
        "Processing exclusions"
    });
}
