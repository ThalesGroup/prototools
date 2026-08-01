// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::line_patch::{LinePatch, LinePatchTarget};
use super::override_resolve::ParentFieldOrExt;
use super::preview_truncate::{insert_truncation_marker, trunc_shape_for, truncate_interior};
use super::*;

use prost_reflect::prost_types::field_descriptor_proto::Type;
use prototext_core::serialize::render_text::NodeSpan;

/// TEMPORARY debug instrumentation for the arena-growth investigation.
/// Counters are reset at the start of each `render_overrides` batch and
/// reported at its end, under `PROTOLENS_TRACE`.
pub(super) mod probe {
    use std::sync::atomic::AtomicUsize;
    /// `render_overrides_inner` calls.
    pub(super) static VISITS: AtomicUsize = AtomicUsize::new(0);
    /// Of those, ones where `resettle_node` actually spliced.
    pub(super) static SPLICES: AtomicUsize = AtomicUsize::new(0);
    /// Arena nodes rewritten by those splices — the descendants of each
    /// resettled node. Since spec 0216 a splice appends nothing, so this
    /// measures how much of a fixed arena the batch touched, not growth.
    pub(super) static NODES: AtomicUsize = AtomicUsize::new(0);
    /// Microseconds spent in `compute_descend_marks`.
    pub(super) static MARKS_US: AtomicUsize = AtomicUsize::new(0);
    /// Microseconds spent in `splice_override`, summed over the batch.
    pub(super) static SPLICE_US: AtomicUsize = AtomicUsize::new(0);
    /// Microseconds spent in `materialize_line_patches` — spec 0210's
    /// residual whole-document pass over `lines`, and the entire
    /// justification for its step 2.
    pub(super) static TEXT_US: AtomicUsize = AtomicUsize::new(0);
    /// Microseconds in the batch's top-level `render_overrides_inner`
    /// (splices included), against `FINALIZE_US` for what follows it.
    pub(super) static INNER_US: AtomicUsize = AtomicUsize::new(0);
    /// Microseconds in `finalize_override_batch` (`TEXT_US` included).
    pub(super) static FINALIZE_US: AtomicUsize = AtomicUsize::new(0);
}

/// Spec 0185 S3: one node's complete rendering under a candidate type,
/// with no tree mutation whatsoever — everything `splice_override` used
/// to compute inline before its splice proper begins. The live preview
/// takes the same value and stops there, which is what makes the
/// preview and the commit byte-identical (G3).
pub(super) struct RenderedAs {
    pub(super) lines: Vec<String>,
    /// Discarded by the preview (spec 0185 N6: overlay rows have no
    /// identity); consumed by `splice_override` to build the new subtree.
    pub(super) spans: Vec<NodeSpan>,
}

impl App {
    /// Recursively collect every current descendant of `idx` (any depth),
    /// via `first_child`/`next_sibling` pointer traversal — never array
    /// position (spec 0114 §5's splice design: post-order array
    /// contiguity does not survive a *second* override of the same node,
    /// since the first override's new nodes are appended at the array's
    /// end, breaking it). Used to find which array entries become orphans
    /// once `idx`'s subtree is replaced, so they can be scrubbed from
    /// `self.folded`.
    pub(super) fn collect_descendants(&self, idx: usize, out: &mut Vec<usize>) {
        let mut child = self.first_child(idx);
        while let Some(c) = child {
            out.push(c);
            self.collect_descendants(c, out);
            child = self.next_sibling(c);
        }
    }

