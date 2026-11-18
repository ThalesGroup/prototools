// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What phases 4 and 5 share: the nodes an FQDN pattern names, and what
//! they say of one that names none.

use crate::events::Events;
use crate::fuzz;
use crate::graph::Graph;
use crate::pathmatch::fqdn_match;

/// `_find_matching_nodes`: the nodes whose FQDN `pattern` matches, in
/// node order.
pub fn matching(graph: &Graph, pattern: &str) -> Vec<usize> {
    (0..graph.nodes.len())
        .filter(|&i| fqdn_match(pattern, &graph.nodes[i].fqdn))
        .collect()
}

/// `_fuzzy_suggest`: the first node of the best `fuzz.ratio` over 85, its
/// FQDN without the leading `.` of its name, to be pasted as given.
fn suggestion(graph: &Graph, pattern: &str) -> Option<String> {
    let mut best: Option<&str> = None;
    let mut best_score = 85.0;
    let ratio = fuzz::Ratio::new(pattern);
    for n in &graph.nodes {
        let score = ratio.of(&n.fqdn);
        if score > best_score {
            best_score = score;
            best = Some(&n.fqdn);
        }
    }
    let best = best?;
    Some(match best.split_once(':') {
        Some((prefix, rest)) if rest.starts_with('.') => format!("{prefix}:{}", &rest[1..]),
        _ => best.to_string(),
    })
}

/// A pattern that names no node: `what not found`, and, unless it is a
/// glob, the closest FQDN, if one is close.
pub fn not_found(events: &mut Events, graph: &Graph, what: &str, pattern: &str) {
    // W12 for a seed, W11 for a prune pattern.
    let code = if what == "Seed" { "W12" } else { "W11" };
    let message = format!("{what} not found: {pattern}");
    crate::report::idle(code, pattern, &message);
    events.warn(code, &message);
    if pattern.contains(['*', '?']) {
        return;
    }
    if let Some(s) = suggestion(graph, pattern) {
        events.attention(&format!("  Did you mean: {s}?"));
    }
}
