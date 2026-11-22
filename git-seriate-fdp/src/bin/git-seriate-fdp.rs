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
use git_seriate_fdp::extract::{extract_files, Options};

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
            let (id, build_time, order_hint, group, metadata, increasing_field_numbers, files) =
                match command {
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
                        files,
                    } => (
                        id,
                        build_time,
                        order_hint,
                        group,
                        metadata,
                        increasing_field_numbers,
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
            };
            let mut warnings = Vec::new();
            let outcomes = extract_files(repo, kit, &files, &options, &mut warnings)?;
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            for o in outcomes {
                match o.added {
                    Added::Written { branch, rejected } => {
                        for r in &rejected {
                            eprintln!("{}: refused {} ({})", o.file.display(), r.name, r.reason);
                        }
                        println!("added {branch}");
                    }
                    Added::Unchanged { branch } => println!("unchanged {branch}"),
                    Added::Nothing => println!("{}: no descriptor", o.file.display()),
                }
            }
            Ok(())
        },
    )
}
