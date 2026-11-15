// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What the phases share: the run's settings, and what they decide that is
//! not in the topology or the nodes (reproto's `Context`).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use crate::events::Events;
use crate::pathmatch::PathPatterns;
use crate::variant::Variant;

pub struct Run {
    pub roots: Vec<PathBuf>,
    pub pb_files: Vec<PathBuf>,
    pub pruned_fqdns: BTreeSet<String>,
    pub pruned_paths: PathPatterns,
    pub path_seeds: PathPatterns,
    pub path_seed_fqdns: BTreeSet<String>,
    pub fallback_protos: Vec<String>,
    pub variant: Variant,
    pub quiet: bool,
    pub debug: bool,
    pub events: Events,
    /// `blob_members`' memo, by root.
    pub blobs: HashMap<PathBuf, Vec<(PathBuf, Vec<u8>)>>,
}
