// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::path::PathBuf;

use clap::{CommandFactory, Parser};
use clap_complete::engine::ArgValueCompleter;

use complete::{complete_input_paths, complete_pb_files, complete_type_names};

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
///   Schema-aware: supply a compiled .pb descriptor and a root message type
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
    // ── Mode (exactly one required) ───────────────────────────────────────────
    /// Decode: treat input as binary protobuf, emit text.
    #[arg(short = 'd', long = "decode", conflicts_with = "encode")]
    pub decode: bool,

    /// Encode: treat input as textual prototext, emit binary.
    #[arg(short = 'e', long = "encode", conflicts_with = "decode")]
    pub encode: bool,

    // ── Schema ────────────────────────────────────────────────────────────────
    /// Compiled .pb descriptor file.
    #[arg(
        short = 'D',
        long = "descriptor",
        value_name = "PATH",
        add = ArgValueCompleter::new(complete_pb_files),
    )]
    pub descriptor: Option<PathBuf>,

    /// Fully-qualified root message type name (e.g. pkg.MyMessage).
    /// When given without --descriptor, uses the embedded descriptor
    /// (covers all google.protobuf.* types).
    #[arg(
        short = 't',
        long = "type",
        value_name = "NAME",
        add = ArgValueCompleter::new(complete_type_names),
    )]
    pub r#type: Option<String>,

    // ── Output ────────────────────────────────────────────────────────────────
    /// Emit inline wire-type/field-number comments (default).
    /// Required for lossless round-trip encode.
    #[arg(long, default_value_t = true, overrides_with = "no_annotations")]
    pub annotations: bool,

    /// Suppress inline wire-type/field-number comments.
    /// Output will not be losslessly round-trippable.
    #[arg(long, overrides_with = "annotations")]
    pub no_annotations: bool,

    /// Write output to PATH (single input only; exclusive with --output-root).
    #[arg(
        short = 'o',
        long = "output",
        value_name = "PATH",
        conflicts_with = "output_root"
    )]
    pub output: Option<PathBuf>,

    /// Root directory for output files in batch mode (exclusive with --output
    /// and --in-place).
    #[arg(
        short = 'O',
        long = "output-root",
        value_name = "DIR",
        conflicts_with_all = ["output", "in_place"],
    )]
    pub output_root: Option<PathBuf>,

    // ── Input ─────────────────────────────────────────────────────────────────
    /// Root directory: positional paths and globs are resolved relative to DIR.
    /// Absolute positional paths are an error when --input-root is given.
    #[arg(short = 'I', long = "input-root", value_name = "DIR")]
    pub input_root: Option<PathBuf>,

    /// Rewrite each input file in place (exclusive with --output-root).
    /// Each file is read fully into memory before being overwritten (sponge).
    #[arg(short = 'i', long = "in-place", conflicts_with = "output_root")]
    pub in_place: bool,

    // ── Other ─────────────────────────────────────────────────────────────────
    /// Suppress warnings on stderr (errors are always printed).
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// Input files, glob patterns, or directories (recursive).
    /// When absent, reads from stdin.
    #[arg(
        value_name = "PATH",
        add = ArgValueCompleter::new(complete_input_paths),
    )]
    pub paths: Vec<String>,
}

/// Return a fully-built [`clap::Command`] for `prototext`.
///
/// Used by `prototext-gen-man` to generate man pages without running the
/// full binary.
pub fn command() -> clap::Command {
    Cli::command()
}
