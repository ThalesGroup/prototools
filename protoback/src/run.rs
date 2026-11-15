// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What the phases share: the run's settings, and what they decide that is
//! not in the topology or the nodes (reproto's `Context`).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use crate::db::Database;
use crate::events::Events;
use crate::pathmatch::PathPatterns;
use crate::phase2::EditionDefaults;
use crate::variant::Variant;

pub struct Run {
    pub roots: Vec<PathBuf>,
    pub pb_files: Vec<PathBuf>,
    /// The `-p` FQDN patterns as given, which phase 2 matches files with.
    pub prunings: Vec<String>,
    /// The same, less the fallbacks' files (`ctx.pruned_fqdns`).
    pub pruned_fqdns: BTreeSet<String>,
    pub pruned_file_names: BTreeSet<String>,
    pub pruned_paths: PathPatterns,
    pub path_seeds: PathPatterns,
    pub path_seed_fqdns: BTreeSet<String>,
    pub fallback_protos: Vec<String>,
    pub variant: Variant,
    pub keep_duplicates: bool,
    pub go_root: Option<String>,
    pub quiet: bool,
    pub debug: bool,
    pub debug_fqdn: bool,
    pub events: Events,
    /// `blob_members`' memo, by root.
    pub blobs: HashMap<PathBuf, Vec<(PathBuf, Vec<u8>)>>,
    /// Phase 2's pool, and the edition defaults read from its
    /// descriptor.proto.
    pub db: Database,
    pub edition_defaults: EditionDefaults,
}
