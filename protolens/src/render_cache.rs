// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Render cache: `(range, type) -> (text, spans)` (spec 0116 §8) — a
//! byte-bounded MRU cache, and the only one in the crate.
//!
//! It used to have a twin in `override_pane`, which is why an older note
//! may ask whether the two should share a generic. They should not,
//! because there is no second one: `tiered::TieredBounded` is bounded by
//! entry count rather than by bytes, has four tiers, and does not promote
//! on a low-tier read.

use std::ops::Range;

use prototext_core::serialize::render_text::NodeSpan;

/// Key: the same `payload_range` `apply_override` already computes via
/// `extract::message_payload_range`, plus the type it was rendered under
/// (`None` = raw/schema-less override) — the exact two inputs that
/// determine `decode_and_render_indexed`'s output. The synthetic
/// wrapper's field name is deliberately *not* part of this key (spec
/// 0135 §G1/§G2): the wrapper's field name is always the fixed
/// placeholder `"_"`, and the real display name is patched in as a
/// post-render substring replacement, so the cached render itself is
/// field-name-invariant.
///
/// The trailing `bool` is `splice_override`'s own `is_preview` (spec
/// 0174) — a live preview is rendered from at most
/// `override_preview_byte_budget` interior bytes, i.e. literally not the
/// same input as the confirmed render, which always renders completely;
/// these must be cached separately, or confirming an override could
/// silently reuse a truncated preview render of the same `(range, type)`.
type RenderKey = (Range<usize>, Option<String>, bool);

/// Value: everything `apply_override` derives from a fresh
/// `decode_and_render_indexed` call. Spec 0187 S4: highlighting is
/// recomputed per frame over the on-screen window only, so there is
/// nothing render-scoped left to cache.
type RenderValue = (Vec<String>, Vec<NodeSpan>);

/// Approximate heap footprint of a cached render, for `RenderCache`'s
/// byte budget — rendered lines' string bytes plus `new_spans.len() *
/// size_of::<NodeSpan>()`.
///
/// Deliberately approximate: it ignores each `String`'s and `Vec`'s own
/// spare capacity and header. The number bounds a cache, so undercounting
/// costs memory and never correctness.
fn render_bytes(value: &RenderValue) -> usize {
    let (lines, spans) = value;
    let lines_bytes: usize = lines.iter().map(String::len).sum();
    lines_bytes + spans.len() * std::mem::size_of::<NodeSpan>()
}

/// Session-scoped, byte-bounded MRU cache of `(range, type) -> (lines,
/// spans)` renders (spec 0116 §8/Goal 10) — no invalidation
/// beyond ordinary MRU eviction needed, since a cached entry's key is
/// tied to immutable input (`App::blob`'s bytes never change once a
/// document is loaded).
pub struct RenderCache {
    /// Most-recently-used entry at the back; least-recently-used (next
    /// to evict) at the front.
    entries: Vec<(RenderKey, RenderValue)>,
    total_bytes: usize,
    max_bytes: usize,
}

impl RenderCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            total_bytes: 0,
            max_bytes,
        }
    }

    /// Look up `key`'s cached render, promoting it to most-recently-used
    /// on a hit.
    pub fn get(&mut self, key: &RenderKey) -> Option<RenderValue> {
        let pos = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(pos);
        let result = entry.1.clone();
        self.entries.push(entry);
        Some(result)
    }

    /// Insert (or replace) `key`'s cached render, evicting
    /// least-recently-used entries until back under the byte budget.
    ///
    /// An entry that alone exceeds the whole budget is **rejected**
    /// (spec 0251 S7). Keeping it — which is what this used to do, under
    /// an `entries.len() > 1` floor on the eviction loop — meant one
    /// large render evicted every other entry and then sat alone, so the
    /// cache degenerated to a single entry precisely when hits were
    /// wanted. Re-rendering one thing is cheaper than losing everything
    /// else.
    pub fn insert(&mut self, key: RenderKey, value: RenderValue) {
        let value_bytes = render_bytes(&value);
        if value_bytes > self.max_bytes {
            return;
        }
        if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
            let (_, old) = self.entries.remove(pos);
            self.total_bytes -= render_bytes(&old);
        }
        self.total_bytes += value_bytes;
        self.entries.push((key, value));
        // Terminates without a length floor: the entry just pushed fits
        // the budget on its own, so the loop stops at the latest when it
        // is the only one left.
        while self.total_bytes > self.max_bytes {
            let (_, evicted) = self.entries.remove(0);
            self.total_bytes -= render_bytes(&evicted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(line: &str) -> RenderValue {
        (vec![line.to_string()], Vec::new())
    }

    #[test]
    fn render_cache_hit_promotes_to_most_recently_used() {
        let mut cache = RenderCache::new(1_000_000);
        cache.insert((0..10, None, false), value("a"));
        cache.insert((10..20, Some("pkg.A".to_string()), false), value("b"));
        assert!(cache.get(&(0..10, None, false)).is_some());
        assert!(cache
            .get(&(10..20, Some("pkg.A".to_string()), false))
            .is_some());
        assert!(cache.get(&(20..30, None, false)).is_none());
    }

    #[test]
    fn render_cache_keeps_preview_and_confirmed_renders_separate() {
        let mut cache = RenderCache::new(1_000_000);
        cache.insert((0..10, None, true), value("truncated preview"));
        assert!(cache.get(&(0..10, None, false)).is_none());
        cache.insert((0..10, None, false), value("full"));
        assert!(cache.get(&(0..10, None, true)).is_some());
        assert!(cache.get(&(0..10, None, false)).is_some());
    }

    #[test]
    fn render_cache_evicts_least_recently_used_past_byte_budget() {
        // Each entry costs len("a") = 1 byte; budget of 2 fits exactly
        // two entries at a time.
        let mut cache = RenderCache::new(2);
        cache.insert((0..10, None, false), value("a"));
        cache.insert((10..20, None, false), value("b"));
        cache.insert((20..30, None, false), value("c"));
        // First insert (0..10) should have been evicted.
        assert!(cache.get(&(0..10, None, false)).is_none());
        assert!(cache.get(&(10..20, None, false)).is_some());
        assert!(cache.get(&(20..30, None, false)).is_some());
    }

    /// Spec 0251 S7. The point is the *second* assertion: an entry too
    /// big to cache must not take the rest of the cache down with it.
    #[test]
    fn an_oversized_entry_evicts_nothing() {
        let mut cache = RenderCache::new(2);
        cache.insert((0..10, None, false), value("a"));
        cache.insert((10..20, None, false), value("b"));

        cache.insert((20..30, None, false), value("way too big for the budget"));

        assert!(cache.get(&(20..30, None, false)).is_none());
        assert!(cache.get(&(0..10, None, false)).is_some());
        assert!(cache.get(&(10..20, None, false)).is_some());
    }
}
