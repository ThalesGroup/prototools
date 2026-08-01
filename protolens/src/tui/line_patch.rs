// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0167: the batch's line-buffer patches, and the one pass that
//! applies them.
//!
//! A splice does not touch `self.lines` when it happens — it leaves a
//! `LinePatch` behind, and `materialize_line_patches` merges the whole
//! batch in a single pass at the end. That is one closed idea with two
//! types and two methods, and it is the only part of `override_apply.rs`
//! that is about the line buffer rather than about the tree.

use super::*;

/// Spec 0167 (N1 follow-up to spec 0160): where a single
/// `splice_override` call's freshly decoded content ultimately belongs
/// once the active `render_overrides` batch finishes — see
/// `render_overrides_inner`'s `patch_scope` doc comment for why a patch
/// can be nested inside another, not-yet-materialized one.
pub(super) enum LinePatchTarget {
    /// A range in `self.lines` as it stood before this batch began.
    Original(Range<usize>),
    /// `(parent_patch_index, local_range_within_that_patch's_own_lines)`.
    Nested(usize, Range<usize>),
}

/// Spec 0167: one collected line-buffer patch. `global_start` is the
/// patch's own start position as the batch has the document so far
/// (i.e. `node_lines(idx).start` at the moment of the splice).
/// `children_base_shift` is `App::pending_shift`'s value right after
/// this patch's own delta was folded in — i.e. at the exact moment this
/// patch's freshly decoded children were translated into the tree. Both
/// exist solely so a *further-nested* child patch of this one can
/// compute its own local offset in O(1): a node's position is derived
/// from the line counters, so it keeps growing as later-processed
/// nested siblings get spliced, while this patch's own `lines` is a
/// frozen snapshot never touched again after creation. A child patch's
/// local offset must undo exactly that growth (everything accumulated
/// since `children_base_shift`) to land back in this patch's own frozen
/// coordinate frame — see `splice_override`'s `Nested` branch.
pub(super) struct LinePatch {
    pub(super) target: LinePatchTarget,
    pub(super) global_start: usize,
    pub(super) children_base_shift: isize,
    pub(super) lines: Vec<String>,
}

/// `base` rebuilt with each of `pieces`' ranges replaced by its own
/// lines, both consumed.
///
/// Spec 0186 S1/G2: consuming rather than slicing. `extend_from_slice`
/// on a `Vec<String>` *clones* every element — one `malloc` plus one
/// `memcpy` per line, across the whole document, to apply a patch that
/// replaces a handful of them. Moving 24-byte `String` headers instead
/// leaves this pass touching the heap only for the lines the batch
/// actually replaces — it is the batch's only per-line heap work.
///
/// `pieces` must be in ascending, non-overlapping order; both callers
/// sort. `by_ref().take(range.start - cursor)` would *underflow* on a
/// piece sitting behind the cursor, where the slicing version would at
/// least have panicked, so the two asserts keep both failure modes
/// loud. `describe` names the offending piece for them, and is called
/// only when one fires.
fn merge_replacements(
    base: Vec<String>,
    pieces: Vec<(Range<usize>, Vec<String>)>,
    describe: impl Fn(usize) -> String,
) -> Vec<String> {
    let base_len = base.len();
    let mut out = Vec::with_capacity(base_len);
    let mut base = base.into_iter();
    let mut cursor = 0usize;
    for (k, (range, lines)) in pieces.into_iter().enumerate() {
        assert!(
            range.start >= cursor,
            "spec 0167 (flaw C2): overlapping line patches — the previous \
             one ends at line {cursor}, {} starts at line {}",
            describe(k),
            range.start
        );
        assert!(
            range.end <= base_len,
            "spec 0186 (S1): {} covers lines {}..{}, past the {base_len} \
             lines it is being merged into",
            describe(k),
            range.start,
            range.end
        );
        out.extend(base.by_ref().take(range.start - cursor));
        out.extend(lines);
        // Discard the lines this piece replaces.
        for _ in range.start..range.end {
            base.next();
        }
        cursor = range.end;
    }
    out.extend(base);
    out
}

