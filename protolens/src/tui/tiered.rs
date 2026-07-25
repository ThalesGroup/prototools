// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0164 G2: bounded, tier-prioritized map backing the heat-cue
//! request queue (`heat_worker::HeatRequestQueue`) and both result
//! caches (`heat_worker::HeatCaches`), replacing spec 0151's
//! `BoundedMru`. See the module's own doc comments below and spec
//! 0164's Specification section for the full design rationale.

use std::collections::HashMap;
use std::hash::Hash;

/// Spec 0164: replaces `Priority` (`UserEvent`/`Background`). A key's
/// tier only ever moves up — see `TieredBounded::upsert`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Tier {
    Prefetch,
    Visible,
    User,
}

struct Slot<K, V> {
    key: K,
    value: V,
    tier: Tier,
    /// Intrusive recency-list links, shared by every band. To unlink
    /// a slot without a per-slot "which band am I in" tag: if `prev`
    /// is `None`, the slot is *some* band's head — check each
    /// plausible band's `head` pointer (at most two, for `Prefetch`)
    /// and fix up whichever one matches; symmetrically for `next`/
    /// `tail`. This is O(1) (a handful of pointer comparisons, not
    /// proportional to list length) and needs no extra state.
    prev: Option<usize>,
    next: Option<usize>,
}

#[derive(Default)]
struct Band {
    head: Option<usize>, // pop end (see TieredBounded doc)
    tail: Option<usize>, // evict/insert end (see TieredBounded doc)
}

/// Links `idx` at `band`'s head, pushing whatever was previously
/// there behind it. Free function (not a method) so callers can name
/// `self.slots` and a specific `Band` field as two disjoint borrows
/// directly, without an intermediate accessor forcing a whole-`self`
/// borrow.
fn link_at_head<K, V>(slots: &mut [Option<Slot<K, V>>], band: &mut Band, idx: usize) {
    let old_head = band.head;
    match old_head {
        Some(h) => slots[h].as_mut().expect("head slot must be occupied").prev = Some(idx),
        None => band.tail = Some(idx),
    }
    let s = slots[idx].as_mut().expect("linked slot must be occupied");
    s.prev = None;
    s.next = old_head;
    band.head = Some(idx);
}

/// Symmetric with `link_at_head`, at `band`'s tail.
fn link_at_tail<K, V>(slots: &mut [Option<Slot<K, V>>], band: &mut Band, idx: usize) {
    let old_tail = band.tail;
    match old_tail {
        Some(t) => slots[t].as_mut().expect("tail slot must be occupied").next = Some(idx),
        None => band.head = Some(idx),
    }
    let s = slots[idx].as_mut().expect("linked slot must be occupied");
    s.next = None;
    s.prev = old_tail;
    band.tail = Some(idx);
}

/// Spec 0164: bounded, tier-prioritized map — O(1) insert/promote/
/// pop/evict via a `HashMap` index plus one intrusive doubly-linked
/// `Band` per band. `User` and `Visible` get one `Band` each;
/// `Prefetch` gets two — `prefetch_current` and `prefetch_previous` —
/// so a walk restart (G7) can demote a whole superseded wave in one
/// O(1) splice instead of tracking per-entry wave numbers (G2).
/// `pop_highest` always reads a band's *head*; `evict_one` always
/// reads a band's *tail*, except `Visible` (evicts from its own
/// head too — see G2 for why). Which end *insertion* uses is the
/// only other per-band variation: `User` inserts at head (LIFO),
/// `Visible` and `prefetch_current` insert at tail (FIFO);
/// `prefetch_previous` is never inserted into directly. Shared backing
/// store for `HeatRequestQueue` and both of `HeatCaches`' per-range
/// maps — structurally-identical but independently-instantiated
/// structures, not one shared pool (G2).
pub(super) struct TieredBounded<K: Eq + Hash + Clone, V: Clone> {
    slots: Vec<Option<Slot<K, V>>>,
    free: Vec<usize>,
    index: HashMap<K, usize>,
    user: Band,
    visible: Band,
    prefetch_current: Band,
    prefetch_previous: Band,
    max_entries: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum UpsertOutcome<K> {
    Applied { evicted: Option<K> },
    Rejected,
}

impl<K: Eq + Hash + Clone, V: Clone> TieredBounded<K, V> {
    pub(super) fn new(max_entries: usize) -> Self {
        TieredBounded {
            slots: Vec::new(),
            free: Vec::new(),
            index: HashMap::new(),
            user: Band::default(),
            visible: Band::default(),
            prefetch_current: Band::default(),
            prefetch_previous: Band::default(),
            max_entries,
        }
    }

