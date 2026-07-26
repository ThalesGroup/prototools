// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::engine::ArgValueCompleter;

use complete::{
    complete_any_path, complete_descriptor_path, complete_dir_path, complete_input_paths,
    complete_type_names,
};

pub mod complete;
pub mod inputs;
pub mod lazy_pool;
pub mod run;

// ── Embedded descriptor ───────────────────────────────────────────────────────

/// `descriptor.pb` compiled from `google/protobuf/descriptor.proto`.
/// Covers all `google.protobuf.*` types without requiring a file on disk.
pub static EMBEDDED_DESCRIPTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/descriptor.pb"));

/// Pre-built Hopcroft scoring graph for all WKT types (feature `wkt-db`).
/// Embedded at compile time from `$OUT_DIR/wkt.rkyv` produced by `build.rs`.
#[cfg(feature = "wkt-db")]
pub static WKT_GRAPH: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wkt.rkyv"));

/// Pre-built lazy FDS index for all WKT types (feature `wkt-db`).
/// Embedded at compile time from `$OUT_DIR/wkt_index.rkyv` produced by `build.rs`.
#[cfg(feature = "wkt-db")]
pub static WKT_INDEX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wkt_index.rkyv"));

// ── CLI definition ────────────────────────────────────────────────────────────

/// Convert between binary protobuf and protoc-style text (enhanced textproto).
///
/// Three guarantees:
///
///   Schema-aware: supply a compiled descriptor and a root message type
///   to get field names, proto types, and enum values.  A schema is never
///   required; without one every field is decoded by wire type and field number.
///
///   Lossless round-trip: binary → text → binary is byte-for-byte identical for
///   any input — well-formed, non-canonical, or malformed.
///
///   protoc-compatible: for canonical protobuf messages the text output is
///   identical to protoc --decode.
///
/// Enable shell completion:
///   bash:  source <(PROTOTEXT_COMPLETE=bash prototext)
///   zsh:   source <(PROTOTEXT_COMPLETE=zsh prototext)
///   fish:  PROTOTEXT_COMPLETE=fish prototext | source
#[derive(Debug, Parser)]
#[command(
    name = "prototext",
    version,
    about,
    long_about = None,
    after_help = "\
Examples:
  # Decode to stdout (schemaless)
  prototext decode foo.pb

  # Decode with schema; annotations included by default
  prototext --descriptor-set my.desc decode --type com.example.Foo foo.pb

  # Suppress annotations (protoc-compatible output)
  prototext --descriptor-set my.desc decode --type com.example.Foo \\
      --no-annotations foo.pb

  # Identify the schema of an unknown binary
  prototext --descriptor-set my.desc list-schemas foo.pb

  # Encode prototext back to binary
  prototext encode foo.txtpb -o foo.pb",
)]
pub struct Cli {
    // ── Descriptor ────────────────────────────────────────────────────────────
    /// FileDescriptorSet for type lookup and scoring.
    ///
    /// The file may be a raw binary FileDescriptorSet, a #@ prototext-format
    /// FileDescriptorSet, or a single FileDescriptorProto.  If a sibling
    /// <stem>/hopcroft.rkyv file exists it is loaded automatically and enables
    /// scoring and auto-inference.
    ///
    /// Resolution order: (1) this flag, (2) PROTOTEXT_DESCRIPTOR_SET env var
    /// (PROTOTEXT_DEFAULT_DESCRIPTOR accepted with a deprecation warning),
    /// (3) built-in google.protobuf.* fallback.
    #[arg(
        long = "descriptor-set",
        alias = "descriptor",
        value_name = "DESCRIPTOR_SET_FILE",
        env = "PROTOTEXT_DESCRIPTOR_SET",
        add = ArgValueCompleter::new(complete_descriptor_path),
    )]
    pub descriptor: Option<PathBuf>,

    // ── Output routing ────────────────────────────────────────────────────────
    /// Write output to PATH (single input only; exclusive with --output-root).
    #[arg(
        short = 'o',
        long = "output",
        value_name = "PATH",
        conflicts_with = "output_root",
        add = ArgValueCompleter::new(complete_any_path),
    )]
    pub output: Option<PathBuf>,

    /// Write output files under DIR, mirroring the input tree (exclusive with
    /// --output and --in-place).
    #[arg(
        short = 'O',
        long = "output-root",
        value_name = "DIR",
        conflicts_with_all = ["output"],
        add = ArgValueCompleter::new(complete_dir_path),
    )]
    pub output_root: Option<PathBuf>,

    // ── Input ─────────────────────────────────────────────────────────────────
    /// Resolve positional paths and globs relative to DIR.
    /// Absolute positional paths are an error when set.
    #[arg(
        short = 'I',
        long = "input-root",
        value_name = "DIR",
        add = ArgValueCompleter::new(complete_dir_path),
    )]
    pub input_root: Option<PathBuf>,

    // ── Other ─────────────────────────────────────────────────────────────────
    /// Suppress warnings on stderr (errors are always printed).
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    // ── Subcommand ────────────────────────────────────────────────────────────
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommands for prototext.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Decode binary protobuf to prototext.
    Decode {
        /// Decode as this fully-qualified message type.
        #[arg(
            short = 't',
            long = "type",
            value_name = "NAME",
            conflicts_with = "raw",
            add = ArgValueCompleter::new(complete_type_names),
        )]
        r#type: Option<String>,

        /// Decode without a schema: render raw field numbers and wire types.
        /// Does not require --descriptor-set.  Mutually exclusive with --type.
        #[arg(long, conflicts_with = "type")]
        raw: bool,

        /// Rewrite each input file in place (exclusive with --output-root).
        /// Files are read fully before writing.
        #[arg(short = 'i', long = "in-place")]
        in_place: bool,

        /// Treat PATH arguments as raw binary protobuf; skip #@ prototext
        /// auto-detection on the input files.
        #[arg(long = "assume-binary")]
        assume_binary: bool,

        /// Suppress inline wire-type/field-number comments.
        /// Output will not round-trip losslessly without them.
        #[arg(long = "no-annotations")]
        no_annotations: bool,

        /// Add individual score dimensions (matched, unknown, out_of_range,
        /// non_canonical, mismatches) to the ambiguous-type warning YAML.
        /// Only effective when type inference is ambiguous.
        #[arg(long = "detailed-score", help_heading = "Advanced options")]
        detailed_score: bool,

        /// Suppress google.protobuf.Any expansion; render value as raw bytes.
        #[arg(long = "no-expand-any", help_heading = "Advanced options")]
        no_expand_any: bool,

        /// Suppress inline expansion of MessageSet groups.
        /// Independent of --no-expand-any.
        #[arg(long = "no-expand-message-set", help_heading = "Advanced options")]
        no_expand_message_set: bool,

        /// Suppress fields absent from the schema (unknown fields, wire-type
        /// mismatches). Has no effect in --raw mode.
        /// With --no-annotations, restores protoc-compatible output.
        #[arg(long = "hide-unknown-fields")]
        hide_unknown_fields: bool,

        /// Treat type-inference warnings (ambiguous type) as errors:
        /// exit 1 instead of exit 2.  Only applicable when auto-inferring type
        /// (no --type given and DB-backed descriptor present).
        #[arg(long = "strict", help_heading = "Advanced options")]
        strict: bool,

        /// Input files, glob patterns, or directories (recursive).
        /// When absent, reads from stdin.
        #[arg(
            value_name = "PATH",
            add = ArgValueCompleter::new(complete_input_paths),
        )]
        paths: Vec<String>,
    },

    /// Encode prototext to binary protobuf.
    Encode {
        /// Rewrite each input file in place (exclusive with --output-root).
        /// Files are read fully before writing.
        #[arg(short = 'i', long = "in-place")]
        in_place: bool,

        /// Input files, glob patterns, or directories (recursive).
        /// When absent, reads from stdin.
        #[arg(
            value_name = "PATH",
            add = ArgValueCompleter::new(complete_input_paths),
        )]
        paths: Vec<String>,
    },

    /// Score input(s) against the DB and list candidate types, score-descending.
    ///
    /// Requires a DB-backed descriptor (hopcroft.rkyv must be present).
    #[command(name = "list-schemas")]
    ListSchemas {
        /// Print only the top N entries (score-descending, ties broken by FQDN).
        /// When absent or 0, only entries tying at the highest score are printed.
        #[arg(long = "top", value_name = "N")]
        top: Option<usize>,

        /// Treat PATH arguments as raw binary protobuf; skip #@ prototext
        /// auto-detection on the input files.
        #[arg(long = "assume-binary")]
        assume_binary: bool,

        /// Add individual score dimensions (matched, unknown, out_of_range,
        /// non_canonical, mismatches) alongside the consolidated score for
        /// each type.
        #[arg(long = "detailed-score", help_heading = "Advanced options")]
        detailed_score: bool,

        /// Suppress google.protobuf.Any expansion; score value as plain bytes.
        #[arg(long = "no-expand-any", help_heading = "Advanced options")]
        no_expand_any: bool,

        /// Input files, glob patterns, or directories (recursive).
        /// When absent, reads from stdin.
        #[arg(
            value_name = "PATH",
            add = ArgValueCompleter::new(complete_input_paths),
        )]
        paths: Vec<String>,
    },

    /// Score input(s) against a known schema type and report the match breakdown.
    ///
    /// Requires a DB-backed descriptor (hopcroft.rkyv must be present).
    Score {
        /// Required. Fully-qualified root message type name.
        #[arg(
            short = 't',
            long = "type",
            value_name = "NAME",
            required = true,
            add = ArgValueCompleter::new(complete_type_names),
        )]
        r#type: String,

        /// Treat PATH arguments as raw binary protobuf; skip #@ prototext
        /// auto-detection on the input files.
        #[arg(long = "assume-binary")]
        assume_binary: bool,

        /// Suppress google.protobuf.Any expansion; score value as plain bytes.
        #[arg(long = "no-expand-any", help_heading = "Advanced options")]
        no_expand_any: bool,

        /// Input files, glob patterns, or directories (recursive).
        /// When absent, reads from stdin.
        #[arg(
            value_name = "PATH",
            add = ArgValueCompleter::new(complete_input_paths),
        )]
        paths: Vec<String>,
    },
}

/// Return a fully-built [`clap::Command`] for `prototext`.
///
/// Used by `prototext-gen-man` to generate man pages without running the
/// full binary.
pub fn command() -> clap::Command {
    Cli::command()
}
