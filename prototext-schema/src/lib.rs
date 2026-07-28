// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! prototext-schema — on-demand descriptor loading shared by the binaries.
//!
//! The crate exists so that `prototext` and `protolens` can both use
//! [`LazyPool`] without either depending on the other, and without dragging
//! `prost-reflect` into `prototext-graph` (spec 0197 §S1).

pub mod lazy_pool;

pub use lazy_pool::LazyPool;
