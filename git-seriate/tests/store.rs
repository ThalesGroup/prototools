// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The store, end to end (spec 0452 test plan 1): the binary run on
//! repositories in temporary directories, every tree checked with
//! `git fsck --strict`.

use std::path::Path;
use std::process::Command;

use git_seriate::format::{Block, Source};
use git_seriate::store::{self, Added, NewSnapshot};

fn git_seriate(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_git-seriate"))
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn fsck(repo: &Path) {
    git(repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

fn new_repo(root: &Path, name: &str) -> std::path::PathBuf {
    let repo = root.join(name);
    gix::init_bare(&repo).unwrap();
    repo
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
}

#[test]
fn a_directory_is_added_once() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path(), "repo.git");
    let snap = tmp.path().join("snap");
    write(
        &snap,
        &[("a.txt", "a\n"), ("d/b.txt", "b\n"), ("d/e/c.txt", "c\n")],
    );

    let add = [
        "add",
        "--id",
        "build0001/part0",
        "--build-time",
        "1700000000",
        "--order-hint",
        "3",
    ];
    let mut args = add.to_vec();
    let snap_arg = snap.to_str().unwrap();
    args.push(snap_arg);
    let (ok, out, err) = git_seriate(&repo, &args);
    assert!(ok, "{err}");
    assert_eq!(out, "added build0001/part0\n");
    fsck(&repo);

    // The commit: parentless, dated, its tree the directory.
    let shown = git(
        &repo,
        &["log", "-1", "--format=%P|%at|%ct|%an", "build0001/part0"],
    );
    assert_eq!(shown, "|3|1700000000|git-seriate\n");
    assert_eq!(
        git(&repo, &["ls-tree", "-r", "--name-only", "build0001/part0"]),
        "a.txt\nd/b.txt\nd/e/c.txt\n"
    );
    let message = git(&repo, &["log", "-1", "--format=%B", "build0001/part0"]);
    let block = Block::read(&message).unwrap().unwrap();
    assert_eq!(block.source.identity, "build0001/part0");
    assert_eq!(block.contents.unwrap().len(), 3);

    // Again: a no-op. Changed: refused.
    let (ok, out, _) = git_seriate(&repo, &args);
    assert!(ok);
    assert_eq!(out, "unchanged build0001/part0\n");
    write(&snap, &[("a.txt", "changed\n")]);
    let (ok, _, err) = git_seriate(&repo, &args);
    assert!(!ok);
    assert!(err.contains("exists with another commit"), "{err}");

    let (ok, out, _) = git_seriate(&repo, &["list"]);
    assert!(ok);
    assert_eq!(out, "build0001/part0\n");
}

#[test]
fn the_same_snapshot_is_the_same_commit_anywhere() {
    let tmp = tempfile::tempdir().unwrap();
    let snap = tmp.path().join("snap");
    write(&snap, &[("x.txt", "x\n")]);
    let commits: Vec<String> = ["one.git", "two.git"]
        .iter()
        .map(|name| {
            let repo = new_repo(tmp.path(), name);
            let (ok, _, err) = git_seriate(
                &repo,
                &[
                    "add",
                    "--id",
                    "s",
                    "--build-time",
                    "5",
                    snap.to_str().unwrap(),
                ],
            );
            assert!(ok, "{err}");
            git(&repo, &["rev-parse", "s"])
        })
        .collect();
    assert_eq!(commits[0], commits[1]);
}

#[test]
fn refused_paths_are_reported_and_the_tree_stays_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path(), "repo.git");
    let snap = tmp.path().join("snap");
    write(
        &snap,
        &[
            ("ok.txt", "ok\n"),
            ("x/.GIT/f.txt", "no\n"),
            ("y/git~1/g.txt", "no\n"),
        ],
    );
    let (ok, out, err) = git_seriate(
        &repo,
        &[
            "add",
            "--id",
            "s",
            "--build-time",
            "1",
            snap.to_str().unwrap(),
        ],
    );
    assert!(ok, "{err}");
    assert_eq!(out, "added s\n");
    assert!(err.contains("x/.GIT/f.txt: refused (invalid"), "{err}");
    assert!(err.contains("y/git~1/g.txt: refused (invalid"), "{err}");
    fsck(&repo);
    assert_eq!(
        git(&repo, &["ls-tree", "-r", "--name-only", "s"]),
        "ok.txt\n"
    );
    let message = git(&repo, &["log", "-1", "--format=%B", "s"]);
    assert_eq!(Block::read(&message).unwrap().unwrap().rejected.len(), 2);
}

