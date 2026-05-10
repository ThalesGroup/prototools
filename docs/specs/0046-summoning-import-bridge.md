<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0046 — reproto: tighter summoning via import bridging

**Status:** implemented
**Implemented in:** 2026-05-10
**App:** reproto

---

## Background

Phase 6 (summoning) marks the set of files that reproto will render.  Its
goal is to ensure the output is a self-contained, compilable set of `.proto`
files: every import declaration that appears in a rendered file must refer to
another rendered file.

### Why import declarations can go missing

reproto reconstructs `.proto` files from descriptor data.  Import statements
are not copied verbatim from the input — they are re-derived from the
summoning state.  A file's rendered output only includes `import` lines for
files that are themselves summoned.

This creates a correctness obligation: suppose file A imports B which imports
C, and a type TA in A has a field whose type TC is defined in C.  reproto
seeds on TA, so TA and TC become reachable, and A and C are summoned.  But A
does not directly import C — its only path to TC is through B.  If B is not
summoned, A's rendered output has no `import "B.proto"` line, TC is
unreachable, and the output does not compile.

### The current algorithm and its flaw

The current phase 6 sibling-file rule (phases.py:884–891) attempts to handle
this by summoning any file that imports an already-summoned file.  This is
correct in the sense that it never misses a required bridge — but it is far
too broad.  It summons every reverse-import dependent of every summoned file,
regardless of whether those dependents are needed by anything.

Example: seeding on `desc:.google.protobuf.Duration` produces 75 output
files, of which only `duration.proto` is needed.  The other 74 are files from
the corpus that import `duration.proto` — they were summoned by the sibling
rule even though nothing references them.

---

## Goals

1. Replace the current sibling-file rule in `_phase6_summoning` with a
   tighter algorithm that only summons files that are strictly necessary to
   make the output compilable.
2. The new algorithm summons the minimal set of intermediate ("bridge") files
   needed to connect each summoned file A to each foreign summoned file C that
   A's types reference.
3. The output of the new algorithm must be a strict subset of the output of
   the current algorithm (never summons more, sometimes summons less).
4. The choice of bridge path is deterministic: when multiple shortest paths
   exist between A and C in the import graph, the lexicographically smallest
   one is chosen (lexicographic comparison of the sequence of intermediate
   file names, applied only among paths of equal — minimum — length).

## Non-goals

- Changing the reachability (forward) pass.
- Optimal minimisation of the summoned set beyond what the algorithm below
  delivers.
- Any change to the YAML or CLI.

---

## Specification

### §1 — Definitions

**Import graph**: a directed graph where nodes are file nodes
(`ReFileDescriptorProto` instances in `ctx.nodes`) and there is an edge
A → B iff A directly imports B (i.e. B is in A's `targets` set and B is also
a `ReFileDescriptorProto`).

**File-level targets of A**: the subset of A's `targets` that are
`ReFileDescriptorProto` instances — i.e. the files A directly imports.

