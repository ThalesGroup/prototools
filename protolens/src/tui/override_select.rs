// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

use prost_reflect::prost_types::field_descriptor_proto::Type;

use super::tiered::Tier;

impl App {
    /// Whether `idx` is eligible as an override target (`t`, `type-as`,
    /// `type-as-raw`): a message/group node already (`NodeSpan::
    /// is_message`, spec 0114 §1.2 — *not* `type_fqdn.is_some()`, which is
    /// ambiguous between a scalar and a schema-unresolved message/group),
    /// or any scalar with a decodable tag — `wire_type` one of `WT_LEN`
    /// (string, bytes, or an unresolved LEN-wire field, reinterpretable
    /// as an embedded message), `WT_VARINT`, `WT_I32`, `WT_I64` (spec
    /// 0135 §G3: primitive-type overrides, no longer categorically
    /// excluded). For a packed-repeated element (`packed_record_start.
    /// is_some()`), eligibility is evaluated against the whole record's
    /// own reconstructed wire type, always `WT_LEN` (spec 0135 §G1) — not
    /// the individual element's own `wire_type`. Any/MessageSet
    /// auto-expansion (spec 0120) already reinterprets exactly this kind
    /// of scalar unconditionally; a manual override that turns out to
    /// target genuinely incompatible bytes simply fails to parse and
    /// `splice_override` reports it — the user is trusted to judge
    /// whether the result is meaningful (2026-07-14 feedback: `t` used
    /// to unconditionally refuse every string/bytes field).
    pub(super) fn can_override(&self, idx: usize) -> bool {
        use prototext_core::helpers::{WT_I32, WT_I64, WT_LEN, WT_VARINT};
        let span = &self.tree[idx].span;
        if span.packed_record_start.is_some() {
            return true;
        }
        span.is_message || matches!(span.wire_type, WT_LEN | WT_VARINT | WT_I32 | WT_I64)
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
        self.override_scroll = 0;
        self.last_override_highlight = None;
        self.override_pan_offset = 0;

        // Spec 0139: smart initial sort-mode/highlight. Step A: an
        // active override on the cursor node; else Step B: the first
        // inactive-but-applicable entry from the management list
        // (`first_entry_matching_origin_candidates` — by construction,
        // since Step A found no active match, any entry it returns here
        // is necessarily inactive); else Step B.5: with no override at
        // all, the field's own *currently effective* type — for a
        // message/group node, `span.type_fqdn` itself (2026-07-20 fix:
        // previously routed through `natural_type`/`parent_field`,
        // i.e. the *parent's* schema-declared field type; but a node's
        // own `span.type_fqdn` can legitimately differ from that —
        // e.g. a field decoded via best-effort inference rather than a
        // strict schema declaration — in which case `parent_field`
        // fails and `natural_type` returns `None` even though the main
        // pane is already showing a perfectly good resolved type for
        // this exact node. `span.type_fqdn` is the same value `status_
        // type_label` shows in the status line, so it's always exactly
        // "the type already shown in the main pane" the override pane
        // should seed its highlight on); for any other kind (enum,
        // primitive), `natural_type` as before — those have no
        // standalone `span.type_fqdn` of their own to fall back on.
        // Without this step such a field fell through to
        // `open_override_on_default`'s `Inferred`-mode scoring, which is
        // meaningless for a scalar (it scores the bytes as a prospective
        // *message*) and so landed on the unrelated `None` sentinel row
        // (or the top-scored message FQDN) instead of the field's own
        // current type (2026-07-18 feedback, extended from enum-only to
        // every primitive keyword 2026-07-19, then to message/group
        // nodes 2026-07-20: `open_override_on_type` below naturally
        // lands in `Lexicographic` mode for scalar keywords, since no
        // primitive keyword is ever a member of the `Inferred`
        // candidate list).
        let candidate_type = self
            .resolve_active_override_entry(self.cursor)
            .map(|e| e.r#type.clone())
            .or_else(|| {
                self.first_entry_matching_origin_candidates(self.cursor)
                    .map(|i| self.overrides.entries()[i].r#type.clone())
            })
            .or_else(|| {
                let span = &self.tree[self.cursor].span;
                if span.is_message {
                    span.type_fqdn.clone().map(Some)
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
        // 2026-07-20 fix: a cold cache means the two calls above merely
        // queued background requests rather than answering — the
        // *complete* list isn't known yet, so "not found" would be a
        // false negative (exactly what `upgrade_active_override_to_
        // complete` above is meant to avoid). Stay in `Inferred` mode
        // and remember `key` so `poll_pending_override_work` can keep
        // trying to seek it as more of the list arrives in the
        // background, instead of discarding it for `Lexicographic`
        // outright.
        if self.override_candidates_pending || self.override_complete_pending {
            self.override_seek_target = Some(key);
            return;
        }
        // 2026-07-20 fix: the `recompute_override_candidates` call above
        // may have set the "no scoring graph available" message (no
        // `ctx.graph` at all, so `Inferred` mode is unreachable) — this
        // silent fallback already does what that message would have
        // suggested, so it must not leak through (mirrors `open_
        // override_on_default`'s own suppression of the same message).
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
        // 2026-07-20 fix: an empty list while `override_candidates_
        // pending` is set means the cache was merely cold at open time
        // (a background request was just queued) — not that `Inferred`
        // mode has nothing to offer. Falling back to `Lexicographic`
        // here discarded that in-flight fetch's eventual result outright
        // (and, before the pending-flag fix above, could later resurface
        // it as a permanently-capped, un-growable list once it resolved
        // — spec 0152 G7 follow-up). Wait for it instead; the "no
        // scoring graph available" case (no request ever queued, so
        // `pending` stays false) still falls back immediately, as before.
        if self.override_candidates.is_empty() && !self.override_candidates_pending {
            // Not superseded by spec 0147 G5's top-of-`handle_key` clear
            // (which only dismisses a message left over from a *previous*
            // keypress) — this clears the "no scoring graph available"
            // message the `recompute_override_candidates` call just above
            // set during *this* keypress, ahead of the silent fallback
            // below.
            self.message.clear();
            self.override_sort = SortMode::Lexicographic;
            self.recompute_override_candidates();
            return;
        }
        // 2026-07-20 feedback: "too lazy" — eagerly begin fetching the
        // rest of the list right away instead of waiting for the user
        // to scroll to the loaded boundary. `poll_pending_override_
        // work`'s background-driven follow-up (spec 0152 G7 update)
        // takes it the rest of the way to `override_candidates_
        // complete` on its own from here.
        self.upgrade_active_override_to_complete();
    }

    /// `Enter`/double-click on a main-pane node (item 3, spec 0139
    /// follow-up): a smart proxy for `t`/`o` — opens the management
    /// pane (`o`) if an override already applies to the cursor node,
    /// active or not (the same Step A/B check spec 0139's `t` itself
    /// uses to pick its initial highlight); otherwise opens the
    /// selection pane (`t`), which handles eligibility/width refusals
    /// on its own exactly as a direct keypress would.
    pub(super) fn open_smart_override_or_manage(&mut self) {
        let has_override = self.resolve_active_override_entry(self.cursor).is_some()
            || self
                .first_entry_matching_origin_candidates(self.cursor)
                .is_some();
        if has_override {
            self.toggle_manage_pane();
        } else {
            self.toggle_override();
        }
    }

    /// Close the override pane (cancelling — spec 0114 §2), regardless of
    /// which pane currently has focus. Demotes `override_inferred_raw` (if
    /// any) into `heat_caches`' shared `by_range` cache, capped to however
    /// many rows the pane was actually showing — spec 0114 §6's "other
    /// entries keep only the first N lines" (spec 0152 N8: relocked onto
    /// the shared cache, logic otherwise unchanged).
    ///
    /// Spec 0185 G2/Q3: closing re-renders **nothing**. It used to open
    /// with a full recursive `render_overrides(override_target)` pass,
    /// because the live preview's own `splice_override` rebuilt the
    /// target's whole subtree from scratch with no overrides applied to
    /// any of the fresh descendants, so anything less than the full
    /// recursion would leave every previously-auto-expanded
    /// `Any`/`MessageSet` descendant un-re-expanded. That damage existed
    /// *because* the preview mutated the tree. An overlay never does, so
    /// there is nothing to re-seed and nothing to revert — dropping the
    /// overlay is the whole revert. A re-render is now triggered only by
    /// the events that change committed state: confirming a type, or
    /// (de)activating an entry in the management pane.
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
                heat_worker::RangeHeatEntry {
                    best_score: stats.best_score,
                    best_count: stats.best_count,
                    top_n: self
                        .override_inferred_raw
                        .iter()
                        .take(top_n_len.max(1))
                        .cloned()
                        .collect(),
                },
                Tier::Visible,
            );
            if self.override_candidates_complete {
                caches.complete = Some((range, self.override_inferred_raw.clone()));
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
        // Item 11 (2026-07-17 feedback): a pane opened from the
        // management pane always returns there on close — the Enter-
        // confirm call site (`handle_override_key`) already sets these
        // same three fields itself right after calling this, so setting
        // them here too is harmless there; it's the cancelling call
        // sites (`Esc`/`t`/`q`) that actually need it.
        if self.override_opened_from_manage {
            self.override_opened_from_manage = false;
            self.manage_open = true;
            self.manage_focus = true;
        }
    }

    /// `Enter`/double-click on an entry in the override management pane
    /// (item 11, 2026-07-17 feedback): opens the selection pane on that
    /// entry's own origin, initially highlighted on its own current type
    /// (Step A/B's mode-selection rule, reused via `open_override_on_
    /// type`), to let the user pick an alternate type. Confirming lands
    /// back in the management pane (spec 0119 G3, unconditional); Esc/
    /// `t`/`q` also return there — without mutating the entry — via
    /// `override_opened_from_manage` (`close_override`).
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
        self.override_scroll = 0;
        self.last_override_highlight = None;
        self.override_pan_offset = 0;
        self.override_opened_from_manage = true;
        self.open_override_on_type(current_type);
        self.preview_override_highlight();
    }

    /// Recompute `override_candidates` for the current `override_target`
    /// under the currently active `override_sort` (spec 0114 §3.2), and
    /// reset the highlight to the first candidate (index `0`) — in
    /// alphabetic mode this is always the `None` sentinel (spec 0137
    /// §G1/§G4), not necessarily what was previously highlighted;
    /// there is no separate pinned row to preserve any more. Called
    /// both when the pane first opens and whenever `i` toggles the sort
    /// mode.
    ///
    /// `SortMode::Inferred` consults `active_override_range` (spec 0114
    /// §6) before calling `self.heat_lookup` (spec 0152 G7): toggling
    /// back to `Inferred` within the same open-pane session reuses
    /// `override_inferred_raw` as-is (no recomputation at all); a
    /// genuinely new range asks the shared cache — a hit applies
    /// immediately, a miss sets `override_candidates_pending` (the
    /// request itself already pushed inside `heat_lookup`) and leaves
    /// the pane showing its "Scoring candidates…" placeholder until a
    /// worker-progress wakeup resolves it (`poll_pending_override_work`).
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
                    let node = &self.tree[idx].span;
                    let range = extract::message_payload_range(
                        &self.blob,
                        &node.raw_range,
                        node.packed_record_start,
                    );
                    if self.active_override_range.as_ref() != Some(&range) {
                        // 2026-07-20 feedback: this directly follows the
                        // user pressing `t`/`i` — it must jump the queue
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
                                // Request already pushed inside
                                // `heat_lookup`; leave `override_
                                // inferred_raw` as whatever it already
                                // was (typically empty) — the pane
                                // shows a "Scoring candidates…"
                                // placeholder instead (spec 0152 G7/N3).
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
        self.override_scroll = 0;
        self.last_override_highlight = None;
        self.override_pan_offset = 0;
    }

    /// Fetches the *complete*, full, unbounded candidate list for the
    /// current override target in one shot (2026-07-20 feedback:
    /// replaces growing `override_inferred_raw` one bounded page at a
    /// time, which required the user to keep scrolling/paging all the
    /// way to the end before the pane's real candidate count was ever
    /// known). Requests `[0, usize::MAX)`: `HeatCaches::window`'s
    /// `complete`-slot fallback always clamps `end` to the actual
    /// candidate count (spec 0152 G5) rather than requiring `top_n` to
    /// literally hold `usize::MAX` entries, so this resolves to the
    /// true, full list — whatever that list's real length turns out to
    /// be — as soon as the worker's one full sweep for this range
    /// lands, instead of growing in bounded increments. A hit replaces
    /// `override_inferred_raw` wholesale and marks the pane complete;
    /// a miss (the request is already pushed inside `heat_lookup`)
    /// sets `self.override_complete_pending = true` for `poll_pending_
    /// override_work` to retry on the next worker-progress wakeup —
    /// the pane's list stays at whatever it already had (typically the
    /// fast bounded first page from `recompute_override_candidates`)
    /// until then. No-op if already complete, or if `override_target`/
    /// `ctx.graph` is absent. Called both when the pane first opens
    /// (spec 0139, so the full fetch begins immediately rather than
    /// waiting for the user to scroll) and, defensively, when the user
    /// scrolls past the loaded window (spec 0114 §6).
    pub(super) fn upgrade_active_override_to_complete(&mut self) {
        if self.override_candidates_complete {
            return;
        }
        let (Some(idx), Some(_graph)) = (self.override_target, &self.ctx.graph) else {
            return;
        };
        let node = &self.tree[idx].span;
        let range =
            extract::message_payload_range(&self.blob, &node.raw_range, node.packed_record_start);
        // 2026-07-20 feedback: directly follows the user opening the
        // pane or scrolling past the loaded window — a user event,
        // jumps the queue.
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
        // 2026-07-20 fix: only sync the on-screen list when `Inferred` is
        // still the active sort mode. `poll_pending_override_work`'s
        // `override_complete_pending` branch calls this unguarded on a
        // background wakeup — if the pane has since fallen back to
        // `Lexicographic` (a cold-cache miss at open time, spec 0139's
        // `open_override_on_type`/`open_override_on_default`), the
        // now-resolved `Inferred` data must stay parked in `override_
        // inferred_raw` for a later `i` toggle rather than clobbering the
        // Lexicographic list actually on screen.
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
    /// for a worker-progress event and either pending flag is set. A
    /// hit applies the result and clears the flag; a miss (`heat_
    /// lookup` just re-pushed the same request, merged by range and so
    /// harmless per G3) leaves both alone. Also retries `override_
    /// seek_target`, if any, once more data has arrived (2026-07-20
    /// feedback).
    pub(super) fn poll_pending_override_work(&mut self) {
        let Some(idx) = self.override_target else {
            return;
        };
        // Spec 0132 §G2's live preview is driven off the highlighted row,
        // and every arrival below can change which type that row names —
        // most visibly on a cold-cache open, where `toggle_override`'s
        // preview ran against a still-empty list and so spliced *raw*.
        // Snapshot the highlighted type now and re-preview at the end iff
        // it moved: that covers the seek retry and the plain top-scored
        // row alike, and skips the (expensive) re-splice when the arrival
        // left the highlighted type unchanged.
        let previewed = self
            .override_candidates
            .get(self.override_highlight)
            .map(|(f, _)| f.clone());
        if self.override_candidates_pending {
            let node = &self.tree[idx].span;
            let range = extract::message_payload_range(
                &self.blob,
                &node.raw_range,
                node.packed_record_start,
            );
            // 2026-07-20 feedback: a passive re-check after a worker-
            // progress wakeup, not a fresh user action — must not
            // preempt whatever the user has since asked for.
            let lookup =
                self.heat_lookup(&range, None, 0, self.override_list_height, Tier::Visible);
            if let Some(top_n) = lookup {
                self.override_inferred_raw = top_n;
                self.override_candidates_complete = false;
                self.override_candidates_pending = false;
                // 2026-07-20 fix: `open_override_on_default`'s Lexicographic
                // fallback (an Inferred-mode cache miss with nothing to
                // show) leaves this pending flag set even though the pane
                // has since moved on to a different sort mode — resolving
                // it here must only refresh what's actually on screen when
                // that mode is still `Inferred`. Otherwise the freshly-
                // cached raw list stays parked in `override_inferred_raw`
                // for a later `i` toggle to pick up, without clobbering the
                // currently-displayed (and already complete) Lexicographic
                // list.
                if self.override_sort == SortMode::Inferred {
                    self.override_candidates = self
                        .override_inferred_raw
                        .iter()
                        .map(|(f, s)| (f.clone(), Some(*s)))
                        .collect();
                    self.override_highlight = 0;
                    self.override_scroll = 0;
                    self.last_override_highlight = None;
                    self.message.clear();
                }
            }
        }
        if self.override_complete_pending {
            self.upgrade_active_override_to_complete();
        }
        // 2026-07-20 feedback: once more of the list has arrived, retry
        // seeking whatever `open_override_on_type` was still trying to
        // highlight when the cache was cold at open time
        // (`override_seek_target`). Falls back to `Lexicographic` mode
        // — mirroring `open_override_on_type`'s own synchronous
        // fallback — once `override_candidates_complete` is reached
        // without finding it; the fixed Lexicographic universe is
        // guaranteed to contain it.
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
    /// §3.2's `j`/`k`), clamped to `0..=override_candidates.len() - 1`
    /// (spec 0137 §G4: `override_candidates` is indexed directly — no
    /// more pinned row 0). Upgrades a capped preview to the complete
    /// list first (spec 0114 §6) if the requested move would go past
    /// what's currently loaded.
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
        // Spec 0132 §G2: live-preview the newly-highlighted candidate.
        self.preview_override_highlight();
    }

    /// Vertical pan for the override pane (Ctrl-Up/Ctrl-Down at `step ==
    /// PAN_STEP`, plain mouse wheel at `step == WHEEL_PAN_STEP`,
    /// 2026-07-19 feedback items 1/2): scrolls the candidate list
    /// without moving the highlight, bounded only by the content itself.
    pub(super) fn override_pan_vertical(&mut self, step: usize, up: bool) {
        let max_scroll = self
            .override_candidates
            .len()
            .saturating_sub(self.override_list_height);
        pan_vertical_by_step(&mut self.override_scroll, max_scroll, step, up);
    }

    /// Horizontal pan for the override pane (Ctrl-Left/Ctrl-Right,
    /// Shift+wheel/native horizontal scroll, 2026-07-19 feedback item 4):
    /// mirrors the main pane's own `pan_right`, stopping once the
    /// rightmost character of the widest currently-visible row would be
    /// shown — never further.
    pub(super) fn override_pan_horizontal(&mut self, step: usize, left: bool) {
        let width = self.side_area.width as usize;
        let max_offset = self.override_max_visible_line_len().saturating_sub(width);
        pan_by_step_clamped(&mut self.override_pan_offset, max_offset, step, left);
    }

    /// Pre-registers the synthetic wrapper descriptor
    /// (`decode::register_wrapper`) for every candidate FQDN currently
    /// visible in the override pane's `[start, end)` row window
    /// (2026-07-20 feedback: moving the cursor to a not-yet-visited
    /// candidate used to stall for as long as `splice_override`'s own
    /// `register_wrapper` call took to run). Called once per frame
    /// from `render_override_pane`, right after `start`/`end` are
    /// computed, so every row the user can currently see is already
    /// registered in `self.ctx.pool` by the time `j`/`k` actually
    /// lands on it. A no-op, cheap hashmap lookup for a candidate
    /// that's already registered — the same fast path
    /// `register_wrapper` itself uses — so calling this every frame
    /// costs nothing once a row's been warmed once; a genuinely new
    /// registration only happens the first frame a given candidate
    /// scrolls into view, not once per keystroke that later visits it.
    ///
    /// Doesn't (and can't, without a `Mutex`-guarded pool shared
    /// across threads — a much larger change) run on a real
    /// background thread: registration mutates `self.ctx.pool` in
    /// place, which is only ever safely mutable from this one thread.
    /// Running it here, ahead of any specific row being highlighted,
    /// still decouples the cost from individual keystrokes — the
    /// whole visible batch is warmed in one pass instead of one row
    /// at a time as the user arrows through it.
    ///
    /// Silently skips a candidate that fails to resolve (unknown
    /// name, or a schema link error) — best-effort only; the real
    /// error, if any, still surfaces the ordinary way once the user
    /// actually highlights that row and `splice_override` runs for
    /// real.
    pub(super) fn warm_visible_override_wrappers(&mut self, start: usize, end: usize) {
        let Some(idx) = self.override_target else {
            return;
        };
        let span = &self.tree[idx].span;
        let field_number = span.field_number;
        let is_group = span.wire_type == prototext_core::helpers::WT_START_GROUP;
        let end = end.min(self.override_candidates.len());
        for row in start..end {
            let name = self.override_candidates[row].0.clone();
            if name == "protolens_internal.None" {
                continue;
            }
            let (target_desc, field_type) = if let Some(desc) = self.ctx.message(&name) {
                let ft = if is_group { Type::Group } else { Type::Message };
                (Some(decode::WrapperTarget::Message(desc)), ft)
            } else if let Some(prim) = decode::primitive_type_for_keyword(&name) {
                (None, prim)
            } else if let Some(enum_desc) = self.ctx.enumeration(&name) {
                (Some(decode::WrapperTarget::Enum(enum_desc)), Type::Enum)
            } else {
                continue;
            };
            let _ = decode::register_wrapper(
                self.ctx.pool_mut(),
                field_number,
                field_type,
                target_desc,
            );
        }
    }

    /// One override-pane candidate row's display text + base style —
    /// factored out of `render_override_pane`'s per-row loop (2026-07-19
    /// feedback item 4) so `override_max_visible_line_len` can compute
    /// exactly what will be rendered without duplicating the FQDN-
    /// formatting logic.
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
            let display = if override_apply::fqdn_needs_dot_prefix(fqdn) {
                format!(".{fqdn} [enum]")
            } else {
                format!("{fqdn} [enum]")
            };
            (display, theme::style_for(SyntaxRole::Attribute, self.theme))
        } else {
            let display = if override_apply::fqdn_needs_dot_prefix(fqdn) {
                format!(".{fqdn}")
            } else {
                fqdn.clone()
            };
            (display, Style::default())
        };
        let text = match score {
            Some(s) => format!("{display_fqdn}  (score: {s})"),
            None => display_fqdn,
        };
        (text, base_style)
    }

    /// Longest rendered row (in characters) among the override pane's
    /// currently-visible window — the basis for `override_pan_horizontal`'s
    /// clamp (2026-07-19 feedback item 4), mirroring the main pane's own
    /// `max_visible_line_len`.
    pub(super) fn override_max_visible_line_len(&self) -> usize {
        let total = self.override_candidates.len();
        let start = self.override_scroll.min(total);
        let end = (self.override_scroll + self.override_list_height).min(total);
        (start..end)
            .map(|row| self.override_row_display(row).0.chars().count())
            .max()
            .unwrap_or(0)
    }

    /// Spec 0132 §G2, reworked by spec 0185: live-previews the
    /// currently-highlighted override candidate as a **render-time
    /// overlay** — a block of rendered lines held beside the committed
    /// document and substituted for the target's rows while drawing.
    /// No-op if the override pane isn't open. `override_highlight`
    /// indexes `override_candidates` directly (spec 0137 §G4) — no more
    /// pinned row 0; raw (`Option::None`) is reached only via the
    /// `None` sentinel entry in alphabetic mode.
    ///
    /// It used to *apply* the override instead, calling the same
    /// `splice_override` a confirmed `Enter` calls with one flag
    /// flipped. A splice is a document-wide operation, so every `j`/`k`
    /// in the candidate list rebuilt `lines`/`line_styles`, walked
    /// `doc_next` to shift every following node, refilled
    /// `line_to_node`/`footer_line_to_node`, and re-scanned
    /// `visible_rows`. Backing that out then needed six further pieces
    /// of state (spec 0161's tree watermark, three `retain`s, hand-nulled
    /// child pointers, a force-`None`d `rendered_as`) plus a full
    /// `render_overrides` pass in `close_override` — all of it existing
    /// only to undo something that should never have been done.
    ///
    /// A preview needs to change what the user *sees*, not what the
    /// document *is*: nothing downstream reads the spliced state (the
    /// candidate list is already computed, scoring runs against raw byte
    /// ranges, and confirming re-derives everything from the entry).
    /// So this now renders and stops. Rebuilding is a plain overwrite
    /// and discarding is a plain assignment.
    ///
    /// `render_node_as` is shared verbatim with the committed path,
    /// which is what makes the preview byte-identical to the splice it
    /// stands in for (G3), and it resolves a packed-run element to the
    /// run's leader, so previewing one element of a run covers the whole
    /// record — what a commit would actually replace (S1, spec 0184).
    pub(super) fn preview_override_highlight(&mut self) {
        let Some(idx) = self.override_target else {
            return;
        };
        let tentative = self
            .override_candidates
            .get(self.override_highlight)
            .map(|(fqdn, _)| fqdn.clone());
        match self.render_node_as(idx, tentative.as_deref(), true) {
            Ok((_target, span, rendered)) => {
                // `visible_rows` is sorted ascending, so the run of rows
                // the committed node covers is a binary search away.
                // Both numbers stay valid for the overlay's whole
                // lifetime because S5's focus lock makes the only two
                // things that rebuild `visible_rows` — folding and
                // splicing — unreachable while it is up.
                let first_row = self
                    .visible_rows
                    .partition_point(|&l| l < span.text_range.start);
                let covered_rows =
                    self.visible_rows[first_row..].partition_point(|&l| l < span.text_range.end);
                self.preview_overlay = Some(PreviewOverlay {
                    first_row,
                    covered_rows,
                    lines: rendered.lines,
                });
            }
            // Spec 0185 S6: a candidate that fails to render leaves the
            // main pane showing committed content — the same outcome as
            // a failed preview before this spec, reached without a
            // partial mutation to back out.
            Err(e) => {
                self.preview_overlay = None;
                self.message = format!("cannot preview override: {e}");
            }
        }
    }

    /// Find the next `override_candidates` entry (0-based, direct index
    /// — spec 0137 §G4: no more pinned raw row excluded from matching)
    /// whose FQDN contains `pattern` (smartcase — spec 0195 S2),
    /// searching in `dir` from just past the currently highlighted row,
    /// wrapping around. Moves the highlight there on success; otherwise
    /// leaves it unchanged and sets a status-line message.
    pub(super) fn jump_to_override_match(&mut self, dir: SearchDir, pattern: &str) {
        if pattern.is_empty() || self.override_candidates.is_empty() {
            return;
        }
        let needle = SearchPattern::new(pattern);
        let n = self.override_candidates.len();
        let start = match dir {
            SearchDir::Forward => (self.override_highlight + 1) % n,
            SearchDir::Backward => (self.override_highlight + n - 1) % n,
        };
        match search_wrap(n, start, dir, |i| {
            needle.is_match(&self.override_candidates[i].0)
        }) {
            Some(i) => {
                self.override_highlight = i;
                // Spec 0132 §G2: live-preview the newly-highlighted
                // candidate, same as arrow-key movement (2026-07-17
                // feedback: search-jump landed silently, without a
                // main-pane preview).
                self.preview_override_highlight();
            }
            None => self.message = format!("pattern not found: {pattern}"),
        }
    }

    /// Find the next node (walking the whole document-order chain via
    /// `doc_next`/`doc_prev` — not just currently visible/unfolded nodes,
    /// so a folded-away match is still found and then revealed) whose own
    /// opening line (`self.lines[node.span.text_range.start]`) contains
    /// `pattern` (smartcase — spec 0195 S2), searching in `dir` from just
    /// past the cursor and wrapping around at the ends of the chain via
    /// `first_node`/`last_node()` (spec 0114 §4, extended to the main
    /// pane). Always matches against `self.lines`' *current* rendered
    /// text, so a range whose type has been overridden (spec 0114 §5)
    /// searches the post-override rendering, not the original one — no
    /// special-casing needed, since overrides mutate `self.lines` in
    /// place rather than being tracked separately. On a match, moves the
    /// cursor there (recording a jumplist entry) and unfolds its
    /// ancestors so it's visible; otherwise leaves the cursor unchanged
    /// and sets a status-line message.
    pub(super) fn jump_to_match(&mut self, dir: SearchDir, pattern: &str) {
        if pattern.is_empty() || self.tree.is_empty() {
            return;
        }
        let needle = SearchPattern::new(pattern);
        let mut cur = self.cursor;
        loop {
            // Spec 0195 S1: `unwrap_or_else`, not `unwrap_or`. The
            // latter evaluates its argument on every step, and
            // `last_node()` walks the whole `doc_next` chain — which
            // made a backward search cost O(steps x nodes) where the
            // forward arm, reading the `first_node` *field*, cost
            // O(steps).
            cur = match dir {
                SearchDir::Forward => self.tree[cur].doc_next.unwrap_or(self.first_node),
                SearchDir::Backward => self.tree[cur].doc_prev.unwrap_or_else(|| self.last_node()),
            };
            let line_idx = self.tree[cur].span.text_range.start;
            if let Some(byte) = needle.find(&self.lines[line_idx]) {
                if cur != self.cursor {
                    self.record_jump();
                    self.set_cursor(cur);
                    self.unfold_ancestors(cur);
                }
                // Spec 0194 S8: land on the match, not merely on its
                // row. Clamped, since a match inside the row's own
                // indentation is left of the leftmost reachable column
                // (S3) — and since `set_cursor` above has already put
                // the caret at the first non-blank, that clamp is the
                // whole correction.
                self.cursor_column = self.lines[line_idx][..byte].chars().count();
                self.clamp_caret_column();
                self.desired_column = self.cursor_column;
                return;
            }
            if cur == self.cursor {
                break;
            }
        }
        self.message = format!("pattern not found: {pattern}");
    }
}
