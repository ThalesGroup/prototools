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
pub mod run;

// ── Embedded descriptor ───────────────────────────────────────────────────────

/// `descriptor.pb` compiled from `google/protobuf/descriptor.proto`.
/// Covers all `google.protobuf.*` types without requiring a file on disk.
pub static EMBEDDED_DESCRIPTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/descriptor.pb"));

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
#[command(name = "prototext", version, about, long_about = None)]
pub struct Cli {
    // ── Descriptor ────────────────────────────────────────────────────────────
    /// FileDescriptorSet for type lookup and scoring.
    ///
    /// The file may be a raw binary FileDescriptorSet, a #@ prototext-format
    /// FileDescriptorSet, or a single FileDescriptorProto.  If a sibling
    /// <stem>/hopcroft.rkyv file exists it is loaded automatically and enables
    /// scoring and auto-inference.
    ///
    /// Resolution order: (1) this flag, (2) PROTOTEXT_DEFAULT_DESCRIPTOR env
    /// var, (3) built-in google.protobuf.* fallback.
    #[arg(
        long = "descriptor",
        value_name = "DESCRIPTOR_FILE",
        env = "PROTOTEXT_DEFAULT_DESCRIPTOR",
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
            add = ArgValueCompleter::new(complete_type_names),
        )]
        r#type: Option<String>,

        /// Rewrite each input file in place (exclusive with --output-root).
        /// Files are read fully before writing.
        #[arg(short = 'i', long = "in-place", conflicts_with = "output_root")]
        in_place: bool,

        /// Treat PATH arguments as raw binary protobuf; skip #@ prototext
        /// auto-detection on the input files.
        #[arg(long = "assume-binary")]
        assume_binary: bool,

        /// Emit inline wire-type/field-number comments (default).
        /// Required for lossless round-trip encode.
        #[arg(long, default_value_t = true, overrides_with = "no_annotations")]
        annotations: bool,

        /// Suppress inline wire-type/field-number comments.
        /// Output will not round-trip losslessly.
        #[arg(long, overrides_with = "annotations")]
        no_annotations: bool,

        /// Root directory for output files in batch mode (exclusive with
        /// --output and --in-place).
        #[arg(
            short = 'O',
            long = "output-root",
            value_name = "DIR",
            conflicts_with_all = ["in_place"],
            add = ArgValueCompleter::new(complete_dir_path),
        )]
        output_root: Option<PathBuf>,

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
        #[arg(short = 'i', long = "in-place", conflicts_with = "output_root")]
        in_place: bool,

        /// Root directory for output files in batch mode (exclusive with
        /// --output and --in-place).
        #[arg(
            short = 'O',
            long = "output-root",
            value_name = "DIR",
            conflicts_with_all = ["in_place"],
            add = ArgValueCompleter::new(complete_dir_path),
        )]
        output_root: Option<PathBuf>,

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

        /// Input files, glob patterns, or directories (recursive).
        /// When absent, reads from stdin.
        #[arg(
            value_name = "PATH",
            add = ArgValueCompleter::new(complete_input_paths),
        )]
        paths: Vec<String>,
    },

    /// Generate a pseudo-random valid protobuf instance for one or more message types.
    ///
    /// Output is #@ prototext with type and seed hint comments.
    /// Multiple TYPEs require --output-root (-O).
    #[command(name = "instantiate-schema")]
    InstantiateSchema {
        /// One or more fully-qualified message type names (e.g. google.protobuf.Timestamp).
        #[arg(value_name = "TYPE", required = true)]
        types: Vec<String>,

        /// Integer seed (default 0).
        #[arg(long, default_value_t = 0)]
        seed: i64,

        /// Maximum recursion depth for nested messages (default 4).
        #[arg(long, default_value_t = 4)]
        max_depth: usize,

        /// Maximum number of elements for repeated fields (default 3).
        #[arg(long, default_value_t = 3)]
        max_repeated: usize,

        /// Probability of populating an optional field (default 0.7).
        #[arg(long, default_value_t = 0.7)]
        p_optional: f64,
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