**Type-level targets of A**: the union of all non-`ReFileDescriptorProto`
targets reachable by walking the full `contains` tree of A.  This includes
targets of A's contained nodes (messages, fields, etc.) at any nesting depth,
because field-level type references (e.g. a field's `type_name`) are recorded
as targets of the field node, not of the file or message node directly.

**Host file of a node N**: the unique `ReFileDescriptorProto` that contains N
(found by walking `N.parent` upward until reaching a node with `parent is
None`).

**Bridge file**: a file Z that is not itself summoned at the start of phase 6
but must be summoned to preserve a reachable import path from some summoned
file A to some summoned file C.

### §2 — Correct summoning invariant

The summoned set S satisfies the *compilability invariant* iff:

> For every file A in S, and for every type-level target T of A whose host
> file C is also in S: there exists a file B in S such that A directly imports
> B, and B transitively imports C (or B == C).

Informally: every foreign type reference from A to C must be reachable via at
least one summoned direct import of A.

### §3 — Algorithm

Phase 6 proceeds in two sub-passes.

**Sub-pass 1 — seed summoning (unchanged):**

Mark every reachable node as summoned.  Propagate upward through the `parent`
relation until file nodes are reached.  This produces the initial summoned
set S₀ — exactly the files that contain at least one reachable node.  This
sub-pass is identical to the current implementation.

**Sub-pass 2 — import bridging (replaces the sibling-file rule):**

Repeat until no new files are added to S:

1. For each summoned file A in S:
2.   For each type-level target T of A (i.e. each non-`ReFileDescriptorProto`
     node reachable via A's `contains` tree, collected by `_all_type_targets`):
3.     Let C = host file of T.
4.     If C is not in S: skip (T's file is not summoned; no bridge needed).
5.     Find all shortest paths A = F₀ → F₁ → … → Fₖ = C in the import
       graph (note: if A directly imports C, the shortest path has length 1
       and there are no intermediate files).  Among those shortest paths,
       choose the lexicographically smallest one by comparing the sequences
       of intermediate file names F₁, …, Fₖ₋₁.  Summon all intermediate
       files F₁ … Fₖ₋₁ (possibly none if k = 1).

The loop terminates because each iteration either adds at least one file to S
(making progress) or finds all invariants satisfied (and stops).  The import
graph is a DAG so shortest paths always exist and are finite.

### §4 — Shortest path with lexicographic tie-breaking

BFS from A in the import graph finds all shortest paths to C.  Among those,
the lexicographically smallest is selected by running BFS with neighbours
expanded in sorted order by file name: since all neighbours at the same BFS
depth are processed in lexicographic order, the first time a node is reached
is always via the lexicographically smallest shortest path.  Record, for each
reached node, the predecessor that first reached it.

Concretely:

```python
from collections import deque

def shortest_lex_path(
    start: ReFileDescriptorProto,
    end: ReFileDescriptorProto,
    import_graph: dict[ReFileDescriptorProto, list[ReFileDescriptorProto]],
) -> list[ReFileDescriptorProto]:
    """Return the shortest path from start to end; lex-smallest if tied.

    Shortest = fewest hops.  Among all shortest paths, the one whose
    sequence of intermediate node names is lexicographically smallest.
    Returns the full path [start, ..., end], or [] if no path exists.
    import_graph[F] must be F's direct imports sorted by name.
    """
    if start is end:
        return [start]
    prev: dict[ReFileDescriptorProto, ReFileDescriptorProto] = {start: start}
    queue: deque[ReFileDescriptorProto] = deque([start])
    while queue:
        node = queue.popleft()
        for neighbour in import_graph[node]:  # already sorted by name
            if neighbour not in prev:
                prev[neighbour] = node
                if neighbour is end:
                    # Reconstruct path
                    path = []
                    cur = end
                    while cur is not start:
                        path.append(cur)
                        cur = prev[cur]
                    path.append(start)
                    path.reverse()
                    return path
                queue.append(neighbour)
    return []  # no path found
```

The import adjacency list `import_graph[F]` must be sorted by file name
before BFS begins (once, as a pre-computation step).

### §5 — Implementation notes

**Where**: `_phase6_summoning` in `phases.py`.  Sub-pass 1 is unchanged.
Sub-pass 2 replaces lines 883–891 (the sibling-file loop) entirely.

**Type-level target collection**: a helper `_all_type_targets(file_node)`
walks the `contains` tree of the file node (DFS) and collects all
non-`ReFileDescriptorProto` targets at every level.  This is necessary
because field-level type references live on field nodes, not on the file or
message node directly.

**Import graph construction**: iterate over all `ReFileDescriptorProto` nodes
in `ctx.nodes`.  For each, collect its file-level targets (the subset of
`node.targets` that are `ReFileDescriptorProto` instances) sorted by
`node.name`.  This gives `import_graph`.

**Efficiency**: the BFS is run once per (A, T) pair where T is a type-level
target of A pointing to a foreign summoned file.  In practice the number of
such pairs is small (bounded by the total number of cross-file field
references in the summoned set), and import graphs are shallow (typically 2–5
hops deep).

**Pruned files**: files with `is_pruned = True` must not be added to S, and
must not be traversed as intermediate nodes in the BFS.  If no unpruned path
exists from A to C, skip silently (this mirrors the current behaviour for
pruned files).

**Present files only**: only files where `is_present()` is True participate
as nodes in the import graph.  Reference-only file nodes (stubs for files
that were declared as imports but never loaded) are excluded.

### §6 — Changes to existing files

| File | Change |
|---|---|
| `phases.py` | Replace sibling-file loop in `_phase6_summoning` with sub-pass 2 as specified above |
| `phases.py` | Fix phase 7 output filter: replace `any(target.is_summoned for target in re_fdp.targets)` with `re_fdp.is_summoned` |
| `phases.py` | Fix phase 4 pruning: enforce "prune overrides seed" by clearing `topo_file.is_seed` when a file node is explicitly pruned, so the default-seed path in phase 5 does not mark pruned files reachable |

### §7 — Tests

Unit tests in `reproto/src/reproto/tests/`, added to a new file
`test_summoning.py`.  Tests use existing fixtures compiled on demand via
`compile_proto()`.

**TC-1 — direct import, no bridge needed**: A imports C directly; A has a
field of type TC.  Assert only A and C are summoned (no bridge files).

**TC-2 — one-hop bridge**: A imports B imports C; A has a field of type TC.
Assert A, B, and C are all summoned.  Assert no other files are summoned.

**TC-3 — longer chain**: A imports B imports D imports C; A has a field of
type TC.  Assert A, B, D, C are all summoned.

**TC-4 — no spurious summoning**: seed on a leaf type in `duration.proto`
(no message-type fields).  Assert only `duration.proto` is summoned — no
files that import `duration.proto` are summoned.

**TC-5 — lexicographic tie-breaking**: two equal-length paths from A to C
exist: A → B1 → C and A → B2 → C where B1 < B2 lexicographically.  Assert
B1 is summoned and B2 is not.

**TC-6 — multiple references, shared bridge**: A has two fields, one of type
TB (in B) and one of type TC (in C), where A imports B imports C.  Assert B
is summoned once (not duplicated).

**TC-7 — import-only file not written**: file U imports file S; a `desc:`
seed targets a message in S only.  U has no reachable nodes.  Assert S is
written and U is not — regression for the old phase 7 filter
`any(target.is_summoned for target in re_fdp.targets)` which wrote any file
importing a summoned file.

**TC-8 — pruned seed file not written**: file P is passed as input (a seed by
default) and also explicitly `--prune`d.  File Q imports P but has no field
of type from P.  Assert Q is written and P is not — regression for the phase
5 default-seed path not respecting `is_pruned`.
