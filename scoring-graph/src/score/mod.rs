// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Score a binary protobuf against a compiled scoring graph.

pub mod load;
pub(crate) mod walk;

pub use walk::{score_all, EntryScore};

#[cfg(test)]
mod tests;
