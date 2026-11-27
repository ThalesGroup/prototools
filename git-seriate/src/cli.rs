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
        /// Its identity, and so its ref, refs/seriate/snapshots/<ID>; DIR's
        /// name by default.
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
        /// Write the snapshot as an archive of a bare repository to FILE,
        /// or to standard output for -, instead of into a repository: none
        /// is needed (unpack with `mkdir r.git && tar xf FILE -C r.git`).
        #[arg(long, value_name = "FILE")]
        tar_out: Option<PathBuf>,
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
        /// Canonicalize every item, trusting no history commit, and report
        /// each one a commit claims canonical that isn't: exit 1 if any.
        /// With --dry-run, a check that writes nothing.
        #[arg(long)]
        verify_canon: bool,
        /// Print, on standard error, what each phase of the sort did, and
        /// every move of the greedy phases: which snapshot, from where to
        /// where (positions among the snapshots sorted, 0 the oldest).
        #[arg(long)]
        stats: bool,
        /// The snapshots to settle; every new one by default.
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
    /// List the snapshots.
    List {
        /// The record-only ones instead.
        #[arg(long)]
        record_only: bool,
    },
    /// Forget snapshots: out of the history, which is rewritten from the
    /// oldest of them (the previous tip saved), and out of the store; each
    /// kept, with the reason, under refs/seriate/forgotten/.
    Forget {
        /// The history branch; the current one by default.
        #[arg(long)]
        into: Option<String>,
        /// Why: kept with each forgotten snapshot.
        #[arg(long, required = true)]
        reason: String,
        /// Show what would be done, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// The snapshots' identities, or globs of them.
        #[arg(required = true)]
        identities: Vec<String>,
    },
    /// Delete snapshots the history holds: it rebuilds one when it needs
    /// it. (Formerly `drop`.)
    Prune {
        /// Only the snapshots matching this glob.
        #[arg(long)]
        glob: Option<String>,
        /// The record-only ones instead.
        #[arg(long)]
        record_only: bool,
    },
    /// `prune`, under its former name: warns, then prunes.
    #[command(hide = true)]
    Drop {
        /// Only the snapshots matching this glob.
        #[arg(long)]
        glob: Option<String>,
        /// The record-only ones instead.
        #[arg(long)]
        record_only: bool,
    },
    /// Copy the snapshots of other repositories, of either layout.
    Import {
        /// The repositories to copy them from.
        #[arg(required = true)]
        repos: Vec<PathBuf>,
    },
    /// State which producer's lineage snapshots belong to, after extracting
    /// them; `settle` sorts each lineage on its own.
    Lineage {
        #[command(subcommand)]
        action: LineageAction,
    },
    /// Move a repository of before spec 0499 to its layout: its snapshot
    /// branches under refs/seriate/snapshots/.
    Migrate {
        /// Show what would be moved, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum LineageAction {
    /// Put snapshots in lineage NAME: `set NAME IDENTITY...`, or `set
    /// --from FILE`, one `<identity> <lineage>` per line (- for standard
    /// input).
    Set {
        /// The statements, one `<identity> <lineage>` per line.
        #[arg(long, value_name = "FILE", conflicts_with_all = ["name", "identities"])]
        from: Option<PathBuf>,
        /// Show what would change, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// The lineage.
        name: Option<String>,
        /// The snapshots' identities.
        identities: Vec<String>,
    },
    /// Put snapshots back in the default lineage.
    Unset {
        /// Show what would change, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// The snapshots' identities.
        #[arg(required = true)]
        identities: Vec<String>,
    },
    /// The statements in force, `<identity> <lineage>`, those of lineage
    /// NAME only when given.
    List {
        /// Only this lineage's.
        name: Option<String>,
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
    // A command given `--tar-out` writes an archive: it needs no repository
    // (spec 0466 S3), the generic `add` as a front end's own.
    let tar_out = matches
        .subcommand()
        .and_then(|(_, m)| m.try_get_one::<PathBuf>("tar_out").ok().flatten())
        .is_some();
    match dispatch(cli, tar_out, kit, extra) {
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
    tar_out: bool,
    kit: impl FnOnce(&gix::Repository) -> Result<Kit, Error>,
    extra: impl FnOnce(E, &gix::Repository, &Kit) -> Result<(), Error>,
) -> Result<(), Error> {
    let start = cli.directory.unwrap_or_else(|| PathBuf::from("."));
    // With `--tar-out` and no repository, an empty one stands in, for the
    // settings a front end reads (all defaults); in a repository, only its
    // configuration is read (spec 0466 S3).
    let (repo, _stand_in) = match gix::discover(&start) {
        Ok(repo) => (repo, None),
        Err(_) if tar_out => {
            let dir = tempfile::tempdir()?;
            let repo = gix::init_bare(dir.path()).map_err(Error::git)?;
            (repo, Some(dir))
        }
        Err(e) => return Err(Error::Git(e.to_string())),
    };
    let kit = &kit(&repo)?;
    // Every command but `migrate` and `import` reads or writes the store as
    // laid out since spec 0499: a repository of before it is refused.
    let former_ok = matches!(
        cli.command,
        Command::Migrate { .. } | Command::Import { .. } | Command::Extra(_)
    ) || tar_out;
    if !former_ok {
        refuse_former_layout(&repo, kit)?;
    }
    match cli.command {
        Command::Extra(e) => extra(e, &repo, kit)?,
        Command::Add {
            id,
            build_time,
            order_hint,
            group,
            metadata,
            tar_out,
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
            let mut archive = crate::archive::Archive::new();
            let added = match &tar_out {
                Some(_) => store::add_to(&mut store::Sink::Archive(&mut archive), snapshot)?,
                None => store::add(&repo, snapshot)?,
            };
            if let Some(out) = &tar_out {
                archive.write_to_path(out)?;
            }
            // Standard output holds the archive with `--tar-out -`.
            let say = |line: String| {
                if tar_out.as_deref() == Some(Path::new("-")) {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            };
            match added {
                Added::Written { identity, rejected } => {
                    for r in &rejected {
                        eprintln!("warning: {}: refused ({})", r.name, r.reason);
                    }
                    say(format!("added {identity}"));
                }
                Added::Unchanged { identity } => say(format!("unchanged {identity}")),
                Added::Nothing => say(format!("nothing to add: {} holds no file", dir.display())),
            }
        }
        Command::Settle {
            into,
            cutoff,
            no_cutoff,
            settling_days,
            dry_run,
            verify_canon,
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
                verify_canon,
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
            if !report.false_markers.is_empty() {
                for f in &report.false_markers {
                    eprintln!("not canonical: {f}");
                }
                return Err(Error::Refused(format!(
                    "{} item(s) of the history are not what their commit's canon_rules and \
                     renderer claim",
                    report.false_markers.len()
                )));
            }
        }
        Command::Forget {
            into,
            reason,
            dry_run,
            identities,
        } => {
            let pattern = repo
                .config_snapshot()
                .string("seriate.itemPattern")
                .map(|p| p.to_string())
                .or_else(|| kit.item_pattern.clone());
            let item_paths = Pattern(pattern);
            let forgotten = history::forget(
                &repo,
                &history::ForgetOptions {
                    into,
                    patterns: identities,
                    reason,
                    dry_run,
                    sort: seriation::Settings::default(),
                    plugins: Plugins {
                        renderer: kit.renderer.as_ref(),
                        canonicalizer: kit.canonicalizer.as_ref(),
                        item_paths: &item_paths,
                        comparator: kit.comparator.as_ref(),
                        legacy: kit.legacy.as_deref(),
                    },
                },
            )?;
            let report = &forgotten.settle;
            for w in &report.warnings {
                eprintln!("warning: {w}");
            }
            for n in &report.notes {
                eprintln!("{n}");
            }
            let verb = if dry_run { "would forget" } else { "forgot" };
            for identity in &forgotten.forgotten {
                println!("{verb} {identity}");
            }
            for b in &forgotten.deleted {
                let verb = if dry_run { "would delete" } else { "deleted" };
                println!("{verb} its snapshot {b}");
            }
            println!(
                "{} {}: kept {}, rewrote {} snapshot(s), replayed {} commit(s){}",
                if dry_run { "would rewrite" } else { "rewrote" },
                report.branch,
                report.kept,
                report.written.len(),
                report.replayed.len(),
                report
                    .backup
                    .as_ref()
                    .map_or(String::new(), |b| format!("; previous tip saved as {b}"))
            );
            let kept = if dry_run {
                "would be kept by"
            } else {
                "kept by"
            };
            println!("{kept} {}", forgotten.record);
            println!(
                "next: extract their replacements, if any, then settle, before sharing {}",
                report.branch
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
            let plugins = crate::compact::Plugins {
                legacy: kit.legacy.as_deref(),
                renderer: kit.renderer.as_ref(),
                canonicalizer: kit.canonicalizer.as_ref(),
            };
            let report = crate::compact::compact(&repo, &options, &plugins)?;
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
                "{verb} {}: dropped {}, reduced {}, converted {}, marked {}{}",
                report.branch,
                report.dropped,
                report.reduced,
                report.converted,
                report.marked,
                report
                    .backup
                    .as_ref()
                    .map_or(String::new(), |b| format!("; previous tip saved as {b}"))
            );
        }
        Command::List { record_only } => {
            for listed in store::list_with(&repo, kit.legacy.as_deref())? {
                if listed.block.contents.is_none() == record_only {
                    println!("{}", listed.identity);
                }
            }
        }
        Command::Drop { glob, record_only } => {
            eprintln!("warning: drop is now prune, which says what it does: use prune");
            prune(&repo, glob.as_deref(), record_only, kit)?;
        }
        Command::Prune { glob, record_only } => prune(&repo, glob.as_deref(), record_only, kit)?,
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
        Command::Lineage { action } => lineage(&repo, action, kit)?,
        Command::Migrate { dry_run } => {
            let moved = crate::layout::migrate_snapshots(&repo, dry_run, kit.legacy.as_deref())?;
            let verb = if dry_run { "would move" } else { "moved" };
            println!(
                "{verb} {} snapshot(s) to {}",
                moved.len(),
                crate::layout::SNAPSHOTS
            );
        }
    }
    Ok(())
}

/// `lineage set`, `unset`, `list` (spec 0499 S1).
fn lineage(repo: &gix::Repository, action: LineageAction, kit: &Kit) -> Result<(), Error> {
    use crate::layout::DEFAULT_LINEAGE;
    let (statements, command, dry_run) = match action {
        LineageAction::List { name } => {
            for (identity, lineage) in crate::lineage::read(repo)?.lineages {
                if name.as_deref().is_none_or(|n| n == lineage) {
                    println!("{identity} {lineage}");
                }
            }
            return Ok(());
        }
        LineageAction::Set {
            from: Some(file),
            dry_run,
            ..
        } => {
            let text = if file.as_os_str() == "-" {
                std::io::read_to_string(std::io::stdin())?
            } else {
                std::fs::read_to_string(&file).map_err(|e| {
                    Error::Io(std::io::Error::new(
                        e.kind(),
                        format!("{}: {e}", file.display()),
                    ))
                })?
            };
            let statements = crate::lineage::parse_statements(&text)
                .map_err(|e| Error::Refused(format!("{}: {e}", file.display())))?;
            if let Some((_, l)) = statements.iter().find(|(_, l)| l == DEFAULT_LINEAGE) {
                crate::layout::check_lineage_name(l)?;
            }
            (
                statements,
                format!("set --from {}", file.display()),
                dry_run,
            )
        }
        LineageAction::Set {
            from: None,
            dry_run,
            name,
            identities,
        } => {
            let Some(name) = name.filter(|_| !identities.is_empty()) else {
                return Err(Error::Refused(
                    "lineage set: give NAME and IDENTITY..., or --from FILE".into(),
                ));
            };
            crate::layout::check_lineage_name(&name)?;
            let statements = identities.into_iter().map(|i| (i, name.clone())).collect();
            (statements, format!("set {name}"), dry_run)
        }
        LineageAction::Unset {
            dry_run,
            identities,
        } => {
            let statements = identities
                .into_iter()
                .map(|i| (i, DEFAULT_LINEAGE.to_string()))
                .collect();
            (statements, "unset".to_string(), dry_run)
        }
    };
    let stated =
        crate::lineage::state(repo, &statements, &command, dry_run, kit.legacy.as_deref())?;
    for (identity, from, to) in &stated.changed {
        let verb = if dry_run { "would move" } else { "moved" };
        println!("{verb} {identity}: {from} -> {to}");
    }
    for identity in &stated.unchanged {
        println!("unchanged {identity}");
    }
    match stated.record {
        Some(record) => println!(
            "recorded in {} ({}); settle to move them",
            crate::layout::LINEAGES,
            record.to_hex_with_len(8)
        ),
        None if !stated.changed.is_empty() => {}
        None => println!("nothing to record"),
    }
    Ok(())
}

/// A repository whose snapshots are still branches, as before spec 0499,
/// refused, naming `migrate`: for a front end's own commands that read or
/// write the store.
pub fn refuse_former_layout(repo: &gix::Repository, kit: &Kit) -> Result<(), Error> {
    store::refuse_former_layout(repo, kit.legacy.as_deref())
}

/// `prune`, and `drop` under its former name: the snapshots deleted,
/// listed.
fn prune(
    repo: &gix::Repository,
    glob: Option<&str>,
    record_only: bool,
    kit: &Kit,
) -> Result<(), Error> {
    for identity in store::prune(repo, glob, record_only, kit.legacy.as_deref())? {
        println!("deleted {identity}");
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
                    record: Vec::new(),
                    origin: None,
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
