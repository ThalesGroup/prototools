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
    // s1 holds only f.txt, which s2 holds again: level 2 drops s1 (spec
    // 0501), and keeps s2, f.txt's owner, though it changes nothing.
    let repo = history(tmp.path(), &GROWING[..3]);
    anchored_commit(&repo, "h.txt", "s1");

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
        out.contains("is anchored to s1, which compact drops"),
        "{out}"
    );
}

/// Runs git with an index of its own, at `time` for a commit it writes.
fn plumbing(repo: &Path, index: &Path, time: i64, args: &[&str], stdin: Option<&[u8]>) -> String {
    let mut command = Command::new("git");
    isolated(&mut command, repo);
    let date = format!("@{time} +0000");
    command
        .env("GIT_INDEX_FILE", index)
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_AUTHOR_DATE", &date)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    {
        use std::io::Write;
        let mut input = child.stdin.take().unwrap();
        if let Some(bytes) = stdin {
            input.write_all(bytes).unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A manual commit on `parent` (none: a root) at `time`, writing `set` and
/// deleting `remove`; its id.
fn manual(
    repo: &Path,
    parent: Option<&str>,
    set: &[(&str, &str)],
    remove: &[&str],
    message: &str,
    time: i64,
) -> String {
    let index = repo.join("manual-index");
    let _ = std::fs::remove_file(&index);
    let git = |args: &[&str], stdin: Option<&[u8]>| plumbing(repo, &index, time, args, stdin);
    if let Some(parent) = parent {
        git(&["read-tree", parent], None);
    }
    for (path, text) in set {
        let blob = git(&["hash-object", "-w", "--stdin"], Some(text.as_bytes()));
        git(
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{blob},{path}"),
            ],
            None,
        );
    }
    // Mode 0 removes an entry, with no work tree (the repository is bare).
    for path in remove {
        let line = format!("0 {}\t{path}\n", "0".repeat(40));
        git(&["update-index", "--index-info"], Some(line.as_bytes()));
    }
    let tree = git(&["write-tree"], None);
    let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
    if let Some(parent) = parent {
        args.extend(["-p", parent]);
    }
    git(&args, None)
}

/// `main/default`'s commits put back on `base`, oldest first, each tree its
/// new parent's with the commit's files written over it, as a snapshot
/// commit writes its items; the branch moved to the last.
fn on_a_manual_base(repo: &Path, base: &str) {
    let index = repo.join("manual-index");
    let commits = git(repo, &["rev-list", "--reverse", "main/default"]);
    let mut parent = base.to_string();
    for commit in commits.lines() {
        let _ = std::fs::remove_file(&index);
        let g = |args: &[&str]| plumbing(repo, &index, T0, args, None);
        g(&["read-tree", &parent]);
        for line in git(repo, &["ls-tree", "-r", commit]).lines() {
            let (meta, path) = line.split_once('\t').unwrap();
            let fields: Vec<&str> = meta.split_whitespace().collect();
            g(&[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("{},{},{path}", fields[0], fields[2]),
            ]);
        }
        let tree = g(&["write-tree"]);
        let message = git(repo, &["log", "-1", "--format=%B", commit]);
        parent = g(&[
            "commit-tree",
            &tree,
            "-p",
            &parent,
            "-m",
            message.trim_end(),
        ]);
    }
    git(repo, &["update-ref", "refs/heads/main/default", &parent]);
}

fn compact(repo: &Path, level: &str, cutoff: &str, extra: &[&str]) -> String {
    let mut args = vec![
        "compact", "--into", "main", "--cutoff", cutoff, "--level", level,
    ];
    args.extend(extra);
    git_seriate(repo, &args)
}

fn tree_of(repo: &Path) -> String {
    git(repo, &["rev-parse", "main/default^{tree}"])
}

/// Spec 0501 test 1: a manual base commit whose every file a later snapshot
/// holds is dropped; the snapshot, which changed nothing, takes the files
/// as its changes; the tree at the cutoff unchanged.
#[test]
fn a_base_commit_held_by_a_later_snapshot_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &[&[("a.txt", "1\n"), ("b.txt", "2\n")]]);
    let base = manual(
        &repo,
        None,
        &[("a.txt", "1\n"), ("b.txt", "2\n")],
        &[],
        "Base",
        T0 - DAY,
    );
    on_a_manual_base(&repo, &base);
    let tree = tree_of(&repo);
    let out = compact(&repo, "2", &after_everything(), &[]);
    assert!(
        out.contains("('Base'): every path it holds is held later, by 1 commit ("),
        "{out}"
    );
    assert!(
        out.contains("('Snapshot s0'): 2 items it held become its changes"),
        "{out}"
    );
    assert!(out.contains("dropped 1, reduced 0"), "{out}");
    assert_eq!(subjects(&repo), "Snapshot s0\n");
    assert_eq!(tree_of(&repo), tree);
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

