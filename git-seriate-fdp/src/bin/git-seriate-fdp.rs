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
        #[arg(long)]
        order_hint: Option<u64>,
        #[arg(long)]
        group: Option<String>,
        /// KEY=VALUE, shown, never interpreted; repeatable.
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        metadata: Vec<String>,
        /// Require fields in number order, as protoc writes them (spec
        /// 0449): for inputs known to come from protoc.
        #[arg(long)]
        increasing_field_numbers: bool,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    git_seriate::cli::run::<Fdp>(
        "git seriate-fdp",
        git_seriate_fdp::kit,
        |command, repo, kit| {
            let Fdp::Extract {
                id,
                build_time,
                order_hint,
                group,
                metadata,
                increasing_field_numbers,
                files,
            } = command;
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
