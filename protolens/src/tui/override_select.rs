// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

use super::tiered::Tier;

impl App {
    /// Whether `idx` is eligible as an override target (`t`, `type-as`,
    /// `type-as-raw`): a message/group node (`NodeSpan::is_message` —
    /// *not* `type_fqdn != NO_FQDN`, which cannot tell a scalar from a
    /// schema-unresolved message/group), or any scalar with a decodable
    /// tag: `WT_LEN`, `WT_VARINT`, `WT_I32`, `WT_I64` (spec 0135 §G3).
    /// A packed-repeated element is judged by the whole record's
    /// reconstructed wire type, always `WT_LEN` (spec 0135 §G1), not by
    /// the element's own.
    ///
    /// The test is deliberately permissive: an override aimed at
    /// genuinely incompatible bytes just fails to parse and
    /// `splice_override` reports it, so the user is trusted to judge
    /// whether a reinterpretation is meaningful.
    pub(super) fn can_override(&self, idx: usize) -> bool {
        use prototext_core::helpers::{WT_I32, WT_I64, WT_LEN, WT_VARINT};
        let span = &self.tree[idx].span;
        if span.packed_record_start != NO_PACKED_RECORD {
            return true;
        }
        span.is_message
            || matches!(
                u32::from(span.wire_type),
                WT_LEN | WT_VARINT | WT_I32 | WT_I64
            )
    }

