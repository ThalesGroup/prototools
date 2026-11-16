// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 5, reproto's `_phase5_reachability`: the seeds, then all they
//! target, present and not pruned, reachable. The seeds are the nodes the
//! `--seed` FQDN patterns and the path seeds' files name; without any,
//! the seed files.

use std::collections::BTreeSet;

use crate::graph::Kind;
use crate::run::Run;
use crate::select;
use crate::topology::Topology;

pub fn run(run: &mut Run, topo: &Topology) {
    let graph = &mut run.graph;
    // The FQDN seeds as given, then the path seeds' files, sorted.
    let seeds: Vec<&String> = run.seed_fqdns.iter().chain(&run.path_seed_fqdns).collect();
    let seeding = crate::console::phase(
        "Marking seeds",
        crate::console::Label::OnTerminal,
        seeds.len() as u64,
    );
    let mut initial = BTreeSet::new();
    if seeds.is_empty() {
        for (i, n) in graph.nodes.iter_mut().enumerate() {
            if n.kind != Kind::File {
                continue;
            }
            let name = n.fqdn.strip_prefix("file:").unwrap_or(&n.fqdn);
            let Some(f) = topo.find(name) else {
                continue;
            };
            if !topo.files[f].is_ref() && topo.files[f].is_seed {
                n.is_reachable = true;
                initial.insert(i);
            }
        }
    } else {
        for pattern in seeds {
            let matched = select::matching(graph, pattern);
            if matched.is_empty() {
                select::not_found(&mut run.events, graph, "Seed", pattern);
                continue;
            }
            for i in matched {
                let n = &mut graph.nodes[i];
                if !n.present {
                    continue;
                }
                if n.is_pruned {
                    run.events
                        .info(&format!("  Skipping pruned seed: {}", n.fqdn));
                    continue;
                }
                n.is_reachable = true;
                initial.insert(i);
            }
        }
    }
    seeding.done();
    let reaching = crate::console::phase(
        "Computing reachability",
        crate::console::Label::Always,
        if run.quiet {
            0
        } else {
            graph.nodes.len() as u64
        },
    );

    let mut current: Vec<usize> = initial.iter().copied().collect();
    while !current.is_empty() {
        let mut next = Vec::new();
        for i in current {
            let targets: Vec<usize> = graph.nodes[i].targets.iter().copied().collect();
            for t in targets {
                let target = &mut graph.nodes[t];
                if target.present && !target.is_pruned && !target.is_reachable {
                    target.is_reachable = true;
                    next.push(t);
                }
            }
        }
        current = next;
    }
    reaching.done();

    if run.debug_fqdn {
        // reproto tests `node.is_present`, a method, never false: P always.
        for (i, n) in graph.nodes.iter().enumerate() {
            run.events.info(&format!(
                "FQDN {} P {} {}",
                if initial.contains(&i) { "S" } else { "-" },
                if n.is_reachable { "R" } else { "-" },
                n.fqdn
            ));
        }
    }
    run.seed_nodes = initial;
}
