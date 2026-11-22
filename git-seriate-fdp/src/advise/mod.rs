// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `advise`: what a history of descriptors can't hold, and which sources
//! could fix it (spec 0454).
//!
//! The analysis modules see an [`Input`]: the descriptors of the history's
//! tip and the sources its blocks describe. Nothing here knows where the
//! sources came from; a front end words the candidates through [`Wording`].

pub mod actions;
pub mod check;
pub mod decide;
pub mod fdp_info;
pub mod fixups;
pub mod output;
pub mod problems;
pub mod report;
pub mod status;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use gix::bstr::ByteSlice;
use gix::ObjectId;
use serde_json::Value;

use git_seriate::cli::Kit;
use git_seriate::format::Block;
use git_seriate::history::{commit_tree, diff};
use git_seriate::store::{branch_ref, tip};
use git_seriate::Error;

/// A descriptor of the history's tip.
#[derive(Debug, Clone)]
pub struct SnapshotFile {
    /// Its path in the tree (`foo/bar.pb`, `foo/bar~2.pb`).
    pub path: String,
    /// The `FileDescriptorProto`, serialized.
    pub fdp: Vec<u8>,
    /// The author date of the last commit writing it (seconds since the
    /// epoch): an upper bound of its build time, for a file no source lists.
    /// The author date survives every rewrite (spec 0454 S2).
    pub written_at: i64,
    /// That commit's subject.
    pub written_by: String,
}

/// A source, from a snapshot commit's block (spec 0454 S2).
#[derive(Debug, Clone)]
pub struct Source {
    /// Unique per source; a source extracted twice counts once.
    pub identity: String,
    /// Seconds since the epoch.
    pub build_time: i64,
    /// What to extract again to refresh it.
    pub group: Option<String>,
    /// The tree paths it holds.
    pub paths: BTreeSet<String>,
    /// Shown, never interpreted.
    pub metadata: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct Input {
    pub files: Vec<SnapshotFile>,
    pub sources: Vec<Source>,
}

/// How a front end words the candidates (spec 0454 S7).
pub trait Wording {
    fn candidates_heading(&self) -> String {
        "Candidates (a newer source may refresh these files; an educated guess):".into()
    }
    fn no_source_heading(&self) -> String {
        "No known source:".into()
    }
    /// A candidate group's newest source.
    fn newest(&self, source: &Value) -> String {
        format!(
            "newest source: {} ({})",
            source["identity"].as_str().unwrap_or("?"),
            date(&source["build_time"])
        )
    }
    /// A candidate group's sources.
    fn members(&self, sources: &[Value]) -> String {
        let mut ids: Vec<&str> = sources
            .iter()
            .filter_map(|s| s["identity"].as_str())
            .collect();
        ids.sort();
        ids.dedup();
        format!("sources: {}", ids.join(", "))
    }
}

/// The generic wording.
pub struct Generic;
impl Wording for Generic {}

/// `YYYY-MM-DD` of a JSON build time.
pub fn date(t: &Value) -> String {
    decide::date(t.as_i64().unwrap_or(0))
}

/// What to do.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The history branch; the current branch when `None`.
    pub into: Option<String>,
    /// Where to write `fixups.yaml` and `STATUS.md`.
    pub out: Option<PathBuf>,
    pub json: bool,
    pub check: bool,
    /// A protoback build report (spec 0446), read back.
    pub report: Option<PathBuf>,
}

