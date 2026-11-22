// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The command line (spec 0452 S8), as a library entry point (spec 0453
//! S1): `git-seriate`'s binary runs it with the generic plugins, a front
//! end with its own and its own extra commands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::format::Source;
use crate::history::{self, Cutoff, Options, Plugins};
use crate::plugins::{AsIs, Canonicalizer, LegacyReader, NoRules, Pattern, Renderer};
use crate::store::{self, Added, Item, NewSnapshot};
use crate::Error;

/// The plugins a binary runs the commands with (spec 0452 S2).
pub struct Kit {
    pub renderer: Box<dyn Renderer>,
    pub canonicalizer: Box<dyn Canonicalizer>,
    pub comparator: Box<dyn seriation::Comparator>,
    pub legacy: Option<Box<dyn LegacyReader>>,
    /// The item pattern when the repository sets no `seriate.itemPattern`;
    /// every path when `None`.
    pub item_pattern: Option<String>,
}

impl Kit {
    /// The generic plugins: items stored as they are, no rule set, the text
    /// comparator, no legacy reader.
    pub fn generic() -> Self {
        Kit {
            renderer: Box::new(AsIs),
            canonicalizer: Box::new(NoRules),
            comparator: Box::new(seriation::LinesChars),
            legacy: None,
            item_pattern: None,
        }
    }
}

/// No extra command: the generic binary's.
#[derive(Subcommand, Debug, Clone, Copy)]
pub enum NoExtra {}

#[derive(Parser)]
#[command(version, about)]
struct Cli<E: Subcommand> {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    directory: Option<PathBuf>,
    #[command(subcommand)]
    command: Command<E>,
}

#[derive(Subcommand)]
enum Command<E: Subcommand> {
    #[command(flatten)]
    Extra(E),
    /// Add the files under DIR as a snapshot.
    Add {
        /// Its identity, and so its branch name; DIR's name by default.
        #[arg(long)]
        id: Option<String>,
        /// Seconds since the epoch; the newest file's modification time by
        /// default.
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
        /// The directory whose files, at any depth, are the snapshot.
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
        /// Print, on standard error, what each phase of the sort did, and
        /// every move of the greedy phases: which snapshot, from where to
        /// where (positions among the snapshots sorted, 0 the oldest).
        #[arg(long)]
        stats: bool,
        /// The snapshot branches to settle; every new one by default.
        branches: Vec<String>,
    },
    /// Garbage-collect the history before a cutoff.
    Compact {
        /// The history branch; the current one by default.
        #[arg(long)]
        into: Option<String>,
        /// Compact what was built before this date (YYYY-MM-DD, or seconds
        /// since the epoch).
        #[arg(long)]
        cutoff: String,
        /// 1 (lossless): drop manual commits that change nothing, and second
        /// copies of a snapshot. 2: also drop snapshot commits that change
        /// nothing, their sources' records with them, and the commits whose
        /// every change is made again later. 3: also remove from snapshot
        /// commits the items changed again later.
        #[arg(long, default_value_t = 1)]
        level: u8,
        /// List what would be dropped, and write nothing.
        #[arg(long)]
        dry_run: bool,
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
        /// The repositories to copy them from.
        #[arg(required = true)]
        repos: Vec<PathBuf>,
    },
}

/// Parse the command line and run it, as `name` (`"git seriate"` for the
/// generic binary), with the plugins `kit` makes for the repository: a
/// front end may read its own settings from it. `extra` runs the front
/// end's own commands, `E`, given the repository and the kit.
pub fn run<E: Subcommand>(
    name: &'static str,
    kit: impl FnOnce(&gix::Repository) -> Result<Kit, Error>,
    extra: impl FnOnce(E, &gix::Repository, &Kit) -> Result<(), Error>,
) -> ExitCode {
    let command = || Cli::<E>::command().name(name).bin_name(name);
    // `<NAME>_COMPLETE=<shell>` prints a completion script and exits;
    // `<NAME>_GEN_MAN=<dir>` writes the man pages and exits, before any
    // argument is parsed (as protoscan does, spec 0407 S4).
    let env = env_prefix(name);
    // Completions register against the executable, `git-seriate`: the
    // name a shell sees, and the one `git seriate` runs.
    let exe = leak(name.replace(' ', "-"));
    clap_complete::CompleteEnv::with_factory(move || Cli::<E>::command().name(exe).bin_name(exe))
        .var(leak(format!("{env}_COMPLETE")))
        .shells(prototools_complete::shells())
        .complete();
    if let Some(dir) = std::env::var_os(format!("{env}_GEN_MAN")) {
        match write_man(command(), name, &PathBuf::from(dir)) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("{name}: writing the man pages: {e}");
                std::process::exit(1);
            }
        }
    }
    let matches = command().get_matches();
    let cli = match Cli::<E>::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };
    match dispatch(cli, kit, extra) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A name clap keeps for the process's life; made once per run.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// `settle --stats`: a line per phase of the sort, and one per move of the
