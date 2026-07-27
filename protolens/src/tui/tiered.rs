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
///
/// Why `Prefetch` alone gets two bands, when the obvious spelling
/// would be a fourth `Tier` below it (spec 0189 N1): `Slot` stores a
/// `Tier`, so making the band *be* the tier would turn `start_new_wave`
/// from an O(1) pointer splice into a rewrite of every slot in the
/// wave — on the UI thread, in `App::prefetch_step`. The O(1) restart
/// is the entire justification for the subtlety, and it is what pays
/// for the `else if` fallbacks in `fix_head_if_matches` /
/// `fix_tail_if_matches`, which exist solely because one tier maps to
/// two bands.
///
/// Spec 0189: a superseded entry is never *served*. `pop_highest`
/// stops at `prefetch_current`; `prefetch_previous` is drained only by
/// `discard_one_superseded` or by `evict_one`. Owners whose entries are
/// computed results (both `HeatCaches` maps) are unaffected, because
/// they never pop at all — they read through `peek`, for which the two
/// bands are indistinguishable.
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
    /// `tier`, bumps it to `tier`, relinking to the new tier's
    /// insertion end, before returning the value.
    ///
    /// A same-tier `Prefetch` read relinks too, to `prefetch_current`'s
    /// tail — **re-reading revives**, exactly as re-asking does in
    /// `upsert` (spec 0189 G3/S4). Without it a cached result sitting in
    /// `prefetch_previous` stays there no matter how often the live wave
    /// reads it, and since `start_new_wave` *concatenates* onto that band
    /// and `evict_one` drains its tail first, the results a wave is
    /// actively re-reading would be permanently first in the eviction
    /// line. 0189 established this rule for the queue's write path and
    /// this read path was overlooked.
    ///
    /// Relinking to the *tail* is right for the cache as well as the
    /// queue, even though the tail is `evict_one`'s first target within
    /// the band: `prefetch_step` walks outward from the cursor, so
    /// position in `prefetch_current` encodes distance from the cursor,
    /// not age. Appending re-reads in wave order keeps the band sorted
    /// near-to-far, which is exactly what makes evicting its tail evict
    /// the farthest result.
    ///
    /// Any other same-tier read is a no-op reorder, as is a read at a
    /// tier below the entry's own — a tier never moves down.
    pub(super) fn peek(&mut self, key: &K, tier: Tier) -> Option<V> {
        let idx = *self.index.get(key)?;
        let cur_tier = self.slots[idx]
            .as_ref()
            .expect("indexed slot must be occupied")
            .tier;
        let new_tier = cur_tier.max(tier);
        if new_tier > cur_tier || new_tier == Tier::Prefetch {
            self.unlink(idx);
            self.slots[idx]
                .as_mut()
                .expect("indexed slot must be occupied")
                .tier = new_tier;
            self.link_at_insertion_end(new_tier, idx);
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
    /// - Any update landing at `Prefetch`: relinks to `prefetch_
    ///   current`'s tail, *including* when the key currently lives in
    ///   `prefetch_previous`. Re-asking revives (spec 0189 G3/S4).
    ///   This amends spec 0164 G5, which refreshed such an entry in
    ///   place: since 0189 the worker discards superseded entries
    ///   rather than serving them, so leaving a re-asked range in
    ///   `prefetch_previous` would throw away a request the live wave
    ///   is asking for. The price is that a re-push of a key already
    ///   in `prefetch_current` also goes to that band's tail instead
    ///   of holding its FIFO position — a case that does not arise,
    ///   since `prefetch_step` visits each row once per wave and the
    ///   other push site pushes at `Tier::Visible`.
    /// - Same-tier update at `User`/`Visible`: payload updates in
    ///   place, no reordering.
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
            if new_tier > cur_tier || new_tier == Tier::Prefetch {
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

    /// Spec 0190 S1: which bands hold work worth reporting, as a
    /// bitmask — bit 0 `user`, bit 1 `visible`, bit 2
    /// `prefetch_current`. O(1): three head-pointer tests, no
    /// traversal and no counting.
    ///
    /// `prefetch_previous` is deliberately excluded (spec 0189 S6).
    /// Since 0189 those entries are destined to be discarded, not
    /// scored, so counting them would light the activity dot for a
    /// subsystem that has nothing left to do for the user.
    pub(super) fn band_occupancy(&self) -> u8 {
        u8::from(self.user.head.is_some())
            | (u8::from(self.visible.head.is_some()) << 1)
            | (u8::from(self.prefetch_current.head.is_some()) << 2)
    }

    /// Reclaims one superseded entry — `prefetch_previous`'s head —
    /// and reports whether there was one (spec 0189 S2).
    ///
    /// One entry per call rather than a drain, on purpose: the caller
    /// holds a lock, and draining a whole wave under it would block a
    /// `push` for as long as the wave is (G4). `pop_blocking` releases
    /// and retakes the mutex around each call.
    pub(super) fn discard_one_superseded(&mut self) -> bool {
        let Some(idx) = self.prefetch_previous.head else {
            return false;
        };
        self.remove_by_idx(idx);
        true
    }

    /// Unlinks and returns an entry from the highest-priority
    /// non-empty band, checked in this order: `user`'s head,
    /// `visible`'s head, `prefetch_current`'s head.
    ///
    /// `prefetch_previous` is **not** served (spec 0189 S1): a
    /// superseded request is an unpaid `score_all` on a range ranked
    /// from an origin the cursor has left, so it leaves this structure
    /// only via `discard_one_superseded` or `evict_one` — never by
    /// being handed to a caller that would compute it.
    pub(super) fn pop_highest(&mut self) -> Option<(K, V)> {
        let idx = self
            .user
            .head
            .or(self.visible.head)
            .or(self.prefetch_current.head)?;
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

    /// Spec 0189 S4, amending spec 0164 G5: a `Prefetch` re-push
    /// relinks to `prefetch_current`'s tail rather than updating in
    /// place. Pinned rather than merely deleted, because the amendment
    /// is deliberate — the acknowledged price of "re-asking revives".
    #[test]
    fn prefetch_repush_relinks_to_the_current_tail() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        m.upsert(1, 999, Tier::Prefetch);
        assert_eq!(
            m.pop_highest(),
            Some((2, 2)),
            "the re-pushed key moves behind the untouched one"
        );
        assert_eq!(m.pop_highest(), Some((1, 999)));
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

    /// Spec 0189 S1: superseding a wave takes it out of service. The
    /// entries are still *there* — `len` still counts them, and
    /// `evict_one` will spend them — but `pop_highest` will not hand
    /// them to a caller that would score them.
    #[test]
    fn pop_highest_never_serves_a_superseded_wave() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        m.start_new_wave();
        m.upsert(3, 3, Tier::Prefetch);
        m.upsert(4, 4, Tier::Prefetch);

        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(3));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(4));
        assert_eq!(
            m.pop_highest(),
            None,
            "the superseded wave must not be served"
        );
        assert_eq!(m.len(), 2, "but it is still occupying the structure");
    }

    /// Spec 0164 G7: successive restarts layer onto
    /// `prefetch_previous`'s head, so the *oldest* wave stays at its
    /// tail and is spent first. Observed through `evict_one` now that
    /// `pop_highest` no longer reaches the band (spec 0189 S1).
    #[test]
    fn start_new_wave_two_resets_layer_without_disturbing_older_order() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        m.start_new_wave(); // previous: [1, 2]
        m.upsert(3, 3, Tier::Prefetch);
        m.start_new_wave(); // previous: [3, 1, 2]
        assert_eq!(m.evict_one().map(|(k, _)| k), Some(2));
        assert_eq!(m.evict_one().map(|(k, _)| k), Some(1));
        assert_eq!(m.evict_one().map(|(k, _)| k), Some(3));
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

    /// Spec 0190 S1 with spec 0189 S6: one bit per tier, and the
    /// prefetch bit tracks `prefetch_current` alone — superseded
    /// entries are about to be discarded, so reporting them would show
    /// the activity dot as busy for work nobody will do.
    #[test]
    fn band_occupancy_reports_one_bit_per_tier() {
        let mut m = new_map(10);
        assert_eq!(m.band_occupancy(), 0b000, "empty");

        m.upsert(1, 1, Tier::Prefetch);
        assert_eq!(m.band_occupancy(), 0b100, "prefetch_current alone");

        m.start_new_wave();
        assert_eq!(
            m.band_occupancy(),
            0b000,
            "a superseded wave is not reportable activity"
        );

        m.upsert(2, 2, Tier::Visible);
        assert_eq!(m.band_occupancy(), 0b010);
        m.upsert(3, 3, Tier::User);
        assert_eq!(m.band_occupancy(), 0b011);

        m.pop_highest(); // the User entry
        assert_eq!(m.band_occupancy(), 0b010);
        m.pop_highest(); // the Visible entry
        assert_eq!(m.band_occupancy(), 0b000);
    }

    /// Spec 0189 S2: one entry per call, taken from `prefetch_
    /// previous`'s head, and nothing else is touched — it is not a
    /// "clear all prefetch" shortcut.
    #[test]
    fn discard_one_superseded_takes_exactly_one_from_the_previous_band() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        m.start_new_wave(); // previous: [1, 2]
        m.upsert(3, 3, Tier::Prefetch); // current: [3]
        m.upsert(4, 4, Tier::Visible);
        m.upsert(5, 5, Tier::User);

        assert!(m.discard_one_superseded());
        assert_eq!(m.len(), 4, "exactly one entry goes");
        assert!(m.discard_one_superseded());
        assert_eq!(m.len(), 3);
        assert!(
            !m.discard_one_superseded(),
            "an empty previous band reports nothing to reclaim"
        );

        // The live bands are untouched, in their usual order.
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(5));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(4));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(3));
        assert_eq!(m.pop_highest(), None);
    }

    /// Spec 0189 S2: the slot is genuinely reclaimed, not merely
    /// unlinked — an upsert that would have been `Rejected` at
    /// saturation becomes `Applied` once the superseded entry is gone.
    #[test]
    fn discard_one_superseded_reclaims_the_slot() {
        let mut m = new_map(2);
        m.upsert(1, 1, Tier::User);
        m.upsert(2, 2, Tier::Prefetch);
        m.start_new_wave();
        assert_eq!(m.len(), 2);
        assert!(m.discard_one_superseded());
        assert_eq!(m.len(), 1);
        assert_eq!(
            m.upsert(3, 3, Tier::Prefetch),
            UpsertOutcome::Applied { evicted: None },
            "the freed slot must be reusable without evicting the User entry"
        );
    }

    /// Spec 0189 G3/S4: re-asking revives. A `Prefetch` upsert on a key
    /// sitting in `prefetch_previous` moves it into `prefetch_current`,
    /// so a range the live wave wants is served rather than discarded.
    /// Without this, `pop_highest`'s refusal to serve the superseded
    /// band would silently drop it.
    #[test]
    fn a_prefetch_repush_revives_a_superseded_key() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.start_new_wave();
        m.upsert(2, 2, Tier::Prefetch);
        m.upsert(1, 111, Tier::Prefetch);

        assert_eq!(m.pop_highest(), Some((2, 2)));
        assert_eq!(
            m.pop_highest(),
            Some((1, 111)),
            "the revived key must be served, not stranded"
        );
        assert!(
            !m.discard_one_superseded(),
            "and it must have left the superseded band"
        );
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
    fn peek_promotes_to_a_higher_tier() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.upsert(2, 2, Tier::Prefetch);
        assert_eq!(m.peek(&1, Tier::Visible), Some(1));
        // Key 1 promoted to Visible, key 2 stays Prefetch — Visible
        // must now pop before Prefetch.
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(1));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
    }

    #[test]
    fn a_prefetch_peek_revives_a_superseded_key() {
        let mut m = new_map(10);
        m.upsert(1, 1, Tier::Prefetch);
        m.start_new_wave();
        m.upsert(2, 2, Tier::Prefetch);
        assert_eq!(m.peek(&1, Tier::Prefetch), Some(1));

        // Re-reading revives, exactly as re-asking does: key 1 has left
        // the superseded band and is now the current wave's newest.
        assert_eq!(m.pop_highest(), Some((2, 2)));
        assert_eq!(
            m.pop_highest(),
            Some((1, 1)),
            "the re-read key must be served, not stranded"
        );
        assert!(
            !m.discard_one_superseded(),
            "and it must have left the superseded band"
        );
    }
}
