// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The command line, under reproto's spellings (spec 0428 S9).
//!
//! Every option of reproto's is parsed, with its arguments. Those phases 1
//! to 6 read are used; the others act after phase 6: under `--dump-graph`
//! they are ignored, so that the harness's flag sets run unchanged, and
//! without it protoback refuses them, naming the step that brings them.

use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum};

/// The well-known types `--use-variant` can supply, and `all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum UseVariant {
    Any,
    Empty,
    Timestamp,
    Duration,
    Struct,
    Wrappers,
    Descriptor,
    SourceContext,
    FieldMask,
    Type,
    Api,
    All,
}

impl UseVariant {
    /// The well-known types, in the order reproto loads their fallbacks:
    /// leaves before the files importing them. `descriptor` comes first,
    /// apart: its file is the variant's.
    pub const WELL_KNOWN: &'static [UseVariant] = &[
        UseVariant::Any,
        UseVariant::Empty,
        UseVariant::Timestamp,
        UseVariant::Duration,
        UseVariant::Struct,
        UseVariant::Wrappers,
        UseVariant::SourceContext,
        UseVariant::FieldMask,
        UseVariant::Type,
        UseVariant::Api,
    ];

    /// Its spelling, which is also its well-known file's stem.
    pub fn name(self) -> String {
        self.to_possible_value()
            .map(|v| v.get_name().to_string())
            .unwrap_or_default()
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "protoback",
    about = "Give back the .proto sources of protobuf descriptor sets"
)]
pub struct Cli {
    /// Descriptor files and directories, relative to the -I roots.
    #[arg(value_name = "DESCRIPTOR_FILES")]
    pub pb_files: Vec<PathBuf>,

    /// A directory, or a blob of descriptors, to resolve inputs and imports in.
    #[arg(short = 'I', long = "desc-root", action = ArgAction::Append)]
    pub pb_path: Vec<PathBuf>,
    #[arg(long = "pb-path", hide = true, action = ArgAction::Append)]
    pub pb_path_deprecated: Vec<PathBuf>,

    #[arg(short = 'O', long = "proto-out")]
    pub proto_out: Option<PathBuf>,
    #[arg(long = "no-proto-out")]
    pub no_proto_out: bool,
    #[arg(long = "output-root", hide = true)]
    pub output_root_deprecated: Option<PathBuf>,
    #[arg(short = 'b', long = "emit-binary")]
    pub emit_binary: bool,
    #[arg(long = "source-info", overrides_with = "no_source_info")]
    pub source_info: bool,
    #[arg(long = "no-source-info")]
    pub no_source_info: bool,
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    #[arg(long = "proto-variant", env = "REPROTO_VARIANT")]
    pub proto_variant: Option<PathBuf>,
    #[arg(long = "use-variant", value_enum, action = ArgAction::Append)]
    pub use_variant: Vec<UseVariant>,
    #[arg(long = "keep-descriptor-path")]
    pub keep_descriptor_path: bool,
    #[arg(long = "emit-descriptor", overrides_with = "no_emit_descriptor")]
    pub emit_descriptor: bool,
    #[arg(long = "no-emit-descriptor")]
    pub no_emit_descriptor: bool,

    #[arg(short = 's', long = "seed", action = ArgAction::Append)]
    pub seeds: Vec<String>,
    #[arg(short = 'p', long = "prune", action = ArgAction::Append)]
    pub stumps: Vec<String>,
    #[arg(short = 'C', long = "filter-config")]
    pub filter_config: Option<PathBuf>,

    #[arg(long = "force-proto2-output")]
    pub force_proto2_output: bool,
    #[arg(long = "force-proto2-for-editions")]
    pub force_proto2_for_editions: bool,
    #[arg(long = "prost-workaround", hide = true)]
    pub prost_workaround: bool,
    #[arg(long = "redact-comments")]
    pub redact_comments: bool,
    #[arg(long = "redact-orphans")]
    pub redact_orphans: bool,
    #[arg(long = "go-root")]
    pub go_root: Option<String>,

    #[arg(long = "schema-db-out")]
    pub build_schema_db: Option<PathBuf>,
    #[arg(long = "build-schema-db", hide = true)]
    pub build_schema_db_deprecated: Option<PathBuf>,
    #[arg(long = "emit-scoring-yaml")]
    pub emit_scoring_yaml: bool,
    #[arg(long = "emit-extension-ranges")]
    pub emit_extension_ranges: bool,
    #[arg(long = "emit-scoring-graphs", hide = true)]
    pub emit_scoring_graphs: bool,
    #[arg(long = "scoring-html-out")]
    pub emit_scoring_html: Option<PathBuf>,
    #[arg(long = "emit-scoring-html", hide = true)]
    pub emit_scoring_html_deprecated: Option<PathBuf>,
    #[arg(long = "emit-pyvis", hide = true)]
    pub emit_pyvis: Option<PathBuf>,
    #[arg(long = "with-leaf-nodes")]
    pub with_leaf_nodes: bool,
    #[arg(long = "hide", action = ArgAction::Append)]
    pub pyvis_hide: Vec<String>,

    #[arg(long = "debug")]
    pub debug: bool,
    #[arg(long = "fdp-plugin")]
    pub fdp_plugin: Option<PathBuf>,
    #[arg(long = "phase2-plugin", hide = true)]
    pub phase2_plugin_deprecated: Option<PathBuf>,
    #[arg(long = "keep-duplicates")]
    pub keep_duplicates: bool,
    #[arg(long = "detailed-warnings")]
    pub detailed_warnings: bool,
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,
    #[arg(long = "dump-resolved-features", hide = true)]
    pub dump_resolved_features: Option<String>,
    /// Write what phases 1 to 6 decided to this file, and stop (spec 0428).
    #[arg(long = "dump-graph", hide = true)]
    pub dump_graph: Option<PathBuf>,
    #[arg(long = "debug-fqdn")]
    pub debug_fqdn: bool,
}

impl Cli {
    /// The options that act after phase 6 and that protoback does not
    /// render yet, set on this command line, each with the step of spec
    /// 0424 that brings it (spec 0429 S1).
    pub fn later_options(&self) -> Vec<(&'static str, u8)> {
        let mut later = Vec::new();
        let mut note = |set: bool, name: &'static str, step: u8| {
            if set {
                later.push((name, step));
            }
        };
        note(
            self.build_schema_db.is_some() || self.build_schema_db_deprecated.is_some(),
            "--schema-db-out",
            6,
        );
        note(
            self.emit_scoring_yaml || self.emit_scoring_graphs,
            "--emit-scoring-yaml",
            6,
        );
        note(self.emit_extension_ranges, "--emit-extension-ranges", 6);
        note(
            self.emit_scoring_html.is_some()
                || self.emit_scoring_html_deprecated.is_some()
                || self.emit_pyvis.is_some(),
            "--scoring-html-out",
            6,
        );
        note(self.with_leaf_nodes, "--with-leaf-nodes", 6);
        note(!self.pyvis_hide.is_empty(), "--hide", 6);
        note(
            self.fdp_plugin.is_some() || self.phase2_plugin_deprecated.is_some(),
            "--fdp-plugin",
            7,
        );
        note(
            self.dump_resolved_features.is_some(),
            "--dump-resolved-features",
            9,
        );
        note(self.detailed_warnings, "--detailed-warnings", 9);
        later
    }
}