    /// Promoting read (G9): if `key` is tracked at a tier lower than
    /// `tier`, bumps it to `tier` (relinking to the new tier's
    /// insertion end — `prefetch_current`'s tail if promoting *to*
    /// `Prefetch`, which cannot happen in practice) before returning
    /// the value. No-op reorder if already at `tier` or higher.
    pub(super) fn peek(&mut self, key: &K, tier: Tier) -> Option<V> {
        let idx = *self.index.get(key)?;
        let cur_tier = self.slots[idx]
            .as_ref()
            .expect("indexed slot must be occupied")
            .tier;
        if tier > cur_tier {
            self.unlink(idx);
            self.slots[idx]
                .as_mut()
                .expect("indexed slot must be occupied")
                .tier = tier;
            self.link_at_insertion_end(tier, idx);
        }
        Some(
            self.slots[idx]
                .as_ref()
                .expect("indexed slot must be occupied")
                .value
                .clone(),
        )
    }

    /// `existing_tier.max(tier)` decides the resulting tier (G5).
    /// - Promotion (tier increases): relinks to the new tier's
    ///   insertion end.
    /// - Same-tier update (any tier): payload updates in place, no
    ///   reordering — for `Prefetch` this holds even if the key
    ///   currently lives in `prefetch_previous` (G5): it is refreshed
    ///   in place, not moved to `prefetch_current`.
    /// - Brand-new key: links at its tier's insertion end (new
    ///   `Prefetch` keys always go to `prefetch_current`).
    /// - Whenever this pushes the structure over `max_entries`,
    ///   evicts once via `evict_one`. If the *new* entry itself is
    ///   what gets evicted (only possible when nothing at or below
    ///   its tier exists to evict instead), returns `Rejected` and
    ///   links nothing.
    pub(super) fn upsert(&mut self, key: K, value: V, tier: Tier) -> UpsertOutcome<K> {
        if let Some(&idx) = self.index.get(&key) {
            let cur_tier = self.slots[idx]
                .as_ref()
                .expect("indexed slot must be occupied")
                .tier;
            let new_tier = cur_tier.max(tier);
            if new_tier > cur_tier {
                self.unlink(idx);
                {
                    let s = self.slots[idx]
                        .as_mut()
                        .expect("indexed slot must be occupied");
                    s.tier = new_tier;
                    s.value = value;
                }
                self.link_at_insertion_end(new_tier, idx);
            } else {
                self.slots[idx]
                    .as_mut()
                    .expect("indexed slot must be occupied")
                    .value = value;
            }
            // An update never grows the structure — no eviction needed.
            return UpsertOutcome::Applied { evicted: None };
        }

        let idx = self.alloc_slot(key.clone(), value, tier);
        self.index.insert(key.clone(), idx);
        self.link_at_insertion_end(tier, idx);

        if self.index.len() > self.max_entries {
            match self.evict_one() {
                Some((evicted_key, _)) if evicted_key == key => UpsertOutcome::Rejected,
                Some((evicted_key, _)) => UpsertOutcome::Applied {
                    evicted: Some(evicted_key),
                },
                None => UpsertOutcome::Applied { evicted: None },
            }
        } else {
            UpsertOutcome::Applied { evicted: None }
        }
    }

