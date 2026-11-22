// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `git seriate`: store snapshots of files, and rebuild a history from them
//! (spec 0452 S8). git runs this binary as `git seriate` when it is on the
//! path.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use git_seriate::format::Source;
use git_seriate::store::{self, Added, Item, NewSnapshot};
use git_seriate::Error;

#[derive(Parser)]
#[command(name = "git seriate", version, about)]
struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    directory: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add the files under DIR as a snapshot.
    Add {
        /// Its identity, and so its branch name; DIR's name by default.
        #[arg(long)]
        id: Option<String>,
        /// Seconds since the epoch; the newest file's modification time by
        /// default.
        #[arg(long)]
        build_time: Option<i64>,
        #[arg(long)]
        order_hint: Option<u64>,
        /// What to extract again to refresh this snapshot.
        #[arg(long)]
        group: Option<String>,
        /// KEY=VALUE, shown, never interpreted; repeatable.
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        metadata: Vec<String>,
        dir: PathBuf,
    },
    /// List the snapshot branches.
    List {
        /// The record-only branches instead.
        #[arg(long)]
        record_only: bool,
    },
    /// Delete snapshot branches.
    Drop {
        /// Only the branches matching this glob.
        #[arg(long)]
        glob: Option<String>,
        /// The record-only branches instead.
        #[arg(long)]
        record_only: bool,
    },
    /// Copy the snapshot branches of other repositories.
    Import {
        #[arg(required = true)]
        repos: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("git seriate: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    let start = cli.directory.unwrap_or_else(|| PathBuf::from("."));
    let repo = gix::discover(&start).map_err(|e| Error::Git(e.to_string()))?;
    match cli.command {
        Command::Add {
            id,
            build_time,
            order_hint,
            group,
            metadata,
            dir,
        } => {
            let (items, newest) = read_dir(&dir)?;
            let identity = match id {
                Some(id) => id,
                None => dir
                    .canonicalize()?
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .ok_or_else(|| Error::Refused(format!("{}: give --id", dir.display())))?,
            };
            let metadata = metadata
                .into_iter()
                .map(|kv| match kv.split_once('=') {
                    Some((k, v)) => Ok((k.to_string(), v.to_string())),
                    None => Err(Error::Refused(format!("--meta {kv:?}: expected KEY=VALUE"))),
                })
                .collect::<Result<_, _>>()?;
            let snapshot = NewSnapshot {
                source: Source {
                    identity,
                    build_time: build_time.unwrap_or(newest),
                    order_hint,
                    group,
                    metadata,
                },
                items,
                rejected: Vec::new(),
                record: Vec::new(),
            };
            match store::add(&repo, snapshot)? {
                Added::Written { branch, rejected } => {
                    for r in &rejected {
                        eprintln!("warning: {}: refused ({})", r.name, r.reason);
                    }
                    println!("added {branch}");
                }
                Added::Unchanged { branch } => println!("unchanged {branch}"),
                Added::Nothing => println!("nothing to add: {} holds no file", dir.display()),
            }
        }
        Command::List { record_only } => {
            for listed in store::list(&repo)? {
                if listed.block.contents.is_none() == record_only {
                    println!("{}", listed.branch);
                }
            }
        }
        Command::Drop { glob, record_only } => {
            for branch in store::drop(&repo, glob.as_deref(), record_only)? {
                println!("deleted {branch}");
            }
        }
        Command::Import { repos } => {
            for from in repos {
                let report = store::import(&repo, &from)?;
                println!(
                    "{}: copied {}, already here {}",
                    from.display(),
                    report.copied.len(),
                    report.present.len()
                );
            }
        }
    }
    Ok(())
}

/// Every file under `dir`, path relative to it, and the newest
/// modification time, in seconds.
fn read_dir(dir: &Path) -> Result<(Vec<Item>, i64), Error> {
    let mut items = Vec::new();
    let mut newest = 0i64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let modified = entry.metadata()?.modified()?;
                let seconds = modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
                newest = newest.max(seconds);
                let relative = path
                    .strip_prefix(dir)
                    .expect("walked from dir")
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                items.push(Item {
                    path: relative,
                    bytes: std::fs::read(&path)?,
                });
            }
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((items, newest))
}
