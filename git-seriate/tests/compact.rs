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
        .env("GIT_AUTHOR_NAME", "Tester")
        .env("GIT_AUTHOR_EMAIL", "tester@example.com")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG_GLOBAL");
}

/// Its standard output, then its standard error (where warnings go).
fn git_seriate(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-seriate"));
    isolated(&mut command, repo);
    let out = command.arg("-C").arg(repo).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git seriate {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
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
    git_seriate(&repo, &["settle", "--into", "main"]);
    repo
}

fn subjects(repo: &Path) -> String {
    git(repo, &["log", "--reverse", "--format=%s", "main/default"])
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

/// Level 1 is lossless: s2 changes nothing but is its source's only
/// record, so it stays; nothing is dropped, and the branch doesn't move.
#[test]
fn level_1_keeps_a_sources_only_record() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), GROWING);
    let tip = git(&repo, &["rev-parse", "main/default"]);
    let out = git_seriate(&repo, &["compact", "--into", "main", "--cutoff", &cutoff()]);
    assert!(out.contains("dropped 0, reduced 0, converted 0"), "{out}");
    assert_eq!(
        subjects(&repo),
        "Snapshot s0\nSnapshot s1\nSnapshot s2\nSnapshot s3\nSnapshot s4\n"
    );
    assert_eq!(git(&repo, &["rev-parse", "main/default"]), tip);
}

/// Level 1 drops a manual commit that changes nothing, and the second copy
/// of a snapshot stored twice; the first copy, its record, stays.
#[test]
fn level_1_drops_empty_manual_commits_and_second_copies() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &GROWING[..2]);
    repeat_tip(&repo);
    let note = git(
        &repo,
        &[
            "commit-tree",
            "main/default^{tree}",
            "-p",
            "main/default",
            "-m",
            "A note",
        ],
    );
    git(
        &repo,
        &["update-ref", "refs/heads/main/default", note.trim()],
    );
    let tree = git(&repo, &["rev-parse", "main/default^{tree}"]);

    let out = git_seriate(
        &repo,
        // After the note, which is dated now.
        &["compact", "--into", "main", "--cutoff", "2100-01-01"],
    );
    assert!(out.contains("changes nothing, a second copy of"), "{out}");
    assert!(out.contains("('A note'): changes nothing"), "{out}");
    assert!(out.contains("dropped 2, reduced 0, converted 0"), "{out}");
    assert_eq!(subjects(&repo), "Snapshot s0\nSnapshot s1\n");
    assert_eq!(git(&repo, &["rev-parse", "main/default^{tree}"]), tree);
    // s1's first copy stays, and so does its snapshot.
    assert!(!out.contains("delete snapshot"), "{out}");
    git(
        &repo,
        &["rev-parse", "--verify", "refs/seriate/snapshots/s1"],
    );
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

/// Two commits of one identity with different blocks are two records that
/// contradict each other: refused, at every level, as by settle.
#[test]
fn compact_refuses_one_identity_with_two_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &GROWING[..2]);
    let message = git(&repo, &["log", "-1", "--format=%B", "main/default"]);
    let other = message.replace(
        &format!("build_time: {}", T0 + DAY),
        &format!("build_time: {}", T0 + 2 * DAY),
    );
    assert_ne!(message, other, "the block names its build time");
    let commit = git(
        &repo,
        &[
            "commit-tree",
            "main/default^{tree}",
            "-p",
            "main/default",
            "-m",
            other.trim_end(),
        ],
    );
    git(
        &repo,
        &["update-ref", "refs/heads/main/default", commit.trim()],
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_git-seriate"));
    isolated(&mut command, &repo);
    let out = command
        .arg("-C")
        .arg(&repo)
        .args(["compact", "--into", "main", "--cutoff", &after_everything()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("holds \"s1\" twice"), "{stderr}");
    assert!(stderr.contains("with different blocks"), "{stderr}");
}

#[test]
fn level_2_drops_what_is_made_again_later() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), GROWING);
    let tree = git(&repo, &["rev-parse", "main/default^{tree}"]);
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
    // Listed beforehand by --dry-run too: the record lost is named.
    assert!(
        out.contains("('Snapshot s2'): changes nothing; the only record of \"s2\" goes with it"),
        "{out}"
    );
    // s0 and s1 change only what s3 changes again; s2 changes nothing.
    assert_eq!(subjects(&repo), "Snapshot s3\nSnapshot s4\n");
    assert_eq!(git(&repo, &["rev-parse", "main/default^{tree}"]), tree);

    // Their snapshot branches go with them, so settle doesn't put them
    // back; the others stay.
    for id in ["s0", "s1", "s2"] {
        assert!(
            out.contains(&format!("delete snapshot {id}: its last commit is dropped")),
            "{out}"
        );
    }
    assert_eq!(
        git(
            &repo,
            &[
                "for-each-ref",
                "--format=%(refname:lstrip=3)",
                "refs/seriate/snapshots/s*"
            ]
        ),
        "s3\ns4\n"
    );
    let settled = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(settled.contains("nothing new to settle"), "{settled}");
    assert_eq!(subjects(&repo), "Snapshot s3\nSnapshot s4\n");
}

