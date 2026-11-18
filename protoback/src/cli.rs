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
use clap_complete::engine::ArgValueCompleter;

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
    version,
    about = "Parse DESCRIPTOR_FILES and generate output based on the options given.",
    after_help = "Examples:

  # Decompile a descriptor to .proto source
  protoback -O out/ schema.desc

  # Build a scoring DB from a descriptor slice
  protoback --schema-db-out slice.desc \\
      --seed desc:.com.example.MyMessage schema.desc

  # Read the descriptors embedded in a blob, writing the DB to
  # out.desc and the .proto files to out/proto/
  protoback --schema-db-out out.desc -I some/blob

  # Decompile all types reachable from a seed, pruning a noisy package
  protoback -O out/ \\
      --seed desc:.com.example.Root \\
      --prune file:google/protobuf/struct.proto \\
      schema.desc

  # Score and visualise the graph (advanced)
  protoback --schema-db-out out.desc \\
      --scoring-html-out out.html \\
      schema.desc"
)]
pub struct Cli {
    /// Descriptor files and directories, relative to the -I roots.
    #[arg(value_name = "DESCRIPTOR_FILES", add = ArgValueCompleter::new(crate::complete::complete_descriptor_files))]
    pub pb_files: Vec<PathBuf>,

    #[arg(short = 'I', long = "desc-root", action = ArgAction::Append, help = "Root for resolving relative DESCRIPTOR_FILES paths and for locating imports (repeatable; like protoc -I). A directory is read as-is; a file is read as a blob, standing for the descriptors embedded in it, one member per FileDescriptorProto found (what protoscan would have extracted). Defaults to the current directory.", help_heading = "Input", value_name = "PATH")]
    pub pb_path: Vec<PathBuf>,
    #[arg(long = "pb-path", hide = true, action = ArgAction::Append, help = "Deprecated alias for --desc-root.", value_name = "DIRECTORY")]
    pub pb_path_deprecated: Vec<PathBuf>,

