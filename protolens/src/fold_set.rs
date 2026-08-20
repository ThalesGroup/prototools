// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0323 S1: fold state as one bit per arena slot.
//!
//! The two sets this replaces — `App::user_folded` and
//! `App::auto_folded` —
//! were `HashSet<usize>`, chosen when a fold was a rare thing a reader
//! did by hand. Neither premise survived:
//!
//! - `auto_folded`'s capacity peaks near **84 000 mid-bake** without
//!   shrinking, which is what forced `bake_queue` into existence
//!   (`HashSet::iter().next()` scans buckets from the top) and what made
//!   `HashSet::retain` unusable in the splice's fold scrub (retaining
//!   unconditionally took a bake drain from 5.4 s to 17.5 s — spec 0338
//!   G1 has since removed the scrub itself).
//! - Probing a set once per slot measured **5.0% of a startup** on
//!   googleapis, so `rebuild_status` used to flatten it into a bitset —
//!   allocated and filled on every call — before its reverse pass. A
//!   faster hasher was tried and came out 3.3% slower on 3.7% fewer
//!   instructions: the probe is the cost, not the hash.
//! - Spec 0323 folds every bracketed node by default, so as the bake
//!   completes the "sparse" set holds every bracketed slot in the arena
//!   — 4.74 M on googleapis.
//!
//! One bit per slot is 594 KB there, allocated once because the arena is
//! immutable (spec 0216), and read in the order the callers already walk.

/// A set of arena slots, as a bitmap.
///
/// `len` is maintained rather than popcounted because two callers ask
/// for it on a hot path and neither wants a scan: the bake's idle check
/// (`is_empty`) runs per frame, and spec 0249 S13's "N subtrees not yet
/// baked" needs the count itself.
#[derive(Debug, Default, Clone)]
pub struct FoldSet {
    words: Box<[u64]>,
    len: usize,
}

impl FoldSet {
    /// Room for `slots` members, none set.
    pub fn new(slots: usize) -> Self {
        FoldSet {
            words: vec![0u64; slots.div_ceil(64)].into_boxed_slice(),
            len: 0,
        }
    }

    /// Room for `slots` members, **all** set.
    ///
    /// Spec 0338 S1: this is how "a document opens closed" is said in
    /// one line. The version that shipped first said it one bit at a
    /// time — a walk of the whole arena testing each slot for
    /// foldability — which cost 227 MiB of reads and 115.6 M
    /// instructions on `googleapis.desc` to arrive at a value the
    /// constructor can simply start from. A default belongs in an
    /// initial value, not in a loop that writes the default out.
    ///
    /// The set is deliberately *wider* than the foldable slots: a leaf
    /// is a member too, since excluding leaves is the only thing that
    /// walk was still doing. That is sound because the read gates on
    /// foldability — see `App::is_folded` — so a leaf's bit is never
    /// consulted.
    ///
    /// The tail past `slots` is masked off. Nothing indexes there, but
    /// `len` and `iter` would otherwise report members the arena has no
    /// slot for, and `iter` is how a test compares two sets.
    pub fn full(slots: usize) -> Self {
        let mut words = vec![u64::MAX; slots.div_ceil(64)].into_boxed_slice();
        let used = slots % 64;
        if used != 0 {
            if let Some(last) = words.last_mut() {
                *last = (1u64 << used) - 1;
            }
        }
        FoldSet { words, len: slots }
    }

    #[inline]
    fn split(idx: usize) -> (usize, u64) {
        (idx / 64, 1u64 << (idx % 64))
    }

    #[inline]
    pub fn contains(&self, idx: usize) -> bool {
        let (w, bit) = Self::split(idx);
        self.words.get(w).is_some_and(|word| word & bit != 0)
    }

    /// Adds `idx`, reporting whether it was absent — `HashSet::insert`'s
    /// contract, because every call site tests it to decide whether the
    /// document moved.
    #[inline]
    pub fn insert(&mut self, idx: usize) -> bool {
        let (w, bit) = Self::split(idx);
        let word = &mut self.words[w];
        let absent = *word & bit == 0;
        *word |= bit;
        self.len += usize::from(absent);
        absent
    }