impl App {
    /// Spec 0167: applies every patch collected during the current batch
    /// to `self.lines` in one pass, instead of one `Vec::splice` per
    /// patch (each an O(document length) memmove — spec 0160 N1).
    ///
    /// Patches form a tree, not a flat sequence: a `Nested` patch's
    /// content must be resolved into its parent's own content *before*
    /// the parent itself is resolved (recursively, all the way up to
    /// whichever `Original`-targeted patch owns the top of that chain) —
    /// see `render_overrides_inner`'s `patch_scope` doc comment for why
    /// nesting happens at all. Resolving bottom-up like this keeps each
    /// individual merge bounded to the size of the patch's own content
    /// (not the whole document); only the single final merge against
    /// `self.lines` (for the top-level `Original` patches) is
    /// O(document length), and it happens exactly once.
    pub(super) fn materialize_line_patches(&mut self) {
        if self.pending_line_patches.is_empty() {
            return;
        }
        let raw_patches = std::mem::take(&mut self.pending_line_patches);

        // Group each patch under its parent (`Nested`) or as a root
        // (`Original`). `render_overrides_inner`'s strict pre-order,
        // left-to-right walk (spec 0160 G2) is *expected* to hand these
        // over already in ascending order; the sort below does not rely
        // on that being true.
        let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut top_level: Vec<usize> = Vec::new();
        for (i, p) in raw_patches.iter().enumerate() {
            match &p.target {
                LinePatchTarget::Original(_) => top_level.push(i),
                LinePatchTarget::Nested(parent, _) => {
                    children_of.entry(*parent).or_default().push(i)
                }
            }
        }

        // Flaw C2: the forward merge below is only correct for ascending,
        // non-overlapping ranges. Resting that on callers queueing in
        // order means enforcing it with a `debug_assert!`, which a
        // `--release`-only project never compiles, against a walk that
        // can violate it at a distance (a `doc_next` cycle did).
        // Sorting here makes ordering the merge's own business instead
        // of a caller's obligation. It is O(k log k) in the batch's
        // patch count (one per splice), not in document length, and a
        // no-op whenever the walk behaved. Overlap stays an `assert!`
        // below: it is a real contradiction, not something a reordering
        // can repair.
        let range_start = |i: usize| match &raw_patches[i].target {
            LinePatchTarget::Original(r) | LinePatchTarget::Nested(_, r) => r.start,
        };
        top_level.sort_unstable_by_key(|&i| range_start(i));
        for children in children_of.values_mut() {
            children.sort_unstable_by_key(|&i| range_start(i));
        }

        let mut patches: Vec<Option<LinePatch>> = raw_patches.into_iter().map(Some).collect();
        // Spec 0210 S2: the text is the only array a splice moves. A
        // line's owner is derived from the tree's own counters, so no
        // line map has to ride along with `lines` through this merge.
        let old_lines = std::mem::take(&mut self.lines);

        // Read out while the patches are still whole: resolving one takes
        // it, and `global_start` is wanted afterwards, by the assert.
        let starts: Vec<usize> = top_level
            .iter()
            .map(|&i| patches[i].as_ref().unwrap().global_start)
            .collect();
        let pieces: Vec<(Range<usize>, Vec<String>)> = top_level
            .iter()
            .map(|&idx| {
                let range = match &patches[idx].as_ref().unwrap().target {
                    LinePatchTarget::Original(r) => r.clone(),
                    LinePatchTarget::Nested(..) => unreachable!("filtered to Original above"),
                };
                (
                    range,
                    Self::resolve_line_patch(&mut patches, &children_of, idx),
                )
            })
            .collect();
        self.lines = merge_replacements(old_lines, pieces, |k| {
            format!(
                "top-level patch {} (global_start {})",
                top_level[k], starts[k]
            )
        });
    }

    /// Spec 0167: recursively resolves patch `idx` — splicing in every
    /// one of its own direct `Nested` children (themselves first
    /// resolved the same way) — into a single flat `lines` vector.
    /// `patches[idx]` is taken (never visited twice; every patch is
    /// either a `top_level` entry or exactly one patch's `Nested` child,
    /// per `materialize_line_patches`'s grouping pass).
    fn resolve_line_patch(
        patches: &mut [Option<LinePatch>],
        children_of: &HashMap<usize, Vec<usize>>,
        idx: usize,
    ) -> Vec<String> {
        let LinePatch { lines, .. } = patches[idx]
            .take()
            .expect("spec 0167: each patch is resolved at most once");
        let Some(children) = children_of.get(&idx) else {
            return lines;
        };
        // The same merge as `materialize_line_patches`, bounded here by
        // the patch's own content rather than by the document.
        let pieces: Vec<(Range<usize>, Vec<String>)> = children
            .iter()
            .map(|&child_idx| {
                let local_range = match &patches[child_idx].as_ref().unwrap().target {
                    LinePatchTarget::Nested(_, r) => r.clone(),
                    LinePatchTarget::Original(_) => {
                        unreachable!("children_of only ever contains Nested-targeted patches")
                    }
                };
                (
                    local_range,
                    Self::resolve_line_patch(patches, children_of, child_idx),
                )
            })
            .collect();
        merge_replacements(lines, pieces, |k| {
            format!("nested patch {} under patch {idx}", children[k])
        })
    }
}
