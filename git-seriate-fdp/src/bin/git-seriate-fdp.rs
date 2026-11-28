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
        /// `sets`: each framed set an input embeds is a snapshot of its own,
        /// `<input>!set@<start>`, the rest keeping the input's identity;
        /// `none`: each input whole (spec 0495 S2).
        #[arg(long, value_name = "HOW", default_value = "sets",
              value_parser = ["sets", "none"])]
        split: String,
        /// One snapshot of every file, named ID: the files are one build,
        /// such as a runfiles tree's modules (spec 0495 S3).
        #[arg(long, value_name = "ID", conflicts_with = "id")]
        as_one: Option<String>,
        /// The files to scan, one per line, from FILE, or standard input for
        /// -: for more than a command line holds.
        #[arg(long, value_name = "FILE")]
        files_from: Option<PathBuf>,
        /// The executables, libraries or other files to scan.
        #[arg(required_unless_present = "files_from")]
        files: Vec<PathBuf>,
    },
    /// What each FILE tells about where its descriptors come from, beyond
    /// them (seriate-tuto spec 0007): one JSON line per file.
    Hints {
        /// The executables to read.
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
                split,
                as_one,
                files_from,
                mut files,
            ) = match command {
                Fdp::Hints { files } => {
                    for file in files {
                        let bytes = std::fs::read(&file).map_err(|e| {
                            Error::Io(std::io::Error::new(
                                e.kind(),
                                format!("{}: {e}", file.display()),
                            ))
                        })?;
                        let mut line = git_seriate_fdp::hints::hints(&bytes);
                        line["file"] = file.display().to_string().into();
                        println!("{line}");
                    }
                    return Ok(());
                }
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
                    git_seriate::cli::refuse_former_layout(repo, kit)?;
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
                    split,
                    as_one,
                    files_from,
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
                    split,
                    as_one,
                    files_from,
                    files,
                ),
            };
            if let Some(list) = &files_from {
                let text = if list.as_os_str() == "-" {
                    std::io::read_to_string(std::io::stdin())?
                } else {
                    std::fs::read_to_string(list).map_err(|e| {
                        Error::Io(std::io::Error::new(
                            e.kind(),
                            format!("{}: {e}", list.display()),
                        ))
                    })?
                };
                files.extend(text.lines().filter(|l| !l.is_empty()).map(PathBuf::from));
            }
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
                no_split: split == "none",
                as_one,
            };
            if tar_out.is_none() {
                git_seriate::cli::refuse_former_layout(repo, kit)?;
            }
            let mut warnings = Vec::new();
            let mut archive = git_seriate::archive::Archive::new();
            let mut sink = match &tar_out {
                Some(_) => git_seriate::store::Sink::Archive(&mut archive),
                None => git_seriate::store::Sink::Repo(repo),
            };
            // Into a repository, a snapshot a history holds is met with it
            // (spec 0500); an archive meets none.
            let meeter = git_seriate::cli::meeter(repo, kit)?;
            let meets = tar_out.is_none().then_some(&meeter);
            let outcomes =
                extract_files_into(repo, &mut sink, meets, kit, &files, &options, &mut warnings)?;
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
            // An input's parts (spec 0495): its rest may hold nothing while
            // its sets were written, so "no descriptor" only for an input
            // none of whose parts was.
            let found: std::collections::HashSet<String> = outcomes
                .iter()
                .filter(|o| !matches!(o.added, Added::Nothing))
                .map(|o| o.input.clone())
                .collect();
            for o in outcomes {
                match o.added {
                    Added::Written { identity, rejected } => {
                        for r in &rejected {
                            eprintln!("{}: refused {} ({})", o.input, r.name, r.reason);
                        }
                        say(format!("added {identity}"));
                    }
                    Added::Unchanged { identity } => say(format!("unchanged {identity}")),
                    Added::Replaced { identity } => say(format!(
                        "replaced {identity}: the original of the copy rebuilt from its history"
                    )),
                    Added::Refused { .. } => {}
                    Added::Nothing if !found.contains(&o.input) => {
                        say(format!("{}: no descriptor", o.input));
                    }
                    Added::Nothing => {}
                }
            }
            git_seriate::cli::met(&meeter, kit)
        },
    )
}
