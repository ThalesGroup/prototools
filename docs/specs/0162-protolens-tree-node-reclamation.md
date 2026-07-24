<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0162 — protolens: general reclamation of abandoned tree nodes

Status: draft
App: protolens

## Background

`splice_override` (`override_apply.rs`) never removes anything from
`self.tree: Vec<TreeNode>`. Every call — whether previewing a
candidate or confirming a real override — decodes a fresh local
subtree, appends it to `self.tree`'s tail, and rewires the overridden
node's own pointers to the new content; the previous content becomes
unreachable via live traversal but stays physically resident in the
`Vec` forever ("abandon in place", `override_apply.rs:1223-1226`).
`self.heat_states` is kept parallel to `self.tree` and grows the same
way.

Spec 0161 bounds this growth for the *disposable live-preview* path
specifically (repeatedly moving the highlight in the override pane),
since a preview's content is known, by construction, to be throwaway
the moment a new preview supersedes it.

Confirmed override commits are different: each one permanently
replaces a node's content, and a long interactive session can
confirm overrides on many distinct nodes, or repeatedly re-override
the same node many times, or apply a batch operation touching many
nodes (spec 0160) — all of it appended to the same `self.tree` with
none of it ever reclaimed. Unlike a preview, none of this is knowable
in advance to be safely truncatable by a simple watermark: many
different, unrelated nodes' abandoned subtrees can be interleaved
with live content across the session's lifetime, and other `App`
state (cursor, `folded`, `line_to_node`/`footer_line_to_node`,
override-management-pane state, etc.) may hold indices into any of it.
Reclaiming this general case therefore requires an actual
reachability-based compaction pass and remapping of every index-based
reference across `App`'s state, not just a truncation of the most
recent addition.

Over a sufficiently long interactive session with many confirmed
overrides, `self.tree`/`self.heat_states` therefore still grow
unboundedly even after spec 0161 lands, with no way to reclaim any of
it short of restarting protolens.

## Goals

- **G1**: Reclaim (physically remove from `self.tree`/`self.heat_states`)
  nodes that are no longer reachable from the live document structure
  (`first_node`'s doc-chain and/or parent-child/sibling pointers),
  once nothing in `App`'s state still references them.
- **G2**: Remap every index-based reference across `App`'s state
  consistently whenever reclamation runs, so no field is silently
  left pointing at a stale or now-invalid index. This includes (at
  least) `cursor`, `folded`, `heat_states`, `line_to_node`/
  `footer_line_to_node`, `override_target`, override-management-pane
  state, and `active_override_range`/`override_seek_target`.
- **G3**: Bound total steady-state memory/tree size for arbitrarily
  long interactive sessions with many confirmed overrides, independent
  of how many overrides have cumulatively been applied over the
  session's lifetime.
- **G4**: Run reclamation at a cost and/or frequency that does not
  itself introduce a new user-visible stall — e.g. amortized,
  debounced, or only triggered once accumulated garbage crosses some
  threshold. The exact strategy is not yet decided (see below).

## Non-goals

This spec intentionally stops at stating goals, per explicit request:
the reclamation mechanism, data structures, remapping strategy, and
triggering policy are not designed here. A future revision of this
document (or a follow-up spec) will work out the Specification and
Test plan once spec 0161 has landed and its effect on real-world
session growth can be measured.
