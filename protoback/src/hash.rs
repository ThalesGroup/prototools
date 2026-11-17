// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The hasher of protoback's maps, chosen when it is compiled (spec 0442
//! S4): `rustc-hash`'s FxHash, unless the feature `hash-fold` (`foldhash`)
//! or `hash-sip` (the standard library's SipHash) is enabled; `hash-sip`
//! wins over `hash-fold`. FxHash is no feature of its own, so that a build
//! without default features (the workspace's Nix builds) still has one.
//!
//! SipHash resists keys built to collide (hash flooding), at a price on the
//! short strings protoback hashes; the inputs are the user's own
//! descriptors. No output depends on a map's iteration order: the standard
//! hasher was already seeded at random on every run.

#[cfg(feature = "hash-sip")]
pub type BuildHasher = std::hash::RandomState;

#[cfg(all(feature = "hash-fold", not(feature = "hash-sip")))]
pub type BuildHasher = foldhash::fast::RandomState;

#[cfg(not(any(feature = "hash-fold", feature = "hash-sip")))]
pub type BuildHasher = rustc_hash::FxBuildHasher;

pub type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasher>;
pub type HashSet<K> = std::collections::HashSet<K, BuildHasher>;