/// greedy phases, its snapshot named with its group and its neighbors.
fn sort_stats(report: &crate::history::Report) -> String {
    use seriation::Phase;
    use std::fmt::Write as _;
    let name = |i: usize| {
        let (id, group) = &report.sorted[i];
        match group {
            Some(g) => format!("{id} [{g}]"),
            None => id.clone(),
        }
    };
    let mut out = String::new();
    if report.phases.is_empty() {
        out.push_str("sort: nothing sorted\n");
        return out;
    }
    let _ = writeln!(out, "sort: {} snapshot(s)", report.sorted.len());
    for p in &report.phases {
        let phase = match p.phase {
            Phase::Load => continue,
            Phase::BuildTime => "1 build time",
            Phase::OrderHints => "2 order hints",
            Phase::Containment => "3 containment",
            Phase::Interleaving => "4 interleaving",
        };
        if p.skipped {
            let _ = writeln!(out, "phase {phase}: skipped");
            continue;
        }
        let mut line = if p.phase == Phase::BuildTime {
            format!("phase {phase}: {} position(s) changed", p.moves)
        } else {
            format!("phase {phase}: {} move(s)", p.moves)
        };
        if p.phase == Phase::Containment {
            let _ = write!(
                line,
                ", {} comparison(s), {} ordered",
                p.comparisons, p.signals
            );
        }
        if let Some(s) = p.edge_score {
            let _ = write!(line, "; edge score {s}");
        }
        if let Some(s) = p.interleaving_score {
            let _ = write!(line, "; interleaving score {s}");
        }
        let _ = writeln!(out, "{line}");
        for m in &p.moved {
            let _ = write!(
                out,
                "  move {}: {} -> {}, +{}",
                name(m.snapshot),
                m.from,
                m.to,
                m.gain
            );
            let _ = match (m.after, m.before) {
                (Some(a), Some(b)) => writeln!(out, "; now between {} and {}", name(a), name(b)),
                (Some(a), None) => writeln!(out, "; now last, after {}", name(a)),
                (None, Some(b)) => writeln!(out, "; now first, before {}", name(b)),
                (None, None) => writeln!(out),
            };
        }
    }
    out
}

/// `GIT_SERIATE` for `git seriate`, `GIT_SERIATE_FDP` for `git seriate-fdp`.
fn env_prefix(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// `git-seriate.1`, and `git-seriate-<command>.1` for each command, in
/// `dir`: the names `git help seriate` and `man git-seriate-settle` look
/// for.
fn write_man(command: clap::Command, name: &str, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let page = name.replace(' ', "-");
    let render = |cmd: clap::Command, file: &str, title: &str| -> std::io::Result<()> {
        let man = clap_mangen::Man::new(cmd)
            .title(title.to_uppercase())
            .section("1")
            .source(page.clone())
            .manual("User Commands");
        let mut buf = Vec::new();
        man.render(&mut buf)?;
        std::fs::write(dir.join(format!("{file}.1")), buf)
    };
    for sub in command.get_subcommands() {
        if sub.get_name() == "help" {
            continue;
        }
        let sub_page = format!("{page}-{}", sub.get_name());
        let sub = sub
            .clone()
            .name(leak(sub_page.clone()))
            .bin_name(format!("{name} {}", sub.get_name()));
        render(sub, &sub_page, &sub_page)?;
    }
    render(command, &page, &page)
}

fn dispatch<E: Subcommand>(
    cli: Cli<E>,
    kit: impl FnOnce(&gix::Repository) -> Result<Kit, Error>,
    extra: impl FnOnce(E, &gix::Repository, &Kit) -> Result<(), Error>,
) -> Result<(), Error> {
    let start = cli.directory.unwrap_or_else(|| PathBuf::from("."));
    let repo = gix::discover(&start).map_err(|e| Error::Git(e.to_string()))?;
    let kit = &kit(&repo)?;
    match cli.command {
        Command::Extra(e) => extra(e, &repo, kit)?,
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
            stats,
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
                .map(|p| p.to_string())
                .or_else(|| kit.item_pattern.clone());
            let item_paths = Pattern(pattern);
            let options = Options {
                into,
                cutoff,
                settling_days,
                dry_run,
                branches,
                sort: seriation::Settings::default(),
                plugins: Plugins {
                    renderer: kit.renderer.as_ref(),
                    canonicalizer: kit.canonicalizer.as_ref(),
                    item_paths: &item_paths,
                    comparator: kit.comparator.as_ref(),
                    legacy: kit.legacy.as_deref(),
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
            if stats {
                eprint!("{}", sort_stats(&report));
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
        Command::Compact {
            into,
            cutoff,
            level,
            dry_run,
        } => {
            let options = crate::compact::Options {
                into,
                cutoff: parse_date(&cutoff)?,
                level,
                dry_run,
            };
            let report = crate::compact::compact(&repo, &options, kit.legacy.as_deref())?;
            for a in &report.actions {
                println!("{a}");
            }
            for w in &report.warnings {
                eprintln!("warning: {w}");
            }
            let verb = if dry_run {
                "would compact"
            } else {
                "compacted"
            };
            println!(
                "{verb} {}: dropped {}, reduced {}, converted {}{}",
                report.branch,
                report.dropped,
                report.reduced,
                report.converted,
                report
                    .backup
                    .as_ref()
                    .map_or(String::new(), |b| format!("; previous tip saved as {b}"))
            );
        }
        Command::List { record_only } => {
            for listed in store::list_with(&repo, kit.legacy.as_deref())? {
                if listed.block.contents.is_none() == record_only {
                    println!("{}", listed.branch);
                }
            }
        }
        Command::Drop { glob, record_only } => {
            for branch in store::drop(&repo, glob.as_deref(), record_only, kit.legacy.as_deref())? {
                println!("deleted {branch}");
            }
        }
        Command::Import { repos } => {
            for from in repos {
                let report = store::import(&repo, &from, kit.legacy.as_deref())?;
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
pub fn read_dir(dir: &Path) -> Result<(Vec<Item>, i64), Error> {
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
pub fn parse_date(text: &str) -> Result<i64, Error> {
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
