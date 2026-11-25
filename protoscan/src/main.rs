// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoscan: find the protobuf descriptors embedded in a binary (spec 0407).
//!
//! The scanning is the `fdp-scan` crate's; this binary is its command line:
//! print the name of each `FileDescriptorProto` found and, with
//! `--proto-out DIR`, write each to `DIR/<name>` with `.proto` replaced by
//! `.pb`. With `--inflate`, also those inside zip and gzip members, and with
//! `--origin`, where each was found (spec 0480 S7).

mod man;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;
use fdp_scan::{InflateLimits, Origin, ScanOptions, Visit};
use prototext_upb::descriptor::{Encode, FileDescriptorProto};

/// Find the protobuf descriptors embedded in a binary.
///
/// Prints the file name of each FileDescriptorProto found in FILE, in order.
/// With --proto-out, also writes each one to DIR/<name>, its .proto suffix
/// replaced by .pb, creating directories as needed. With --inflate, also
/// looks into zip and gzip members, nested up to 3 deep.
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

    /// Look into zip and gzip members too, nested up to 3 deep.
    ///
    /// Members skipped (encrypted, another compression method, a limit
    /// reached) are reported on standard error.
    #[arg(long)]
    inflate: bool,

    /// Print where each descriptor was found: NAME, a tab, its origin.
    ///
    /// The origin is its byte range, `START..END`, preceded with --inflate
    /// by the members it was found in, such as `gzip@2203648!0..1536` or
    /// `zip@7340032!assets/api.binpb!88..4210`.
    #[arg(long)]
    origin: bool,
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

    match scan(&data, &cli) {
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

/// Prints each descriptor's name and writes it as `--proto-out` asks.
fn scan(data: &[u8], cli: &Cli) -> Result<(), String> {
    let mut out = Out {
        stdout: std::io::stdout().lock(),
        dir: cli.proto_out.as_deref(),
        origin: cli.origin,
        written: HashMap::new(),
    };
    if !cli.inflate {
        let top = Origin::default();
        for (start, end) in fdp_scan::scan(data) {
            out.descriptor(&top, data, start, end)?;
        }
        return Ok(());
    }
    let mut result = Ok(());
    fdp_scan::scan_inflating(
        data,
        &ScanOptions::default(),
        &InflateLimits::default(),
        |visit| {
            if result.is_err() {
                return;
            }
            result = match visit {
                Visit::Scanned {
                    origin,
                    data,
                    explained,
                } => explained
                    .found
                    .iter()
                    .try_for_each(|&(start, end)| out.descriptor(origin, data, start, end)),
                Visit::Skipped { origin, reason } => {
                    eprintln!("skipped {}: {}", origin.chain(None), reason.code());
                    Ok(())
                }
            };
        },
    );
    result
}

/// Where descriptors go.
struct Out<'a> {
    stdout: std::io::StdoutLock<'a>,
    dir: Option<&'a Path>,
    origin: bool,
    /// Per path written under `dir`, the bytes of each variant, in order:
    /// the same bytes again are not written twice.
    written: HashMap<PathBuf, Vec<Vec<u8>>>,
}

impl Out<'_> {
    fn descriptor(
        &mut self,
        origin: &Origin,
        data: &[u8],
        start: usize,
        end: usize,
    ) -> Result<(), String> {
        let blob = &data[start..end];
        let at = origin.text(None, start, end);
        // The scanner only returns records it has already scored as
        // FileDescriptorProtos (spec 0239 G4): decoding is how the name
        // is read, not a filter.
        let name = FileDescriptorProto::decode(blob)
            .map_err(|e| format!("descriptor at {at} does not decode: {e}"))?
            .name
            .unwrap_or_default();
        if self.origin {
            writeln!(self.stdout, "{name}\t{at}")
        } else {
            writeln!(self.stdout, "{name}")
        }
        .map_err(|e| e.to_string())?;
        let Some(dir) = self.dir else { return Ok(()) };
        let mut path = dir.join(&name);
        if path.extension() == Some(OsStr::new("proto")) {
            path.set_extension("pb");
        }
        let variants = self.written.entry(path.clone()).or_default();
        if variants.iter().any(|v| v == blob) {
            return Ok(());
        }
        variants.push(blob.to_vec());
        // A second variant of a name is `<name>~2.pb`, a third `~3`, as
        // git-seriate's store names them (spec 0480 S7).
        let n = variants.len();
        if n > 1 {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let ext = path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            path.set_file_name(format!("{stem}~{n}{ext}"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, blob).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}
