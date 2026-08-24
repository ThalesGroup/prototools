// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Interned type names for `NodeSpan::type_fqdn` (spec 0212 S3).
//!
//! A `NodeSpan` used to carry its type name as an `Option<String>`: 24
//! bytes inline plus a separate heap allocation per node, for a string
//! drawn from a set of at most a few tens of thousands of distinct values.
//! On `googleapis.desc` that is 58 777 distinct names against 4 501 014
//! nodes.
//!
//! Here the name is stored once and the span holds a 4-byte [`FqdnId`] into
//! this table.

use std::collections::HashMap;

/// An index into a [`FqdnTable`].
///
/// [`NO_FQDN`] means the node carries no type name at all. As with the
/// `Option<String>` it replaces, that is *not* a scalar/message
/// discriminator: a message or group node whose schema could not be
/// resolved also has no type name. Use `NodeSpan::kind` for shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FqdnId(u32);

/// The absent-type sentinel — what `None` used to be.
///
/// `u32::MAX` rather than `0`, so that ids are plain indices and the table
/// needs no reserved first entry.
pub const NO_FQDN: FqdnId = FqdnId(u32::MAX);

/// What [`FqdnTable::id_of`] answers for a name the table has never
/// interned.
///
/// Reserved *separately* from [`NO_FQDN`], and that separation is load
/// bearing. Under S6's idiom the answer is compared straight against a
/// span's `type_fqdn`, and the string form it replaces asked
/// `span.type_fqdn.as_deref() == Some(name)`, which is `false` for a span
/// with no type. Were a missing needle to answer `NO_FQDN` it would
/// instead compare *equal* to every typeless span — so on a document
/// containing no `google.protobuf.Any`, say, every scalar in the tree
/// would report itself as an `Any`. A distinct value that no span can hold
/// makes the substitution exact with no special casing at the call sites.
pub const UNINTERNED: FqdnId = FqdnId(u32::MAX - 1);

/// The set of type names referred to by one or more renders, each stored
/// exactly once.
///
/// The table is supplied by the caller and shared across every render whose
/// spans may be compared with each other — see spec 0212 S4. A per-call
/// table would make `FqdnId(3)` name different types in a spliced span than
/// in the document it is spliced into, which is a silent wrong answer
/// rather than an error.
#[derive(Debug, Clone, Default)]
pub struct FqdnTable {
    names: Vec<String>,
    ids: HashMap<String, FqdnId>,
}

impl FqdnTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// The name behind an id, or `None` for [`NO_FQDN`].
    pub fn get(&self, id: FqdnId) -> Option<&str> {
        self.names.get(id.0 as usize).map(String::as_str)
    }

    /// The id of a name already in the table, or [`UNINTERNED`] if it holds
    /// no such name. Does not insert.
    ///
    /// This is the lookup a *comparison* should use. Resolving each span's
    /// id back to a string inside a loop over the nodes needs the table
    /// borrowed alongside the nodes and costs one lookup per node; interning
    /// the needle once and comparing ids costs one lookup in total.
    pub fn id_of(&self, name: &str) -> FqdnId {
        self.ids.get(name).copied().unwrap_or(UNINTERNED)
    }

    /// The id of a name, inserting it if absent.
    ///
    /// Public even though the renderer is the only writer today: a caller
    /// needing to name a type that no render produced — a user-supplied
    /// override target, say — has no other way in.
    pub fn intern(&mut self, name: &str) -> FqdnId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let next = self.names.len();
        assert!(
            u32::try_from(next).is_ok_and(|n| n < UNINTERNED.0),
            "a render cannot produce enough distinct type names to reach the reserved ids"
        );
        let id = FqdnId(next as u32);
        self.names.push(name.to_owned());
        self.ids.insert(name.to_owned(), id);
        id
    }

    /// How many distinct names the table holds.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_the_same_name_twice_yields_one_entry() {
        let mut t = FqdnTable::new();
        let a = t.intern("foo.Bar");
        let b = t.intern("foo.Bar");
        assert_eq!(a, b);
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(a), Some("foo.Bar"));
    }

    #[test]
    fn distinct_names_get_distinct_ids() {
        let mut t = FqdnTable::new();
        assert_ne!(t.intern("foo.Bar"), t.intern("foo.Baz"));
        assert_eq!(t.len(), 2);
    }

    /// Spec 0212 S6: an unknown name must *not* answer [`NO_FQDN`], or the
    /// interned-needle idiom would report every typeless span as a match
    /// for it. `as_deref() == Some(..)` was `false` for a `None` span, and
    /// the id form has to agree.
    #[test]
    fn an_unknown_name_is_not_the_absent_sentinel() {
        let mut t = FqdnTable::new();
        t.intern("foo.Bar");
        assert_eq!(t.id_of("foo.Nope"), UNINTERNED);
        assert_ne!(UNINTERNED, NO_FQDN);
        assert_eq!(t.get(UNINTERNED), None);
        assert_eq!(t.get(NO_FQDN), None);
    }

    #[test]
    fn id_of_does_not_insert() {
        let t = FqdnTable::new();
        assert_eq!(t.id_of("foo.Bar"), UNINTERNED);
        assert_eq!(t.len(), 0);
    }
}
