// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The hasher of prototext-graph's internal maps, chosen when it is
//! compiled (spec 0443 P7), as protoback's (spec 0442 S4): `rustc-hash`'s
//! FxHash, unless the feature `hash-fold` (`foldhash`) or `hash-sip` (the
//! standard library's SipHash) is enabled; `hash-sip` wins over
//! `hash-fold`. No feature of its own for FxHash, so that a build without
//! default features (the workspace's Nix builds) has one. SipHash was a
//! quarter of the scoring graph's build on googleapis.
//!
//! Internal maps only: no public type holds one. `fds_index`'s map keeps
//! rkyv's FxHasher64, part of a file's format.

#[cfg(feature = "hash-sip")]
pub(crate) type BuildHasher = std::hash::RandomState;

#[cfg(all(feature = "hash-fold", not(feature = "hash-sip")))]
pub(crate) type BuildHasher = foldhash::fast::RandomState;

#[cfg(not(any(feature = "hash-fold", feature = "hash-sip")))]
pub(crate) type BuildHasher = rustc_hash::FxBuildHasher;

pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasher>;
pub(crate) type HashSet<K> = std::collections::HashSet<K, BuildHasher>;