    /// O(1) splice: prepends `prefetch_current`'s whole list onto
    /// `prefetch_previous`'s head, then empties `prefetch_current`. Does
    /// not touch or walk individual slots. No-op if `prefetch_
    /// current` is already empty.
    pub(super) fn start_new_wave(&mut self) {
        let (Some(cur_head), Some(cur_tail)) =
            (self.prefetch_current.head, self.prefetch_current.tail)
        else {
            return;
        };
        match self.prefetch_previous.head {
            Some(prev_head) => {
                self.slots[cur_tail]
                    .as_mut()
                    .expect("prefetch_current tail slot must be occupied")
                    .next = Some(prev_head);
                self.slots[prev_head]
                    .as_mut()
                    .expect("prefetch_previous head slot must be occupied")
                    .prev = Some(cur_tail);
            }
            None => {
                self.prefetch_previous.tail = Some(cur_tail);
            }
        }
        self.prefetch_previous.head = Some(cur_head);
        self.prefetch_current.head = None;
        self.prefetch_current.tail = None;
    }

    /// Unlinks and returns an entry from the highest-priority
    /// non-empty band, checked in this order: `user`'s head,
    /// `visible`'s head, `prefetch_current`'s head, `prefetch_
    /// previous`'s head.
    pub(super) fn pop_highest(&mut self) -> Option<(K, V)> {
        let idx = self
            .user
            .head
            .or(self.visible.head)
            .or(self.prefetch_current.head)
            .or(self.prefetch_previous.head)?;
        Some(self.remove_by_idx(idx))
    }

    /// Unlinks and returns an entry from the lowest-priority
    /// non-empty band, checked in this order: `prefetch_previous`'s
    /// tail, `prefetch_current`'s tail, `visible`'s *head* (the one
    /// exception — see `TieredBounded`'s doc comment), `user`'s
    /// tail.
    fn evict_one(&mut self) -> Option<(K, V)> {
        let idx = self
            .prefetch_previous
            .tail
            .or(self.prefetch_current.tail)
            .or(self.visible.head)
            .or(self.user.tail)?;
        Some(self.remove_by_idx(idx))
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.index.len()
    }

    /// Links `idx` at `tier`'s insertion end — `user`'s head (LIFO),
    /// `visible`'s tail (FIFO), or `prefetch_current`'s tail (FIFO;
    /// `prefetch_previous` is never inserted into directly, per G2).
    fn link_at_insertion_end(&mut self, tier: Tier, idx: usize) {
        match tier {
            Tier::User => link_at_head(&mut self.slots, &mut self.user, idx),
            Tier::Visible => link_at_tail(&mut self.slots, &mut self.visible, idx),
            Tier::Prefetch => link_at_tail(&mut self.slots, &mut self.prefetch_current, idx),
        }
    }

    /// Detaches slot `idx` from whichever band it currently occupies,
    /// without deallocating it — the slot's `key`/`value`/`tier`
    /// remain valid; only its `prev`/`next` links (and, if it was a
    /// boundary slot, its band's `head`/`tail`) are updated. An
    /// interior slot (neither a band's head nor tail) needs no band
    /// lookup at all — only its two neighbors' links change.
    fn unlink(&mut self, idx: usize) {
        let (prev, next, tier) = {
            let s = self.slots[idx]
                .as_ref()
                .expect("unlink: slot must be occupied");
            (s.prev, s.next, s.tier)
        };
        match prev {
            Some(p) => {
                self.slots[p]
                    .as_mut()
                    .expect("prev slot must be occupied")
                    .next = next
            }
            None => self.fix_head_if_matches(tier, idx, next),
        }
        match next {
            Some(n) => {
                self.slots[n]
                    .as_mut()
                    .expect("next slot must be occupied")
                    .prev = prev
            }
            None => self.fix_tail_if_matches(tier, idx, prev),
        }
    }

    /// `unlink`'s head-boundary fixup: `idx` had `prev == None`, so it
    /// was *some* band's head — check each plausible band for `tier`
    /// (at most two, for `Prefetch`) and repoint whichever one matches
    /// to `new_head`.
    fn fix_head_if_matches(&mut self, tier: Tier, idx: usize, new_head: Option<usize>) {
        match tier {
            Tier::User => {
                if self.user.head == Some(idx) {
                    self.user.head = new_head;
                }
            }
            Tier::Visible => {
                if self.visible.head == Some(idx) {
                    self.visible.head = new_head;
                }
            }
            Tier::Prefetch => {
                if self.prefetch_current.head == Some(idx) {
                    self.prefetch_current.head = new_head;
                } else if self.prefetch_previous.head == Some(idx) {
                    self.prefetch_previous.head = new_head;
                }
            }
        }
    }

