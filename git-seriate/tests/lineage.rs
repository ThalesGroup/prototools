// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Lineage statements (spec 0499 S1, S1b, S5a): `lineage set`, `unset`,
//! `list`, and their record, `refs/seriate/lineages`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SNAPSHOTS: &[&str] = &["s0", "s1", "pkg/s2"];

fn command(program: &str, repo: &Path) -> Command {
    let mut c = Command::new(program);
    c.arg("-C")
        .arg(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", repo)
        .env("GIT_COMMITTER_NAME", "Settler")
        .env("GIT_COMMITTER_EMAIL", "settler@example.com")
        .env("GIT_AUTHOR_NAME", "Settler")
        .env("GIT_AUTHOR_EMAIL", "settler@example.com");
    c
}

fn git_seriate_in(repo: &Path, args: &[&str], stdin: &str) -> (bool, String, String) {
    let mut child = command(env!("CARGO_BIN_EXE_git-seriate"), repo)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn git_seriate(repo: &Path, args: &[&str]) -> (bool, String, String) {
    git_seriate_in(repo, args, "")
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = command("git", repo).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository holding SNAPSHOTS.
fn repo(root: &Path) -> PathBuf {
    let repo = root.join("r.git");
    gix::init_bare(&repo).unwrap();
    for (n, id) in SNAPSHOTS.iter().enumerate() {
        let dir = root.join(format!("d{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), format!("{n}\n")).unwrap();
        let time = (1_700_000_000 + n).to_string();
        let (ok, _, err) = git_seriate(
            &repo,
            &[
                "add",
                "--id",
                id,
                "--build-time",
                &time,
                dir.to_str().unwrap(),
            ],
        );
        assert!(ok, "{err}");
    }
    repo
}

fn record_file(repo: &Path) -> String {
    git(repo, &["show", "refs/seriate/lineages:lineages"])
}

#[test]
fn statements_are_set_listed_and_unset_one_record_commit_each() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());

    let (ok, out, err) = git_seriate(&repo, &["lineage", "set", "bundler", "s1", "pkg/s2"]);
    assert!(ok, "{err}");
    assert!(out.contains("moved s1: default -> bundler"), "{out}");
    assert!(out.contains("moved pkg/s2: default -> bundler"), "{out}");
    assert!(out.contains("recorded in refs/seriate/lineages"), "{out}");
    assert_eq!(record_file(&repo), "pkg/s2 bundler\ns1 bundler");
    let first = git(&repo, &["rev-parse", "refs/seriate/lineages"]);
    // S5a: authored and committed by the user; no parent, the first one.
    assert_eq!(
        git(
            &repo,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%P", &first]
        ),
        "Settler <settler@example.com>|Settler <settler@example.com>|"
    );
    let subject = git(&repo, &["log", "-1", "--format=%s", &first]);
    assert_eq!(subject, "Lineages: set bundler, 2 snapshot(s)");

    let (ok, out, _) = git_seriate(&repo, &["lineage", "list"]);
    assert!(ok);
    assert_eq!(out, "pkg/s2 bundler\ns1 bundler\n");
    let (_, out, _) = git_seriate(&repo, &["lineage", "list", "other"]);
    assert_eq!(out, "");

    // The same again: nothing to record, no commit.
    let (ok, out, err) = git_seriate(&repo, &["lineage", "set", "bundler", "s1"]);
    assert!(ok, "{err}");
    assert!(out.contains("unchanged s1"), "{out}");
    assert!(out.contains("nothing to record"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "refs/seriate/lineages"]), first);

    // unset: back to default, the previous record the parent.
    let (ok, out, err) = git_seriate(&repo, &["lineage", "unset", "s1"]);
    assert!(ok, "{err}");
    assert!(out.contains("moved s1: bundler -> default"), "{out}");
    assert_eq!(record_file(&repo), "pkg/s2 bundler");
    assert_eq!(
        git(
            &repo,
            &["log", "-1", "--format=%P", "refs/seriate/lineages"]
        ),
        first
    );
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

#[test]
fn statements_from_a_file_or_standard_input() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let file = tmp.path().join("statements");
    std::fs::write(&file, "s0 bundler\n\npkg/s2 other\n").unwrap();
    let (ok, out, err) = git_seriate(&repo, &["lineage", "set", "--from", file.to_str().unwrap()]);
    assert!(ok, "{err}");
    assert!(out.contains("moved s0: default -> bundler"), "{out}");
    assert!(out.contains("moved pkg/s2: default -> other"), "{out}");
    assert_eq!(record_file(&repo), "pkg/s2 other\ns0 bundler");

    let (ok, out, err) = git_seriate_in(&repo, &["lineage", "set", "--from", "-"], "s1 other\n");
    assert!(ok, "{err}");
    assert!(out.contains("moved s1: default -> other"), "{out}");
    let (_, out, _) = git_seriate(&repo, &["lineage", "list", "other"]);
    assert_eq!(out, "pkg/s2 other\ns1 other\n");
}

#[test]
fn a_statement_that_would_miss_is_refused_and_nothing_written() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let refused = |args: &[&str], stdin: &str, says: &str| {
        let (ok, _, err) = git_seriate_in(&repo, args, stdin);
        assert!(!ok, "{args:?}");
        assert!(err.contains(says), "{args:?}: {err}");
        let refs = git(&repo, &["for-each-ref", "refs/seriate/lineages"]);
        assert_eq!(refs, "", "{args:?}: nothing written");
    };
    refused(
        &["lineage", "set", "bundler", "s0", "nope", "gone"],
        "",
        "2 identities are no snapshot of this repository (nope, …): nothing stated",
    );
    refused(
        &["lineage", "set", "default", "s0"],
        "",
        "use `lineage unset`",
    );
    refused(&["lineage", "set", "a/b", "s0"], "", "one ref component");
    refused(
        &["lineage", "set", "--from", "-"],
        "s0 default\n",
        "use `lineage unset`",
    );
    refused(
        &["lineage", "set", "--from", "-"],
        "s0\n",
        "expected `<identity> <lineage>`",
    );
    refused(
        &["lineage", "set", "bundler"],
        "",
        "give NAME and IDENTITY..., or --from FILE",
    );
    refused(&["lineage", "unset", "nope"], "", "is no snapshot");

    // --dry-run: said, not written.
    let (ok, out, err) = git_seriate(&repo, &["lineage", "set", "--dry-run", "bundler", "s0"]);
    assert!(ok, "{err}");
    assert!(out.contains("would move s0: default -> bundler"), "{out}");
    assert_eq!(git(&repo, &["for-each-ref", "refs/seriate/lineages"]), "");
}

/// A snapshot pruned from the store stays in its history, and may be stated.
#[test]
fn a_snapshot_known_only_to_a_history_may_be_stated() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    let (ok, _, err) = git_seriate(&repo, &["prune"]);
    assert!(ok, "{err}");
    assert_eq!(git(&repo, &["for-each-ref", "refs/seriate/snapshots/"]), "");
    let (ok, out, err) = git_seriate(&repo, &["lineage", "set", "bundler", "s1"]);
    assert!(ok, "{err}");
    assert!(out.contains("moved s1: default -> bundler"), "{out}");
}