#[test]
fn drop_and_record_only_branches() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_path = new_repo(tmp.path(), "repo.git");
    let repo = gix::open(&repo_path).unwrap();
    let source = |id: &str| Source {
        identity: id.into(),
        build_time: 1,
        order_hint: None,
        group: None,
        metadata: Vec::new(),
    };
    for id in ["a/1", "a/2", "b/1"] {
        let added = store::add(
            &repo,
            NewSnapshot {
                source: source(id),
                items: vec![store::Item {
                    path: "f.txt".into(),
                    bytes: id.as_bytes().to_vec(),
                }],
                rejected: Vec::new(),
                record: Vec::new(),
            },
        )
        .unwrap();
        assert!(matches!(added, Added::Written { .. }));
    }
    let record_only = store::add(
        &repo,
        NewSnapshot {
            source: source("r/1"),
            items: Vec::new(),
            rejected: Vec::new(),
            record: vec![("storage".into(), "zip".into())],
        },
    )
    .unwrap();
    assert!(matches!(record_only, Added::Written { .. }));
    fsck(&repo_path);

    let (_, out, _) = git_seriate(&repo_path, &["list", "--record-only"]);
    assert_eq!(out, "r/1\n");
    let (ok, out, _) = git_seriate(&repo_path, &["drop", "--glob", "a/*"]);
    assert!(ok);
    assert_eq!(out, "deleted a/1\ndeleted a/2\n");
    let (_, out, _) = git_seriate(&repo_path, &["list"]);
    assert_eq!(out, "b/1\n", "the record-only branch is not a snapshot's");
    let (_, out, _) = git_seriate(&repo_path, &["drop", "--record-only"]);
    assert_eq!(out, "deleted r/1\n");
}

#[test]
fn import_copies_the_branches_once() {
    let tmp = tempfile::tempdir().unwrap();
    let theirs = new_repo(tmp.path(), "theirs.git");
    let ours = new_repo(tmp.path(), "ours.git");
    let snap = tmp.path().join("snap");
    write(&snap, &[("x.txt", "x\n"), ("d/y.txt", "y\n")]);
    let (ok, _, err) = git_seriate(
        &theirs,
        &[
            "add",
            "--id",
            "s",
            "--build-time",
            "1",
            snap.to_str().unwrap(),
        ],
    );
    assert!(ok, "{err}");

    let (ok, out, err) = git_seriate(&ours, &["import", theirs.to_str().unwrap()]);
    assert!(ok, "{err}");
    assert!(out.ends_with("copied 1, already here 0\n"), "{out}");
    fsck(&ours);
    assert_eq!(
        git(&ours, &["rev-parse", "s"]),
        git(&theirs, &["rev-parse", "s"])
    );
    let (_, out, _) = git_seriate(&ours, &["import", theirs.to_str().unwrap()]);
    assert!(out.ends_with("copied 0, already here 1\n"), "{out}");

    // Another commit under that name, there: refused.
    let other = new_repo(tmp.path(), "other.git");
    write(&snap, &[("x.txt", "changed\n")]);
    let (ok, _, _) = git_seriate(
        &other,
        &[
            "add",
            "--id",
            "s",
            "--build-time",
            "1",
            snap.to_str().unwrap(),
        ],
    );
    assert!(ok);
    let (ok, _, err) = git_seriate(&ours, &["import", other.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("exists here"), "{err}");
}

/// The tree `add` builds is the one git builds of the same files (spec 0466
/// S1: `add` writes its own trees), git's order included: `d.txt` before
/// the directory `d`, `d-` before it too.
#[test]
fn trees_are_the_ones_git_builds() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path(), "repo.git");
    let files = [
        ("a.txt", "a\n"),
        ("d.txt", "d\n"),
        ("d-x", "dash\n"),
        ("d/e/f.txt", "f\n"),
        ("d/g.txt", "g\n"),
        ("z", "z\n"),
    ];
    let dir = tmp.path().join("snap");
    write(&dir, &files);
    let (ok, _, err) = git_seriate(
        &repo,
        &[
            "add",
            "--id",
            "s",
            "--build-time",
            "1",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "{err}");
    let ours = git(&repo, &["rev-parse", "s^{tree}"]);
    // git's: the same files through an index.
    let index = tmp.path().join("index");
    let with_index = |args: &[&str]| {
        let out = Command::new("git")
            .arg("--git-dir")
            .arg(&repo)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    for (path, _) in files {
        let blob = with_index(&["hash-object", "-w", dir.join(path).to_str().unwrap()]);
        with_index(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{},{path}", blob.trim()),
        ]);
    }
    let gits = with_index(&["write-tree"]);
    assert_eq!(ours.trim(), gits.trim());
    fsck(&repo);
}