    /// Removes `idx`, reporting whether it was present.
    #[inline]
    pub fn remove(&mut self, idx: usize) -> bool {
        let (w, bit) = Self::split(idx);
        let Some(word) = self.words.get_mut(w) else {
            return false;
        };
        let present = *word & bit != 0;
        *word &= !bit;
        self.len -= usize::from(present);
        present
    }

    /// Empties the set, keeping the room. One memset over 594 KB at the
    /// googleapis scale, against the walk of the arena the callers would
    /// otherwise write.
    ///
    /// Tests only, and that is the honest scope rather than an
    /// allowance: nothing in the running program ever wants *every*
    /// fold dropped at once. `App::open` clears one bit, and
    /// `script_reset_folds` walks deepest-first because each clear has
    /// to move the line counters with it. A fixture resetting between
    /// phases has no
    /// counters to keep honest yet, which is the one case this serves.
    #[cfg(test)]
    pub fn clear(&mut self) {
        self.words.fill(0);
        self.len = 0;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The raw word holding slots `64w .. 64w+64`, for a caller sweeping
    /// the arena in slot order — `rebuild_status` is the one, and taking
    /// the word once per 64 slots is what makes its stop test free.
    #[inline]
    pub fn word(&self, w: usize) -> u64 {
        self.words[w]
    }

    /// The members, ascending — which is arena order, and since spec
    /// 0216 that is level order: a parent before any child, siblings
    /// contiguous. Zero words are skipped whole, so a nearly empty set
    /// over a large arena costs one read per 64 slots and no branch per
    /// member.
    ///
    /// It is *not* document order. `search_segments` wants the order a
    /// reader meets the stops in and sorts by `raw_start` for it.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words
            .iter()
            .enumerate()
            .filter(|(_, &word)| word != 0)
            .flat_map(|(w, &word)| {
                // Every value the chain carries is nonzero — the filter
                // above guarantees the seed, and the step stops rather
                // than yield zero. `successors` computes the next item
                // eagerly, so a `take_while` on zero would still have
                // evaluated `0 - 1`.
                std::iter::successors(Some(word), |rest| {
                    let cleared = rest & (rest - 1);
                    (cleared != 0).then_some(cleared)
                })
                .map(move |rest| w * 64 + rest.trailing_zeros() as usize)
            })
    }
}

/// Membership, not representation: two sets sized for different arenas
/// are equal when they hold the same slots. Tests compare a `FoldSet`
/// against a snapshot of itself to assert a keypress moved no fold.
impl PartialEq for FoldSet {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl Eq for FoldSet {}

#[cfg(test)]
mod tests {
    use super::FoldSet;

    #[test]
    fn fold_set_round_trips() {
        let mut s = FoldSet::new(200);
        assert!(s.is_empty());
        assert!(s.insert(0));
        assert!(!s.insert(0), "a second insert does not move the count");
        assert!(s.insert(63));
        assert!(s.insert(64));
        assert!(s.insert(199), "the last slot of a partial final word");
        assert_eq!(s.len(), 4);
        assert!(s.contains(0) && s.contains(199));
        assert!(!s.contains(1));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0, 63, 64, 199]);

        assert!(s.remove(64));
        assert!(!s.remove(64), "a second remove does not move the count");
        assert_eq!(s.len(), 3);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0, 63, 199]);

        for i in [0, 63, 199] {
            assert!(s.remove(i));
        }
        assert!(s.is_empty());
        assert_eq!(s.iter().next(), None);
    }

    #[test]
    fn an_out_of_range_slot_is_absent_rather_than_a_panic() {
        // `contains`/`remove` are asked about vacated slots during a
        // splice, and a `FoldSet::default()` (no words at all) is what a
        // test `App` starts from.
        let mut s = FoldSet::default();
        assert!(!s.contains(7));
        assert!(!s.remove(7));
        assert!(s.is_empty());
    }
}
