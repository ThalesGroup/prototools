// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Score a binary protobuf against a compiled scoring graph.

pub mod load;
pub(crate) mod walk;

pub use walk::{partition_roots, score_all, score_one, score_subset, EntryScore, ScoringOpts};

#[cfg(test)]
mod tests;
