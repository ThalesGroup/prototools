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
use git_seriate::history::{self, Cutoff, Options, Plugins};
use git_seriate::plugins::{AsIs, NoRules, Pattern};
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
    /// Rebuild the recent history in sorted order.
    Settle {
        /// The history branch; the current one by default.
        #[arg(long)]
        into: Option<String>,
        /// Re-sort from this date (YYYY-MM-DD, or seconds since the epoch);
        /// by default, the oldest new snapshot's less the settling window.
        #[arg(long, conflicts_with = "no_cutoff")]
        cutoff: Option<String>,
        /// Re-sort the whole history.
        #[arg(long)]
        no_cutoff: bool,
        #[arg(long, default_value_t = 60)]
        settling_days: u32,
        /// Show what would be done, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// The snapshot branches to settle; every new one by default.
        branches: Vec<String>,
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
        Command::Settle {
            into,
            cutoff,
            no_cutoff,
            settling_days,
            dry_run,
            branches,
        } => {
            let cutoff = match (cutoff, no_cutoff) {
                (_, true) => Cutoff::Off,
                (Some(date), _) => Cutoff::At(parse_date(&date)?),
                (None, false) => Cutoff::Auto,
            };
            let pattern = repo
                .config_snapshot()
                .string("seriate.itemPattern")
                .map(|p| p.to_string());
            let item_paths = Pattern(pattern);
            let options = Options {
                into,
                cutoff,
                settling_days,
                dry_run,
                branches,
                sort: seriation::Settings::default(),
                plugins: Plugins {
                    renderer: &AsIs,
                    canonicalizer: &NoRules,
                    item_paths: &item_paths,
                    comparator: &seriation::LinesChars,
                },
            };
            let report = history::settle(&repo, &options)?;
            for w in &report.warnings {
                eprintln!("warning: {w}");
            }
            for n in &report.notes {
                eprintln!("{n}");
            }
            for r in &report.replayed {
                println!("replay {r}");
            }
            let verb = if dry_run { "would settle" } else { "settled" };
            println!(
                "{verb} {}: kept {}, wrote {} snapshot(s), replayed {} commit(s){}",
                report.branch,
                report.kept,
                report.written.len(),
                report.replayed.len(),
                report
                    .backup
                    .as_ref()
                    .map_or(String::new(), |b| format!("; previous tip saved as {b}"))
            );
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

/// `YYYY-MM-DD` (midnight UTC), or seconds since the epoch.
fn parse_date(text: &str) -> Result<i64, Error> {
    if let Ok(seconds) = text.parse::<i64>() {
        return Ok(seconds);
    }
    let bad = || Error::Refused(format!("{text:?}: expected YYYY-MM-DD or seconds"));
    let parts: Vec<&str> = text.split('-').collect();
    let [y, m, d] = parts[..] else {
        return Err(bad());
    };
    let (y, m, d): (i64, i64, i64) = (
        y.parse().map_err(|_| bad())?,
        m.parse().map_err(|_| bad())?,
        d.parse().map_err(|_| bad())?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Ok((era * 146_097 + doe - 719_468) * 86_400)
}
