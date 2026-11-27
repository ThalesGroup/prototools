// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `forget` and `prune` (spec 0497), end to end: repositories in temporary
//! directories, the binary run on them.

use std::path::{Path, PathBuf};
use std::process::Command;

use git_seriate::format::Source;
use git_seriate::store::{self, Item, NewSnapshot};

const ENV: [(&str, &str); 4] = [
    ("GIT_COMMITTER_NAME", "Tester"),
    ("GIT_COMMITTER_EMAIL", "tester@example.com"),
    ("GIT_AUTHOR_NAME", "Tester"),
    ("GIT_AUTHOR_EMAIL", "tester@example.com"),
];

/// No configuration but the repository's.
fn isolated(command: &mut Command, repo: &Path) {
    let home = repo.parent().unwrap();
    command
        .envs(ENV)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("SSH_ASKPASS")
        .env_remove("SSH_ASKPASS_REQUIRE");
}

fn git_seriate(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-seriate"));
    isolated(&mut command, repo);
    let out = command.arg("-C").arg(repo).args(args).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn git(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    isolated(&mut command, repo);
    let out = command.arg("-C").arg(repo).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn new_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo.git");
    gix::init_bare(&repo).unwrap();
    repo
}

fn add(repo: &Path, identity: &str, build_time: i64, files: &[(&str, &str)]) {
    let r = gix::open(repo).unwrap();
    store::add(
        &r,
        NewSnapshot {
            source: Source {
                identity: identity.into(),
                build_time,
                order_hint: None,
                group: None,
                metadata: Vec::new(),
            },
            items: files
                .iter()
                .map(|(path, text)| Item {
                    path: (*path).into(),
                    bytes: text.as_bytes().to_vec(),
                    record: Vec::new(),
                    origin: None,
                })
                .collect(),
            rejected: Vec::new(),
            record: Vec::new(),
        },
    )
    .unwrap();
}

/// The history's subjects, oldest first.
fn subjects(repo: &Path, branch: &str) -> Vec<String> {
    git(repo, &["log", "--reverse", "--format=%s", branch])
        .lines()
        .map(str::to_string)
        .collect()
}

fn file_at(repo: &Path, rev: &str, path: &str) -> Option<String> {
    let mut command = Command::new("git");
    isolated(&mut command, repo);
    let out = command
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{rev}:{path}")])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn ref_exists(repo: &Path, name: &str) -> bool {
    let mut command = Command::new("git");
    isolated(&mut command, repo);
    command
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", name])
        .output()
        .unwrap()
        .status
        .success()
}

/// Three snapshots settled into `main`: s2 changes a.txt and adds b.txt.
fn three(root: &Path) -> PathBuf {
    let repo = new_repo(root);
    add(&repo, "s1", 1_000_000, &[("a.txt", "1\n")]);
    add(
        &repo,
        "s2",
        2_000_000,
        &[("a.txt", "2\n"), ("b.txt", "2\n")],
    );
    add(&repo, "s3", 3_000_000, &[("c.txt", "3\n")]);
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert_eq!(
        subjects(&repo, "main"),
        ["Snapshot s1", "Snapshot s2", "Snapshot s3"]
    );
    repo
}

/// A manual commit on `branch`: `path` written with `text`.
fn manual_commit(repo: &Path, branch: &str, path: &str, text: &str, subject: &str) {
    let worktree = repo.parent().unwrap().join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let index = repo.parent().unwrap().join("index");
    let run = |args: &[&str]| {
        let mut command = Command::new("git");
        isolated(&mut command, repo);
        let out = command
            .arg("--git-dir")
            .arg(repo)
            .arg("--work-tree")
            .arg(&worktree)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run(&["read-tree", branch]);
    std::fs::write(worktree.join(path), text).unwrap();
    run(&["add", path]);
    let tree = run(&["write-tree"]);
    let parent = run(&["rev-parse", branch]);
    let commit = run(&["commit-tree", &tree, "-p", &parent, "-m", subject]);
    run(&["update-ref", &format!("refs/heads/{branch}"), &commit]);
}

#[test]
fn a_walk_stopped_by_a_manual_commit_is_a_refusal_saying_why() {
    // No seriate.itemPattern: every file counts as a snapshot file, so a
    // manual commit at the tip stops the walk, above the snapshot to forget.
    let tmp = tempfile::tempdir().unwrap();
    let repo = three(tmp.path());
    manual_commit(&repo, "main", "README.md", "notes\n", "docs");
    let old = git(&repo, &["rev-parse", "main"]);
    let (ok, _, err) = git_seriate(&repo, &["forget", "--into", "main", "--reason", "x", "s2"]);
    assert!(!ok);
    assert!(!err.contains("internal error"), "{err}");
    assert!(err.contains("cannot forget 1 snapshot(s)"), "{err}");
    assert!(err.contains("(s2)"), "{err}");
    assert!(err.contains("the walk stops at"), "{err}");
    assert!(err.contains("no seriate.itemPattern is set"), "{err}");
    assert!(err.contains("to let the history's rewrites pass"), "{err}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), old);
    assert!(ref_exists(&repo, "refs/heads/s2"));
}

#[test]
fn a_manual_commit_whose_anchor_is_forgotten_keeps_its_place() {
    // s1 s2 s3, A (README 1), s4, B (README 2, anchored to s4). Forgetting
    // s2 and s4 rewrites from s2: B, its anchor gone, goes after the
    // nearest snapshot below it still held, s3, so after A, and the tip
    // keeps B's README; not at the bottom, below A, which would give the
    // tip A's older one.
    let tmp = tempfile::tempdir().unwrap();
    let repo = three(tmp.path());
    git(&repo, &["config", "seriate.itemPattern", "*.txt"]);
    manual_commit(&repo, "main", "README.md", "1\n", "docs A");
    add(&repo, "s4", 4_000_000, &[("d.txt", "4\n")]);
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    manual_commit(&repo, "main", "README.md", "2\n", "docs B");
    assert_eq!(
        subjects(&repo, "main"),
        [
            "Snapshot s1",
            "Snapshot s2",
            "Snapshot s3",
            "docs A",
            "Snapshot s4",
            "docs B"
        ]
    );
    let (ok, _, err) = git_seriate(
        &repo,
        &["forget", "--into", "main", "--reason", "x", "s2", "s4"],
    );
    assert!(ok, "{err}");
    assert!(!err.contains("now replayed before"), "{err}");
    assert!(err.contains("replayed after 's3'"), "{err}");
    assert_eq!(
        subjects(&repo, "main"),
        ["Snapshot s1", "Snapshot s3", "docs A", "docs B"]
    );
    assert_eq!(file_at(&repo, "main", "README.md").as_deref(), Some("2\n"));
}

#[test]
fn a_forgotten_snapshot_leaves_the_history_and_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = three(tmp.path());
    let old = git(&repo, &["rev-parse", "main"]);
    let s2 = git(&repo, &["rev-parse", "refs/heads/s2"]);
    let (ok, out, err) = git_seriate(
        &repo,
        &[
            "forget",
            "--into",
            "main",
            "--reason",
            "read past its frame",
            "s2",
        ],
    );
    assert!(ok, "{err}");
    assert!(out.contains("forgot s2\n"), "{out}");
    assert!(out.contains("kept by refs/seriate/forgotten/1\n"), "{out}");
    assert!(out.contains("next: extract their replacements"), "{out}");
    // The history without it, its files' versions back to the ones below.
    assert_eq!(subjects(&repo, "main"), ["Snapshot s1", "Snapshot s3"]);
    assert_eq!(file_at(&repo, "main", "a.txt").as_deref(), Some("1\n"));
    assert_eq!(file_at(&repo, "main", "b.txt"), None);
    assert_eq!(file_at(&repo, "main", "c.txt").as_deref(), Some("3\n"));
    // The previous tip saved; the branch gone; the snapshot kept by the
    // decision's record, with why, and listed in its tree.
    assert_eq!(git(&repo, &["rev-parse", "main-1"]), old);
    assert!(!ref_exists(&repo, "refs/heads/s2"));
    let record = "refs/seriate/forgotten/1";
    assert_eq!(git(&repo, &["rev-parse", &format!("{record}^1")]), s2);
    let message = git(&repo, &["log", "-1", "--format=%B", record]);
    assert!(message.contains("Reason: read past its frame"), "{message}");
    assert!(message.contains("History: main"), "{message}");
    assert_eq!(
        git(&repo, &["show", &format!("{record}:forgotten")]),
        format!("s2\t{s2}")
    );
}

#[test]
fn a_forgotten_snapshot_is_added_again_the_normal_way() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = three(tmp.path());
    let (ok, _, err) = git_seriate(
        &repo,
        &["forget", "--into", "main", "--reason", "fixed", "s2"],
    );
    assert!(ok, "{err}");
    // The fixed snapshot: another commit for the identity, now accepted.
    add(
        &repo,
        "s2",
        2_000_000,
        &[("a.txt", "2\n"), ("b.txt", "22\n")],
    );
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert_eq!(
        subjects(&repo, "main"),
        ["Snapshot s1", "Snapshot s2", "Snapshot s3"]
    );
    assert_eq!(file_at(&repo, "main", "b.txt").as_deref(), Some("22\n"));
    // Forgotten again: the second decision, its own record.
    let (ok, out, err) = git_seriate(
        &repo,
        &["forget", "--into", "main", "--reason", "again", "s2"],
    );
    assert!(ok, "{err}");
    assert!(out.contains("kept by refs/seriate/forgotten/2"), "{out}");
}

#[test]
fn one_decision_one_record_for_every_snapshot_it_forgets() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = three(tmp.path());
    let (ok, out, err) = git_seriate(
        &repo,
        &["forget", "--into", "main", "--reason", "one", "s2", "s3"],
    );
    assert!(ok, "{err}");
    assert!(out.contains("kept by refs/seriate/forgotten/1\n"), "{out}");
    assert_eq!(subjects(&repo, "main"), ["Snapshot s1"]);
    let parents = git(
        &repo,
        &["log", "-1", "--format=%P", "refs/seriate/forgotten/1"],
    );
    assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
    assert!(!ref_exists(&repo, "refs/seriate/forgotten/2"));
}

