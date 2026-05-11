// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Library interface for the `score-graph` crate.
//!
//! Exposes the build pipeline so that the `scoring_graph_lib` PyO3 extension
//! can call it without going through the CLI binary.

pub mod build_scoring_graph;
pub mod score;
