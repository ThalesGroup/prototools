// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoscan: find the protobuf descriptors embedded in a binary (spec 0407).
//!
//! The scanning is the `fdp-scan` crate's; this binary is its command line:
//! print the name of each `FileDescriptorProto` found and, with
//! `--proto-out DIR`, write each to `DIR/<name>` with `.proto` replaced by
//! `.pb`.

mod man;

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;
use prototext_upb::descriptor::{Encode, FileDescriptorProto};

/// Find the protobuf descriptors embedded in a binary.
///
/// Prints the file name of each FileDescriptorProto found in FILE, in order.
/// With --proto-out, also writes each one to DIR/<name>, its .proto suffix
/// replaced by .pb, creating directories as needed.
#[derive(Parser)]
#[command(name = "protoscan", version, about, long_about)]
struct Cli {
    /// The binary to scan.
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// Write each descriptor found under DIR, as <name>.pb.
    ///
    /// `--proto_out`, the former spelling, is still accepted but deprecated
    /// (spec 0407 S3).
    #[arg(long = "proto-out", value_name = "DIR", alias = "proto_out")]
    proto_out: Option<PathBuf>,
}

fn main() -> ExitCode {
    // When PROTOSCAN_COMPLETE=<shell> is set, print the completion script
    // and exit. Bash completes from the raw command line (spec 0406).
    CompleteEnv::with_factory(Cli::command)
        .var("PROTOSCAN_COMPLETE")
        .shells(prototools_complete::shells())
        .complete();
    // Before parsing, so FILE need not be given to write the man page.
    man::generate_and_exit_if_requested(Cli::command());

    if std::env::args_os()
        .skip(1)
        .any(|a| is_deprecated_proto_out(&a))
    {
        eprintln!("warning: --proto_out is deprecated, use --proto-out");
    }
    let cli = Cli::parse();

    let data = match std::fs::read(&cli.file) {
        Ok(data) => data,
        Err(e) => Cli::command()
            .error(
                ErrorKind::Io,
                format!("cannot read {}: {e}", cli.file.display()),
            )
            .exit(),
    };

    match scan(&data, cli.proto_out.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--proto_out` or `--proto_out=DIR`: the spelling clap accepts through a
/// hidden alias, and which this warns about.
fn is_deprecated_proto_out(arg: &OsStr) -> bool {
    arg.to_str()
        .is_some_and(|a| a == "--proto_out" || a.starts_with("--proto_out="))
}

/// Prints each descriptor's name and, given `out_dir`, writes it there.
fn scan(data: &[u8], out_dir: Option<&Path>) -> Result<(), String> {
    let mut stdout = std::io::stdout().lock();
    for (start, end) in fdp_scan::scan(data) {
        let blob = &data[start..end];
        // The scanner only returns records it has already scored as
        // FileDescriptorProtos (spec 0239 G4): decoding is how the name
        // is read, not a filter.
        let name = FileDescriptorProto::decode(blob)
            .map_err(|e| format!("descriptor at {start}..{end} does not decode: {e}"))?
            .name
            .unwrap_or_default();
        writeln!(stdout, "{name}").map_err(|e| e.to_string())?;
        if let Some(dir) = out_dir {
            let mut path = dir.join(&name);
            if path.extension() == Some(OsStr::new("proto")) {
                path.set_extension("pb");
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            std::fs::write(&path, blob)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
    }
    Ok(())
}