/// A dry run lists the snapshots it would delete, and deletes none.
#[test]
fn a_dry_run_lists_the_snapshots_and_deletes_none() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), GROWING);
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
    assert!(out.contains("delete snapshot s2:"), "{out}");
    assert_eq!(
        git(
            &repo,
            &[
                "for-each-ref",
                "--format=%(refname:lstrip=3)",
                "refs/seriate/snapshots/s*"
            ]
        ),
        "s0\ns1\ns2\ns3\ns4\n"
    );
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
    let tree = git(&repo, &["rev-parse", "main/default^{tree}"]);
    let cutoff = (T0 + DAY + DAY / 2).to_string();
    let out = git_seriate(
        &repo,
        &[
            "compact", "--into", "main", "--cutoff", &cutoff, "--level", "3",
        ],
    );
    assert!(out.contains("reduce "), "{out}");
    assert!(out.contains("dropped 0, reduced 1"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main/default^{tree}"]), tree);
    let first = git(&repo, &["rev-list", "--max-parents=0", "main/default"]);
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
    let before = git(&repo, &["rev-parse", "main/default"]);
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
    assert!(
        out.contains("would compact main/default: dropped 3"),
        "{out}"
    );
    assert_eq!(git(&repo, &["rev-parse", "main/default"]), before);
    assert_eq!(git(&repo, &["count-objects", "-v"]), objects);
    assert!(git(&repo, &["branch", "--list", "main/default-*"]).is_empty());
}

#[test]
fn nothing_to_drop_moves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &GROWING[..2]);
    let before = git(&repo, &["rev-parse", "main/default"]);
    let out = git_seriate(&repo, &["compact", "--into", "main", "--cutoff", &cutoff()]);
    assert!(out.contains("dropped 0, reduced 0, converted 0"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main/default"]), before);
}

/// Stores the snapshot at `main`'s tip again, on top of itself: a commit
/// with the same block and tree as its parent.
fn repeat_tip(repo: &Path) {
    let message = git(repo, &["log", "-1", "--format=%B", "main/default"]);
    let commit = git(
        repo,
        &[
            "commit-tree",
            "main/default^{tree}",
            "-p",
            "main/default",
            "-m",
            message.trim_end(),
        ],
    );
    git(
        repo,
        &["update-ref", "refs/heads/main/default", commit.trim()],
    );
}

/// A manual commit on `main` that adds `path`, anchored to `anchor`.
fn anchored_commit(repo: &Path, path: &str, anchor: &str) {
    let blob = {
        let mut command = Command::new("git");
        isolated(&mut command, repo);
        let mut child = command
            .arg("-C")
            .arg(repo)
            .args(["hash-object", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"by hand\n").unwrap();
        let out = child.wait_with_output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let index = repo.join("manual-index");
    let run = |args: &[&str]| {
        let mut command = Command::new("git");
        isolated(&mut command, repo);
        let out = command
            .env("GIT_INDEX_FILE", &index)
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run(&["read-tree", "main/default"]);
    run(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100644,{blob},{path}"),
    ]);
    let tree = run(&["write-tree"]);
    let message = format!("Add {path} by hand\n\nSnapshot-Anchor: {anchor}");
    let commit = run(&["commit-tree", &tree, "-p", "main/default", "-m", &message]);
    git(repo, &["update-ref", "refs/heads/main/default", &commit]);
}

fn after_everything() -> String {
    (T0 + 30 * DAY).to_string()
}

#[test]
fn settle_names_compact_for_a_snapshot_stored_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &GROWING[..2]);
    repeat_tip(&repo);

    let mut command = Command::new(env!("CARGO_BIN_EXE_git-seriate"));
    isolated(&mut command, &repo);
    let out = command
        .arg("-C")
        .arg(&repo)
        .args(["settle", "--into", "main", "--no-cutoff"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("the history holds \"s1\" twice"),
        "{stderr}"
    );
    assert!(stderr.contains("`compact --level 1` drops it"), "{stderr}");

    // The way out the refusal names works.
    git_seriate(
        &repo,
        &["compact", "--into", "main", "--cutoff", &after_everything()],
    );
    assert_eq!(subjects(&repo), "Snapshot s0\nSnapshot s1\n");
    git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
}

#[test]
fn an_anchor_on_a_snapshot_still_held_is_not_warned_about() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &GROWING[..2]);
    repeat_tip(&repo);
    anchored_commit(&repo, "h.txt", "s1");

    let out = git_seriate(
        &repo,
        &[
            "compact",
            "--into",
            "main",
            "--cutoff",
            &after_everything(),
            "--dry-run",
        ],
    );
    assert!(out.contains("changes nothing"), "{out}");
    assert!(!out.contains("anchored"), "{out}");
}

#[test]
fn an_anchor_on_a_snapshot_no_longer_held_is_warned_about() {
    let tmp = tempfile::tempdir().unwrap();
    // s2 changes nothing, and no other commit holds it: level 2 drops it.
    let repo = history(tmp.path(), &GROWING[..3]);
    anchored_commit(&repo, "h.txt", "s2");

    let out = git_seriate(
        &repo,
        &[
            "compact",
            "--into",
            "main",
            "--cutoff",
            &after_everything(),
            "--level",
            "2",
            "--dry-run",
        ],
    );
    assert!(
        out.contains("is anchored to s2, which compact drops"),
        "{out}"
    );
}
