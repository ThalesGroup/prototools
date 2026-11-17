<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0441 — scoring graphs drawn at scale

Status: draft (embryonic: options and an initial recommendation; not
        scheduled)
App: protoback (reproto keeps pyvis's page)
Refs: docs/specs/0430-protoback-scoring-graphs-and-the-schema-database.md
        (S8: pyvis's page, byte for byte, a port-verification constraint
        this spec would lift)

## Background

`--scoring-html-out` writes pyvis's page: vis-network 9.1.2 with its
physics layout over the whole compiled scoring graph. Measured
2026-11-17 on googleapis: 45,330 nodes, 52,461 edges, a 12 MB page.
vis-network's physics is generally held to struggle beyond a few
thousand nodes (not measured here: no browser on the host); whatever it
draws at that size is a hairball nobody reads.

The graph is an automaton: a state is a type (several, when Hopcroft
merged types the wire cannot tell apart), a transition a field number
leading to a state, the roots named by type.

## Goals (to settle)

- **G1.** A view that answers the questions asked of a scoring graph:
  what a type leads to, by which field numbers; which types are merged
  into one state; why a blob scores as a type.
- **G2.** Usable at googleapis' size.

## Options

1. **An explorer: a neighborhood, not the graph.** The page opens on a
   search box; a type's state is drawn with its 1–2-hop neighborhood,
   expanded on click. A few dozen nodes on screen: vis-network (already
   vendored) or Cytoscape.js handles it. The data embedded as compact
   JSON (a few MB for googleapis). Drops pyvis's template and `utils.js`
   for a small page of protoback's own.
2. **Aggregate, then drill down.** States collapsed by package (or
   file) into an overview graph, edges weighted by transitions (a few
   hundred nodes for googleapis); a package opens its own graph. Shows
   the overall structure, which 1 does not.
3. **A GPU (WebGL) renderer.** sigma.js with graphology (about 10^5
   nodes) or Cosmograph (GPU force layout, millions), the layout computed
   in Rust beforehand (ForceAtlas2-like) so the page loads at once.
   Without 1's search and highlighting, still a hairball; a large
   dependency.
4. **Export for a dedicated tool.** GraphML/GEXF for Gephi (10^5 nodes,
   its own layouts, filters, statistics) or DOT for Graphviz's `sfdp`. A
   small writer, no front end to maintain; for occasional deep analysis.
5. **Not a drawing.** The automaton as a searchable, collapsible tree or
   table (state → field N → state, the merged type names listed), in
   HTML or as a protolens view. No layout, any size; loses the global
   shape, may answer "why did this blob score as X" most directly.

## Initial recommendation

Option 1 as the default `--scoring-html-out`, option 4 as a cheap
export. The explorer reuses what is vendored and works at googleapis'
size; GraphML covers "show me everything" without a renderer of ours.
Option 3 dismissed: it needs 1's search to be useful anyway. Between 1
and 5, the deciding question, open: what the drawing is used for. If
mostly inspecting single types and their transitions, 5 may serve
better than any drawing.

## Non-goals

- **N1.** Changing reproto's page: it stays pyvis's.
- **N2.** Changing the scoring graph itself.
