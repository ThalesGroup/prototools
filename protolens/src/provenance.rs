// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Interned render provenance for `TreeNode::rendered_as` (spec 0213).
//!
//! Held inline, a provenance is an
//! `Option<(Option<Option<String>>, String)>`: 48 bytes plus up to two
//! heap allocations, for the largest field in the node slot and the one
//! least likely to be occupied. On `googleapis.desc` it is empty on
//! 4 499 336 of 4 501 014 nodes — 48 bytes paid 4.5 million times to say
//! "nothing here yet", and paid again for each further copy of the arena
//! a commit holds at its peak. So the value is stored once here and the
//! node holds a 4-byte [`ProvenanceId`] into this table.
//!
//! The *pair* is interned rather than its two halves separately: the type
//! half needs three values that are not a type name — no override,
//! explicit raw, and never rendered — and `FqdnId` has no third sentinel
//! to spare, while the set of distinct pairs is bounded by the overrides
//! in play rather than by nodes.

use std::collections::HashMap;

/// What one node's rendering came from: which override produced the text
/// currently on screen, and under what field name it was rendered.
///
/// None of the nesting is redundant — see
/// `docs/protolens/design/document-tree.md`:
///
/// - `(None, name)` — rendered with no active override at all; the type
///   came from `natural_type`.
/// - `(Some(None), name)` — rendered raw, because an active entry
///   explicitly says raw. Distinguishing this from the case above is what
///   detects a *demotion*.
/// - `(Some(Some(t)), name)` — rendered as type `t`.
///
/// The field name is the second half because a rename (spec 0119 G4)
/// changes the rendered text without changing the type.
pub type Provenance = (Option<Option<String>>, String);

/// An index into a [`ProvenanceTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProvenanceId(u32);

/// The never-rendered sentinel.
///
/// `u32::MAX` rather than `0`, so that ids are plain indices and the table
/// needs no reserved first entry.
pub const NOT_RENDERED: ProvenanceId = ProvenanceId(u32::MAX);

/// The distinct provenances any render pass has referred to, each stored
/// exactly once.
///
/// One per session, on `App`. Unlike spec 0212's `FqdnTable` it never has
/// to be handed to anything: a freshly built node — whether from the
/// initial decode or from a splice's local tree — is always
/// [`NOT_RENDERED`], so `build_tree` never interns, and the table can stay
/// private to the `App` that owns the arena.
///
/// It grows monotonically with distinct provenances. That is bounded by
/// (override targets in play) × (field names under them), which is orders
/// of magnitude below the node count — a document-wide retype gives all
/// 7 771 of its targets the same type and a handful of distinct field
/// names — so there is nothing here worth a reclamation mechanism.
#[derive(Debug, Clone, Default)]
pub struct ProvenanceTable {
    values: Vec<Provenance>,
    ids: HashMap<Provenance, ProvenanceId>,
}

impl ProvenanceTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// The provenance behind an id, or `None` for [`NOT_RENDERED`].
    ///
    /// This used to be `#[cfg(test)]`, on the grounds that production
    /// only ever compares one interned provenance against another.
    /// Spec 0249 S8 is the first caller that genuinely has to resolve
    /// one back: expanding an auto-fold re-renders a node under the
    /// target it is *already* showing, and the node's own id is where
    /// that target is recorded. It is still not a hot path — one lookup
    /// per expanded fold — and it is still the only one.
    pub fn get(&self, id: ProvenanceId) -> Option<&Provenance> {
        self.values.get(id.0 as usize)
    }

    /// The id of a provenance, inserting it if absent.
    ///
    /// Borrows rather than takes, so the hit path — which is the common one,
    /// since a no-op batch re-derives provenances it has already seen —
    /// clones nothing.
    ///
    /// There is deliberately no lookup-without-insert counterpart. Spec
    /// 0212 needed one and therefore needed a *second* reserved id
    /// (`UNINTERNED`) distinct from its absent sentinel, so that a needle
    /// the table had never seen would not compare equal to a span with no
    /// type. That hazard has no cause here, because the only caller
    /// interns and so can only be holding a real id. Adding a lookup that
    /// does not insert means adding that second sentinel with it: a miss
    /// answering [`NOT_RENDERED`] would make every never-rendered node
    /// compare equal to a brand-new provenance and skip the splice it
    /// needs, which at startup is every node in the document.
    pub fn intern(&mut self, p: &Provenance) -> ProvenanceId {
        if let Some(&id) = self.ids.get(p) {
            return id;
        }
        let next = self.values.len();
        assert!(
            u32::try_from(next).is_ok_and(|n| n < NOT_RENDERED.0),
            "a session cannot produce enough distinct provenances to reach the reserved id"
        );
        let id = ProvenanceId(next as u32);
        self.values.push(p.clone());
        self.ids.insert(p.clone(), id);
        id
    }

    /// How many distinct provenances the table holds. `#[cfg(test)]` for
    /// the same reason as `get`.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.values.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(target: Option<Option<&str>>, name: &str) -> Provenance {
        (target.map(|t| t.map(str::to_owned)), name.to_owned())
    }

    #[test]
    fn interning_the_same_provenance_twice_yields_one_entry() {
        let mut t = ProvenanceTable::new();
        let a = t.intern(&p(Some(Some("a.B")), "f"));
        let b = t.intern(&p(Some(Some("a.B")), "f"));
        assert_eq!(a, b);
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(a), Some(&p(Some(Some("a.B")), "f")));
    }

    /// Spec 0213 test 2: the three target states are the distinction the
    /// field exists for — "no active override" falls back to
    /// `natural_type`, "explicitly raw" does not. An encoding that
    /// collapsed any pair of them would silently stop re-splicing on a
    /// demotion.
    #[test]
    fn the_three_target_states_are_three_ids() {
        let mut t = ProvenanceTable::new();
        let none = t.intern(&p(None, "f"));
        let raw = t.intern(&p(Some(None), "f"));
        let typed = t.intern(&p(Some(Some("a.B")), "f"));
        assert_ne!(none, raw);
        assert_ne!(raw, typed);
        assert_ne!(none, typed);
        assert_eq!(t.len(), 3);
    }

    /// Spec 0119 G4: a rename changes the rendered text without changing
    /// the type, so it has to be a different provenance.
    #[test]
    fn a_rename_is_a_different_provenance() {
        let mut t = ProvenanceTable::new();
        assert_ne!(
            t.intern(&p(Some(Some("a.B")), "f")),
            t.intern(&p(Some(Some("a.B")), "g"))
        );
    }

    /// The property that lets this table have only one sentinel: no
    /// interned provenance can ever be mistaken for "never rendered".
    #[test]
    fn no_interned_provenance_is_the_absent_sentinel() {
        let mut t = ProvenanceTable::new();
        for name in ["f", "g", "h"] {
            assert_ne!(t.intern(&p(Some(Some("a.B")), name)), NOT_RENDERED);
        }
        assert_eq!(t.get(NOT_RENDERED), None);
    }
}
