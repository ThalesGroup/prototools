// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::override_resolve::ParentFieldOrExt;
use super::preview_truncate::{
    insert_truncation_marker, reframe_to_actual_length, trunc_shape_for, truncate_interior,
};
use super::*;

use prost_reflect::prost_types::field_descriptor_proto::Type;
use prototext_core::serialize::render_text::NodeSpan;
use std::borrow::Cow;

/// Spec 0185 S3: one node's complete rendering under a candidate type,
/// with no tree mutation whatsoever — everything `splice_override` used
/// to compute inline before its splice proper begins. The live preview
/// takes the same value and stops there, which is what makes the
/// preview and the commit byte-identical (G3).
pub(super) struct RenderedAs {
    pub(super) lines: Vec<String>,
    /// Consumed by `splice_override` to build the new subtree; kept by
    /// the preview purely as display data, to draw its wire rows (spec
    /// 0225 S9). They stay out of `self.tree` either way, so spec 0185
    /// N6 still holds — an overlay row has no identity.
    pub(super) spans: Vec<NodeSpan>,
    /// The bytes those spans index into: the node's own
    /// `tag[+length]+payload`, cut to the preview budget when there was
    /// one. A truncated preview's bytes exist nowhere else, and the
    /// spans' `raw_range`s are relative to this.
    ///
    /// `Some` exactly when `is_preview` (spec 0251 S6). The confirmed
    /// splice discards this field, so materializing it there is a full
    /// copy of the node — 25 MB for a root override — allocated and
    /// immediately dropped. The renderer reads the blob through a
    /// borrow instead.
    pub(super) bytes: Option<Vec<u8>>,
    /// Spec 0249 S1: indices into `spans` of the nodes the row budget
    /// stopped at — emitted with a header and a footer but no body.
    /// Empty unless a budget was asked for, which the preview never
    /// does (it bounds bytes, not rows).
    pub(super) undescended: Vec<u32>,
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
    /// Returns whether `idx` was actually re-spliced — `false` when it
    /// already matched `rendered_as`, or when `splice_override` refused.
    pub(super) fn resettle_node(&mut self, idx: usize, path: &str) -> bool {
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
            match self.splice_override(idx, effective, self.confirm_row_budget()) {
                Ok(()) => {
                    self.tree_mut()[idx].rendered_as = current;
                    // Spec 0221 S1: this node is settled after all, so
                    // an earlier refusal of it in this same pass was not
                    // final and must not be reported. The guard keeps
                    // the common case — nothing refused — free, and the
                    // scan is over the refusals alone, not the tree.
                    if !self.refusals.is_empty() {
                        self.refusals.retain(|(node, _)| *node != idx);
                    }
                    true
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
                    false
                }
            }
        } else {
            false
        }
    }

    /// Drop every fold flag standing on a strict descendant of `idx`
    /// (spec 0256 S4).
    ///
    /// There are two ways to ask this, and which is cheaper is a
    /// property of the call rather than of the question. Asking each
    /// vacated slot whether it is folded is two hash lookups per *slot
    /// of the document*: 78 ms of a 448 ms root override on
    /// `googleapis.desc`, to answer a question about a set that usually
    /// holds tens of entries. Asking the sets instead costs
    /// `HashSet::retain`, which is O(capacity) — and `auto_folded`'s
    /// capacity peaks around 84 000 mid-bake without shrinking, which
    /// the *bake's* 70 893 splices, a handful of descendants each, would
    /// pay 70 893 times over. Measured: retaining unconditionally takes
    /// the drain from 5.4 s to 17.5 s.
    ///
    /// So the smaller side is walked, and neither cost can exceed the
    /// other. Both spellings clear the same flags: `descendants` is the
    /// rendered subtree, and a fold flag can only stand on a node some
    /// rendering showed.
    ///
    /// Ancestry is resolved through `App::parent`, i.e. through the
    /// arena, which is immutable (spec 0216) — the rendered tree is
    /// being taken apart around this call and cannot be walked.
    fn scrub_folds_under(&mut self, idx: usize, descendants: &[usize]) {
        if self.folded.is_empty() && self.auto_folded.is_empty() {
            return;
        }
        if self.folded.capacity() + self.auto_folded.capacity() > descendants.len() {
            for &d in descendants {
                self.unfold(d);
            }
            return;
        }
        // Taken out and put back so the closure can borrow `self` for
        // the ancestry walk; `retain` would otherwise hold the set
        // mutably for the duration.
        let mut folded = std::mem::take(&mut self.folded);
        folded.retain(|&f| !self.descends_from(f, idx));
        self.folded = folded;
        let mut auto_folded = std::mem::take(&mut self.auto_folded);
        auto_folded.retain(|&f| !self.descends_from(f, idx));
        self.auto_folded = auto_folded;
    }

    /// Whether `node` is a strict descendant of `ancestor`. O(depth),
    /// which the arena caps at 13 on the corpus.
    fn descends_from(&self, node: usize, ancestor: usize) -> bool {
        let mut cur = self.parent(node);
        while let Some(n) = cur {
            if n == ancestor {
                return true;
            }
            cur = self.parent(n);
        }
        false
    }

    /// Hand `idx`'s text to the idle loop instead of freeing it here
    /// (spec 0256 S1).
    ///
    /// Freeing the previous interpretation's 2.86 M `Box<str>` is 56% of
    /// a root override on `googleapis.desc`, and four fifths of that
    /// does not even land at the free site: glibc pools small frees
    /// without coalescing and bills `malloc_consolidate` to the next
    /// allocation request, which is inside `overlay_spans`. See spec
    /// 0256's Background — that 0.195 s was read as `overlay_spans`'
    /// own cost for two specs running.
    ///
    /// Gated on `bounded_confirms`, which means "an event loop is
    /// running and will drain this". A headless `export` has none, so it
    /// frees inline; deferring there would grow the vector to hold the
    /// whole previous document with nothing to empty it (the same trap
    /// spec 0255 rule 2 records for the row budget, and deliberately the
    /// same switch).
    fn discard_text(&mut self, idx: usize) {
        match self.node_text_mut()[idx].take() {
            Some(text) if self.bounded_confirms => self.discarded_text.push(text),
            _ => {}
        }
    }

    /// Free at most `DISCARD_CHUNK` of the boxes `discard_text` set
    /// aside, and say whether anything is left (spec 0256 S2).
    ///
    /// Runs ahead of `bake_step` in the idle arm. The bake is what grows
    /// the *new* document, so draining afterwards would hold the old
    /// 180 MB alive alongside it; draining first leaves peak memory
    /// where it was before this spec, at the price of 0.25 s on a 5.5 s
    /// drain.
    ///
    /// Draws no frame and owes none. Nothing on screen is derived from
    /// this vector, so unlike the bake it has no deferred repaint to
    /// arrange and needs no `*_forces` term in `run_loop`.
    pub(super) fn discard_step(&mut self) -> bool {
        /// One step's worth of freeing.
        ///
        /// The 2 864 189 boxes of a root override on `googleapis.desc`
        /// cost 0.251 s to free including the `malloc_consolidate` pass
        /// they defer, i.e. ~88 ns each, so this is ~5.7 ms — comfortably
        /// inside the ~22 ms worst step the bake already spends at
        /// `BAKE_ROW_BUDGET`, which is what sets the event loop's
        /// tolerance.
        const DISCARD_CHUNK: usize = 65_536;

        let len = self.discarded_text.len();
        if len == 0 {
            return false;
        }
        self.discarded_text.truncate(len - len.min(DISCARD_CHUNK));
        true
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
        let mut d_marks = std::time::Duration::ZERO;
        if self.override_batch_depth == 0 {
            // Spec 0221 S1: emptied at the start of the *outermost*
            // pass, so nested `render_overrides` calls contribute to one
            // list and a caller reading it afterwards sees that pass and
            // nothing carried over from an earlier one.
            self.refusals.clear();
            let t_marks = std::time::Instant::now();
            self.compute_descend_marks();
            d_marks = t_marks.elapsed();
        }
        self.override_batch_depth += 1;
        let path = self.positional_path(idx);
        // `idx` itself always starts outside any not-yet-materialized
        // patch (spec 0167): it's an already-existing node, never one
        // freshly created within this very call.
        let t_inner = std::time::Instant::now();
        self.render_overrides_inner(idx, &path);
        let d_inner = t_inner.elapsed();
        self.override_batch_depth -= 1;
        if self.override_batch_depth == 0 {
            let t_finalize = std::time::Instant::now();
            self.finalize_override_batch();
            crate::tui::trace::trace!(
                "render_overrides marks_us={} inner_us={} finalize_us={} overrides={}",
                d_marks.as_micros(),
                d_inner.as_micros(),
                t_finalize.elapsed().as_micros(),
                self.overrides.entries().len(),
            );
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
            //
            // Spec 0258 S3: except when nobody asked. The refusals
            // themselves are still collected — `expand_auto_fold` clears
            // them with the rest of the batch — so a test can assert
            // what was refused without the status line being written.
            match self.refusals.len() {
                _ if self.silent_refusals => {}
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
    ///
    /// Spec 0258 S4: returns whether anything was marked, which is what
    /// tells `expand_auto_fold` whether a resolution pass over the
    /// revealed subtree has anything to find. Inside a batch the answer
    /// is ignored — the walk is already running.
    fn mark_fresh_subtree(&mut self, idx: usize, path: &str) -> bool {
        let mut fresh = Vec::new();
        self.collect_descendants(idx, &mut fresh);
        if fresh.is_empty() {
            return false;
        }
        let targets = self.collect_descend_targets(fresh.iter().copied(), Some(path));
        let marked = !targets.is_empty();
        self.mark_targets(targets);
        marked
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
                        targets.extend(self.children_with_field(parent, *field));
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
    /// Spec 0222 S5: there is no patch scope any more. A splice writes
    /// its own nodes' text into `node_text` as it goes, so a node
    /// re-spliced later in the same batch — a nested message getting its
    /// own override on top of its parent's just-decoded rendering —
    /// simply overwrites what the ancestor's splice wrote. Nothing is
    /// queued, so nothing needs a coordinate system to be queued
    /// against.
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
    fn render_overrides_inner(&mut self, idx: usize, path: &str) {
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
        let spliced = self.resettle_node(idx, path);
        // Spec 0183 S3: the nodes the splice just re-decoded were not
        // marked when the batch's marks were computed, so mark them now.
        if spliced {
            let _ = self.mark_fresh_subtree(idx, path);
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
                self.render_overrides_inner(c, &c_path);
            }
        }
    }

    /// Spec 0160 G1: runs exactly once, when the outermost
    /// `render_overrides` call for a batch of splices returns (or a
    /// standalone `splice_override` call finishes) — not once per
    /// splice. Every node's line *counts* are already correct by this
    /// point, and so is its text: `splice_override` writes both as it
    /// goes, fixing its own target's counts and carrying the change up
    /// the ancestors. So all that is left here is the state that is
    /// about the *batch* rather than about any one node.
    ///
    /// Spec 0222 S5: there is no text merge left to do. Spec 0210's
    /// residual whole-document pass over `lines` — 87 ms of a 102 ms
    /// keystroke at the document's first record — went away with the
    /// buffer it merged into.
    fn finalize_override_batch(&mut self) {
        // Spec 0188 G1: a batch that spliced nothing has nothing to
        // repair, and that is the common case rather than an exotic
        // one: opening or closing the override pane, toggling a
        // management-pane entry that resolves to what is already
        // rendered, and the second of two identical passes all land
        // here.
        //
        // The G3 equivalence check still runs, so every such batch in
        // the suite asserts that skipping was in fact a no-op.
        if !self.batch_spliced {
            #[cfg(test)]
            if self.verify_repair {
                self.assert_line_counts_are_exact();
                self.assert_status_is_exact();
            }
            return;
        }
        self.batch_spliced = false;
        // Line numbers moved, so the read-ahead walk must restart.
        self.structural_version += 1;
        // Spec 0259 S3: they moved under the reader too. Put the row that
        // was at the top of the pane back at the top of the pane, before
        // the clamp below — which then finds the caret inside the pane
        // and moves nothing, except in the one case where content really
        // did grow between the two.
        self.restore_scroll_anchor();
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
            self.assert_status_is_exact();
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
        let n_lines = self.total_lines();

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
                let want_visible = if self.is_folded(n) {
                    1
                } else {
                    sum_visible + 2
                };
                assert_eq!(
                    visible,
                    want_visible,
                    "node {n}'s lines_visible is {visible} but its children's \
                     come to {sum_visible} (folded: {})",
                    self.is_folded(n)
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
            //
            // Spec 0222 S2: the closing brace is *derived* from this
            // very line, so asserting it here would only restate the
            // derivation. What it used to catch — a footer that does not
            // match its header — is checked where the two are both real,
            // in `decode::overlay_spans`, against the renderer's output.
            let own = self.node_text[n].as_deref();
            assert!(
                own.is_some(),
                "node {n} is rendered over {total} lines but holds no text"
            );
            if bracketed {
                let open = own.expect("just asserted");
                let code = open.split("  #@").next().unwrap_or(open).trim_end();
                assert!(
                    code.ends_with('{'),
                    "node {n} starts at line {start} and spans {total} lines, \
                     so that is its opening line, but it reads {open:?}"
                );
            } else {
                let held = own.expect("just asserted").split('\n').count();
                assert_eq!(
                    held, total,
                    "flat node {n} draws {total} rows but holds {held} lines"
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

    /// The smallest row budget that can expand anything (spec 0249 S8).
    ///
    /// A budget of 1 is spent on the node's own header, so `descend`
    /// refuses the body and the node stops at itself again — S6's case,
    /// and no progress at all. Two buys the header plus the first row
    /// under it, which is enough for the walk to move down. It matters
    /// only when the pane's height is unknown or absurd; every real
    /// terminal is far above it.
    ///
    /// `pub(crate)` because spec 0257 S2 clamps `main`'s startup budget
    /// to it too, for the same reason and against the same renderer.
    pub(crate) const MIN_EXPAND_ROWS: usize = 2;

    /// Spec 0255 S1/S2: what a confirm renders — a screenful under an
    /// event loop that can bake the rest, and the whole subtree
    /// otherwise.
    ///
    /// `None` is not a fallback but the correct answer wherever no bake
    /// will run: a headless `export`, and every test that has not asked
    /// for a bounded confirm. A budget there would truncate the document
    /// with nothing to finish it.
    ///
    /// `App::new`'s own startup pass used to be in that list. Spec 0257
    /// S3 took it out: when the document render was itself bounded, that
    /// pass's splices — `Any` expansion, a seeded root — must be bounded
    /// too, or they undo the bound in the one place hardest to notice.
    /// Which is why the flag is now set from `Decoded` before the pass
    /// runs rather than one statement into the event loop.
    pub(super) fn confirm_row_budget(&self) -> Option<usize> {
        self.bounded_confirms
            .then(|| self.document_pane_height().max(Self::MIN_EXPAND_ROWS))
    }

    /// Spec 0249 S8: render the body of a node a bounded render stopped
    /// at, so opening it shows its content rather than an empty pair of
    /// braces.
    ///
    /// The target is the one the node is *already* rendered under —
    /// this is not a change of interpretation, it is the same one
    /// continued — so it comes from the node's own provenance rather
    /// than from a fresh override lookup. The fallback mirrors
    /// `resettle_node`'s: no active entry means the natural type.
    ///
    /// Bounded again. The subtree may be the whole document, which is
    /// the situation this spec exists for, so an unbounded render here
    /// would reintroduce the freeze one keystroke later. The stops this
    /// render leaves behind are expanded the same way when they in turn
    /// come into view, or by the bake.
    ///
    /// `row_budget` is the caller's because the two callers want
    /// different numbers (spec 0255 S1): a keystroke wants the screenful
    /// it is about to draw, the bake wants a slice large enough to
    /// amortize the folded frontier it re-emits.
    pub(super) fn expand_auto_fold(&mut self, idx: usize, row_budget: usize) {
        debug_assert!(
            self.auto_folded.contains(&idx),
            "only a node whose body was never rendered needs expanding"
        );
        let explicit = match self.provenance.get(self.tree[idx].rendered_as) {
            Some((Some(t), _)) => Some(t.clone()),
            _ => None,
        };
        let effective = match explicit {
            Some(t) => t,
            None => self.natural_type(idx),
        };
        let budget = row_budget.max(Self::MIN_EXPAND_ROWS);
        if let Err(e) = self.splice_override(idx, effective, Some(budget)) {
            // Left in `auto_folded` by the failed splice, so the node
            // stays drawn collapsed and the row is not claiming to show
            // a body it does not have.
            self.message = format!("cannot expand: {e}");
            return;
        }
        // Spec 0258 S1: the splice just wrote slots that were scanned
        // long ago and found to be nothing, and `descend` is a watermark
        // (spec 0188 S4) over a fixed-size arena (spec 0216) — so no
        // later batch will ever revisit them. Without this the subtree
        // keeps whatever a schema-blind render produced: an `Any` stays
        // an unexpanded `type_url`/`value` pair for the life of the
        // session.
        //
        // The mark is the gate as well as the repair. Most revealed
        // subtrees hold no `Any`, no MessageSet and no override origin,
        // and those stop here having paid one `collect_descendants` —
        // which matters at the bake's tens of thousands of splices.
        let path = self.positional_path(idx);
        if !self.mark_fresh_subtree(idx, &path) {
            return;
        }
        // `resettle_node` on `idx` itself is a no-op: the splice above
        // just wrote `idx`'s provenance to the target it rendered under.
        // The pass exists for what is below `idx`, and its own nested
        // splices stay bounded through `confirm_row_budget` (S2), so an
        // auto-expanded `Any` under a revealed node registers its own
        // stops and is baked in turn.
        self.silent_refusals = true;
        self.render_overrides(idx);
        self.silent_refusals = false;
    }

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
    /// `row_budget` bounds the render to that many emitted rows (spec
    /// 0249 S1), folding every node it stopped at. `None` renders the
    /// whole subtree, which is what a bake does and what every splice
    /// did before this spec.
    ///
    /// There is deliberately no preview flag here. Spec 0185 S3 made the
    /// live preview an overlay that calls `render_node_as` and stops, so
    /// no preview has reached this function since; the parameter that
    /// used to say so was `false` at every call site in the crate.
    pub(super) fn splice_override(
        &mut self,
        idx: usize,
        target: Option<String>,
        row_budget: Option<usize>,
    ) -> Result<(), String> {
        // Spec 0185 S3: the "what does this node look like as `target`"
        // half lives in `render_node_as`, shared verbatim with the live
        // preview — including the packed-record normalization, which is
        // what resolves `idx` to the run and widens `old_span` to the
        // run's whole extent (spec 0135 G1).
        let (idx, _old_span, rendered) =
            self.render_node_as(idx, target.as_deref(), false, row_budget)?;
        let RenderedAs {
            lines: new_lines,
            spans: new_spans,
            undescended,
            ..
        } = rendered;
        self.batch_spliced = true;

        // A fold flag on a slot this rendering does not show would be
        // honored again the moment some later override brings the slot
        // back, hiding unrelated content. `idx` itself is deliberately
        // left in both sets untouched (spec 0118 §7 — fold state on
        // `idx` survives its own retype); its `auto_folded` entry is
        // dealt with separately, below.
        // Everything the *previous* interpretation showed under `idx`,
        // collected before any of it is vacated.
        let mut old_descendants = Vec::new();
        self.collect_descendants(idx, &mut old_descendants);
        self.scrub_folds_under(idx, &old_descendants);
        for &d in &old_descendants {
            self.tree_mut()[d] = decode::TreeNode::vacant();
            self.discard_text(d);
            // The cue answered a question about a node this rendering no
            // longer has; if the slot comes back it must be asked again.
            self.heat_states[d] = heat_cue::HeatState::default();
            // Spec 0247 S8: same reasoning, and it also keeps the
            // arrays equal to what a full rebuild would give — which is
            // what `assert_status_is_exact` compares against.
            self.clear_status(d);
        }

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
        // put a span in the wrong place. It could produce a span the
        // maximal walk never saw, which `slots_for_spans` maps to
        // `NO_NODE` — and since spec 0249 that is a panic, not a silent
        // drop. No caller splices a byte-budgeted preview.
        //
        // `idx`'s own slot has to be vacated first, alongside the
        // descendants above: `overlay_spans` treats a second span landing
        // on an already-rendered slot as one more row of a packed run and
        // *adds* its lines, so leaving the previous interpretation in
        // place would count `idx`'s lines twice over.
        //
        // Spec 0222 S5: the text goes in with the structure, in the same
        // pass, each node taking its own lines out of `new_lines`. That
        // is the whole of the text update — there is no document-sized
        // buffer left to patch, and so no batch-wide coordinate system
        // to place a patch in. `new_spans`' `text_range` is 0-based
        // against `new_lines`, which is exactly what `overlay_spans`
        // expects.
        self.tree_mut()[idx] = decode::TreeNode::vacant();
        self.discard_text(idx);
        // Spec 0249 S3: `auto_folded` means "this node's body has not
        // been rendered", and the render just above rendered it. A
        // bounded one puts `idx` back in the set; an unbounded one is
        // the reason the entry has to go, and `idx`'s *user* fold is
        // untouched either way.
        self.auto_folded.remove(&idx);
        // Spec 0274 S8: the structure and the text are wanted mutably at
        // the same time, and each accessor borrows the whole `App`.
        // Moved out and put back rather than reached for twice — the
        // empty `Arc` each `take` leaves behind is one word-sized
        // allocation, against a call that renders a subtree.
        self.halt_search_scan();
        let mut tree = std::mem::take(&mut self.tree);
        let mut text = std::mem::take(&mut self.node_text);
        let stopped = decode::overlay_spans(
            Arc::get_mut(&mut tree).expect("the halt above leaves the tree unshared"),
            Arc::get_mut(&mut text).expect("the halt above leaves the text unshared"),
            new_spans,
            &new_lines,
            &self.arena,
            idx,
            &undescended,
        );
        self.tree = tree;
        self.node_text = text;
        // Spec 0249 S1/S3: the budget stopped here, so these nodes have
        // a header and a footer and nothing between. Folding them is
        // what makes each one a single row instead of an empty pair of
        // braces claiming the node is empty — and it is what S8 later
        // reads to know which rows still owe a render.
        //
        // Recorded for every node first, then rolled up, so a parent
        // shared by two of them is recomputed once its children are all
        // in the set rather than once per child.
        for &slot in &stopped {
            debug_assert!(
                self.tree[slot].is_bracketed(),
                "only a message recursion can be undescended (spec 0249 S1)"
            );
            self.auto_folded.insert(slot);
            // Spec 0255 S3: the bake's walk order, appended in the
            // render's own document order so a drain works downward
            // from the viewport rather than diving.
            self.bake_queue.push_back(slot);
        }
        for &slot in &stopped {
            self.refresh_line_counts(slot);
        }
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
        if self.is_folded(idx) {
            self.tree_mut()[idx].lines_visible = 1;
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

        // Spec 0247 S8, and here for the same reason the line counts
        // are: O(k) over the subtree just written, then O(depth · width)
        // up the ancestors with an early stop. Deferring it to
        // `finalize_override_batch` would leave a second splice in the
        // same batch rolling up the first splice's stale ancestors.
        self.refresh_status_subtree(idx);
        self.refresh_status_ancestors(idx);

        // Spec 0142 G6.1: `idx` keeps its own slot across a retype (see
        // this function's own doc comment), but not its shape, so a
        // cursor resting anywhere but the header has to be re-placed.
        // Spec 0216 S7 makes that a coordinate rather than a flag, and
        // the coordinate is not stable: a message's closing brace moves
        // whenever the body it encloses changes size, and the retype may
        // have turned the node flat, in which case the brace is gone.
        //
        // Spec 0259: ahead of the finalizer below, not after it. The
        // ancestor counts written above are the whole structural
        // consequence of this splice, so the tree is already final here —
        // and `finalize_override_batch` reads the caret's row, through
        // `clamp_pan_offset`, to decide whether to scroll it into view.
        // Repairing afterwards left that decision to be made from a
        // coordinate pointing into the body of the node the caret's brace
        // closes, which scrolled the viewport by however far the two
        // differ.
        if self.cursor_line_in_node != 0 {
            let node = &self.tree[self.cursor];
            self.cursor_line_in_node = if node.is_bracketed() {
                node.lines_total - 1
            } else {
                self.cursor_line_in_node.min(node.lines_total - 1)
            };
        }

        // Spec 0160 G2: no eager walk of the document happens here. When
        // called from within a `render_overrides` batch
        // (`override_batch_depth > 0`), reconciliation is deferred to that
        // outer call's own `finalize_override_batch` — which is every
        // production call since spec 0185 made the preview an overlay. A
        // standalone splice (`override_batch_depth == 0`, tests only) must
        // finalize immediately itself.
        if self.override_batch_depth == 0 {
            self.finalize_override_batch();
        }

        Ok(())
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
    ///
    /// `row_budget` caps the *rows emitted* instead (spec 0249 S1), and
    /// is the confirmed path's bound rather than the preview's. The two
    /// are never combined: a byte budget cuts wherever the byte count
    /// runs out and needs the `...` marker to say so, while a row budget
    /// cuts on a node boundary and says so by folding the node.
    pub(super) fn render_node_as(
        &mut self,
        idx: usize,
        target: Option<&str>,
        is_preview: bool,
        row_budget: Option<usize>,
    ) -> Result<(usize, NodeSpan, RenderedAs), String> {
        assert!(
            !(is_preview && row_budget.is_some()),
            "spec 0249 S1: a preview bounds bytes, a confirm bounds rows"
        );
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

        // Spec 0253 S2/S4: the node keeps the cardinality its own field
        // is declared with. Read here, before the splice replaces
        // `idx`'s span; `warm_visible_override_wrappers` asks the same
        // function of the same node, which is what keeps warming and the
        // splice hashing to the same wrapper name.
        let cardinality = self.field_cardinality(idx);

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
            Some(decode::NONE_KEYWORD) => (None, None),
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
        //
        // Spec 0251 S6: borrowed, not copied. The `Arc` clone is a
        // refcount bump and exists only to detach the slice's lifetime
        // from `&self`, which `decode_and_render_indexed` needs because
        // it takes `&mut self.fqdns`. Only a budget-truncated preview
        // owns its bytes, because those exist nowhere else.
        let blob = Arc::clone(&self.blob);
        let mut field_bytes: Cow<'_, [u8]> = Cow::Borrowed(&blob[widen(&old_span.raw_range)]);

        // Spec 0174: only a live preview is speculative and needs
        // bounding — a confirmed override must render completely (G5).
        // Bounding the renderer's *input* bounds its decode, its render,
        // its span count and its line count together, which is why
        // `prototext-core` itself carries no budget.
        //
        // Spec 0302: a TRUNCATED_BYTES node's declared length varint claims
        // more bytes than are present. Both paths fix this:
        // - preview: `truncate_interior` cuts to the budget and rewrites the
        //   varint as a side-effect (the budget is at most the actual bytes).
        // - commit: `reframe_to_actual_length` rewrites the varint to match
        //   the actual payload, without dropping a byte. The arena already
        //   has slots for the children — spec 0302's ArenaSink change walks
        //   the available bytes and allocates them on startup.
        let mut truncated = false;
        if is_preview {
            let shape = trunc_shape_for(field_type, u32::from(old_span.wire_type), packed);
            if let Some(cut) =
                truncate_interior(&field_bytes, self.override_preview_byte_budget, shape)
            {
                field_bytes = Cow::Owned(cut);
                truncated = true;
            }
        } else if field_type.is_some() {
            if let Some(reframed) = reframe_to_actual_length(&field_bytes) {
                field_bytes = Cow::Owned(reframed);
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
        // Spec 0251 S5: the cache serves the preview path alone. A
        // confirmed render is the one that can be enormous — 250 MB for
        // the googleapis root — and caching it cost a full clone to hand
        // back plus a full clone to store, 0.56 s of a 4.12 s override,
        // for an entry that no lookup could ever reach twice. Nothing is
        // lost by not storing it: `is_preview` is part of the key, so a
        // confirmed lookup can only ever hit another confirmed render,
        // i.e. the same node overridden the same way twice — a
        // revert-and-re-apply, which is user-paced and re-renders.
        //
        // What the cache is *for* is arrowing through candidates (spec
        // 0116 §8), where each render is bounded to
        // `override_preview_byte_budget` interior bytes and the same
        // candidate is revisited constantly.
        //
        // A row budget therefore never meets the cache either: it is a
        // confirmed render's bound, and `is_preview` is false there.
        let cached = if is_preview {
            self.render_cache.get(&cache_key)
        } else {
            None
        };
        let (mut new_lines, new_spans, undescended) = match cached {
            Some((lines, spans)) => (lines, spans, Vec::new()),
            None => {
                let wrapper_desc = match field_type {
                    Some(ft) => Some(
                        decode::register_wrapper(
                            self.ctx.pool_mut(),
                            field_number,
                            ft,
                            target_desc,
                            packed,
                            cardinality,
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
                    // Spec 0249 S1.
                    row_budget,
                    ..Default::default()
                };
                // Spec 0248: an extension on a spliced subtree resolves the
                // same way it does in the document render. `self.ctx` is
                // borrowed for the render; `self.fqdns` and
                // `self.render_cache` are disjoint fields and stay reachable.
                let _ext_scope = self.ctx.install_ext_loader();
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
                if is_preview {
                    self.render_cache.insert(cache_key, value.clone());
                }
                (value.0, value.1, rendered.undescended)
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
                bytes: is_preview.then(|| field_bytes.into_owned()),
                undescended,
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