    #[arg(
        short = 'O',
        long = "proto-out",
        add = ArgValueCompleter::new(crate::complete::complete_dir_path),
        help = "Output directory for generated .proto files (created if absent). Not required when using --schema-db-out, --scoring-html-out, or --dry-run. Defaults to the --schema-db-out stub's 'proto' child when that option is given; --no-proto-out suppresses that default. When --schema-db-out is also given, DIR may not be inside its stub directory unless DIR is that stub's immediate 'proto' child.",
        help_heading = "Output",
        value_name = "DIRECTORY"
    )]
    pub proto_out: Option<PathBuf>,
    #[arg(
        long = "no-proto-out",
        help = "Do not write .proto files. Suppresses the -O directory that --schema-db-out otherwise implies; an error if -O is also given.",
        help_heading = "Output"
    )]
    pub no_proto_out: bool,
    #[arg(
        long = "output-root",
        hide = true,
        help = "Deprecated alias for --proto-out.",
        value_name = "DIRECTORY"
    )]
    pub output_root_deprecated: Option<PathBuf>,
    #[arg(
        short = 'b',
        long = "emit-binary",
        help = "Also write binary descriptor files (.pb) alongside .proto output",
        help_heading = "Output"
    )]
    pub emit_binary: bool,
    #[arg(
        long = "source-info",
        overrides_with = "no_source_info",
        help = "Embed a SourceCodeInfo synthesized from the reconstructed .proto text in binary descriptor output (-b, --schema-db-out). On by default.",
        help_heading = "Output"
    )]
    pub source_info: bool,
    #[arg(
        long = "no-source-info",
        help = "The negation of --source-info.",
        help_heading = "Output"
    )]
    pub no_source_info: bool,
    #[arg(
        long = "dry-run",
        help = "Do not actually create .proto files",
        help_heading = "Output"
    )]
    pub dry_run: bool,

    #[arg(
        long = "proto-variant",
        add = ArgValueCompleter::new(crate::complete::complete_yaml_path),
        env = "REPROTO_VARIANT",
        help = "Path to a variant specification YAML file. If omitted, $REPROTO_VARIANT is used; otherwise the built-in google.protobuf default applies.",
        help_heading = "Variant / Schema",
        value_name = "FILE"
    )]
    pub proto_variant: Option<PathBuf>,
    #[arg(long = "use-variant", value_enum, action = ArgAction::Append, help = "Use the variant's embedded copy of a well-known file instead of whatever the input .pb files contain. Accepts: any, empty, timestamp, duration, struct, wrappers, descriptor, all. May be repeated.", help_heading = "Variant / Schema")]
    pub use_variant: Vec<UseVariant>,
    #[arg(
        long = "keep-descriptor-path",
        help = "Keep the variant descriptor import path as-is (do not rewrite to google/protobuf/descriptor.proto)",
        help_heading = "Variant / Schema"
    )]
    pub keep_descriptor_path: bool,
    #[arg(
        long = "emit-descriptor",
        overrides_with = "no_emit_descriptor",
        help = "Write descriptor.proto to the output directory (default: on)",
        help_heading = "Variant / Schema"
    )]
    pub emit_descriptor: bool,
    #[arg(
        long = "no-emit-descriptor",
        help = "The negation of --emit-descriptor.",
        help_heading = "Variant / Schema"
    )]
    pub no_emit_descriptor: bool,

    #[arg(short = 's', long = "seed", action = ArgAction::Append, help = "FQDN or path pattern to treat as an output root. FQDN (needs a prefix): desc:my.pkg.MyMsg, file:foo.proto (the FDP named foo.proto). Path (bare, or path: prefix): foo.pb (every FDP in that physical file), matched against the -I-relative path. Glob (either form): one *.proto (one level), vendor/** (recursive).", help_heading = "Filtering", value_name = "TEXT")]
    pub seeds: Vec<String>,
    #[arg(short = 'p', long = "prune", action = ArgAction::Append, help = "FQDN or path pattern to exclude from output. FQDN (needs a prefix): desc:my.pkg.MyMsg, file:foo.proto (the FDP named foo.proto). Path (bare, or path: prefix): foo.pb (that physical file, never loaded), matched against the -I-relative path. Glob (either form): one *.proto (one level), vendor/** (recursive).", help_heading = "Filtering", value_name = "TEXT")]
    pub stumps: Vec<String>,
    #[arg(
        short = 'C',
        long = "filter-config",
        help = "Path to a YAML file with \"seed:\"/\"prune:\" lists of FQDNs or path patterns (same syntax as -s/--seed and -p/--prune). Merged with any -s/-p flags given on the command line.",
        help_heading = "Filtering",
        value_name = "FILE"
    )]
    pub filter_config: Option<PathBuf>,
    #[arg(
        long = "move",
        action = ArgAction::Append,
        help = "A file moved to one or several others (spec 0445): every import of OLD becomes imports of the TARGETs, and OLD is pruned. Repeatable; also a -C file's move: list (OLD: TARGET or OLD: [TARGET, ...]), and its variants: NAME: sections (seed:, prune:, move: for the variant of that name only).",
        help_heading = "Filtering",
        value_name = "OLD=TARGET[,TARGET...]"
    )]
    pub moves: Vec<String>,

    #[arg(
        long = "force-proto2-output",
        help = "Force all output to proto2 syntax, regardless of the input syntax. Without this flag, output syntax matches the input (polyglot mode).",
        help_heading = "Rendering"
    )]
    pub force_proto2_output: bool,
    #[arg(
        long = "force-proto2-for-editions",
        help = "Translate editions-syntax files to proto2 in output and binary descriptors, the schema DB included. Symmetric with --force-proto2-output but limited to editions files only. Use this for readers that do not support editions, such as prost-reflect.",
        help_heading = "Rendering"
    )]
    pub force_proto2_for_editions: bool,
    #[arg(
        long = "prost-workaround",
        hide = true,
        help = "Alias for --force-proto2-for-editions."
    )]
    pub prost_workaround: bool,
    #[arg(
        long = "redact-comments",
        help = "Redact comments from reconstructed .proto files",
        help_heading = "Rendering"
    )]
    pub redact_comments: bool,
    #[arg(
        long = "redact-orphans",
        help = "Redact orphans from reconstructed .proto files",
        help_heading = "Rendering"
    )]
    pub redact_orphans: bool,
    #[arg(
        long = "go-root",
        help = "Force go_package option in reconstructed .proto files",
        help_heading = "Rendering",
        value_name = "TEXT"
    )]
    pub go_root: Option<String>,

    #[arg(
        long = "schema-db-out",
        help = "Write the schema DB to FILE (must end in .desc). Writes FILE (FileDescriptorSet of all loaded FDPs), FILE-stem/hopcroft.rkyv (compiled scoring graph), and FILE-stem/index.rkyv (lazy-loading FDS index). If -O is also given, it may not be inside FILE-stem unless it is exactly FILE-stem/proto.",
        help_heading = "Advanced",
        value_name = "FILE"
    )]
    pub build_schema_db: Option<PathBuf>,
    #[arg(
        long = "build-schema-db",
        hide = true,
        help = "Deprecated alias for --schema-db-out.",
        value_name = "FILE"
    )]
    pub build_schema_db_deprecated: Option<PathBuf>,
    #[arg(
        long = "emit-scoring-yaml",
        help = "Write per-file scoring-graph YAML files alongside .proto output under --proto-out",
        help_heading = "Advanced"
    )]
    pub emit_scoring_yaml: bool,
    #[arg(
        long = "emit-extension-ranges",
        help = "Record each message's extension ranges in the scoring graph, so that an unknown field number inside a declared range is not counted against the type",
        help_heading = "Advanced"
    )]
    pub emit_extension_ranges: bool,
    #[arg(
        long = "emit-scoring-graphs",
        hide = true,
        help = "Deprecated alias for --emit-scoring-yaml."
    )]
    pub emit_scoring_graphs: bool,
    #[arg(
        long = "scoring-html-out",
        help = "Write scoring-graph HTML visualisations to FILE; requires --schema-db-out",
        help_heading = "Advanced",
        value_name = "FILE"
    )]
    pub emit_scoring_html: Option<PathBuf>,
    #[arg(
        long = "emit-scoring-html",
        hide = true,
        help = "Deprecated alias for --scoring-html-out.",
        value_name = "FILE"
    )]
    pub emit_scoring_html_deprecated: Option<PathBuf>,
    #[arg(
        long = "emit-pyvis",
        hide = true,
        help = "Deprecated alias for --scoring-html-out.",
        value_name = "FILE"
    )]
    pub emit_pyvis: Option<PathBuf>,
    #[arg(
        long = "with-leaf-nodes",
        help = "Include leaf (wire-type sink) nodes in --scoring-html-out graphs (hidden by default)",
        help_heading = "Advanced"
    )]
    pub with_leaf_nodes: bool,
    #[arg(long = "hide", action = ArgAction::Append, help = "FQDN or glob pattern to hide from --scoring-html-out graphs (same syntax as --prune). Matched nodes and their incident edges are dropped from the HTML; no effect on the schema DB.", help_heading = "Advanced", value_name = "TEXT")]
    pub pyvis_hide: Vec<String>,

    #[arg(
        long = "debug",
        help = "Print detailed debug information (development only)",
        help_heading = "Advanced"
    )]
    pub debug: bool,
    #[arg(
        long = "fdp-plugin",
        add = ArgValueCompleter::new(crate::complete::complete_py_path),
        help = "Python file executed as a transformation hook on every FileDescriptorProto reproto parses, before anything reads it. Must define fdp_plugin(fdp), which mutates fdp in place. Called more than once per input file, so it must be deterministic; it must not read reproto state.",
        help_heading = "Advanced",
        value_name = "PATH"
    )]
    pub fdp_plugin: Option<PathBuf>,
    #[arg(
        long = "phase2-plugin",
        add = ArgValueCompleter::new(crate::complete::complete_py_path),
        hide = true,
        help = "Deprecated alias for --fdp-plugin; entry point phase2_plugin(ctx, fdp).",
        value_name = "PATH"
    )]
    pub phase2_plugin_deprecated: Option<PathBuf>,
    #[arg(
        long = "fdp-plugin-cmd",
        value_name = "CMD",
        help = "A plugin in any language: the shell command line run once, spoken to in frames on its stdin and stdout (spec 0431 S2). protoback's own.",
        help_heading = "Advanced"
    )]
    pub fdp_plugin_cmd: Option<String>,
    #[arg(
        long = "jobs",
        value_name = "N",
        help = "The threads files render on; all cores by default. protoback's own (spec 0429 S4): any count writes the same bytes.",
        help_heading = "Advanced"
    )]
    pub jobs: Option<usize>,
    #[arg(
        long = "keep-duplicates",
        help = "Do not prune files whose symbols conflict with already-loaded files (legacy behaviour; by default conflicting files are silently pruned and a warning is emitted).",
        help_heading = "Advanced"
    )]
    pub keep_duplicates: bool,
    #[arg(
        long = "detailed-warnings",
        help = "Print every warning occurrence immediately as it is emitted (default: squash repeated warnings and show a count summary at the end).",
        help_heading = "Diagnostics"
    )]
    pub detailed_warnings: bool,
    #[arg(
        long = "fail-on",
        value_name = "CODE",
        help = "Make the warnings of CODE (shown as \"Warning [CODE]:\") fail the run: it completes, then exits with status 3. Repeatable.",
        help_heading = "Diagnostics"
    )]
    pub fail_on: Vec<String>,
    #[arg(
        short = 'q',
        long = "quiet",
        help = "Suppress progress messages and warnings. Errors are always printed.",
        help_heading = "Diagnostics"
    )]
    pub quiet: bool,
    #[arg(
        long = "dump-resolved-features",
        hide = true,
        help = "(diagnostic) Dump resolved FeatureSet YAML for the named proto file and exit",
        value_name = "TEXT"
    )]
    pub dump_resolved_features: Option<String>,
    #[arg(
        long = "dump-graph",
        hide = true,
        help = "(diagnostic) Write what phases 1 to 6 decided to the given file, and stop",
        value_name = "TEXT"
    )]
    pub dump_graph: Option<PathBuf>,
    #[arg(
        long = "debug-fqdn",
        help = "Print detailed information about FQDNs (development only)",
        help_heading = "Diagnostics"
    )]
    pub debug_fqdn: bool,
}
