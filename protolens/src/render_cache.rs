// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Render cache: `(range, type) -> (text, spans, style hints)` (spec 0116
//! §8) — a byte-bounded MRU cache, structurally identical to
//! `override_pane::CandidateCache` (spec 0114 §6).

use std::ops::Range;

use prototext_core::serialize::render_text::NodeSpan;

use crate::colorize::StyleHint;

/// Key: the same `payload_range` `apply_override` already computes via
/// `extract::message_payload_range`, plus the type it was rendered under
/// (`None` = raw/schema-less override) — the exact two inputs that
/// determine `decode_and_render_indexed`'s output. The synthetic
/// wrapper's field name is deliberately *not* part of this key (spec
/// 0135 §G1/§G2): the wrapper's field name is now always the fixed
/// placeholder `"_"`, and the real display name is patched in as a
/// post-render substring replacement, so the cached render itself is
/// field-name-invariant.
///
/// The trailing `bool` is `splice_override`'s own `is_preview` (spec
/// 0163) — a live preview may be truncated by `OVERRIDE_SPLICE_NODE_
/// BUDGET`, while a confirmed override always renders completely; these
/// must be cached separately, or confirming an override could silently
/// reuse a truncated preview render of the same `(range, type)`.
type RenderKey = (Range<usize>, Option<String>, bool);

/// Value: everything `apply_override` derives from a fresh
/// `decode_and_render_indexed` call plus the colorize pass (§7) — a
/// cache hit skips *both* passes.
type RenderValue = (Vec<String>, Vec<NodeSpan>, Vec<StyleHint>);

/// Approximate heap footprint of a cached render, for `RenderCache`'s
/// byte budget — rendered lines' string bytes plus `new_spans.len() *
/// size_of::<NodeSpan>()` plus `style_hints.len() *
/// size_of::<StyleHint>()`, deliberately approximate (same
/// "not correctness-sensitive" caveat as `CandidateCache::
/// candidates_bytes`'s doc comment).
fn render_bytes(value: &RenderValue) -> usize {
    let (lines, spans, hints) = value;
    let lines_bytes: usize = lines.iter().map(String::len).sum();
    lines_bytes
        + spans.len() * std::mem::size_of::<NodeSpan>()
        + hints.len() * std::mem::size_of::<StyleHint>()
}

/// Session-scoped, byte-bounded MRU cache of `(range, type) -> (lines,
/// spans, style hints)` renders (spec 0116 §8/Goal 10) — no invalidation
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
    pub fn insert(&mut self, key: RenderKey, value: RenderValue) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
            let (_, old) = self.entries.remove(pos);
            self.total_bytes -= render_bytes(&old);
        }
        self.total_bytes += render_bytes(&value);
        self.entries.push((key, value));
        // Always keep at least the entry just inserted, even if it alone
        // exceeds the budget.
        while self.total_bytes > self.max_bytes && self.entries.len() > 1 {
            let (_, evicted) = self.entries.remove(0);
            self.total_bytes -= render_bytes(&evicted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colorize::SyntaxRole;

    fn value(line: &str) -> RenderValue {
        (vec![line.to_string()], Vec::new(), Vec::new())
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

    #[test]
    fn render_cache_keeps_oversized_entry_alone() {
        let mut cache = RenderCache::new(1);
        cache.insert((0..10, None, false), value("way too big for the budget"));
        assert!(cache.get(&(0..10, None, false)).is_some());
    }

    #[test]
    fn render_cache_stores_style_hints() {
        let mut cache = RenderCache::new(1_000_000);
        let hints = vec![StyleHint {
            range: 0..4,
            role: SyntaxRole::Attribute,
        }];
        cache.insert(
            (0..10, None, false),
            (vec!["flag".to_string()], Vec::new(), hints.clone()),
        );
        let (_, _, cached_hints) = cache.get(&(0..10, None, false)).unwrap();
        assert_eq!(cached_hints, hints);
    }
}
