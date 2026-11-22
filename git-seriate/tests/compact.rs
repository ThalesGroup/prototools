// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `compact`, end to end (spec 0452 test plan 6).

use std::path::{Path, PathBuf};
use std::process::Command;

use git_seriate::format::{Block, Source};
use git_seriate::store::{self, Item, NewSnapshot};

const DAY: i64 = 86_400;
const T0: i64 = 1_700_000_000;

fn isolated(command: &mut Command, repo: &Path) {
    let home = repo.parent().unwrap();
    command
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.com")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG_GLOBAL");
}

fn git_seriate(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-seriate"));
    isolated(&mut command, repo);
    let out = command.arg("-C").arg(repo).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git seriate {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    isolated(&mut command, repo);
    let out = command.arg("-C").arg(repo).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Snapshots `s0`… a day apart, each holding the files given, settled
/// into `main`.
fn history(root: &Path, snapshots: &[&[(&str, &str)]]) -> PathBuf {
    let repo = root.join("repo.git");
    gix::init_bare(&repo).unwrap();
    let r = gix::open(&repo).unwrap();
    for (i, files) in snapshots.iter().enumerate() {
        store::add(
            &r,
            NewSnapshot {
                source: Source {
                    identity: format!("s{i}"),
                    build_time: T0 + i as i64 * DAY,
                    order_hint: Some(i as u64),
                    group: None,
                    metadata: Vec::new(),
                },
                items: files
                    .iter()
                    .map(|(p, t)| Item {
                        path: p.to_string(),
                        bytes: t.as_bytes().to_vec(),
                    })
                    .collect(),
                rejected: Vec::new(),
                record: Vec::new(),
            },
        )
        .unwrap();
    }
    git_seriate(&repo, &["settle", "--into", "main"]);
    repo
}

fn subjects(repo: &Path) -> String {
    git(repo, &["log", "--reverse", "--format=%s", "main"])
}

/// f.txt grows; s2 changes nothing; s3 rewrites g.txt; s4 is after the
/// cutoff.
const GROWING: &[&[(&str, &str)]] = &[
    &[("f.txt", "a\n"), ("g.txt", "1\n")],
    &[("f.txt", "a\nb\n")],
    &[("f.txt", "a\nb\n")],
    &[("f.txt", "a\nb\nc\n"), ("g.txt", "1\n2\n")],
    &[("f.txt", "a\nb\nc\nd\n")],
];

fn cutoff() -> String {
    (T0 + 3 * DAY + DAY / 2).to_string()
}

#[test]
fn level_1_drops_what_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), GROWING);
    let tree = git(&repo, &["rev-parse", "main^{tree}"]);
    assert_eq!(
        subjects(&repo),
        "Snapshot s0\nSnapshot s1\nSnapshot s2\nSnapshot s3\nSnapshot s4\n"
    );

    let out = git_seriate(&repo, &["compact", "--into", "main", "--cutoff", &cutoff()]);
    assert!(out.contains("changes nothing"), "{out}");
    assert!(
        out.contains("dropped 1, reduced 0, converted 0; previous tip saved as main-1"),
        "{out}"
    );
    assert_eq!(
        subjects(&repo),
        "Snapshot s0\nSnapshot s1\nSnapshot s3\nSnapshot s4\n"
    );
    assert_eq!(git(&repo, &["rev-parse", "main^{tree}"]), tree);
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

#[test]
fn level_2_drops_what_is_made_again_later() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), GROWING);
    let tree = git(&repo, &["rev-parse", "main^{tree}"]);
    let out = git_seriate(
        &repo,
        &[
            "compact",
            "--into",
            "main",
            "--cutoff",
            &cutoff(),
            "--level",
            "2",
        ],
    );
    assert!(out.contains("dropped 3, reduced 0"), "{out}");
    // s0 and s1 change only what s3 changes again; s2 changes nothing.
    assert_eq!(subjects(&repo), "Snapshot s3\nSnapshot s4\n");
    assert_eq!(git(&repo, &["rev-parse", "main^{tree}"]), tree);
}

#[test]
fn level_3_reduces_snapshots_to_what_reaches_the_cutoff() {
    let tmp = tempfile::tempdir().unwrap();
    // s0 holds f.txt, which s1 rewrites, and g.txt, which nothing does.
    let repo = history(
        tmp.path(),
        &[
            &[("f.txt", "a\n"), ("g.txt", "1\n")],
            &[("f.txt", "a\nb\n")],
            &[("f.txt", "a\nb\nc\n")],
        ],
    );
    let tree = git(&repo, &["rev-parse", "main^{tree}"]);
    let cutoff = (T0 + DAY + DAY / 2).to_string();
    let out = git_seriate(
        &repo,
        &[
            "compact", "--into", "main", "--cutoff", &cutoff, "--level", "3",
        ],
    );
    assert!(out.contains("reduce "), "{out}");
    assert!(out.contains("dropped 0, reduced 1"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main^{tree}"]), tree);
    let first = git(&repo, &["rev-list", "--max-parents=0", "main"]);
    let message = git(&repo, &["log", "-1", "--format=%B", first.trim()]);
    let block = Block::read(&message).unwrap().unwrap();
    assert!(block.reduced);
    assert_eq!(block.contents.unwrap(), ["g.txt"]);
    assert_eq!(
        git(&repo, &["ls-tree", "--name-only", first.trim()]),
        "g.txt\n"
    );
}

#[test]
fn a_dry_run_lists_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), GROWING);
    let before = git(&repo, &["rev-parse", "main"]);
    let objects = git(&repo, &["count-objects", "-v"]);
    let out = git_seriate(
        &repo,
        &[
            "compact",
            "--into",
            "main",
            "--cutoff",
            &cutoff(),
            "--level",
            "2",
            "--dry-run",
        ],
    );
    assert!(out.contains("would compact main: dropped 3"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), before);
    assert_eq!(git(&repo, &["count-objects", "-v"]), objects);
    assert!(git(&repo, &["branch", "--list", "main-*"]).is_empty());
}

#[test]
fn nothing_to_drop_moves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &GROWING[..2]);
    let before = git(&repo, &["rev-parse", "main"]);
    let out = git_seriate(&repo, &["compact", "--into", "main", "--cutoff", &cutoff()]);
    assert!(out.contains("dropped 0, reduced 0, converted 0"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), before);
}