    /// Symmetric with `fix_head_if_matches`, for the tail boundary.
    fn fix_tail_if_matches(&mut self, tier: Tier, idx: usize, new_tail: Option<usize>) {
        match tier {
            Tier::User => {
                if self.user.tail == Some(idx) {
                    self.user.tail = new_tail;
                }
            }
            Tier::Visible => {
                if self.visible.tail == Some(idx) {
                    self.visible.tail = new_tail;
                }
            }
            Tier::Prefetch => {
                if self.prefetch_current.tail == Some(idx) {
                    self.prefetch_current.tail = new_tail;
                } else if self.prefetch_previous.tail == Some(idx) {
                    self.prefetch_previous.tail = new_tail;
                }
            }
        }
    }

    fn alloc_slot(&mut self, key: K, value: V, tier: Tier) -> usize {
        let slot = Slot {
            key,
            value,
            tier,
            prev: None,
            next: None,
        };
        if let Some(idx) = self.free.pop() {
            self.slots[idx] = Some(slot);
            idx
        } else {
            self.slots.push(Some(slot));
            self.slots.len() - 1
        }
    }

    fn remove_by_idx(&mut self, idx: usize) -> (K, V) {
        self.unlink(idx);
        let slot = self.slots[idx]
            .take()
            .expect("removed slot must be occupied");
        self.index.remove(&slot.key);
        self.free.push(idx);
        (slot.key, slot.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_map(max_entries: usize) -> TieredBounded<i32, i32> {
        TieredBounded::new(max_entries)
    }

    #[test]
    fn user_promotes_over_visible_and_never_demotes() {
        let mut m = new_map(10);
        m.upsert(1, 100, Tier::Visible);
        m.upsert(1, 200, Tier::User);
        assert_eq!(
            m.pop_highest(),
            Some((1, 200)),
            "must be tracked as User now"
        );

        let mut m = new_map(10);
        m.upsert(1, 100, Tier::User);
        m.upsert(1, 200, Tier::Visible);
        // Tier stays User (no demotion); payload updates; still at the
        // User band's head (LIFO) — a second User key pushed after must
        // still pop before it only if pushed *after* this upsert.
        m.upsert(2, 900, Tier::User);
        assert_eq!(m.pop_highest(), Some((2, 900)));
        assert_eq!(
            m.pop_highest(),
            Some((1, 200)),
            "payload from the Visible upsert"
        );
    }

    #[test]
    fn user_pops_lifo_visible_pops_fifo() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::User);
        m.upsert(2, 2, Tier::User);
        // LIFO: most-recently-pushed User key pops first.
        assert_eq!(m.pop_highest(), Some((2, 2)));
        assert_eq!(m.pop_highest(), Some((1, 1)));

        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Visible);
        m.upsert(2, 2, Tier::Visible);
        // FIFO: oldest-pushed Visible key pops first.
        assert_eq!(m.pop_highest(), Some((1, 1)));
        assert_eq!(m.pop_highest(), Some((2, 2)));
    }

