// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `git seriate-fdp`: git-seriate for the protobuf descriptors embedded in
//! executables (spec 0453 S5). Every `git seriate` command, with the
//! descriptor plugins, plus `extract`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

use git_seriate::store::Added;
use git_seriate::Error;
use git_seriate_fdp::advise;
use git_seriate_fdp::extract::{extract_files_into, Options};

#[derive(Subcommand)]
enum Fdp {
    /// Add the descriptors each FILE embeds as a snapshot.
    Extract {
        /// The identity, for one file; its path by default.
        #[arg(long)]
        id: Option<String>,
        /// Seconds since the epoch; the file's modification time by default.
        #[arg(long)]
        build_time: Option<i64>,
        /// A revision number that grows with time, when the source has
        /// one: recorded and shown, not used to sort.
        #[arg(long)]
        order_hint: Option<u64>,
        /// What to extract again to refresh this snapshot.
        #[arg(long)]
        group: Option<String>,
        /// KEY=VALUE, shown, never interpreted; repeatable.
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        metadata: Vec<String>,
        /// Require fields in number order, as protoc writes them (spec
        /// 0449): for inputs known to come from protoc.
        #[arg(long)]
        increasing_field_numbers: bool,
        /// Look into zip and gzip members too, nested up to 3 deep (spec
        /// 0480). Each descriptor found in one records its origin; changes
        /// what a snapshot holds, so a choice to make once for a history.
        #[arg(long)]
        inflate: bool,
        /// Write the snapshots as an archive of a bare repository to FILE,
        /// or to standard output for -, instead of into a repository: none
        /// is needed (unpack with `mkdir r.git && tar xf FILE -C r.git`).
        #[arg(long, value_name = "FILE")]
        tar_out: Option<PathBuf>,
        /// The executables, libraries or other files to scan.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// What the history can't hold in one schema database, and which
    /// sources could fix it.
    Advise {
        /// The history branch; the current one by default.
        #[arg(long)]
        into: Option<String>,
        /// Write fixups.yaml and STATUS.md into DIR.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// The actions as JSON.
        #[arg(long)]
        json: bool,
        /// Only check whether the generated files in --out (or .) are
        /// current.
        #[arg(long)]
        check: bool,
        /// A protoback report of a build with the current fixups.yaml.
        #[arg(long, value_name = "FILE")]
        report: Option<PathBuf>,
        /// With --report: exit 1 if the build found what advise didn't
        /// foresee (a finding attached to no problem, an idle directive),
        /// listed under "Not foreseen:".
        #[arg(long, requires = "report")]
        strict: bool,
    },
}

fn main() -> ExitCode {
    git_seriate::cli::run::<Fdp>(
        "git seriate-fdp",
        git_seriate_fdp::kit,
        |command, repo, kit| {
            let (
                id,
                build_time,
                order_hint,
                group,
                metadata,
                increasing_field_numbers,
                inflate,
                tar_out,
                files,
            ) = match command {
                Fdp::Advise {
                    into,
                    out,
                    json,
                    check,
                    report,
                    strict,
                } => {
                    let options = advise::Options {
                        into,
                        out,
                        json,
                        check,
                        report,
                        strict,
                    };
                    let mut text = String::new();
                    let status = advise::run(repo, kit, &advise::Generic, &options, &mut text)?;
                    print!("{text}");
                    if status != 0 {
                        std::process::exit(i32::from(status));
                    }
                    return Ok(());
                }
                Fdp::Extract {
                    id,
                    build_time,
                    order_hint,
                    group,
                    metadata,
                    increasing_field_numbers,
                    inflate,
                    tar_out,
                    files,
                } => (
                    id,
                    build_time,
                    order_hint,
                    group,
                    metadata,
                    increasing_field_numbers,
                    inflate,
                    tar_out,
                    files,
                ),
            };
            let metadata = metadata
                .into_iter()
                .map(|kv| match kv.split_once('=') {
                    Some((k, v)) => Ok((k.to_string(), v.to_string())),
                    None => Err(Error::Refused(format!("--meta {kv:?}: expected KEY=VALUE"))),
                })
                .collect::<Result<_, _>>()?;
            let options = Options {
                id,
                build_time,
                order_hint,
                group,
                metadata,
                increasing_field_numbers,
                inflate,
            };
            let mut warnings = Vec::new();
            let mut archive = git_seriate::archive::Archive::new();
            let mut sink = match &tar_out {
                Some(_) => git_seriate::store::Sink::Archive(&mut archive),
                None => git_seriate::store::Sink::Repo(repo),
            };
            let outcomes =
                extract_files_into(repo, &mut sink, kit, &files, &options, &mut warnings)?;
            if let Some(out) = &tar_out {
                archive.write_to_path(out)?;
            }
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            // Standard output holds the archive with `--tar-out -`.
            let say = |line: String| {
                if tar_out.as_deref() == Some(std::path::Path::new("-")) {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            };
            for o in outcomes {
                match o.added {
                    Added::Written { branch, rejected } => {
                        for r in &rejected {
                            eprintln!("{}: refused {} ({})", o.file.display(), r.name, r.reason);
                        }
                        say(format!("added {branch}"));
                    }
                    Added::Unchanged { branch } => say(format!("unchanged {branch}")),
                    Added::Nothing => say(format!("{}: no descriptor", o.file.display())),
                }
            }
            Ok(())
        },
    )
}
