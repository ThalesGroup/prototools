// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Score a binary protobuf against a compiled scoring graph.

pub mod load;
mod min_score;
pub(crate) mod walk;

pub use min_score::MinScore;
pub use walk::{
    partition_roots, score_all, score_one, score_subset, Ending, EntryScore, Policy, ScoringOpts,
};

#[cfg(test)]
mod tests;
