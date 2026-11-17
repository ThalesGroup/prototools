// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The hasher of protoback's maps, chosen when it is compiled (spec 0442
//! S4): the feature `hash-fx` (`rustc-hash`, the default), `hash-fold`
//! (`foldhash`) or `hash-sip` (the standard library's SipHash). Features
//! add up: `hash-sip` wins over `hash-fold`, which wins over the default,
//! so `--features hash-fold` alone selects it.
//!
//! SipHash resists keys built to collide (hash flooding), at a price on the
//! short strings protoback hashes; the inputs are the user's own
//! descriptors. No output depends on a map's iteration order: the standard
//! hasher was already seeded at random on every run.

#[cfg(feature = "hash-sip")]
pub type BuildHasher = std::hash::RandomState;

#[cfg(all(feature = "hash-fold", not(feature = "hash-sip")))]
pub type BuildHasher = foldhash::fast::RandomState;

#[cfg(all(
    feature = "hash-fx",
    not(any(feature = "hash-fold", feature = "hash-sip"))
))]
pub type BuildHasher = rustc_hash::FxBuildHasher;

#[cfg(not(any(feature = "hash-fx", feature = "hash-fold", feature = "hash-sip")))]
compile_error!("protoback needs one of the features hash-fx, hash-fold or hash-sip");

pub type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasher>;
pub type HashSet<K> = std::collections::HashSet<K, BuildHasher>;