/// Run `advise` (spec 0454 S1): its output on `out`, its exit status
/// returned, 0 or 1.
pub fn run(
    repo: &gix::Repository,
    kit: &Kit,
    wording: &dyn Wording,
    options: &Options,
    out: &mut String,
) -> Result<u8, Error> {
    let input = read(repo, kit, options.into.as_deref())?;
    if options.check {
        let dir = options.out.clone().unwrap_or_else(|| PathBuf::from("."));
        let mut files = Vec::new();
        for name in ["fixups.yaml", "STATUS.md"] {
            if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
                files.push((name.to_string(), text));
            }
        }
        let verdict = output::check(&input.sources, &files);
        let mut stale = false;
        for (name, v) in verdict.as_object().into_iter().flatten() {
            match v {
                Value::Null => writeln!(out, "{name}: up to date").unwrap(),
                Value::String(s) => {
                    stale = true;
                    writeln!(out, "{name}: {s}").unwrap();
                }
                v => {
                    stale = true;
                    let list = |k: &str| {
                        v[k].as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    writeln!(
                        out,
                        "{name}: out of date; new: {}; gone: {}",
                        list("new"),
                        list("gone")
                    )
                    .unwrap();
                }
            }
        }
        return Ok(u8::from(stale));
    }
    let report = match &options.report {
        Some(p) => Some(
            serde_json::from_slice::<Value>(&std::fs::read(p)?)
                .map_err(|e| Error::Refused(format!("{}: {e}", p.display())))?,
        ),
        None => None,
    };
    let result = output::advise(&input, report.as_ref()).map_err(Error::Refused)?;
    if let Some(dir) = &options.out {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("fixups.yaml"),
            result["fixups"].as_str().unwrap_or(""),
        )?;
        std::fs::write(
            dir.join("STATUS.md"),
            result["status"].as_str().unwrap_or(""),
        )?;
    }
    if options.json {
        let actions = serde_json::json!({
            "candidates": result["candidates"],
            "no_source": result["no_source"],
        });
        out.push_str(&serde_json::to_string_pretty(&actions).unwrap());
        out.push('\n');
    } else {
        out.push_str(&actions_text(&result, wording));
    }
    let undecided = result["no_source"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(result["problems"].as_array().into_iter().flatten())
        .any(|p| matches!(p["kind"].as_str(), Some("tie") | Some("out-of-order")));
    Ok(u8::from(undecided))
}

fn problem_line(p: &Value) -> String {
    let files: Vec<&str> = p["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut line = format!(
        "   - {:<16} {}: {}",
        p["kind"].as_str().unwrap_or(""),
        files.join(", "),
        p["detail"].as_str().unwrap_or("")
    );
    for note in p["build"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        write!(line, "\n       build: {note}").unwrap();
    }
    line
}