#[test]
fn a_dry_run_lists_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = three(tmp.path());
    let old = git(&repo, &["rev-parse", "main"]);
    let (ok, out, err) = git_seriate(
        &repo,
        &[
            "forget",
            "--into",
            "main",
            "--reason",
            "x",
            "--dry-run",
            "s*2",
        ],
    );
    assert!(ok, "{err}");
    assert!(out.contains("would forget s2"), "{out}");
    assert!(out.contains("would delete its snapshot branch s2"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), old);
    assert!(ref_exists(&repo, "refs/heads/s2"));
    assert!(!ref_exists(&repo, "refs/seriate/forgotten/1"));
}

#[test]
fn what_matches_nothing_or_everything_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = three(tmp.path());
    let (ok, _, err) = git_seriate(&repo, &["forget", "--into", "main", "--reason", "x", "s9"]);
    assert!(!ok);
    assert!(err.contains("no snapshot of main matches \"s9\""), "{err}");
    let (ok, _, err) = git_seriate(&repo, &["forget", "--into", "main", "--reason", "x", "s*"]);
    assert!(!ok);
    assert!(err.contains("would leave it empty"), "{err}");
    let (ok, _, err) = git_seriate(&repo, &["forget", "--into", "main", "s2"]);
    assert!(!ok, "a reason is required");
    assert!(err.contains("--reason"), "{err}");
}

#[test]
fn prune_is_what_drop_was_and_drop_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = three(tmp.path());
    let (ok, out, err) = git_seriate(&repo, &["prune", "--glob", "s1"]);
    assert!(ok, "{err}");
    assert!(out.contains("deleted s1"), "{out}");
    assert!(err.is_empty(), "{err}");
    let (ok, out, err) = git_seriate(&repo, &["drop", "--glob", "s3"]);
    assert!(ok, "{err}");
    assert!(out.contains("deleted s3"), "{out}");
    assert!(err.contains("drop is now prune"), "{err}");
    // Pruned, not forgotten: the history still holds them.
    assert_eq!(
        subjects(&repo, "main"),
        ["Snapshot s1", "Snapshot s2", "Snapshot s3"]
    );
}