    /// `t`: toggle the override pane for the node under the cursor (spec
    /// 0114 §1/§2). Closes it (cancelling) if already open, regardless of
    /// which pane currently has focus. Otherwise opens it — moving focus
    /// there — if the cursor sits on an eligible node (`can_override`)
    /// and the terminal is wide enough; an ineligible target or an
    /// over-narrow terminal instead leaves a status-line message.
    pub(super) fn toggle_override(&mut self) {
        if self.override_target.is_some() {
            self.close_override();
            return;
        }
        if !self.can_override(self.cursor) {
            self.message =
                "cannot override: not a message/group or length-delimited field".to_string();
            return;
        }
        if self.term_width < MIN_OVERRIDE_WIDTH {
            self.message = format!(
                "terminal too narrow for override pane (need >= {MIN_OVERRIDE_WIDTH} columns)"
            );
            return;
        }
        // Mutually exclusive with the management pane (spec 0117 §3):
        // they share one right-hand UI slot.
        if self.manage_open {
            self.close_manage_pane();
        }
        self.override_target = Some(self.cursor);
        self.override_focus = true;
        self.override_scroll = PaneScroll::default();
        self.last_override_highlight = None;
        self.override_pan_offset = 0;

        // Spec 0139: smart initial sort-mode/highlight, in order —
        // A: an active override on the cursor node; else
        // B: the first inactive-but-applicable entry from the
        //    management list (necessarily inactive, since A found no
        //    active match); else
        // C: the field's own currently effective type.
        //
        // C reads a message/group's `span.type_fqdn` directly rather
        // than `natural_type`, which resolves the *parent's*
        // schema-declared field type. The two legitimately differ when
        // a field was decoded by inference rather than declaration:
        // `natural_type` then returns `None` while the main pane is
        // already showing a good resolved type. `span.type_fqdn` is
        // what the status line displays, so it is always "the type the
        // user can already see". Enums and primitives have no
        // `span.type_fqdn` of their own and so still use
        // `natural_type`.
        //
        // Without C, a scalar fell through to `open_override_on_default`'s
        // `Inferred` scoring, which is meaningless for one — it scores
        // the bytes as a prospective *message* — and landed on the
        // unrelated `None` sentinel.
        let candidate_type = self
            .applicable_override_entry_index(self.cursor)
            .map(|i| self.overrides.entries()[i].r#type.clone())
            .or_else(|| {
                let span = &self.tree[self.cursor].span;
                if span.is_message {
                    self.fqdns.get(span.type_fqdn).map(|f| Some(f.to_owned()))
                } else {
                    self.natural_type(self.cursor).map(Some)
                }
            });

        match candidate_type {
            Some(fqdn_or_raw) => self.open_override_on_type(fqdn_or_raw),
            None => self.open_override_on_default(),
        }

        // Spec 0132 §G2: live-preview the initial highlighted row from
        // the very first frame the pane is shown, not just after the
        // first navigation keystroke.
        self.preview_override_highlight();
    }

    /// Spec 0139's mode-selection rule, shared by Steps A and B: open
    /// in `Inferred` mode with the highlight on `fqdn_or_raw`'s row if
    /// that type is present in the node's *complete* inferred candidate
    /// list (`upgrade_active_override_to_complete` avoids a false
    /// "not found" from a stale capped preview);
    /// otherwise open in `Lexicographic` mode, whose candidate set is
    /// the fixed universe of every selectable type and so is guaranteed
    /// to contain it.
    fn open_override_on_type(&mut self, fqdn_or_raw: Option<String>) {
        // Spec 0137 §G4: raw (`Option::None`) maps to the `None`
        // sentinel string.
        let key = fqdn_or_raw.unwrap_or_else(|| "protolens_internal.None".to_string());
        self.override_sort = SortMode::Inferred;
        self.recompute_override_candidates();
        self.upgrade_active_override_to_complete();
        if self.seek_override_highlight(&key) {
            return;
        }
        // A cold cache means the two calls above only queued background
        // requests, so the complete list is not known yet and "not
        // found" would be a false negative. Stay in `Inferred` mode and
        // remember `key` for `poll_pending_override_work` to retry as
        // the list arrives, rather than discarding it for
        // `Lexicographic`.
        if self.override_candidates_pending || self.override_complete_pending {
            self.override_seek_target = Some(key);
            return;
        }
        // `recompute_override_candidates` may have set the "no scoring
        // graph available" message. This fallback already does what
        // that message would suggest, so it must not leak through —
        // `open_override_on_default` suppresses it for the same reason.
        self.message.clear();
        self.override_sort = SortMode::Lexicographic;
        self.recompute_override_candidates();
        self.seek_override_highlight(&key);
    }

    /// Looks for `key` among the candidates currently on screen
    /// (`override_candidates` — whichever sort mode is active) and, if
    /// found, moves the highlight there and clears `override_seek_
    /// target`. Shared by `open_override_on_type`'s own immediate
    /// attempt and `poll_pending_override_work`'s follow-up retries.
    fn seek_override_highlight(&mut self, key: &str) -> bool {
        let Some(row) = self.override_candidates.iter().position(|(f, _)| f == key) else {
            return false;
        };
        self.override_highlight = row;
        self.override_seek_target = None;
        true
    }

    /// Spec 0139 Steps C/D: neither an active nor an applicable-inactive
    /// override exists for the cursor node — default to `Inferred` mode
    /// (highlight on the top-scored row) when that list is non-empty;
    /// otherwise fall back to `Lexicographic` mode (highlight on the
    /// `None` sentinel row), silently — the "no scoring graph available"
    /// message `recompute_override_candidates` sets in the no-graph case
    /// would be redundant here, since this fallback already performs
    /// exactly what that message suggests.
    fn open_override_on_default(&mut self) {
        self.override_sort = SortMode::Inferred;
        self.recompute_override_candidates();
        // An empty list while `override_candidates_pending` is set means
        // the cache was merely cold at open time, not that `Inferred`
        // has nothing to offer; falling back would discard the in-flight
        // fetch. Wait for it. With no scoring graph at all no request is
        // ever queued, `pending` stays false, and the fallback runs
        // immediately.
        if self.override_candidates.is_empty() && !self.override_candidates_pending {
            // Spec 0147 G5's top-of-`handle_key` clear does not cover
            // this: it dismisses messages left over from a *previous*
            // keypress, whereas `recompute_override_candidates` set this
            // one during this keypress.
            self.message.clear();
            self.override_sort = SortMode::Lexicographic;
            self.recompute_override_candidates();
            return;
        }
        // Start fetching the rest of the list now rather than waiting
        // for the user to scroll to the loaded boundary;
        // `poll_pending_override_work` carries it to
        // `override_candidates_complete` from here.
        self.upgrade_active_override_to_complete();
    }

    /// `Enter` on a main-pane node (item 3, spec 0139 follow-up): a
    /// smart proxy for `t`/`o` — opens the management
    /// pane (`o`) if an override already applies to the cursor node,
    /// active or not (the same Step A/B check spec 0139's `t` itself
    /// uses to pick its initial highlight); otherwise opens the
    /// selection pane (`t`), which handles eligibility/width refusals
    /// on its own exactly as a direct keypress would.
    pub(super) fn open_smart_override_or_manage(&mut self) {
        let has_override = self.applicable_override_entry_index(self.cursor).is_some();
        if has_override {
            self.toggle_manage_pane();
        } else {
            self.toggle_override();
        }
    }

    /// Close the override pane (cancelling — spec 0114 §2), regardless of
    /// which pane currently has focus. Demotes `override_inferred_raw`
    /// into the shared `by_range` cache, capped to however many rows the
    /// pane was actually showing (spec 0114 §6).
    ///
    /// Closing re-renders **nothing**: the preview is an overlay and
    /// mutates no tree state, so dropping it is the whole revert (spec
    /// 0185 G2). Only a change to committed state — confirming a type,
    /// or (de)activating an entry — triggers a re-render.
    pub(super) fn close_override(&mut self) {
        self.preview_overlay = None;
        if let Some(range) = self.active_override_range.take() {
            let n = self.override_list_height.max(1);
            let stats = heat_cue::derive_stats(&self.override_inferred_raw);
            let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
            // Never shrinks an already-wider `top_n` (mirrors the
            // worker's own widening-not-shrinking rule, spec 0152 G5).
            let top_n_len = caches
                .by_range
                .peek(&range.start, Tier::Visible)
                .map_or(0, |e| e.top_n.len())
                .max(n);
            caches.by_range.upsert(
                range.start,
                heat_worker::RangeHeatEntry::new(
                    stats,
                    self.override_inferred_raw
                        .iter()
                        .take(top_n_len.max(1))
                        .cloned()
                        .collect(),
                ),
                Tier::Visible,
            );
            if self.override_candidates_complete {
                caches
                    .complete
                    .insert(range, self.override_inferred_raw.clone());
            }
        }
        self.override_inferred_raw.clear();
        self.override_candidates_complete = false;
        // Spec 0152 G7: the in-flight worker request (if any) isn't
        // cancelled — it finishes and writes into the shared cache
        // regardless (N7) — the pane just stops waiting for it.
        self.override_candidates_pending = false;
        self.override_complete_pending = false;
        self.override_seek_target = None;
        self.override_target = None;
        self.override_focus = false;
        // Spec 0200 S3: unconditional, unlike the flag below — it is
        // read inside an `if`, whereas a leaked origin kind would apply
        // to the *next* pane opening, where it would be wrong.
        self.override_origin_kind = None;
        // Spec 0200 S2: a pane opened from the management pane always
        // returns there on close, and this is the only place that
        // decides so. The `Enter`-confirm call site must therefore read
        // `override_opened_from_manage` *before* calling here, since
        // this clears it.
        if self.override_opened_from_manage {
            self.override_opened_from_manage = false;
            self.manage_open = true;
            self.manage_focus = true;
        }
    }

    /// `Enter`/double-click on an entry in the override management pane:
    /// opens the selection pane on that entry's own origin, highlighted
    /// on its current type, to let the user pick an alternate. Every
    /// exit — `Enter`, `Esc` and `t` — returns to the management pane
    /// via `override_opened_from_manage`; only `Enter` mutates the entry.
    ///
    /// The entry's origin kind is recorded too (spec 0200 S3), so
    /// confirming retypes *this* entry rather than creating a
    /// `path:field` one beside it.
    pub(super) fn open_override_from_manage(&mut self) {
        let Some(entry) = self.overrides.entries().get(self.manage_highlight) else {
            return;
        };
        let origin = entry.origin.clone();
        let current_type = entry.r#type.clone();
        let affected = self.manage_affected_nodes(&origin);
        let target = affected
            .iter()
            .find(|&&i| i == self.cursor)
            .or_else(|| affected.first());
        let Some(&target) = target else {
            return;
        };
        self.manage_open = false;
        self.override_target = Some(target);
        self.override_focus = true;
        self.override_scroll = PaneScroll::default();
        self.last_override_highlight = None;
        self.override_pan_offset = 0;
        self.override_opened_from_manage = true;
        self.override_origin_kind = Some(origin.kind());
        self.open_override_on_type(current_type);
        self.preview_override_highlight();
    }

    /// Recompute `override_candidates` for the current `override_target`
    /// under the active `override_sort` (spec 0114 §3.2), resetting the
    /// highlight to row 0 — in alphabetic mode always the `None`
    /// sentinel (spec 0137 §G1). Called when the pane opens and whenever
    /// `i` toggles the sort mode.
    ///
    /// `SortMode::Inferred` consults `active_override_range` before
    /// calling `heat_lookup`: toggling back to `Inferred` within one
    /// open-pane session reuses `override_inferred_raw` as-is. A
    /// genuinely new range asks the shared cache — a hit applies at
    /// once, a miss sets `override_candidates_pending` and leaves the
    /// "Scoring candidates…" placeholder up until a worker wakeup
    /// resolves it (`poll_pending_override_work`).
    pub(super) fn recompute_override_candidates(&mut self) {
        let Some(idx) = self.override_target else {
            return;
        };
        self.override_candidates = match self.override_sort {
            // Spec 0137 §G1/§G4: the `None` sentinel + the 15 primitive
            // keywords are prepended, in that fixed order, ahead of the
            // sorted message/group/enum FQDNs — alphabetic mode only
            // (§G7).
            SortMode::Lexicographic => std::iter::once("protolens_internal.None".to_string())
                .chain(decode::ALL_PRIMITIVE_KEYWORDS.iter().map(|s| s.to_string()))
                .chain(self.all_type_fqdns.iter().cloned())
                .map(|f| (f, None))
                .collect(),
            SortMode::Inferred => match &self.ctx.graph {
                Some(_graph) => {
                    let range = self.heat_scored_range(idx);
                    if self.active_override_range.as_ref() != Some(&range) {
                        // `Tier::User`: this directly follows the user
                        // pressing `t`/`i`, so it must jump the queue
                        // ahead of unrelated background polling.
                        match self.heat_lookup(
                            &range,
                            None,
                            0,
                            self.override_list_height,
                            Tier::User,
                        ) {
                            Some(top_n) => {
                                self.override_inferred_raw = top_n;
                                self.override_candidates_complete = false;
                                self.override_candidates_pending = false;
                            }
                            None => {
                                // `heat_lookup` already pushed the
                                // request; leave `override_inferred_raw`
                                // as it stands (typically empty) and let
                                // the pane show its placeholder.
                                self.override_candidates_pending = true;
                                self.message = "Scoring candidates…".to_string();
                            }
                        }
                        self.active_override_range = Some(range);
                    }
                    self.override_inferred_raw
                        .iter()
                        .map(|(f, s)| (f.clone(), Some(*s)))
                        .collect()
                }
                None => {
                    self.message = "no scoring graph available for inferred order; press 'i' \
                                     for alphanumeric"
                        .to_string();
                    Vec::new()
                }
            },
        };
        self.override_highlight = 0;
        self.override_scroll = PaneScroll::default();
        self.last_override_highlight = None;
        self.override_pan_offset = 0;
    }

    /// Fetches the complete, unbounded candidate list for the current
    /// override target in one shot, so the pane's real candidate count
    /// is known without the user paging to the end.
    ///
    /// Requests `[0, usize::MAX)`: `HeatCaches::window`'s `complete`-slot
    /// fallback clamps `end` to the actual candidate count (spec 0152
    /// G5) rather than requiring `top_n` to hold `usize::MAX` entries,
    /// so this resolves to the true full list as soon as the worker's
    /// one sweep for this range lands. A hit replaces
    /// `override_inferred_raw` wholesale and marks the pane complete; a
    /// miss sets `override_complete_pending` for
    /// `poll_pending_override_work` to retry, leaving the pane on the
    /// bounded first page until then.
    ///
    /// No-op if already complete or if `override_target`/`ctx.graph` is
    /// absent. Called when the pane opens and, defensively, when the
    /// user scrolls past the loaded window (spec 0114 §6).
    pub(super) fn upgrade_active_override_to_complete(&mut self) {
        if self.override_candidates_complete {
            return;
        }
        let (Some(idx), Some(_graph)) = (self.override_target, &self.ctx.graph) else {
            return;
        };
        let range = self.heat_scored_range(idx);
        // `Tier::User`: directly follows the user opening the pane or
        // scrolling past the loaded window, so it jumps the queue.
        match self.heat_lookup(&range, None, 0, usize::MAX, Tier::User) {
            Some(candidates) => {
                self.override_inferred_raw = candidates;
                self.override_candidates_complete = true;
                self.override_complete_pending = false;
            }
            None => {
                self.override_complete_pending = true;
            }
        }
        self.active_override_range = Some(range);
        // Only sync the on-screen list while `Inferred` is still the
        // active sort mode. `poll_pending_override_work` calls this
        // unguarded on a background wakeup, and the pane may since have
        // fallen back to `Lexicographic`; the resolved `Inferred` data
        // must then stay parked in `override_inferred_raw` for a later
        // `i` toggle rather than clobbering what is on screen.
        if self.override_sort == SortMode::Inferred {
            self.override_candidates = self
                .override_inferred_raw
                .iter()
                .map(|(f, s)| (f.clone(), Some(*s)))
                .collect();
        }
    }

    /// Re-checks the shared cache for the override pane's outstanding
    /// requests (spec 0152 G7) — called whenever the main thread wakes
    /// for a worker-progress event and either pending flag is set. A hit
    /// applies the result and clears the flag; a miss leaves both alone,
    /// the re-pushed request being merged by range and so harmless. Also
    /// retries `override_seek_target` once more data has arrived.
    pub(super) fn poll_pending_override_work(&mut self) {
        let Some(idx) = self.override_target else {
            return;
        };
        // The live preview is driven off the highlighted row, and every
        // arrival below can change which type that row names — most
        // visibly on a cold-cache open, where `toggle_override`'s
        // preview ran against a still-empty list and so showed raw.
        // Snapshot the highlighted type now and re-preview at the end
        // only if it moved: that covers the seek retry and the plain
        // top-scored row alike, and skips the expensive re-render when
        // the arrival left the highlighted type unchanged.
        let previewed = self
            .override_candidates
            .get(self.override_highlight)
            .map(|(f, _)| f.clone());
        if self.override_candidates_pending {
            let range = self.heat_scored_range(idx);
            // `Tier::Visible`: a passive re-check after a worker
            // wakeup, not a fresh user action, so it must not preempt
            // whatever the user has since asked for.
            let lookup =
                self.heat_lookup(&range, None, 0, self.override_list_height, Tier::Visible);
            if let Some(top_n) = lookup {
                self.override_inferred_raw = top_n;
                self.override_candidates_complete = false;
                self.override_candidates_pending = false;
                // Same guard as `upgrade_active_override_to_complete`:
                // the pending flag outlives a fallback to
                // `Lexicographic`, so only refresh the on-screen list
                // while `Inferred` is still active. Otherwise the
                // freshly-cached list waits in `override_inferred_raw`
                // for a later `i` toggle.
                if self.override_sort == SortMode::Inferred {
                    self.override_candidates = self
                        .override_inferred_raw
                        .iter()
                        .map(|(f, s)| (f.clone(), Some(*s)))
                        .collect();
                    self.override_highlight = 0;
                    self.override_scroll = PaneScroll::default();
                    self.last_override_highlight = None;
                    self.message.clear();
                }
            }
        }
        if self.override_complete_pending {
            self.upgrade_active_override_to_complete();
        }
        // Retry the highlight `open_override_on_type` could not seek
        // against a cold cache. Once the list is complete without
        // finding it, fall back to `Lexicographic` — as
        // `open_override_on_type` does synchronously — whose fixed
        // universe is guaranteed to contain it.
        if let Some(key) = self.override_seek_target.clone() {
            if !self.seek_override_highlight(&key) && self.override_candidates_complete {
                self.override_sort = SortMode::Lexicographic;
                self.recompute_override_candidates();
                self.seek_override_highlight(&key);
                self.override_seek_target = None;
            }
        }
        let now = self
            .override_candidates
            .get(self.override_highlight)
            .map(|(f, _)| f.as_str());
        if now != previewed.as_deref() {
            self.preview_override_highlight();
        }
    }

    /// Move the override pane's highlighted row by `delta` (spec 0114
    /// §3.2's `j`/`k`), clamped to `0..=override_candidates.len() - 1`.
    /// Upgrades a capped preview to the complete list first (spec 0114
    /// §6) if the move would go past what is currently loaded.
    pub(super) fn move_override_highlight(&mut self, delta: isize) {
        let max_index = self.override_candidates.len().saturating_sub(1);
        if delta > 0
            && !self.override_candidates_complete
            && self.override_sort == SortMode::Inferred
            && self.override_highlight as isize + delta > max_index as isize
        {
            self.upgrade_active_override_to_complete();
        }
        let max_index = self.override_candidates.len().saturating_sub(1);
        self.override_highlight = clamp_highlight(self.override_highlight, delta, max_index);
        self.preview_override_highlight();
    }

    /// Vertical pan for the override pane (Ctrl-Up/Ctrl-Down at `step ==
    /// PAN_STEP`, plain mouse wheel at `step == WHEEL_PAN_STEP`):
    /// scrolls the candidate list without moving the highlight, bounded
    /// only by the content itself — and, per spec 0244 S7, past either
    /// end of it, by the same `pan_top_bounds` the main pane uses.
    pub(super) fn override_pan_vertical(&mut self, step: usize, up: bool) {
        let (min_top, max_top) =
            pan_top_bounds(self.override_candidates.len(), self.override_list_height);
        let top = self.override_scroll.top(&FLAT_ROWS);
        let moved = if up {
            top - step as isize
        } else {
            top + step as isize
        };
        let landed = moved.clamp(min_top, max_top);
        // Spec 0245 S2: a pan that hit its bound asks for no frame.
        self.event_changed_nothing = landed == top;
        self.override_scroll.set_top(landed, &FLAT_ROWS);
    }

    /// Horizontal pan for the override pane (Ctrl-Left/Ctrl-Right,
    /// Shift+wheel/native horizontal scroll): mirrors the main pane's
    /// own `pan_right`, stopping once the
    /// rightmost character of the widest currently-visible row would be
    /// shown — never further.
    pub(super) fn override_pan_horizontal(&mut self, step: usize, left: bool) {
        let width = self.side_area.width as usize;
        let max_offset = self.override_max_visible_line_len().saturating_sub(width);
        let before = self.override_pan_offset;
        pan_by_step_clamped(&mut self.override_pan_offset, max_offset, step, left);
        // Spec 0245 S2.
        self.event_changed_nothing = self.override_pan_offset == before;
    }

    /// Pre-registers the synthetic wrapper descriptor
    /// (`decode::register_wrapper`) for every candidate FQDN visible in
    /// the override pane's `[start, end)` row window, so that arrowing
    /// onto a not-yet-visited candidate does not stall on registration.
    /// Called once per frame from `render_override_pane`. For an
    /// already-registered candidate this is a cheap hashmap lookup, so
    /// running it every frame costs nothing; a real registration happens
    /// only on the first frame a candidate scrolls into view.
    ///
    /// Cannot run on a background thread: registration mutates
    /// `self.ctx.pool` in place, which is only safely mutable from this
    /// thread. Sharing the pool behind a `Mutex` would be a much larger
    /// change. Warming the whole visible batch in one pass still
    /// decouples the cost from individual keystrokes.
    ///
    /// Silently skips a candidate that fails to resolve — best-effort
    /// only. The real error still surfaces the ordinary way when the
    /// user highlights that row and `splice_override` runs.
    pub(super) fn warm_visible_override_wrappers(&mut self, start: usize, end: usize) {
        let Some(idx) = self.override_target else {
            return;
        };
        let span = &self.tree[idx].span;
        let field_number = span.field_number;
        let is_group = u32::from(span.wire_type) == prototext_core::helpers::WT_START_GROUP;
        // Spec 0219 S4: the same predicate `render_node_as` uses, so
        // warming cannot register a wrapper the splice then never looks
        // up — which would restore the per-keystroke registration stall
        // this function exists to remove.
        let packed = decode::packed_framing(span);
        // Spec 0253 S4: the same reason, for the other half of the
        // wrapper name — warming under a different label would register
        // a wrapper the splice never looks up, restoring the
        // per-keystroke registration stall this function exists to
        // remove.
        let cardinality = self.field_cardinality(idx);
        let end = end.min(self.override_candidates.len());
        for row in start..end {
            let name = self.override_candidates[row].0.clone();
            if name == "protolens_internal.None" {
                continue;
            }
            let Some((target_desc, field_type)) = self.ctx.wrapper_target_for(&name, is_group)
            else {
                continue;
            };
            let _ = decode::register_wrapper(
                self.ctx.pool_mut(),
                field_number,
                field_type,
                target_desc,
                packed,
                cardinality,
            );
        }
    }

    /// One override-pane candidate row's display text + base style.
    /// Shared with `override_max_visible_line_len`, so that the pan
    /// clamp measures exactly what will be rendered rather than
    /// duplicating the FQDN-formatting logic.
    ///
    /// The ` [enum]` suffix needs the name resolved in the pool, which on
    /// the lazy branch (spec 0197) means its file must already be loaded.
    /// It is: `render_override_pane` calls `warm_visible_override_wrappers`
    /// over the same row window first, and that resolves every candidate —
    /// an enum name misses `get_message_by_name` but has still had its
    /// file's closure loaded by then.
    pub(super) fn override_row_display(&self, row: usize) -> (String, Style) {
        let (fqdn, score) = &self.override_candidates[row];
        let (display_fqdn, base_style) = if fqdn == "protolens_internal.None" {
            ("None".to_string(), Style::default())
        } else if decode::primitive_type_for_keyword(fqdn).is_some() {
            (fqdn.clone(), Style::default())
        } else if self.ctx.pool().get_enum_by_name(fqdn).is_some() {
            let display = format!("{} [enum]", override_display::format_fqdn_label(fqdn));
            (display, theme::style_for(SyntaxRole::Attribute, self.theme))
        } else {
            (override_display::format_fqdn_label(fqdn), Style::default())
        };
        let text = match score {
            Some(s) => format!("{display_fqdn}  (score: {s})"),
            None => display_fqdn,
        };
        (text, base_style)
    }

    /// Longest rendered row (in characters) among the override pane's
    /// currently-visible window — the basis for
    /// `override_pan_horizontal`'s clamp, mirroring the main pane's own
    /// `max_visible_line_len`.
    pub(super) fn override_max_visible_line_len(&self) -> usize {
        let total = self.override_candidates.len();
        let (_, visible) =
            self.override_scroll
                .window(self.override_list_height, &FLAT_ROWS, total);
        visible
            .map(|row| self.override_row_display(row).0.chars().count())
            .max()
            .unwrap_or(0)
    }

    /// Live-previews the currently-highlighted override candidate as a
    /// **render-time overlay** (spec 0185) — a block of rendered lines
    /// held beside the committed document and substituted for the
    /// target's rows while drawing. No-op if the override pane is not
    /// open. Raw (`Option::None`) is reached only via the `None`
    /// sentinel entry in alphabetic mode.
    ///
    /// A preview must change what the user *sees*, not what the document
    /// *is*. Nothing downstream reads a spliced preview: the candidate
    /// list is already computed, scoring runs against raw byte ranges,
    /// and confirming re-derives everything from the entry. So this
    /// renders and stops — rebuilding is a plain overwrite, discarding a
    /// plain assignment, and there is no mutation to back out.
    ///
    /// `render_node_as` is shared verbatim with the committed path,
    /// which is what makes the preview byte-identical to the splice it
    /// stands in for, and it resolves a packed-run element to the run's
    /// leader, so previewing one element of a run covers the whole
    /// record — what a commit would actually replace (spec 0184 S1).
    pub(super) fn preview_override_highlight(&mut self) {
        let Some(idx) = self.override_target else {
            return;
        };
        let tentative = self
            .override_candidates
            .get(self.override_highlight)
            .map(|(fqdn, _)| fqdn.clone());
        match self.render_node_as(idx, tentative.as_deref(), true, None) {
            Ok((_target, span, rendered)) => {
                // Spec 0210 S2: the committed node's line range, carried
                // into visible-row space at both ends. `span.text_range`
                // is the freshly derived range `render_node_as` puts
                // there, and for a packed run it is the whole run's.
                // Both numbers stay valid for the overlay's whole
                // lifetime because S5's focus lock makes the only two
                // things that move a row — folding and splicing —
                // unreachable while it is up.
                let rows = self.visible_row_count();
                let lines = crate::decode::widen(&span.text_range);
                let first_row = self.visible_row_of_line(lines.start).unwrap_or(rows);
                let covered_rows = self
                    .visible_row_of_line(lines.end)
                    .unwrap_or(rows)
                    .saturating_sub(first_row);
                self.preview_overlay = Some(PreviewOverlay {
                    first_row,
                    covered_rows,
                    lines: rendered.lines,
                    spans: rendered.spans,
                    // Spec 0251 S6: `Some` exactly when `is_preview`,
                    // which the call above passes unconditionally.
                    bytes: rendered.bytes.expect("a preview render owns its bytes"),
                });
            }
            // Spec 0185 S6: a candidate that fails to render leaves the
            // main pane showing committed content.
            Err(e) => {
                self.preview_overlay = None;
                self.message = format!("cannot preview override: {e}");
            }
        }
    }

    /// Find the next `override_candidates` entry whose FQDN contains
    /// `pattern` (smartcase — spec 0195 S2), searching in `dir` from
    /// just past the highlighted row and wrapping around. Moves the
    /// highlight there on success; otherwise leaves it unchanged and
    /// sets a status-line message.
    pub(super) fn jump_to_override_match(&mut self, dir: SearchDir, pattern: &str) {
        self.run_search(SearchScope::Override, dir, pattern);
    }

    /// Find the next node whose own opening line contains `pattern`
    /// (smartcase — spec 0195 S2), searching in `dir` from just past the
    /// cursor and wrapping around at the ends of the document (spec 0114
    /// §4, extended to the main pane). Folded-away matches are found and
    /// then revealed, since the scan covers the whole document rather
    /// than the visible rows. Matching is against the nodes' current
    /// rendered text, so an overridden range searches its post-override
    /// rendering with no special-casing. On a match, moves the cursor
    /// there (recording a jumplist entry) and unfolds its ancestors;
    /// otherwise leaves the cursor put and sets a status-line message.
    ///
    /// Steps line by line with `next_line`/`prev_line` (spec 0222 S6)
    /// rather than node by node. The cursor can rest on a closing `}`,
    /// and a node-level step from there would descend back into the
    /// node's own children — so only a line-level walk visits the
    /// document in the order the old text scan did. The wrap endpoints
    /// are resolved on demand, and only if the walk actually wraps, so
    /// neither direction pays spec 0195 S1's eager `last_node()`.
    pub(super) fn jump_to_match(&mut self, dir: SearchDir, pattern: &str) {
        self.run_search(SearchScope::Main, dir, pattern);
    }
}