/// The actions as text (spec 0454 S7).
pub fn actions_text(result: &Value, wording: &dyn Wording) -> String {
    let mut lines: Vec<String> = Vec::new();
    let candidates = result["candidates"].as_array().cloned().unwrap_or_default();
    if !candidates.is_empty() {
        lines.push(wording.candidates_heading());
        for (i, c) in candidates.iter().enumerate() {
            let sources = c["sources"].as_array().cloned().unwrap_or_default();
            let problems = c["problems"].as_array().cloned().unwrap_or_default();
            let n = problems.len();
            lines.push(String::new());
            lines.push(format!(
                "{}. {} — {n} problem{}",
                i + 1,
                c["group"].as_str().unwrap_or("?"),
                if n == 1 { "" } else { "s" }
            ));
            if let Some(newest) = sources.first() {
                lines.push(format!("   {}", wording.newest(newest)));
            }
            lines.push(format!("   {}", wording.members(&sources)));
            lines.extend(problems.iter().map(problem_line));
        }
    }
    let no_source = result["no_source"].as_array().cloned().unwrap_or_default();
    if !no_source.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(wording.no_source_heading());
        lines.extend(
            no_source
                .iter()
                .filter(|p| p["kind"] != "manual")
                .map(problem_line),
        );
        let manual: Vec<&Value> = no_source.iter().filter(|p| p["kind"] == "manual").collect();
        if !manual.is_empty() {
            let total: usize = manual
                .iter()
                .map(|p| p["files"].as_array().map_or(0, Vec::len))
                .sum();
            let mut by_commit = manual.clone();
            by_commit.sort_by_key(|p| std::cmp::Reverse(p["files"].as_array().map_or(0, Vec::len)));
            let commits: Vec<String> = by_commit
                .iter()
                .map(|p| {
                    let detail = p["detail"].as_str().unwrap_or("");
                    let subject = detail
                        .rsplit("commit ")
                        .next()
                        .unwrap_or(detail)
                        .trim_matches('"');
                    format!(
                        "\"{subject}\" ({})",
                        p["files"].as_array().map_or(0, Vec::len)
                    )
                })
                .collect();
            lines.push(format!(
                "   - {:<16} {total} files, from commits: {}",
                "manual",
                commits.join(", ")
            ));
        }
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// The history's descriptors and sources (spec 0454 S2).
pub fn read(repo: &gix::Repository, kit: &Kit, into: Option<&str>) -> Result<Input, Error> {
    let branch = match into {
        Some(b) => b.to_string(),
        None => repo
            .head_name()
            .map_err(|e| Error::Git(e.to_string()))?
            .map(|n| n.shorten().to_string())
            .ok_or_else(|| Error::Refused("HEAD is detached: give --into".into()))?,
    };
    let head = tip(repo, &branch_ref(&branch)?)?
        .ok_or_else(|| Error::Refused(format!("no branch {branch:?}")))?;
    let pattern = git_seriate::plugins::Pattern(
        repo.config_snapshot()
            .string("seriate.itemPattern")
            .map(|p| p.to_string())
            .or_else(|| kit.item_pattern.clone()),
    );
    use git_seriate::plugins::ItemPaths;
    let blobs: Vec<(String, ObjectId)> = tree_files(repo, commit_tree(repo, head)?, "")?
        .into_iter()
        .filter(|(p, _)| pattern.is_item(p))
        .collect();
    let wanted: HashSet<&str> = blobs.iter().map(|(p, _)| p.as_str()).collect();

    let mut writers: HashMap<String, (i64, String)> = HashMap::new();
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(head);
    while let Some(id) = current {
        let commit = repo
            .find_commit(id)
            .map_err(|e| Error::Git(e.to_string()))?;
        let decoded = commit.decode().map_err(|e| Error::Git(e.to_string()))?;
        let parent = decoded.parents().next();
        let message = decoded.message.to_str_lossy().into_owned();
        let author_time = decoded
            .author()
            .map_err(|e| Error::Git(e.to_string()))?
            .seconds();
        let subject = message.lines().next().unwrap_or("").to_string();
        if writers.len() < wanted.len() {
            let parent_tree = match parent {
                Some(p) => commit_tree(repo, p)?,
                None => ObjectId::empty_tree(repo.object_hash()),
            };
            for change in diff(repo, parent_tree, decoded.tree())? {
                if change.new.is_some()
                    && wanted.contains(change.path.as_str())
                    && !writers.contains_key(&change.path)
                {
                    writers.insert(change.path, (author_time, subject.clone()));
                }
            }
        }
        if let Ok(Some(block)) = Block::read_with(&message, kit.legacy.as_deref()) {
            if let Some(contents) = block.contents {
                if seen.insert(block.source.identity.clone()) {
                    sources.push(Source {
                        identity: block.source.identity,
                        build_time: block.source.build_time,
                        group: block.source.group,
                        paths: contents.into_iter().collect(),
                        metadata: block.source.metadata,
                    });
                }
            }
        }
        current = parent;
    }

    let mut files = Vec::with_capacity(blobs.len());
    for (path, oid) in blobs {
        let data = repo
            .find_blob(oid)
            .map_err(|e| Error::Git(e.to_string()))?
            .data
            .clone();
        let fdp = kit.renderer.parse(&path, &data).unwrap_or_default();
        let (written_at, written_by) = writers.get(&path).cloned().unwrap_or_default();
        files.push(SnapshotFile {
            path,
            fdp,
            written_at,
            written_by,
        });
    }
    Ok(Input { files, sources })
}

fn tree_files(
    repo: &gix::Repository,
    tree: ObjectId,
    prefix: &str,
) -> Result<Vec<(String, ObjectId)>, Error> {
    let object = repo
        .find_tree(tree)
        .map_err(|e| Error::Git(e.to_string()))?;
    let decoded = object.decode().map_err(|e| Error::Git(e.to_string()))?;
    let mut out = Vec::new();
    for entry in &decoded.entries {
        let path = format!("{prefix}{}", entry.filename);
        if entry.mode.is_tree() {
            out.extend(tree_files(repo, entry.oid.to_owned(), &format!("{path}/"))?);
        } else if entry.mode.is_blob() {
            out.push((path, entry.oid.to_owned()));
        }
    }
    Ok(out)
}