/// Test 2: a base commit with one file no snapshot holds is kept, at level
/// 2 as at level 3 (a manual commit is never reduced).
#[test]
fn a_partly_held_commit_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &[&[("a.txt", "1\n")]]);
    let base = manual(
        &repo,
        None,
        &[("a.txt", "1\n"), ("c.txt", "3\n")],
        &[],
        "Base",
        T0 - DAY,
    );
    on_a_manual_base(&repo, &base);
    let tree = tree_of(&repo);
    for level in ["2", "3"] {
        let out = compact(&repo, level, &after_everything(), &["--dry-run"]);
        assert!(out.contains("dropped 0, reduced 0"), "level {level}: {out}");
    }
    assert_eq!(tree_of(&repo), tree);
}

/// Test 3: a snapshot that changes nothing but owns its path is kept, and
/// the older one that wrote it dropped; before 0501, level 2 dropped the
/// one changing nothing.
#[test]
fn no_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &[&[("f.txt", "a\n")], &[("f.txt", "a\n")]]);
    let tree = tree_of(&repo);
    let out = compact(&repo, "2", &after_everything(), &[]);
    assert!(
        out.contains("('Snapshot s0'): every path it holds is held later"),
        "{out}"
    );
    assert!(
        out.contains("('Snapshot s1'): 1 items it held become its changes"),
        "{out}"
    );
    assert_eq!(subjects(&repo), "Snapshot s1\n");
    assert_eq!(tree_of(&repo), tree);
}

/// Test 4: a path held again only after the cutoff keeps its older owner.
#[test]
fn held_after_the_cutoff_counts_for_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &[&[("a.txt", "1\n")]]);
    let base = manual(&repo, None, &[("a.txt", "1\n")], &[], "Base", T0 - DAY);
    on_a_manual_base(&repo, &base);
    let before = git(&repo, &["rev-parse", "main/default"]);
    let out = compact(&repo, "2", &(T0 - DAY / 2).to_string(), &[]);
    assert!(out.contains("dropped 0, reduced 0"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main/default"]), before);
}

/// Test 5: at level 3, a kept snapshot's item held identical later is
/// removed from it; the later snapshot takes it as its change.
#[test]
fn level_3_removes_held_items() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(
        tmp.path(),
        &[&[("f.txt", "a\n"), ("g.txt", "1\n")], &[("f.txt", "a\n")]],
    );
    let tree = tree_of(&repo);
    let out = compact(&repo, "3", &after_everything(), &[]);
    assert!(
        out.contains("('Snapshot s0'): 1 of the 2 paths it holds are held later"),
        "{out}"
    );
    assert!(out.contains("dropped 0, reduced 1"), "{out}");
    assert_eq!(tree_of(&repo), tree);
    let first = git(&repo, &["rev-list", "--max-parents=0", "main/default"]);
    let block = Block::read(&git(&repo, &["log", "-1", "--format=%B", first.trim()]))
        .unwrap()
        .unwrap();
    assert!(block.reduced);
    assert_eq!(block.contents.unwrap(), ["g.txt"]);
    assert_eq!(
        git(&repo, &["ls-tree", "--name-only", first.trim()]),
        "g.txt\n"
    );
}