    #[test]
    fn pop_highest_drains_user_then_visible_then_prefetch() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Visible);
        m.upsert(3, 3, Tier::User);
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(3));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(1));
    }

    #[test]
    fn visible_evicts_from_its_own_head_not_the_newest() {
        let mut m = new_map(2);
        m.upsert(1, 1, Tier::Visible);
        m.upsert(2, 2, Tier::Visible);
        // At capacity; a third Visible push must evict the oldest (head),
        // not the one just inserted.
        let outcome = m.upsert(3, 3, Tier::Visible);
        assert_eq!(outcome, UpsertOutcome::Applied { evicted: Some(1) });
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(3));
    }

    #[test]
    fn prefetch_single_wave_pops_arrival_order_evicts_newest_first() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        m.upsert(3, 3, Tier::Prefetch);
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(1));

        let mut m = new_map(2);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        // Over capacity: evict_one prefers prefetch_current's tail,
        // which — right after linking — is the entry just inserted
        // itself, since prefetch_previous is empty. This self-eviction
        // is exactly G6's Rejected case, and is the signal a prefetch
        // walk uses to know the queue is saturated and stop.
        let outcome = m.upsert(3, 3, Tier::Prefetch);
        assert_eq!(outcome, UpsertOutcome::Rejected);
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(1));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
    }

    #[test]
    fn prefetch_repush_updates_in_place_without_moving() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        // Re-pushing key 1 must not move it ahead of key 2.
        m.upsert(1, 999, Tier::Prefetch);
        assert_eq!(m.pop_highest(), Some((1, 999)));
        assert_eq!(m.pop_highest(), Some((2, 2)));
    }

    #[test]
    fn prefetch_promotion_relinks_to_target_tier_head() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::User);
        m.upsert(1, 1, Tier::User);
        // Both User now; the more-recently-promoted (key 1) is at the
        // User head, so it pops first (LIFO).
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(1));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
    }

    #[test]
    fn start_new_wave_preserves_order_and_layers_correctly() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        m.start_new_wave();
        assert_eq!(m.len(), 2);
        // Both now served from prefetch_previous, in original order.
        m.upsert(3, 3, Tier::Prefetch);
        m.upsert(4, 4, Tier::Prefetch);
        // Fresh prefetch_current entries are served before any
        // prefetch_previous leftover.
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(3));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(4));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(1));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
    }

    #[test]
    fn start_new_wave_two_resets_layer_without_disturbing_older_order() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        m.start_new_wave(); // previous: [1, 2]
        m.upsert(3, 3, Tier::Prefetch);
        m.start_new_wave(); // previous: [3, 1, 2]
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(3));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(1));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
    }

    #[test]
    fn start_new_wave_evicts_previous_before_current() {
        let mut m = new_map(3);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        m.start_new_wave();
        m.upsert(3, 3, Tier::Prefetch);
        // At capacity (3 entries). A fourth push must evict from
        // prefetch_previous before ever touching prefetch_current.
        let outcome = m.upsert(4, 4, Tier::Prefetch);
        assert_eq!(outcome, UpsertOutcome::Applied { evicted: Some(2) });
    }

    #[test]
    fn repush_of_a_previous_layer_key_stays_in_previous() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.start_new_wave();
        m.upsert(2, 2, Tier::Prefetch);
        // Re-push key 1 (in prefetch_previous) — updates in place,
        // must not jump ahead of key 2 (in prefetch_current).
        m.upsert(1, 111, Tier::Prefetch);
        assert_eq!(m.pop_highest(), Some((2, 2)));
        assert_eq!(m.pop_highest(), Some((1, 111)));
    }

    #[test]
    fn upsert_rejected_only_when_nothing_at_or_below_tier_to_evict() {
        let mut m = new_map(1);
        m.upsert(1, 1, Tier::User);
        // Structure is saturated by a User entry — nothing at or below
        // Prefetch's tier exists to evict, so the new Prefetch entry
        // itself would be evicted: Rejected.
        let outcome = m.upsert(2, 2, Tier::Prefetch);
        assert_eq!(outcome, UpsertOutcome::Rejected);
        assert_eq!(m.len(), 1, "the rejected entry must not be linked in");

        // A same-or-lower-tier victim exists: ordinary churn, not
        // Rejected.
        let outcome = m.upsert(3, 3, Tier::User);
        assert_eq!(outcome, UpsertOutcome::Applied { evicted: Some(1) });
    }

    #[test]
    fn peek_promotes_and_is_a_no_op_reorder_at_same_or_higher_tier() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        assert_eq!(m.peek(&1, Tier::Visible), Some(1));
        // Key 1 promoted to Visible, key 2 stays Prefetch — Visible
        // must now pop before Prefetch.
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(1));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
    }
}
