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
///
/// **One recency rule governs the whole structure** (spec 0208 G4,
/// superseding spec 0164 G2/G3/G5's per-band variations): *the most
/// recently asked query is served first.* Insert at the head, pop at
/// the head (`pop_highest`), evict at the tail (`evict_one`), and
/// asking again — `upsert` *or* `peek`, at the entry's own tier or
/// higher — moves the entry back to that tier's head.
///
/// `prefetch_current` is the one deliberate exception: it inserts at
/// its **tail**, because position in that band encodes distance from
/// the cursor rather than age (`prefetch_step` walks outward), so
/// head-insertion would serve the farthest-away read-ahead first.
/// `prefetch_previous` is never inserted into directly.
///
/// Shared backing store for `HeatRequestQueue` and both of
/// `HeatCaches`' per-range maps — structurally-identical but
/// independently-instantiated structures, not one shared pool (G2).
/// The caches never call `pop_highest`; for them the rule shows up
/// purely as eviction order, which is therefore least-recently-*read*.
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

    /// Promoting read (G9), obeying the same recency rule as `upsert`
    /// (spec 0208 S4d): a read is a query. When the caller asks at
    /// least as urgently as the entry is already ranked, the entry is
    /// retagged to `tier` and relinked to that tier's insertion end.
    ///
    /// `tier >= cur_tier` rather than a strict promotion test, so that
    /// re-reading an entry at its own tier refreshes it. That is what
    /// makes both `HeatCaches` maps evict least-recently-*read* instead
    /// of least-recently-written — they never call `pop_highest`, so
    /// eviction order is the only thing a cached result's band position
    /// decides.
    ///
    /// The same condition subsumes spec 0189 G3/S4's "**re-reading
    /// revives**": a same-tier `Prefetch` read relinks to `prefetch_
    /// current`'s tail, so a cached result sitting in `prefetch_
    /// previous` leaves it rather than staying permanently first in the
    /// eviction line — `start_new_wave` *concatenates* onto that band
    /// and `evict_one` drains its tail first, so without this the
    /// results a wave is actively re-reading would be spent first.
    /// Relinking to the tail (not the head) is right there for the
    /// cache as much as for the queue: `prefetch_step` walks outward
    /// from the cursor, so appending re-reads in wave order keeps the
    /// band sorted near-to-far, which is what makes evicting its tail
    /// evict the farthest result.
    ///
    /// A read at a tier *below* the entry's own is a pure read: a tier
    /// never moves down, and a background re-check must not re-rank an
    /// entry the user asked for.
    pub(super) fn peek(&mut self, key: &K, tier: Tier) -> Option<V> {
        let idx = *self.index.get(key)?;
        let cur_tier = self.slots[idx]
            .as_ref()
            .expect("indexed slot must be occupied")
            .tier;
        if tier >= cur_tier {
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

    /// `tier >= cur_tier` — the caller asked at least as urgently as
    /// the entry is already ranked — decides whether this counts as a
    /// fresh query (spec 0208 S4c, superseding spec 0164 G5's
    /// per-tier rules). When it does, the entry is retagged to `tier`
    /// and relinked to that tier's insertion end; otherwise only the
    /// payload is refreshed and the entry does not move.
    ///
    /// Five cases, three of which the condition merges into one rule:
    /// - Promotion (`tier > cur_tier`): relinks at the new tier.
    /// - Re-ask at the entry's own tier: relinks at its band's
    ///   insertion end. This is spec 0208's change — asking again is
    ///   the most direct evidence available that somebody still wants
    ///   the answer, so it must not be discarded.
    /// - A `Prefetch` re-ask is the same case, and is exactly spec 0189
    ///   G3/S4's "re-asking revives": it relinks to `prefetch_current`'s
    ///   tail *including* when the key lives in `prefetch_previous`,
    ///   which since 0189 the worker discards rather than serves. It
    ///   needs no clause of its own any more.
    /// - A push *below* the entry's tier: payload updates in place, no
    ///   retag and no reorder. Load-bearing — a background `Visible`
    ///   re-check merging into a request the user asked for must not
    ///   re-rank it (spec 0164 G5, and the reason the condition is
    ///   `tier >= cur_tier` rather than "any update relinks").
    /// - Brand-new key: links at its tier's insertion end (new
    ///   `Prefetch` keys always go to `prefetch_current`).
    ///
    /// Whenever this pushes the structure over `max_entries`, evicts
    /// once via `evict_one`. If the *new* entry itself is what gets
    /// evicted (only possible when nothing at or below its tier exists
    /// to evict instead), returns `Rejected` and links nothing.
    pub(super) fn upsert(&mut self, key: K, value: V, tier: Tier) -> UpsertOutcome<K> {
        if let Some(&idx) = self.index.get(&key) {
            let cur_tier = self.slots[idx]
                .as_ref()
                .expect("indexed slot must be occupied")
                .tier;
            if tier >= cur_tier {
                self.unlink(idx);
                {
                    let s = self.slots[idx]
                        .as_mut()
                        .expect("indexed slot must be occupied");
                    s.tier = tier;
                    s.value = value;
                }
                self.link_at_insertion_end(tier, idx);
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
    /// non-empty band's **tail**, checked in this order:
    /// `prefetch_previous`, `prefetch_current`, `visible`, `user`.
    ///
    /// Uniform since spec 0208 S4b. `Visible` used to be evicted from
    /// its *head* instead, because it inserted at its tail and
    /// tail-eviction would then have discarded the entry just pushed
    /// (spec 0164 G2). S4a flipped `Visible` to head-insertion, which
    /// makes its tail the oldest entry again — so evicting there now
    /// means what evicting its head meant before, and the exception is
    /// gone. The two edits are one change: either alone reintroduces
    /// the evict-on-arrival thrash 0164 was avoiding.
    fn evict_one(&mut self) -> Option<(K, V)> {
        let idx = self
            .prefetch_previous
            .tail
            .or(self.prefetch_current.tail)
            .or(self.visible.tail)
            .or(self.user.tail)?;
        Some(self.remove_by_idx(idx))
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.index.len()
    }

    /// Links `idx` at `tier`'s insertion end — `user`'s head and (since
    /// spec 0208 S4a) `visible`'s head, both LIFO; or `prefetch_
    /// current`'s tail, the one band whose order is distance-from-
    /// cursor rather than age (`prefetch_previous` is never inserted
    /// into directly, per G2).
    fn link_at_insertion_end(&mut self, tier: Tier, idx: usize) {
        match tier {
            Tier::User => link_at_head(&mut self.slots, &mut self.user, idx),
            Tier::Visible => link_at_head(&mut self.slots, &mut self.visible, idx),
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
        // Tier stays User (no demotion); payload updates; and the entry
        // does not move, so a second User key pushed after it still pops
        // first (spec 0208 S4c's last row).
        m.upsert(2, 900, Tier::User);
        assert_eq!(m.pop_highest(), Some((2, 900)));
        assert_eq!(
            m.pop_highest(),
            Some((1, 200)),
            "payload from the Visible upsert"
        );
    }

    /// Spec 0208 S4a/G4: both `User` and `Visible` are LIFO — the most
    /// recently asked query is served first. `Visible` used to be FIFO.
    #[test]
    fn user_and_visible_both_pop_lifo() {
        for tier in [Tier::User, Tier::Visible] {
            let mut m = new_map(10);
            m.upsert(1, 1, tier);
            m.upsert(2, 2, tier);
            assert_eq!(m.pop_highest(), Some((2, 2)), "{tier:?}");
            assert_eq!(m.pop_highest(), Some((1, 1)), "{tier:?}");
        }
    }

    /// Spec 0208 S4c: asking again at an entry's own tier moves it back
    /// to that tier's head. Both bands, since both changed.
    #[test]
    fn a_same_tier_reask_moves_the_entry_to_its_bands_head() {
        for tier in [Tier::User, Tier::Visible] {
            let mut m = new_map(10);
            m.upsert(1, 1, tier);
            m.upsert(2, 2, tier);
            m.upsert(1, 111, tier);
            assert_eq!(
                m.pop_highest(),
                Some((1, 111)),
                "the re-asked key must be served first ({tier:?})"
            );
            assert_eq!(m.pop_highest(), Some((2, 2)), "{tier:?}");
        }
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

    /// Spec 0208 S4b: `Visible` evicts from its **tail**, like every
    /// other band. Under head-insertion the tail is the oldest entry, so
    /// this is what evicting its head meant before S4a — what must not
    /// happen is evicting the entry just inserted (spec 0164 G2's
    /// evict-on-arrival thrash, which S4a would have reintroduced had
    /// eviction stayed at the head).
    #[test]
    fn visible_evicts_its_oldest_not_the_newest() {
        let mut m = new_map(2);
        m.upsert(1, 1, Tier::Visible);
        m.upsert(2, 2, Tier::Visible);
        let outcome = m.upsert(3, 3, Tier::Visible);
        assert_eq!(outcome, UpsertOutcome::Applied { evicted: Some(1) });
        // LIFO among the survivors.
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(3));
        assert_eq!(m.pop_highest().map(|(k, _)| k), Some(2));
    }

    /// Spec 0208 S4d/G5: re-reading at the entry's own tier refreshes
    /// it, so a cache evicts least-recently-*read* rather than
    /// least-recently-written. `peek` is the only way the two
    /// `HeatCaches` maps ever touch band order — they never pop.
    #[test]
    fn a_same_tier_peek_refreshes_against_eviction() {
        let mut m = new_map(2);
        m.upsert(1, 1, Tier::Visible);
        m.upsert(2, 2, Tier::Visible);
        // Key 1 is the older write, so it is next in the eviction line —
        // until it is read.
        assert_eq!(m.peek(&1, Tier::Visible), Some(1));
        let outcome = m.upsert(3, 3, Tier::Visible);
        assert_eq!(
            outcome,
            UpsertOutcome::Applied { evicted: Some(2) },
            "the entry that was not re-read must be spent"
        );
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