/// Test 6: the commit deleting a path owns it. Kept while there is
/// something to delete; dropped once the path's writers are, the tree at
/// the cutoff unchanged either way.
#[test]
fn a_deletion_owns_its_path() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &[&[("a.txt", "1\n"), ("b.txt", "2\n")]]);
    let tip = git(&repo, &["rev-parse", "main/default"]);
    let deletion = manual(
        &repo,
        Some(tip.trim()),
        &[],
        &["a.txt"],
        "Delete a",
        T0 + DAY,
    );
    git(&repo, &["update-ref", "refs/heads/main/default", &deletion]);
    let tree = tree_of(&repo);
    // Level 2: s0 owns b.txt and keeps writing a.txt; the deletion stays.
    let out = compact(&repo, "2", &after_everything(), &["--dry-run"]);
    assert!(out.contains("dropped 0, reduced 0"), "{out}");
    // Level 3: s0 no longer writes a.txt; nothing is left to delete.
    let out = compact(&repo, "3", &after_everything(), &[]);
    assert!(
        out.contains("('Delete a'): its deletions have nothing left to delete"),
        "{out}"
    );
    assert!(out.contains("dropped 1, reduced 1"), "{out}");
    assert_eq!(subjects(&repo), "Snapshot s0\n");
    assert_eq!(tree_of(&repo), tree);

    // A manual writer, then the deletion, then a snapshot: at level 2 the
    // writer owns nothing, and the deletion then deletes nothing.
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &[&[("b.txt", "2\n")]]);
    let writer = manual(
        &repo,
        None,
        &[("a.txt", "1\n")],
        &[],
        "Write a",
        T0 - 2 * DAY,
    );
    let deletion = manual(&repo, Some(&writer), &[], &["a.txt"], "Delete a", T0 - DAY);
    on_a_manual_base(&repo, &deletion);
    let tree = tree_of(&repo);
    let out = compact(&repo, "2", &after_everything(), &[]);
    assert!(
        out.contains("('Write a'): every path it holds is deleted later"),
        "{out}"
    );
    assert!(
        out.contains("('Delete a'): its deletions have nothing left to delete"),
        "{out}"
    );
    assert_eq!(subjects(&repo), "Snapshot s0\n");
    assert_eq!(tree_of(&repo), tree);
}

/// Test 7: a second copy, dropped at level 1, owns nothing, though it is
/// the last to hold its paths: its first copy keeps them.
#[test]
fn a_second_copy_owns_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &GROWING[..2]);
    repeat_tip(&repo);
    let tree = tree_of(&repo);
    let out = compact(&repo, "2", &after_everything(), &[]);
    assert!(out.contains("changes nothing, a second copy of"), "{out}");
    assert!(out.contains("dropped 1, reduced 0"), "{out}");
    assert_eq!(subjects(&repo), "Snapshot s0\nSnapshot s1\n");
    assert_eq!(tree_of(&repo), tree);
}

/// Test 8: a dropped commit names the first two of the commits owning its
/// paths, and all of them with `--verbose`.
#[test]
fn the_listing_names_two_owners_and_all_with_verbose() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(
        tmp.path(),
        &[
            &[("a.txt", "1\n")],
            &[("b.txt", "2\n")],
            &[("c.txt", "3\n")],
        ],
    );
    let base = manual(
        &repo,
        None,
        &[("a.txt", "1\n"), ("b.txt", "2\n"), ("c.txt", "3\n")],
        &[],
        "Base",
        T0 - DAY,
    );
    on_a_manual_base(&repo, &base);
    let owners: Vec<String> = git(&repo, &["rev-list", "--reverse", "main/default"])
        .lines()
        .skip(1)
        .map(|c| c[..8].to_string())
        .collect();
    let out = compact(&repo, "2", &after_everything(), &["--dry-run"]);
    assert!(
        out.contains(&format!("by 3 commits ({}, {}, …)", owners[0], owners[1])),
        "{out}"
    );
    let mut args = vec!["--verbose", "compact", "--into", "main", "--cutoff"];
    let cutoff = after_everything();
    args.extend([cutoff.as_str(), "--level", "2", "--dry-run"]);
    let out = git_seriate(&repo, &args);
    assert!(
        out.contains(&format!(
            "by 3 commits ({}, {}, {})",
            owners[0], owners[1], owners[2]
        )),
        "{out}"
    );
}