    /// Spec 0132 §G3: settles `idx`'s main-pane rendering to its current
    /// "effective" override target (`resolve_active_override`'s
    /// explicit type if one is active, else `natural_type(idx)` when
    /// nothing is active at all) — splicing only if it doesn't already
    /// match `self.tree[idx].rendered_as` (the same no-op-when-already-
    /// current guard `render_overrides` always used, verbatim). Factored
    /// out of `render_overrides` itself (which calls this for `idx`
    /// before recursing into children) so the override-pane's live-
    /// preview revert (on close/cancel) can reuse the exact same
    /// "effective type" computation — including the natural-type
    /// fallback a plain `resolve_active_override_entry`-only revert
    /// would get wrong.
    ///
    /// A stale `auto` entry whose ancestor context has since changed is
    /// *not* demoted: `auto`/`manual` is provenance only (how an entry
    /// was created, shown via `manage_entry_style`), and must have no
    /// effect on whether an *active* entry applies. An active entry,
    /// auto-derived or not, applies exactly as long as its path still
    /// resolves to a live node — the same fallback `splice_override`
    /// relies on for a manual override that stops cleanly matching its
    /// target (a `TYPE_MISMATCH`-style annotation, not a silent revert
    /// to raw).
    ///
    /// `path` is `idx`'s own already-known positional path, passed down
    /// by the sole caller (`render_overrides_inner`'s hot full-document
    /// walk) rather than recomputed here — see `resolve_active_override_
    /// entry_index_by_path`'s doc comment for why that matters.
    /// Returns the index of the freshly recorded line-buffer patch (spec
    /// 0167) if `idx` was actually re-spliced, `None` otherwise (already
    /// matched `rendered_as`, or `splice_override` returned `Err`).
    /// `patch_scope` is `idx`'s own patch-nesting context — see
    /// `render_overrides_inner`'s doc comment — threaded straight through
    /// to `splice_override`.
    pub(super) fn resettle_node(
        &mut self,
        idx: usize,
        path: &str,
        patch_scope: Option<usize>,
    ) -> Option<usize> {
        let target = self
            .resolve_active_override_entry_index_by_path(idx, path)
            .map(|i| self.overrides.entries()[i].r#type.clone());
        let field_name = self.field_name_for_by_path(idx, path);
        // Spec 0213: intern first, so the comparison below is one `u32`
        // against the node's own. A provenance whose splice then fails is
        // left in the table — bounded by the number of failed splices,
        // and cheaper than a second lookup on every visit.
        let current = self.provenance.intern(&(target.clone(), field_name));
        if current != self.tree[idx].rendered_as {
            let effective = match &target {
                Some(explicit) => explicit.clone(),
                None => self.natural_type(idx),
            };
            let t_splice = std::time::Instant::now();
            let spliced = self.splice_override(idx, effective, false, patch_scope);
            if crate::tui::trace::enabled() {
                probe::SPLICE_US.fetch_add(
                    t_splice.elapsed().as_micros() as usize,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            match spliced {
                Ok(patch_idx) => {
                    self.tree[idx].rendered_as = current;
                    // Spec 0221 S1: this node is settled after all, so
                    // an earlier refusal of it in this same pass was not
                    // final and must not be reported. The guard keeps
                    // the common case — nothing refused — free, and the
                    // scan is over the refusals alone, not the tree.
                    if !self.refusals.is_empty() {
                        self.refusals.retain(|(node, _)| *node != idx);
                    }
                    Some(patch_idx)
                }
                Err(e) => {
                    // Spec 0221 S1/S2: collected, not printed, and not
                    // assigned over — a pass that refuses N nodes has to
                    // be able to report N. The three parts recorded here
                    // are the ones only this frame knows: which node,
                    // what it was asked to be, and why that failed. The
                    // caller prefixes where the request came from (the
                    // `--load-overrides` file, or the typed command).
                    //
                    // `effective` was moved into the splice, so the
                    // requested type is re-derived here rather than
                    // cloned above: `resettle_node` runs once per node
                    // of a pass and this arm runs only for the few that
                    // fail. `None` is the raw rendering, spelled the way
                    // `main.rs`'s "rendering root node as ..." spells
                    // it.
                    let requested = match &target {
                        Some(explicit) => explicit.clone(),
                        None => self.natural_type(idx),
                    }
                    .unwrap_or_else(|| "<raw / no type>".to_string());
                    self.refusals
                        .push((idx, format!("{path}: cannot render as '{requested}': {e}")));
                    None
                }
            }
        } else {
            None
        }
    }

    /// Whether `entry` (assumed `auto == true`) would still be re-derived
    /// with the same `r#type` if `render_overrides` visited its node
    /// again right now — i.e. it is still "in scope" (spec 0125 §G2).
    /// Sole use: `handle_manage_key`'s `Delete`/`Backspace` handling,
    /// which needs to distinguish "deleting this would just make the
    /// next `render_overrides` pass re-seed an identical entry" (still
    /// in scope) from "deleting this is final" (out of scope). Lives on
    /// `App` (not `OverrideCollection`) because it
    /// needs `auto_expand_type`, which resolves against the live tree/
    /// descriptor pool, not just the override collection itself.
    /// Auto-seeded entries only ever have a `Path` origin
    /// (`render_overrides` always calls `activate_auto` with
    /// `OverrideOrigin::Path`), so a single `resolve_path` lookup
    /// suffices.
    pub(super) fn auto_entry_in_scope(&mut self, entry: &override_pane::OverrideEntry) -> bool {
        let OverrideOrigin::Path { path } = &entry.origin else {
            return false;
        };
        let Some(idx) = self.resolve_path(path) else {
            return false;
        };
        self.auto_expand_type(idx) == entry.r#type
    }

    /// Recursive override-driven rendering pass (spec 0118 §3): resolves
    /// `idx`'s applicable override and splices a fresh render whenever the
    /// resolved target stops matching what's currently displayed
    /// (`TreeNode::rendered_as`, spec 0118 §2.1) — comparing against
    /// provenance, not just "is there an override right now?", is what
    /// detects a *demotion* as well as a fresh promotion or retype.
    ///
    /// Any/MessageSet auto-expansion (spec 0120) is seeded as a real,
    /// persisted `OverrideEntry` (`OverrideOrigin::Path`) the first time
    /// `idx` is visited with *no entry at all yet existing* for its path —
    /// checked via `self.overrides.entries()`, not via
    /// `resolve_active_override`: the latter can't distinguish "never
    /// seeded" from "user explicitly deactivated the seeded entry", and
    /// naively re-seeding (calling `activate` again) on every subsequent
    /// pass would both silently resurrect a deactivation the user just
    /// made in the manage pane, and — since `activate` unconditionally
    /// resorts the entries list — reshuffle `manage_highlight`'s raw index
    /// out from under the very keypress that triggered this pass. Once
    /// truly first-seeded, `auto_expand_type(idx)` computes the derived
    /// type, `self.overrides.activate` records it, and — because this
    /// happens *before* `target`/`current` are computed below — the very
    /// same pass's `resolve_active_override` already sees it, so no
    /// separate fallback tier is needed in the splice logic itself. This
    /// makes the derived type a real, visible, user-editable/removable
    /// entry in the override management pane (rather than a silent
    /// dynamic fallback), and means every subsequent pass resolves it via
    /// the ordinary entries scan instead of re-deriving it from the wire
    /// each time. When no active override applies at all after seeding
    /// (`target == None`, e.g. the type wasn't resolvable, or the user
    /// deactivated it), the effective splice target falls back to
    /// `natural_type(idx)` — `idx`'s inherited type from its parent's
    /// schema. That fallback never fires when an active entry explicitly
    /// says raw (`target == Some(None)`), which still renders raw, since
    /// that's an explicit user choice. The *outer* `Option` of `target` is
    /// still what gets stored into `rendered_as`, preserving the
    /// provenance distinction for the next pass — paired with
    /// `field_name_for(idx)` (spec 0119 §G4): either half changing (a
    /// retype, or a name-only rename of the governing entry) is enough to
    /// trigger a re-splice, since both feed directly into the rendered
    /// text.
    ///
    /// Named `render_overrides` (not `render`) to avoid colliding with the
    /// unrelated `render(&mut self, frame: &mut Frame)` ratatui draw
    /// method below.
    pub(super) fn render_overrides(&mut self, idx: usize) {
        if self.override_batch_depth == 0 {
            // Spec 0221 S1: emptied at the start of the *outermost*
            // pass, so nested `render_overrides` calls contribute to one
            // list and a caller reading it afterwards sees that pass and
            // nothing carried over from an earlier one.
            self.refusals.clear();
            let t_marks = std::time::Instant::now();
            self.compute_descend_marks();
            let d_marks = t_marks.elapsed();
            if crate::tui::trace::enabled() {
                use std::sync::atomic::Ordering::Relaxed;
                probe::VISITS.store(0, Relaxed);
                probe::SPLICES.store(0, Relaxed);
                probe::NODES.store(0, Relaxed);
                probe::SPLICE_US.store(0, Relaxed);
                probe::TEXT_US.store(0, Relaxed);
                probe::MARKS_US.store(d_marks.as_micros() as usize, Relaxed);
                crate::tui::trace::trace!(
                    "PROBE batch start tree={} marks={}",
                    self.tree.len(),
                    self.descend.iter().filter(|m| **m).count(),
                );
            }
        }
        self.override_batch_depth += 1;
        let path = self.positional_path(idx);
        // `idx` itself always starts outside any not-yet-materialized
        // patch (spec 0167): it's an already-existing node, never one
        // freshly created within this very call.
        let t_inner = std::time::Instant::now();
        self.render_overrides_inner(idx, &path, None);
        let d_inner = t_inner.elapsed();
        self.override_batch_depth -= 1;
        if self.override_batch_depth == 0 {
            let t_finalize = std::time::Instant::now();
            self.finalize_override_batch();
            if crate::tui::trace::enabled() {
                use std::sync::atomic::Ordering::Relaxed;
                probe::INNER_US.store(d_inner.as_micros() as usize, Relaxed);
                probe::FINALIZE_US.store(t_finalize.elapsed().as_micros() as usize, Relaxed);
            }
            if crate::tui::trace::enabled() {
                let rss = std::fs::read_to_string("/proc/self/statm")
                    .ok()
                    .and_then(|s| {
                        s.split_whitespace()
                            .nth(1)
                            .and_then(|v| v.parse::<u64>().ok())
                    })
                    .unwrap_or(0)
                    * 4096
                    / (1 << 20);
                use std::sync::atomic::Ordering::Relaxed;
                crate::tui::trace::trace!(
                    "PROBE render_overrides tree={} tree_cap={} lines={} lines_cap={} overrides={} rss_mib={} visits={} splices={} nodes={} marks_us={} inner_us={} splice_us={} finalize_us={} text_us={}",
                    self.tree.len(),
                    self.tree.capacity(),
                    self.lines.len(),
                    self.lines.capacity(),
                    self.overrides.entries().len(),
                    rss,
                    probe::VISITS.load(Relaxed),
                    probe::SPLICES.load(Relaxed),
                    probe::NODES.load(Relaxed),
                    probe::MARKS_US.load(Relaxed),
                    probe::INNER_US.load(Relaxed),
                    probe::SPLICE_US.load(Relaxed),
                    probe::FINALIZE_US.load(Relaxed),
                    probe::TEXT_US.load(Relaxed),
                );
            }
            // Spec 0221 S5: the status line keeps reporting a refusal,
            // as it has since spec 0202 — but summarizing the whole pass
            // rather than being assigned once per refused node, so N
            // refusals no longer collapse into whichever happened last.
            // Set here instead of at the fifteen `render_overrides` call
            // sites, which would otherwise each have to remember to.
            //
            // The startup pass sets it too, harmlessly: `main.rs`
            // replaces it with the "see stderr" wording once it has
            // printed the detail there (S4).
            match self.refusals.len() {
                0 => {}
                1 => self.message = format!("cannot apply override: {}", self.refusals[0].1),
                n => {
                    self.message = format!("{n} overrides refused, first: {}", self.refusals[0].1);
                }
            }
        }
    }

    /// Spec 0183 S2: extends `self.descend` for a batch about to
    /// start. A node is a *target* if this pass could change how it
    /// renders; every target and every ancestor of one gets marked, so
    /// that `render_overrides_inner`'s child gate can prune whole
    /// subtrees instead of descending into every message in the
    /// document.
    ///
    /// Marking ancestors — not just targets — is the point, and it is
    /// the part that is easy to get wrong (spec 0183 L3). A node-level
    /// predicate like `rendered_as != NOT_RENDERED` is only ever
    /// consulted if the walk *reaches* the node. Without the upward walk
    /// below, a marked node under unmarked ancestors would simply never
    /// be visited, and would keep the text it was last rendered with
    /// forever — no panic, no assertion, just stale content.
    ///
    /// Over-marking is safe (it costs a wasted descent); under-marking
    /// is the silent failure. The arena also holds slots this
    /// interpretation does not show, which are unreachable from the
    /// walk; they are skipped here, since a vacant slot carries no span
    /// to test.
    ///
    /// Spec 0188 S4/S5: the marks are kept between batches and only the
    /// arena's unexamined suffix is scanned. Rescanning the whole arena
    /// every batch costs a measured 35-44 ns/node — 17 ms on a 382 k-node
    /// arena — and two of the three per-node target sources cannot have
    /// changed: a node's auto-expand eligibility is a structural property
    /// that can only change by re-decoding the node (which produces a
    /// *different* node, scanned as fresh), and `rendered_as` only ever
    /// goes from `None` to `Some` in production. Spec 0216 makes the
    /// arena a fixed size, so after the first batch that suffix is empty:
    /// the whole scan happens once, and a splice's re-decoded slots are
    /// picked up by `mark_fresh_subtree` instead.
    ///
    /// The exception, and the reason `start` is not simply the
    /// watermark, is an `FqdnField` origin: "field N of every message
    /// of type T, anywhere" is a genuine search with no path to
    /// follow, and a newly activated one has to find its matches among
    /// nodes that were examined long ago. That case pays the full-arena
    /// scan; it is also the rare one (it has no keyboard shortcut and is
    /// never auto-seeded).
    fn compute_descend_marks(&mut self) {
        // The already-examined prefix — see `descend`'s own comment for
        // why the mark array's length is exactly that watermark.
        let scanned = self.descend.len();
        self.descend.resize(self.tree.len(), false);
        let has_fqdn_origin = self
            .overrides
            .entries()
            .iter()
            .any(|e| matches!(e.origin, OverrideOrigin::FqdnField { .. }));
        let start = if has_fqdn_origin { 0 } else { scanned };
        let targets = self.collect_descend_targets(start..self.tree.len(), None);
        self.mark_targets(targets);
    }

    /// Spec 0183 S3: extend the marks to cover a subtree that was just
    /// re-decoded, whose nodes did not exist when `compute_descend_marks`
    /// ran and so could not have been marked by it.
    ///
    /// The obvious alternative — carry a `fresh` flag down the walk and
    /// descend unconditionally beneath a splice — is what this replaces,
    /// and it is worth recording why, because it looks correct and is
    /// catastrophic. "Bounded by the size of the spliced content" is only
    /// reassuring while the spliced content is small; for a *root*
    /// retype it is the whole document. The flag then re-enables exactly
    /// the blanket descent this spec exists to delete, and worse: it
    /// visits plain scalar leaves too, and every fresh node has
    /// `rendered_as == None`, so `resettle_node` re-splices every single
    /// one of them. It presents as a hang at 100% CPU right after a root
    /// retype lands.
    ///
    /// Marking instead keeps the bound honest: the cost is one pass over
    /// the fresh nodes, and only the ones that are genuinely targets get
    /// descended into.
    ///
    /// Spec 0216 S12: the fresh set is `idx`'s subtree, not a range. A
    /// splice does not append to the arena — it rewrites the overlay on
    /// the slots those bytes already had — so there is no new tail a
    /// range could name.
    fn mark_fresh_subtree(&mut self, idx: usize, path: &str) {
        let mut fresh = Vec::new();
        self.collect_descendants(idx, &mut fresh);
        if fresh.is_empty() {
            return;
        }
        let targets = self.collect_descend_targets(fresh.iter().copied(), Some(path));
        self.mark_targets(targets);
    }

    /// Set `descend` on every target and on every ancestor of one,
    /// stopping each upward walk at the first already-marked node —
    /// everything above it is marked by construction.
    fn mark_targets(&mut self, targets: Vec<usize>) {
        for t in targets {
            let mut cur = Some(t);
            while let Some(c) = cur {
                if self.descend[c] {
                    break;
                }
                self.descend[c] = true;
                cur = self.parent(c);
            }
        }
    }

    /// The nodes among `nodes` whose rendering this batch could change.
    /// `under`, when set, restricts the path-shaped sources to override
    /// origins at or under that path — used by `mark_fresh_subtree`,
    /// where scanning every entry would defeat the point of bounding
    /// the work by the splice.
    fn collect_descend_targets(
        &self,
        nodes: impl Iterator<Item = usize>,
        under: Option<&str>,
    ) -> Vec<usize> {
        let mut targets: Vec<usize> = Vec::new();

        // Spec 0188 S5: the guard is on the whole loop, not just on the
        // `FqdnField` test inside it. With the marks kept across
        // batches (S4) an empty `nodes` is the ordinary case, and then
        // there is nothing here to allocate, hash or walk at all.
        let mut nodes = nodes.peekable();
        if nodes.peek().is_some() {
            // Active `FqdnField` origins as a set, so the per-node test
            // below is one hash lookup rather than a scan of
            // `entries()` (spec 0183 G3). This is exact — every node
            // carries its parent's resolved type — which is what spec
            // 0183 S5 asked of an FQDN index, without an index to
            // build or to keep patched across splices.
            // Spec 0212 S6: the origins' names are interned once, here,
            // rather than each node's id being resolved back to a string.
            let fqdn_fields: HashSet<(FqdnId, u64)> = self
                .overrides
                .entries()
                .iter()
                .filter_map(|e| match &e.origin {
                    OverrideOrigin::FqdnField { fqdn, field } => {
                        Some((self.fqdns.id_of(fqdn), *field))
                    }
                    _ => None,
                })
                .collect();

            for i in nodes {
                // Spec 0216 S12: a slot this interpretation does not
                // show has no span to test, and nothing can reach it.
                if !self.tree[i].is_rendered() {
                    continue;
                }
                // Source 2: a node spliced under an override at least
                // once must keep being revisited, so it can fall back
                // to its natural type once that override goes away.
                if self.tree[i].rendered_as != NOT_RENDERED {
                    targets.push(i);
                    continue;
                }
                // Source 3: the Any/MessageSet auto-expansion seeds.
                if self.is_auto_expand_candidate(i) {
                    targets.push(i);
                    continue;
                }
                // Source 1, the part that is not path-shaped.
                if !fqdn_fields.is_empty() {
                    let field = u64::from(self.tree[i].span.field_number);
                    if let Some(fqdn) = self.parent(i).map(|p| self.tree[p].span.type_fqdn) {
                        if fqdn_fields.contains(&(fqdn, field)) {
                            targets.push(i);
                        }
                    }
                }
            }
        }

        // Source 1, the path-shaped part. Resolved per entry rather
        // than per node, since a node does not know its own path
        // without an O(depth) walk. Entry state is not filtered on
        // `active`: a deactivated entry still has to be reached, so
        // that its node can be settled back to its natural type.
        //
        // Outside the `start < end` guard on purpose: this part costs
        // O(entries x depth) and never touches the arena, so it is
        // re-derived every batch. That is what keeps S4's kept marks
        // from having to be *removed* — an entry that goes away simply
        // stops being re-derived here.
        let scope = under.map(|p| OverrideOrigin::Path {
            path: p.to_string(),
        });
        for e in self.overrides.entries() {
            if let Some(scope) = &scope {
                if !override_pane::origin_is_at_or_under(&e.origin, scope) {
                    continue;
                }
            }
            match &e.origin {
                OverrideOrigin::Path { path } => {
                    targets.extend(self.resolve_path(path));
                }
                OverrideOrigin::PathField { path, field } => {
                    // The entry names the *parent*; the nodes whose
                    // rendering it governs are that parent's children
                    // bearing `field`.
                    if let Some(parent) = self.resolve_path(path) {
                        let mut c = self.first_child(parent);
                        while let Some(ci) = c {
                            if u64::from(self.tree[ci].span.field_number) == *field {
                                targets.push(ci);
                            }
                            c = self.next_sibling(ci);
                        }
                    }
                }
                OverrideOrigin::FqdnField { .. } => {}
            }
        }

        targets
    }

    /// `parent_path`'s display-format child path for the child at
    /// 1-based ordinal `ordinal` among its siblings — the same format
    /// `positional_path` builds (root is `"/"`; a root child is `"/1"`,
    /// `"/2"`, ...; deeper nodes append further `"/n"` segments) but
    /// computed in O(1) from an already-known parent path plus an
    /// already-known ordinal, rather than walking the tree. See
    /// `render_overrides_inner`'s use of this: it already visits every
    /// child in sibling order via `next_sibling`, so it can track the
    /// ordinal with a plain loop counter instead of `sibling_position`'s
    /// O(k) backward walk.
    fn child_path(parent_path: &str, ordinal: usize) -> String {
        if parent_path == "/" {
            format!("/{ordinal}")
        } else {
            format!("{parent_path}/{ordinal}")
        }
    }

    /// The actual (self-recursive) body of `render_overrides`.
    ///
    /// Spec 0210 S11: no line-count correction is carried down the tree.
    /// `span.text_range` is exact when the renderer emits it and is used
    /// then, to derive `lines_total`; afterwards every caller that wants
    /// a line range asks `node_lines`, which derives it from the
    /// counters and cannot go stale. Correcting it instead would be a
    /// walk proportional to the document rather than to the splice: on
    /// the reference corpus an override on the document's *first*
    /// top-level record shifts 4 500 963 spans, 402 ms of a 500 ms
    /// commit, while the same override on the last record shifts none.
    ///
    /// `path` is `idx`'s own already-known positional path (spec 0163
    /// follow-up), passed down from the caller rather than recomputed
    /// via `positional_path(idx)`: this walk visits every node in the
    /// whole document exactly once, so recomputing each node's path from
    /// scratch (`positional_path` is O(depth) ancestor hops, each paying
    /// an O(k) `sibling_position` walk) turns an O(n) walk into
    /// something far worse on a document with large sibling groups —
    /// observed to make a single `render_overrides` pass take minutes on
    /// a ~600k-node document with sibling groups in the hundreds. Since
    /// children are visited in sibling order via `next_sibling` below, a
    /// child's own path is available in O(1) from `path` plus a running
    /// ordinal counter (`child_path`).
    ///
    /// `patch_scope` (spec 0167, N1 follow-up to spec 0160): the index
    /// into `self.pending_line_patches` of the nearest still-open
    /// ancestor patch whose freshly decoded content `idx`'s own position
    /// currently lies within, or `None` if `idx`'s position is still
    /// within `self.lines`/`self.line_styles` as they stood before this
    /// batch began. A node freshly created by an ancestor's own splice,
    /// within this very same batch, can itself need its own re-splice
    /// (e.g. a nested message getting its own override applied on top of
    /// its parent's just-decoded natural rendering) — such a re-splice's
    /// content must be recorded as *nested inside* that ancestor's own
    /// not-yet-materialized patch, not as a patch against `self.lines`
    /// directly (which, for the whole duration of the batch, still holds
    /// only pre-batch content — see `splice_override`).
    ///
    /// `fresh` (spec 0183 S3) is `true` when `idx` lies inside content
    /// re-decoded earlier in this very batch. Such nodes did not exist
    /// when `compute_descend_marks` ran, so they carry no mark and are
    /// descended into unconditionally instead — bounded by the size of
    /// the spliced content, which is work the splice implies anyway.
    /// This is also what makes it sound for the mark scan to ignore
    /// auto-expand candidates that only *become* candidates during the
    /// batch (MessageSet tier 2, whose eligibility depends on its
    /// parent having just been retyped by tier 1): they are always
    /// inside fresh content by construction.
    fn render_overrides_inner(&mut self, idx: usize, path: &str, patch_scope: Option<usize>) {
        let origin = OverrideOrigin::Path {
            path: path.to_string(),
        };
        let already_seeded = self.overrides.entries().iter().any(|e| e.origin == origin);
        if !already_seeded {
            if let Some(t) = self.auto_expand_type(idx) {
                // MessageSet tier 1's synthetic wrapper field has no
                // schema-declared name to fall back on (`field_name_for`
                // would otherwise show the bare field number "1"), so
                // seed it with the display name `prototext-core`'s native
                // MessageSet rendering uses for it ("Item") — spec 0120
                // §G2's follow-up cosmetic fix.
                let is_message_set_item = self.tree[idx].span.field_number == 1
                    && u32::from(self.tree[idx].span.wire_type)
                        == prototext_core::helpers::WT_START_GROUP
                    && self
                        .parent(idx)
                        .is_some_and(|p| self.is_message_set_typed(p));
                self.overrides.activate_auto(origin.clone(), Some(t));
                if is_message_set_item {
                    if let Some(entry_idx) = self
                        .overrides
                        .entries()
                        .iter()
                        .position(|e| e.origin == origin)
                    {
                        self.overrides.rename(entry_idx, Some("Item".to_string()));
                    }
                }
            }
        }
        let spliced_patch = self.resettle_node(idx, path, patch_scope);
        let spliced = spliced_patch.is_some();
        if crate::tui::trace::enabled() {
            use std::sync::atomic::Ordering::Relaxed;
            probe::VISITS.fetch_add(1, Relaxed);
            if spliced {
                // Spec 0216 S12: the splice does not append, so the size
                // of what it produced is the size of `idx`'s subtree
                // rather than the arena's growth.
                let mut sub = Vec::new();
                self.collect_descendants(idx, &mut sub);
                let n = sub.len();
                probe::SPLICES.fetch_add(1, Relaxed);
                probe::NODES.fetch_add(n, Relaxed);
                if n >= 10_000 {
                    crate::tui::trace::trace!("PROBE big splice n={n} path={path}");
                }
            }
        }
        // Spec 0167: if `idx` was just spliced, its fresh children's
        // content lives inside `idx`'s own new patch; otherwise, they
        // remain wherever `idx` itself was already found (the same
        // ancestor patch, if any, or `self.lines` directly).
        let child_scope = spliced_patch.or(patch_scope);
        // Spec 0183 S3: the nodes the splice just re-decoded were not
        // marked when the batch's marks were computed, so mark them now.
        if spliced {
            self.mark_fresh_subtree(idx, path);
        }
        // Spec 0216 S22: a packed run is one slot, so a child's ordinal
        // is just its position in the block — no `same_packed_record`
        // merge is needed here (nor in spec 0184 S4's backward walk).
        for k in 0..self.child_count(idx) {
            let c = self.nth_child(idx, k).expect("k is below the child count");
            let ordinal = k + 1;
            // Spec 0183 G1: descend only where something can actually
            // change. The obvious gate — `span.is_message`, plus the
            // three real target sources — is true of essentially every
            // interior node in a real document, which is what makes a
            // single pass cost seconds on a 600k-node one. It is not
            // really a claim that a message node needs work; it is a
            // stand-in for "something under here might", the walk having
            // no cheaper way to find out. `descend` is that cheaper way,
            // computed once per batch by `compute_descend_marks` from
            // the same three sources (override entries, `rendered_as`,
            // auto-expand seeds) but lifted to cover ancestors, which is
            // what makes it safe to stop at an unmarked node.
            let c_path = Self::child_path(path, ordinal);
            #[cfg(not(test))]
            let descend_here = self.descend.get(c).copied().unwrap_or(false);
            #[cfg(test)]
            let descend_here = if self.unpruned_walk {
                self.tree[c].span.is_message
                    || self.is_auto_expand_candidate(c)
                    || self
                        .resolve_active_override_entry_index_by_path(c, &c_path)
                        .is_some()
                    || self.tree[c].rendered_as != NOT_RENDERED
            } else {
                self.descend.get(c).copied().unwrap_or(false)
            };
            if descend_here {
                self.render_overrides_inner(c, &c_path, child_scope);
            }
        }
    }

    /// Spec 0160 G1: runs exactly once, when the outermost
    /// `render_overrides` call for a batch of splices returns (or a
    /// standalone `splice_override` call finishes) — not once per
    /// splice. Every node's line *counts* are already correct by this
    /// point: `splice_override` fixes its own target's and carries the
    /// change up the ancestors as it goes, because the rest of the batch
    /// derives its patch positions from them. So all that is left here
    /// is the document *text* — the batch's queued line patches, merged
    /// into `self.lines` in a single pass.
    fn finalize_override_batch(&mut self) {
        // Spec 0186 S2: the first line this batch can have disturbed, in
        // the final buffer's frame.
        //
        // Spec 0188 G1: `None` means the batch queued no patches at all,
        // and that is not a "no safe lower bound, rebuild everything"
        // case — it is a batch that spliced nothing. `pending_patch_min_
        // line` and `pending_line_patches` are written at one site, so
        // no patch means no text was replaced; `pending_shift` is
        // accumulated at that same site, so it is zero and no span was
        // shifted either. Nothing below has anything to repair.
        //
        // This is the common case, not an exotic one: opening or
        // closing the override pane, toggling a management-pane entry
        // that resolves to what is already rendered, and the second of
        // two identical passes all land here. Repairing unconditionally
        // costs 20 ms on a 382 k-node arena, to fix nothing.
        //
        // The G3 equivalence check still runs, so every no-patch batch
        // in the suite asserts that skipping was in fact a no-op.
        if self.pending_patch_min_line.is_none() {
            #[cfg(test)]
            if self.verify_repair {
                self.assert_line_counts_are_exact();
            }
            return;
        }
        // The batch's line-buffer patches, applied in one pass.
        let t_text = std::time::Instant::now();
        self.materialize_line_patches();
        if crate::tui::trace::enabled() {
            probe::TEXT_US.fetch_add(
                t_text.elapsed().as_micros() as usize,
                std::sync::atomic::Ordering::Relaxed,
            );
        }

        // Spec 0210 S3: patching the text is the entire repair.
        //
        // A node stores its subtree's line *count*, not its absolute
        // position, and a count does not care what happens above it.
        // Were positions stored, a splice would owe a walk of every
        // node after it in document order — all four and a half million
        // of them, which is what made overriding the document's first
        // field cost a second. As it is, a splice owes only its
        // ancestors' counts, and pays that itself as it goes, in
        // `splice_override`, because the rest of the batch derives its
        // patch positions from them.
        self.pending_shift = 0;
        self.pending_patch_min_line = None;
        // Line numbers moved, so the read-ahead walk must restart and
        // `window_nodes` must be ignored.
        self.structural_version += 1;
        self.clamp_pan_offset();

        // Spec 0186 G3, carried onto spec 0210's own invariant. Hung
        // off the finalizer rather than written as one dedicated test,
        // so that *every* splice in the whole suite is a case: the
        // interesting inputs (nested patches, packed runs, repeated
        // overrides of one node, folded targets, auto-expanded `Any`/
        // `MessageSet` descendants) are already fixtured, and none of
        // them would think to opt in.
        #[cfg(test)]
        if self.verify_repair {
            self.assert_line_counts_are_exact();
        }
    }

    /// Spec 0210's invariant, checked over the whole document: every
    /// node's two counts are exactly what its children's are, and the
    /// positions they imply are exactly where the text has its braces.
    ///
    /// The question is not spec 0186 G3's "does the incremental repair
    /// match a full rebuild" — there is nothing to repair and nothing to
    /// rebuild — but whether the counters still describe the document.
    /// That is the stronger property: a comparison against a rebuild
    /// *derives* its reference from the same `text_range` fields it is
    /// checking, so a range corrupted consistently corrupts both sides
    /// and the comparison succeeds.
    ///
    /// The failure it exists to catch is silent. A count that is wrong
    /// by one puts every position after it out by one, and nothing
    /// panics — the fold handle, the heat cue and the cursor simply
    /// attach to a neighboring line, and the reported symptom is "the
    /// marker is on the closing brace".
    ///
    /// A single pre-order walk, carrying each node's header line down
    /// to its children, so it is O(nodes) rather than a descent per
    /// node. Not reachable from a release build.
    #[cfg(test)]
    fn assert_line_counts_are_exact(&self) {
        let n_lines = self.lines.len();
        let indent_of = |l: usize| self.lines[l].len() - self.lines[l].trim_start().len();

        // The top level is a forest in the fixtures and a single root in
        // a real document, so start from the head of whatever sibling
        // chain `first_node` sits on.
        let mut top = self.first_node;
        while let Some(p) = self.parent(top) {
            top = p;
        }
        while let Some(s) = self.prev_sibling(top) {
            top = s;
        }

        // `(node, its header line)`, pushed in reverse so that popping
        // yields document order.
        let mut stack: Vec<(usize, usize)> = Vec::new();
        let mut start = 0usize;
        let mut roots = Vec::new();
        let mut r = Some(top);
        while let Some(n) = r {
            roots.push((n, start));
            start += self.tree[n].lines_total as usize;
            r = self.next_sibling(n);
        }
        assert_eq!(
            start, n_lines,
            "the document's roots account for {start} lines but it has {n_lines}"
        );
        stack.extend(roots.into_iter().rev());

        while let Some((n, start)) = stack.pop() {
            let total = self.tree[n].lines_total as usize;
            let visible = self.tree[n].lines_visible as usize;

            let mut kids = Vec::new();
            let mut sum_total = 0usize;
            let mut sum_visible = 0usize;
            let mut child_start = start + 1;
            let mut c = self.first_child(n);
            while let Some(ci) = c {
                kids.push((ci, child_start));
                child_start += self.tree[ci].lines_total as usize;
                sum_total += self.tree[ci].lines_total as usize;
                sum_visible += self.tree[ci].lines_visible as usize;
                c = self.next_sibling(ci);
            }

            // Spec 0216: whether a node brackets its children is the
            // node's own property, not something the child count can be
            // asked about — a bracketed node may legitimately have none
            // (an empty message), and a flat one may draw many rows (a
            // packed record draws one per element) while having none.
            let bracketed = self.tree[n].is_bracketed();
            if bracketed {
                // Header and footer, one line each, whatever is between.
                assert_eq!(
                    total,
                    sum_total + 2,
                    "node {n}'s lines_total is {total}, but its {} children \
                     occupy {sum_total} lines",
                    kids.len()
                );
                let want_visible = if self.folded.contains(&n) {
                    1
                } else {
                    sum_visible + 2
                };
                assert_eq!(
                    visible,
                    want_visible,
                    "node {n}'s lines_visible is {visible} but its children's \
                     come to {sum_visible} (folded: {})",
                    self.folded.contains(&n)
                );
            } else {
                assert_eq!(
                    sum_total,
                    0,
                    "flat node {n} owns all {total} of its rows, so it can \
                     have no children, but it has {}",
                    kids.len()
                );
                assert_eq!(
                    visible, total,
                    "flat node {n} cannot be folded, so its two counts must \
                     agree"
                );
            }

            // The one check tied to the text rather than to the tree,
            // and the only thing that can catch a set of counts that is
            // self-consistent and still wrong.
            if bracketed {
                let open = &self.lines[start];
                let code = open.split("  #@").next().unwrap_or(open).trim_end();
                assert!(
                    code.ends_with('{'),
                    "node {n} starts at line {start} and spans {total} lines, \
                     so that is its opening line, but it reads {open:?}"
                );
                let close_at = start + total - 1;
                let close = &self.lines[close_at];
                assert!(
                    close.trim_start().starts_with('}'),
                    "node {n} closes at line {close_at}, but that line \
                     reads {close:?}"
                );
                assert_eq!(
                    indent_of(close_at),
                    indent_of(start),
                    "node {n}'s closing line {close_at} is indented \
                     differently from its opening line {start}: {close:?} vs {:?}",
                    self.lines[start]
                );
            }

            stack.extend(kids.into_iter().rev());
        }
    }

    /// Spec 0174: default for `App::override_preview_byte_budget` — the
    /// maximum number of *interior* bytes of a *live-preview*
    /// `splice_override` candidate that are handed to the renderer.
    /// Guards against a structurally mismatched candidate type causing
    /// the recursive-descent decoder to mis-parse arbitrary bytes into a
    /// pathologically large synthetic tree (observed: 1,083,626 spans
    /// from a single splice on a ~1.1MB field — larger than the entire
    /// 635,052-node original document). Bounding the *input* bounds the
    /// decode, the render, the span count and the line count together,
    /// which is why the renderer itself needs no budget of its own
    /// (spec 0174 G1 removed `DecodeRenderOpts::node_budget`).
    ///
    /// Only applies while a candidate is being *previewed*
    /// (`splice_override`'s `is_preview: true`, `preview_override_
    /// highlight`'s sole call site) — once a candidate is actually
    /// confirmed as a real override, its rendering must be complete, not
    /// truncated, so every other `splice_override` call site (routed
    /// through `resettle_node`) passes `is_preview: false` and gets the
    /// candidate's bytes untouched.
    ///
    /// 4096 is generous in lines while still bounding the work: the
    /// smallest interior field is two bytes, so it admits at most ~2000
    /// nodes, and a realistic mixed payload yields a few hundred lines —
    /// more than any pane shows, which is the point. A preview only
    /// needs to show enough of a wrong-type candidate's shape for the
    /// user to judge it's the wrong one and move on.
    /// Overridable at startup via `--override-preview-byte-budget`.
    pub(crate) const OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT: usize = 4096;

    /// Unified splice mechanic (spec 0118 §4, reworked spec 0135 G1):
    /// regenerates the *whole* rendering of `idx` — header, interior, and
    /// footer alike — under `target` (`None` = revert to raw, `Some(fqdn)`
    /// = retype/promote to a message FQDN, `Some(keyword)` = retype to a
    /// wire-compatible primitive type, G3/G4). No existing rendering of
    /// `idx` is ever reused verbatim: decodes `idx`'s own real tag+payload
    /// bytes (`old_span.raw_range`) directly against a synthetic one-field
    /// descriptor (`decode::register_wrapper`) whose sole field has `idx`'s
    /// own real field number and the target's declared type — the node's
    /// real wire framing (message/group/scalar) is reproduced by
    /// `TextSink` for free, no header patching needed (spec 0135
    /// Background). This is what fixes task #34 (a stale `#@` type
    /// annotation surviving a retype) as a byproduct, for every node.
    ///
    /// `idx` keeps its own slot (so `cursor`/`folded`/back-jump state
    /// referencing it stays valid), and so does every node under it that
    /// the new interpretation still shows — spec 0216 S12: the structure
    /// belongs to the arena, which is a function of the bytes and does
    /// not move when the type assignment changes. All that happens here
    /// is that the overlay under `idx` is vacated and rewritten — no slot
    /// is allocated, abandoned or renumbered, so no index held anywhere
    /// else needs translating.
    ///
    /// A packed run is one slot (S22), so no "sibling merge" is needed
    /// either: `render_node_as` resolves `idx` to the run and widens
    /// `old_span` to its whole extent, and there are no sibling nodes to
    /// absorb.
    ///
    /// `is_preview`: `true` from `preview_override_highlight`'s sole live-
    /// preview call site — caps the *interior bytes* handed to the
    /// renderer at `override_preview_byte_budget` (spec 0174). `false`
    /// from every other call site (routed through `resettle_node`, i.e.
    /// an already-confirmed/active override being (re)applied) — renders
    /// completely, unbounded, since this is the content that actually
    /// gets shown as the real override, not a speculative preview.
    pub(super) fn splice_override(
        &mut self,
        idx: usize,
        target: Option<String>,
        is_preview: bool,
        patch_scope: Option<usize>,
    ) -> Result<usize, String> {
        // Spec 0185 S3: the "what does this node look like as `target`"
        // half lives in `render_node_as`, shared verbatim with the live
        // preview — including the packed-record normalization, which is
        // what resolves `idx` to the run and widens `old_span` to the
        // run's whole extent (spec 0135 G1).
        let (idx, old_span, rendered) = self.render_node_as(idx, target.as_deref(), is_preview)?;
        let RenderedAs {
            lines: new_lines,
            spans: new_spans,
            ..
        } = rendered;

        let delta = new_lines.len() as isize
            - (old_span.text_range.end - old_span.text_range.start) as isize;
        // Spec 0160 G2 / spec 0210 S11: `pending_shift` is the batch's
        // running line offset, and its only remaining consumer is patch
        // placement. No node's position is stored, so nothing downstream
        // has to be walked and corrected; a node's line range is derived
        // from the counters whenever it is asked for.
        //
        // Spec 0167: capture `pending_shift`'s value *before* this call's
        // own delta is folded in — `old_span.text_range` above is
        // `node_lines(idx)`, so it already reflects every earlier splice
        // in this batch, and subtracting this pre-increment value
        // recovers the position `self.lines` still has it at, that buffer
        // being patched only at the end of the batch (see below).
        let pending_shift_before = self.pending_shift;
        self.pending_shift += delta;

        // Everything the *previous* interpretation showed under `idx`,
        // collected before any of it is vacated. Scrub the whole set from
        // `folded`: a fold flag on a slot this rendering does not show
        // would be honored again the moment some later override brings
        // the slot back, hiding unrelated content. `idx` itself is
        // deliberately left in `folded` untouched (spec 0118 §7 — fold
        // state on `idx` survives its own retype).
        let mut old_descendants = Vec::new();
        self.collect_descendants(idx, &mut old_descendants);
        for &d in &old_descendants {
            self.folded.remove(&d);
            self.tree[d] = decode::TreeNode::vacant();
            // The cue answered a question about a node this rendering no
            // longer has; if the slot comes back it must be asked again.
            self.heat_states[d] = heat_cue::HeatState::default();
        }

        // Replace `idx`'s *whole* line range (header, interior, and
        // footer alike) — not just its interior, unlike the old
        // `apply_override`. Spec 0167: rather than eagerly
        // `Vec::splice`-ing `self.lines` here (an
        // O(document length) memmove *per splice*, dominating a batch
        // with many qualifying splices — spec 0160 N1), record a patch
        // and defer the actual buffer write to a single materialization
        // pass in `finalize_override_batch`. `patch_scope` is `None` when
        // `idx` itself lives in `self.lines` as it stood before this
        // batch began (`old_span.text_range` is already batch-corrected
        // — spec 0160 G2 — so recovering the position `self.lines` still
        // has it at just means subtracting `pending_shift_before`); it's
        // `Some(parent_idx)` when `idx` is a node freshly created by an
        // ancestor's own not-yet-materialized splice earlier in this same
        // batch (`render_overrides_inner`'s doc comment), in which case
        // the recorded range is local to that parent patch's own content
        // instead, recovered via the parent's stored `global_start`.
        let old_lines = widen(&old_span.text_range);
        let global_start = old_lines.start;
        let target_range = match patch_scope {
            None => {
                let original_start = (old_lines.start as isize - pending_shift_before) as usize;
                let original_end = (old_lines.end as isize - pending_shift_before) as usize;
                LinePatchTarget::Original(original_start..original_end)
            }
            Some(parent_idx) => {
                // `old_span.text_range` is `node_lines(idx)` (see the top
                // of this function), i.e. this node's true *current*
                // document position, which grows every time a sibling
                // processed earlier within the same parent gets its own
                // splice — but the parent's own `lines` is a frozen
                // snapshot, never touched again after creation.
                // Undo exactly that extra growth (everything accumulated
                // since the parent's `children_base_shift`) to recover
                // the offset that's actually valid in the parent's frozen
                // `lines`.
                let parent = &self.pending_line_patches[parent_idx];
                let parent_start = parent.global_start as isize;
                let extra_growth = pending_shift_before - parent.children_base_shift;
                let local_start = (old_lines.start as isize - parent_start - extra_growth) as usize;
                let local_end = (old_lines.end as isize - parent_start - extra_growth) as usize;
                LinePatchTarget::Nested(parent_idx, local_start..local_end)
            }
        };
        // Spec 0186 S2: remember the earliest line this batch can have
        // disturbed. `global_start` is `old_span.text_range.start`, i.e.
        // already batch-corrected (spec 0160 G2), so it is this patch's
        // position in the *final* buffer — the frame the repair works
        // in. A `Nested` patch lies inside its parent's content and so
        // can only raise this, but it
        // costs nothing to fold it in at the one site where every patch
        // passes.
        self.pending_patch_min_line = Some(match self.pending_patch_min_line {
            Some(existing) => existing.min(global_start),
            None => global_start,
        });
        let patch_idx = self.pending_line_patches.len();
        self.pending_line_patches.push(LinePatch {
            target: target_range,
            global_start,
            children_base_shift: self.pending_shift,
            lines: new_lines,
        });

        // Spec 0216 S12: write the new rendering into the slots the arena
        // already has for these bytes. `idx` is the local render's root,
        // so the local root span lands back on `idx` itself and every
        // descendant span lands on the slot the wire structure gives it —
        // no append, no coordinate translation, no pointer repair.
        //
        // The byte ranges need no translation either, so no `span_shift`
        // (spec 0174) accompanies them: `overlay_spans` takes
        // `raw_range` and `packed_record_start` from the arena, and the
        // arena is expressed against `self.blob` by construction. A
        // truncated preview's narrower length varint therefore cannot
        // put a span in the wrong place; at worst it produces a span the
        // maximal walk never saw, which `slots_for_spans` rejects.
        //
        // `idx`'s own slot has to be vacated first, alongside the
        // descendants above: `overlay_spans` treats a second span landing
        // on an already-rendered slot as one more row of a packed run and
        // *adds* its lines, so leaving the previous interpretation in
        // place would count `idx`'s lines twice over.
        self.tree[idx] = decode::TreeNode::vacant();
        decode::overlay_spans(&mut self.tree, new_spans, &self.arena, idx);
        // `idx` itself was just retyped, so a cue resolved for it before
        // now answers a question about the superseded interpretation
        // (spec 0152 G6). Its descendants were reset above, when their
        // slots were vacated.
        self.heat_states[idx] = heat_cue::HeatState::default();

        // The subtree under `idx` comes over unfolded, but `idx`'s own
        // fold survives a retype (only its descendants' folds are
        // scrubbed, above — spec 0118 §7), and a folded node shows one
        // line whatever is beneath it. `overlay_spans` cannot know that,
        // so it set both counts to the full size.
        if self.folded.contains(&idx) {
            self.tree[idx].lines_visible = 1;
        }

        // Spec 0210 S3: the ancestors' sizes, and nothing else. It
        // belongs *here*, per splice, rather than once per batch in
        // `finalize_override_batch`: a batch splices many nodes, and the
        // position every one of them is patched at is derived from these
        // counts, so deferring the refresh would leave the second splice
        // reading the first splice's stale ancestors.
        //
        // O(depth), and it stops as soon as a node's counts come out
        // unchanged. `idx`'s own counts were taken from the subtree just
        // built, above.
        if let Some(parent) = self.parent(idx) {
            self.refresh_line_counts(parent);
        }

        // Spec 0160 G2: no eager walk of the document happens here — the
        // ancestor counts above are the whole structural consequence of
        // this splice, and `self.pending_shift += delta` records what the
        // rest of the batch needs to place its patches. When called from
        // within a `render_overrides` batch (`override_batch_depth > 0`),
        // reconciliation is deferred to that outer call's own
        // `finalize_override_batch` — which is every production call
        // since spec 0185 made the preview an overlay. A standalone
        // splice (`override_batch_depth == 0`, tests only) must finalize
        // immediately itself.
        if self.override_batch_depth == 0 {
            self.finalize_override_batch();
        }

        // Spec 0142 G6.1: `idx` keeps its own slot across a retype (see
        // this function's own doc comment), but not its shape, so a
        // cursor resting anywhere but the header has to be re-placed.
        // Spec 0216 S7 makes that a coordinate rather than a flag, and
        // the coordinate is not stable: a message's closing brace moves
        // whenever the body it encloses changes size, and the retype may
        // have turned the node flat, in which case the brace is gone.
        if self.cursor_line_in_node != 0 {
            let node = &self.tree[self.cursor];
            self.cursor_line_in_node = if node.is_bracketed() {
                node.lines_total - 1
            } else {
                self.cursor_line_in_node.min(node.lines_total - 1)
            };
        }

        Ok(patch_idx)
    }

    /// Spec 0185 S3: render `idx` as if it were `target`, without
    /// touching the tree, the line buffers, or anything else document-
    /// sized. Returns the node the rendering actually applies to — for a
    /// packed-repeated element that is the run's *leader*, not the
    /// element the caller named (spec 0135 G1's sibling merge, spec
    /// 0184's "the record is the addressable unit") — that node's span
    /// widened to the whole run's byte and line extent, and the
    /// rendering itself.
    ///
    /// `splice_override` calls this and proceeds to splice the result in;
    /// the live preview (`preview_override_highlight`) calls it and
    /// stops, holding the result as an overlay. There is deliberately no
    /// second rendering path: a preview and the commit that follows it
    /// must be byte-identical (spec 0185 G3), and sharing this function
    /// is what guarantees it.
    ///
    /// `is_preview` caps the *interior bytes* handed to the renderer at
    /// `override_preview_byte_budget` (spec 0174) and is part of the
    /// render-cache key, so a truncated preview render and a full
    /// confirmed render of the same `(range, target)` are never
    /// conflated.
    pub(super) fn render_node_as(
        &mut self,
        idx: usize,
        target: Option<&str>,
        is_preview: bool,
    ) -> Result<(usize, NodeSpan, RenderedAs), String> {
        // Spec 0160 G2: `self.tree[idx].span` is already authoritative
        // by the time this is called — either `render_overrides_inner`'s
        // prologue already applied `idx`'s own pending correction (and,
        // for a packed member, every other member of its run too), or
        // this is a standalone call (`override_batch_depth == 0`), where
        // `pending_shift == 0` because the tree is already fully
        // reconciled from the previous batch.
        let mut old_span = self.tree[idx].span.clone();
        // Spec 0210 S1: `span.text_range` is the *build-time* line range
        // and nothing repairs it, so re-derive it from the counters
        // before either caller reads it. Both of them want the line
        // range as it is right now — the preview to pick the rows its
        // overlay stands in for, the splice to know what it replaces.
        old_span.text_range = decode::narrow(self.node_lines(idx));

        // Packed-record reconstruction (spec 0135 G1): the whole run is
        // one addressable record, so widen `old_span` to the record's
        // own extent before proceeding.
        let in_packed_run = old_span.packed_record_start != NO_PACKED_RECORD;
        if in_packed_run {
            let (raw_range, text_range) = self.packed_record_extent(idx);
            old_span.raw_range = decode::narrow(raw_range);
            old_span.text_range = decode::narrow(text_range);
        }

        // Spec 0219 S3: whether the synthetic field below is declared
        // `repeated [packed=true]` rather than `optional`. The rule, and
        // why neither of its two halves works alone, is at
        // `decode::packed_framing`; `warm_visible_override_wrappers`
        // asks the same function, which is what keeps warming and the
        // splice looking up the same wrapper.
        let packed = decode::packed_framing(&old_span);

        let field_number = old_span.field_number;
        let field_name = self.field_name_for(idx);
        let renamed = self
            .resolve_active_override_entry(idx)
            .and_then(|e| e.name.clone())
            .is_some();
        // An un-renamed extension's schema name must be shown in
        // prototext's `[fqdn]` bracket convention (mirrors `prototext_
        // core::FieldOrExt::display_name`) so the patched header both
        // reads correctly and re-colorizes as an extension reference
        // (`colorize`'s highlight query keys off the brackets) — plain
        // `field_name_for` deliberately returns the bare name here since
        // its other callers (`export_descriptor::synthetic_field_name`/
        // `synthetic_message_name`, which need a valid identifier) must
        // not see brackets.
        let header_field_name =
            if !renamed && matches!(self.parent_field(idx), Some(ParentFieldOrExt::Ext(_))) {
                format!("[{field_name}]")
            } else {
                field_name.clone()
            };

        // Resolve `target` into the synthetic field's declared `Type`
        // (spec 0135 G1's "second subtlety" + G3, spec 0137 §G3/§G4): a
        // message FQDN yields `Type::Group` only when the node's real
        // wire framing is `WT_START_GROUP`, else `Type::Message`; a
        // primitive keyword yields the matching primitive `Type`
        // directly; an enum FQDN yields `Type::Enum`; the reserved
        // `None` sentinel string and a plain `Option::None` (raw) both
        // yield no synthetic field at all.
        let (target_desc, field_type) = match target {
            None => (None, None),
            Some("protolens_internal.None") => (None, None),
            Some(name) => {
                let is_group =
                    u32::from(old_span.wire_type) == prototext_core::helpers::WT_START_GROUP;
                let Some((desc, ft)) = self.ctx.wrapper_target_for(name, is_group) else {
                    return Err(format!("type '{name}' not found in descriptor set"));
                };
                (desc, Some(ft))
            }
        };

        // Decode `idx`'s own real tag+payload bytes directly (spec 0135
        // G1) — no synthetic tag prepended.
        let mut field_bytes = self.blob[widen(&old_span.raw_range)].to_vec();

        // Spec 0174: only a live preview is speculative and needs
        // bounding — a confirmed override must render completely (G5).
        // Bounding the renderer's *input* bounds its decode, its render,
        // its span count and its line count together, which is why
        // `prototext-core` itself carries no budget.
        let mut truncated = false;
        if is_preview {
            let shape = trunc_shape_for(field_type, u32::from(old_span.wire_type), packed);
            if let Some(cut) =
                truncate_interior(&field_bytes, self.override_preview_byte_budget, shape)
            {
                field_bytes = cut;
                truncated = true;
            }
        }

        // Render-cache key: `(interior_range, target)`. The field name is
        // deliberately not part of it — G2 patches the name into the
        // rendered header below, so the cached render itself is
        // field-name-invariant. `packed_record_start` is always absent
        // here, the packed case having been normalized above.
        let interior_range = extract::message_payload_range(&self.blob, &old_span.raw_range);
        // Spec 0163: `is_preview` is part of the key -- a budget-
        // truncated preview render and a full confirmed render of the
        // same `(range, target)` must never be conflated, or confirming
        // an override could silently reuse a truncated preview render.
        let cache_key = (interior_range, target.map(str::to_string), is_preview);
        let (mut new_lines, new_spans) = match self.render_cache.get(&cache_key) {
            Some(cached) => cached,
            None => {
                let wrapper_desc = match field_type {
                    Some(ft) => Some(
                        decode::register_wrapper(
                            self.ctx.pool_mut(),
                            field_number,
                            ft,
                            target_desc,
                            packed,
                        )
                        .map_err(|e| e.to_string())?,
                    ),
                    None => None,
                };
                let opts = DecodeRenderOpts {
                    // Always on (spec 0133): annotations are a pure
                    // main-pane display concern, not a decode-time input.
                    annotations: true,
                    indent_size: self.indent_size,
                    initial_level: old_span.level as usize,
                    emit_header: false,
                    // Any/MessageSet expansion is handled by protolens
                    // itself, as automatic overrides (spec 0120), not by
                    // prototext-core's own virtual-node expansion.
                    expand_any: false,
                    expand_message_set: false,
                    ..Default::default()
                };
                // Spec 0212 S4: `self.fqdns` is the one table every span in
                // this document indexes into, so a spliced subtree's ids
                // mean the same thing as the arena's around it.
                let rendered = decode_and_render_indexed(
                    &field_bytes,
                    wrapper_desc.as_ref(),
                    &mut self.fqdns,
                    opts,
                )
                .map_err(|e| e.to_string())?;
                let new_spans = rendered.spans;
                let new_text = String::from_utf8(rendered.text)
                    .map_err(|e| format!("rendered text is not valid UTF-8: {e}"))?;
                let new_lines: Vec<String> = new_text.lines().map(str::to_string).collect();
                let value = (new_lines, new_spans);
                self.render_cache.insert(cache_key, value.clone());
                value
            }
        };

        // G2: the only remaining header patch is a plain substring
        // replacement of the synthetic field's placeholder name (`"_"`,
        // `register_wrapper`'s fixed literal) with the real display name
        // — the header line itself is otherwise already correct (spec
        // 0135 G1). Nor for `Type::Group`: `TextSink::begin_nested` labels
        // a group header with the group's own message type name, never
        // the field's declared name — standard proto2 group text-format
        // convention — so the `"_"` placeholder is never actually
        // present there. Any group target that needs a display name
        // other than its own type name must instead be named that way
        // at the source (e.g. `register_message_set_item`'s synthetic
        // shape is itself named `Item`, matching `prototext-core`'s own
        // native MessageSet rendering convention) rather than patched
        // here after the fact. For the raw (`target: None`) case, there
        // is no synthetic field/placeholder to patch — the header
        // already shows the node's own numeric field number straight off
        // the wire — except when an active override entry gives this
        // node its own rename (spec 0119 §G4), which must still show up
        // in place of that bare field number even though the node stays
        // raw.
        //
        // Spec 0187 S5: the text patch is all there is — there is no
        // parallel style array to repair alongside it. Highlighting is
        // computed per frame over the on-screen window
        // (`render::window_styles_for`), which frames that window in
        // synthetic braces before parsing it; re-`colorize`-ing the
        // patched header on its own would be flaw D4's "highlight one
        // line out of context".
        // A packed run renders one line per element, each carrying its
        // own placeholder, so the patch is per line rather than to the
        // header alone. Only in that case: elsewhere the wrapper's sole
        // field draws exactly one placeholder, and a blanket pass could
        // reach a nested field genuinely named `_`.
        let patch_rows = match field_type {
            Some(ft) if ft != Type::Group && packed && decode::is_packable(ft) => new_lines.len(),
            _ => 1,
        };
        for line in new_lines.iter_mut().take(patch_rows) {
            let patched = match field_type {
                Some(ft) if ft != Type::Group => {
                    decode::patch_synthetic_field_name(line, &header_field_name)
                }
                None if renamed => {
                    decode::patch_raw_field_name(line, u64::from(field_number), &field_name)
                }
                _ => None,
            };
            if let Some(patched) = patched {
                *line = patched;
            }
        }

        // Spec 0174 §S4: a truncated preview ends with a literal `...`,
        // so the user sees there is more. Done here, on the rendered
        // lines, rather than in `prototext-core`: `...` is not part of
        // the prototext grammar. It carries no `NodeSpan`, so it is not
        // selectable, not navigable, and not part of any span range;
        // and per spec 0187 S2 the highlighter never sees it either,
        // because `render::window_text` blanks it before parsing.
        let mut new_spans = new_spans;
        if truncated {
            insert_truncation_marker(&mut new_lines, &mut new_spans, self.indent_size);
        }

        Ok((
            idx,
            old_span,
            RenderedAs {
                lines: new_lines,
                spans: new_spans,
            },
        ))
    }

    /// The raw-byte and text-line extent of the packed record `idx`
    /// draws (spec 0135 G1), re-parsing the record's real tag+length
    /// from `packed_record_start`.
    ///
    /// Spec 0115 gave each element of a run a node of its own, so this
    /// took the whole run — found by scanning the siblings for a
    /// matching `packed_record_start` — and summed it. Spec 0216 makes
    /// the run a single node drawing one row per element, so the run
    /// *is* `idx` and the scan is gone.
    pub(super) fn packed_record_extent(
        &self,
        idx: usize,
    ) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let start = self.tree[idx].span.packed_record_start;
        assert_ne!(
            start, NO_PACKED_RECORD,
            "packed_record_extent called with a non-packed node"
        );
        let start = start as usize;
        let tag = prototext_core::helpers::parse_wiretag(&self.blob, start);
        let len = prototext_core::helpers::parse_varint(&self.blob, tag.next_pos);
        let raw_end = len.next_pos + len.varint.unwrap_or(0) as usize;
        // Spec 0210 S1: derived from the counters, not read off
        // `span.text_range` (which is the build-time range and goes
        // stale on the first splice).
        (start..raw_end, self.node_lines(idx))
    }

    /// Origin for a brand-new override, targeting node `idx` — created
    /// as kind `Path` (spec 0208 S2). A `PathField` default would
    /// survive sibling reordering and insertion better, but robustness
    /// is the wrong thing for a *default* to optimize: the user points
    /// at one node, and a `PathField` entry is expressed in terms of
    /// that node's **parent** plus a field number, so it reads as being
    /// about somewhere else and silently covers every same-numbered
    /// sibling. `z`/`Z` in the management pane (spec 0124 G2) promote an
    /// entry to `path:field` for whoever wants that.
    ///
    /// Keeps returning `Result` although `OverrideKind::Path` cannot
    /// fail: the sole call site sits opposite `origin_for_kind(idx,
    /// kind)`, which genuinely can, and both arms must agree on a type.
    pub(super) fn override_origin_for_kind(&self, idx: usize) -> Result<OverrideOrigin, String> {
        self.origin_for_kind(idx, OverrideKind::Path)
    }

    /// Origin for an arbitrary `kind`, targeting node `idx` (spec 0117
    /// §2's derivation rules, generalized in spec 0124 G2 so the
    /// manage-pane `z` key can rederive an origin under a rotated kind).
    /// `PathField`/`FqdnField` error out when `idx` is the wrapper root
    /// (no parent) or, for `FqdnField`, when the parent's `type_fqdn` is
    /// unresolved.
    pub(super) fn origin_for_kind(
        &self,
        idx: usize,
        kind: OverrideKind,
    ) -> Result<OverrideOrigin, String> {
        match kind {
            OverrideKind::Path => Ok(OverrideOrigin::Path {
                path: self.positional_path(idx),
            }),
            OverrideKind::PathField => {
                let parent = self
                    .parent(idx)
                    .ok_or_else(|| "cursor is the wrapper root (no parent)".to_string())?;
                Ok(OverrideOrigin::PathField {
                    path: self.positional_path(parent),
                    field: u64::from(self.tree[idx].span.field_number),
                })
            }
            OverrideKind::FqdnField => {
                let parent = self
                    .parent(idx)
                    .ok_or_else(|| "cursor is the wrapper root (no parent)".to_string())?;
                let fqdn = self
                    .fqdns
                    .get(self.tree[parent].span.type_fqdn)
                    .ok_or_else(|| "parent's type is unresolved".to_string())?
                    .to_owned();
                Ok(OverrideOrigin::FqdnField {
                    fqdn,
                    field: u64::from(self.tree[idx].span.field_number),
                })
            }
        }
    }

    /// Third `OverrideKind` — the one that is neither `a` nor `b` (spec
    /// 0134 G2 step 5's `other_kind`; there are only 3 kinds total).
    pub(super) fn third_kind(a: OverrideKind, b: OverrideKind) -> OverrideKind {
        [
            OverrideKind::Path,
            OverrideKind::PathField,
            OverrideKind::FqdnField,
        ]
        .into_iter()
        .find(|k| *k != a && *k != b)
        .expect("3 kinds total, 2 excluded, 1 remains")
    }
}
