// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What the phases share: the run's settings, and what they decide that is
//! not in the topology or the nodes (reproto's `Context`).

use crate::hash::HashMap;
use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::db::Database;
use crate::events::Events;
use crate::graph::Graph;
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
    /// The `--seed` FQDN patterns, which phase 5 matches nodes with.
    pub seed_fqdns: Vec<String>,
    pub path_seeds: PathPatterns,
    pub path_seed_fqdns: BTreeSet<String>,
    pub fallback_protos: Vec<String>,
    /// The built-in copies that fill in what the input lacks (spec 0447
    /// S2): loaded only for a file imported but missing, silently.
    pub fill_in_protos: Vec<String>,
    /// No --descriptor, no variant: the descriptor inferred (spec 0447 S1).
    pub infer_descriptor: bool,
    /// The profiles' folders (spec 0447 S6): each file in one replaces the
    /// input's copy.
    pub profile_folders: Vec<PathBuf>,
    /// The inference's failure: custom options extending several
    /// descriptors' options.
    pub descriptor_error: Option<String>,
    pub variant: Variant,
    pub keep_duplicates: bool,
    /// `--schema-db-out`: phase 6 sets the schema database's extra files
    /// aside.
    pub build_schema_db: bool,
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
    /// Phase 3's nodes, which phases 4 to 6 mark.
    pub graph: Graph,
    /// Phase 5's seeds.
    pub seed_nodes: BTreeSet<usize>,
    /// Phase 6's files for the schema database, beyond the summoned ones.
    pub schema_db_extra_nodes: Vec<usize>,
    /// `--fdp-plugin` or `--fdp-plugin-cmd`, running (spec 0431).
    pub plugin: Option<crate::plugin::Plugin>,
    /// The `move:` fixups, and what they did (spec 0445).
    pub moves: crate::moves::Moves,
}