/// A downstream's case (spec 0501 S4): a manual commit adding two files a
/// later snapshot holds, and deleting a file a dropped commit wrote. Its
/// deletion has nothing left to delete and owns nothing, so the commit
/// goes in the same pass: a second pass drops nothing.
#[test]
fn one_pass_is_final_when_a_deletion_has_nothing_left() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &[&[("a.txt", "1\n"), ("b.txt", "2\n")]]);
    let writer = manual(
        &repo,
        None,
        &[("p.txt", "old\n")],
        &[],
        "Write p",
        T0 - 2 * DAY,
    );
    let mixed = manual(
        &repo,
        Some(&writer),
        &[("a.txt", "1\n"), ("b.txt", "2\n")],
        &["p.txt"],
        "Add a and b, delete p",
        T0 - DAY,
    );
    on_a_manual_base(&repo, &mixed);
    let tree = tree_of(&repo);
    let out = compact(&repo, "2", &after_everything(), &[]);
    assert!(
        out.contains("('Write p'): every path it holds is deleted later"),
        "{out}"
    );
    assert!(
        out.contains(
            "('Add a and b, delete p'): its deletions have nothing left to delete, and every \
             other path it holds is held later, by 1 commit ("
        ),
        "{out}"
    );
    assert!(
        out.contains("('Snapshot s0'): 2 items it held become its changes"),
        "{out}"
    );
    assert!(out.contains("dropped 2, reduced 0"), "{out}");
    assert_eq!(subjects(&repo), "Snapshot s0\n");
    assert_eq!(tree_of(&repo), tree);

    let again = compact(&repo, "2", &after_everything(), &[]);
    assert!(again.contains("dropped 0, reduced 0"), "{again}");
}

/// Spec 0501 S6: a drop line names only kept commits. A writer's paths
/// held later, one by a snapshot kept, one by a deletion dropped too (it
/// has nothing left to delete): the snapshot named, the other path
/// "deleted later".
#[test]
fn a_drop_line_names_only_kept_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = history(tmp.path(), &[&[("a.txt", "1\n"), ("q.txt", "2\n")]]);
    let writer = manual(
        &repo,
        None,
        &[("p.txt", "old\n"), ("q.txt", "2\n")],
        &[],
        "Write p and q",
        T0 - 2 * DAY,
    );
    let mixed = manual(
        &repo,
        Some(&writer),
        &[("a.txt", "1\n")],
        &["p.txt"],
        "Add a, delete p",
        T0 - DAY,
    );
    on_a_manual_base(&repo, &mixed);
    let snapshot = git(&repo, &["rev-parse", "main/default"]);
    let tree = tree_of(&repo);
    let out = compact(&repo, "2", &after_everything(), &["--dry-run"]);
    let mixed_short = &mixed[..8];
    assert!(
        out.contains(&format!(
            "('Write p and q'): every path it holds is held later, by 1 commit ({}), or deleted later",
            &snapshot[..8]
        )),
        "{out}"
    );
    assert!(
        !out.contains(&format!("by 1 commit ({mixed_short})")),
        "{out}"
    );
    assert!(
        out.contains("('Add a, delete p'): its deletions have nothing left to delete, and every"),
        "{out}"
    );
    assert!(out.contains("dropped 2, reduced 0"), "{out}");
    assert_eq!(tree_of(&repo), tree);
}
