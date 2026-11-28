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
    // The record is shared: no local path in its message.
    let subject = git(
        &repo,
        &["log", "-1", "--format=%s", "refs/seriate/lineages"],
    );
    assert_eq!(subject, "Lineages: set --from FILE, 2 snapshot(s)");

    let (ok, out, err) = git_seriate_in(&repo, &["lineage", "set", "--from", "-"], "s1 other\n");
    assert!(ok, "{err}");
    assert!(out.contains("moved s1: default -> other"), "{out}");
    let subject = git(
        &repo,
        &["log", "-1", "--format=%s", "refs/seriate/lineages"],
    );
    assert_eq!(subject, "Lineages: set --from -, 1 snapshot(s)");
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
    refused(
        &["lineage", "unset", "nope"],
        "",
        "is no snapshot of this repository, nor named by a statement (nope)",
    );

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

// ── settle, per lineage (S2, S3, S3a) ────────────────────────────────────────

const DAY: u64 = 86_400;

/// Snapshots of one enum file: `exe0`..`exe3`, executables holding its
/// full copy, one version per build; and `exe2!set@100`, a bundler's set
/// in `exe2`, built with it, its copy stripped of the options.
fn toy(root: &Path) -> PathBuf {
    let repo = root.join("toy.git");
    gix::init_bare(&repo).unwrap();
    let full = |n: u64| format!("enum E {{\n  X = {n} [deprecated = true];\n}}\n");
    let snapshots: [(&str, u64, String); 5] = [
        ("exe0", 0, full(0)),
        ("exe1", 1, full(1)),
        ("exe2", 2, full(2)),
        ("exe2!set@100", 2, "enum E {\n  X = 2;\n}\n".into()),
        ("exe3", 3, full(3)),
    ];
    for (n, (id, day, text)) in snapshots.iter().enumerate() {
        let dir = root.join(format!("toy{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("e.proto"), text).unwrap();
        let time = (1_700_000_000 + day * DAY).to_string();
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

fn subjects(repo: &Path, branch: &str) -> String {
    git(repo, &["log", "--reverse", "--format=%s", branch])
}

/// The record's tree file `name`.
fn record(repo: &Path, history: &str, name: &str) -> String {
    git(
        repo,
        &["show", &format!("refs/seriate/histories/{history}:{name}")],
    )
}

#[test]
fn a_stated_producer_is_sorted_on_a_line_of_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = toy(tmp.path());
    let (ok, _, err) = git_seriate(&repo, &["lineage", "set", "bundler", "exe2!set@100"]);
    assert!(ok, "{err}");
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(
        out.contains("settled main/bundler: kept 0, wrote 1"),
        "{out}"
    );
    assert!(
        out.contains("settled main/default: kept 0, wrote 4"),
        "{out}"
    );
    assert!(
        out.contains("recorded in refs/seriate/histories/main"),
        "{out}"
    );

    // Each line holds its producer's snapshots only, in build order.
    assert_eq!(
        subjects(&repo, "main/default"),
        "Snapshot exe0\nSnapshot exe1\nSnapshot exe2\nSnapshot exe3"
    );
    assert_eq!(subjects(&repo, "main/bundler"), "Snapshot exe2!set@100");
    assert_eq!(
        git(&repo, &["show", "main/bundler:e.proto"]),
        "enum E {\n  X = 2;\n}"
    );

    // The record: each lineage's tip, the statements routed by; its
    // parents the tips and the statements' record (S2, S5a).
    let default = git(&repo, &["rev-parse", "main/default"]);
    let bundler = git(&repo, &["rev-parse", "main/bundler"]);
    let statements = git(&repo, &["rev-parse", "refs/seriate/lineages"]);
    assert_eq!(
        record(&repo, "main", "lineages"),
        format!("bundler {bundler}\ndefault {default}")
    );
    assert_eq!(record(&repo, "main", "statements"), statements);
    let parents = git(
        &repo,
        &["log", "-1", "--format=%P", "refs/seriate/histories/main"],
    );
    assert_eq!(parents, format!("{bundler} {default} {statements}"));
    assert_eq!(
        git(
            &repo,
            &[
                "log",
                "-1",
                "--format=%an|%cn",
                "refs/seriate/histories/main"
            ]
        ),
        "Settler|Settler"
    );
    // Nothing new: nothing settled, no record written.
    let before = git(&repo, &["rev-parse", "refs/seriate/histories/main"]);
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(err.contains("nothing new to settle"), "{err}");
    assert_eq!(
        git(&repo, &["rev-parse", "refs/seriate/histories/main"]),
        before
    );
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

#[test]
fn a_lineage_stated_after_a_settle_moves_the_snapshot_at_the_next() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = toy(tmp.path());
    // Nothing stated: one line, the set inside the executables' span.
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(subjects(&repo, "main/default").contains("exe2!set@100"));
    let first = git(&repo, &["rev-parse", "refs/seriate/histories/main"]);

    let (ok, _, err) = git_seriate(&repo, &["lineage", "set", "bundler", "exe2!set@100"]);
    assert!(ok, "{err}");
    // Nothing moves before a settle; a dry run names the move.
    assert!(subjects(&repo, "main/default").contains("exe2!set@100"));
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--dry-run"]);
    assert!(ok, "{err}");
    assert!(
        out.contains("would move exe2!set@100: main/default -> main/bundler"),
        "{out}"
    );
    assert!(subjects(&repo, "main/default").contains("exe2!set@100"));

    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(
        out.contains("moved exe2!set@100: main/default -> main/bundler"),
        "{out}"
    );
    assert!(
        out.contains("previous tip saved as main/default-1"),
        "{out}"
    );
    assert_eq!(
        subjects(&repo, "main/default"),
        "Snapshot exe0\nSnapshot exe1\nSnapshot exe2\nSnapshot exe3"
    );
    assert_eq!(subjects(&repo, "main/bundler"), "Snapshot exe2!set@100");
    // The new record follows the first.
    let parents = git(
        &repo,
        &["log", "-1", "--format=%P", "refs/seriate/histories/main"],
    );
    assert!(parents.starts_with(&first), "{parents}");

    // Back to default, every snapshot of bundler moving out: the lineage
    // emptied, its tip saved, gone from the record.
    let (ok, _, err) = git_seriate(&repo, &["lineage", "unset", "exe2!set@100"]);
    assert!(ok, "{err}");
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(
        out.contains(
            "emptied main/bundler: every snapshot of it moved; its tip saved as main/bundler-1"
        ),
        "{out}"
    );
    assert!(subjects(&repo, "main/default").contains("exe2!set@100"));
    assert_eq!(git(&repo, &["branch", "--list", "main/bundler"]), "");
    let default = git(&repo, &["rev-parse", "main/default"]);
    assert_eq!(
        record(&repo, "main", "lineages"),
        format!("default {default}")
    );
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

#[test]
fn a_lineage_branch_names_its_history() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = toy(tmp.path());
    let (ok, _, err) = git_seriate(&repo, &["lineage", "set", "bundler", "exe2!set@100"]);
    assert!(ok, "{err}");
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    // --into main/bundler: the history main, both lineages re-sorted.
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main/bundler", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert!(out.contains("settled main/bundler"), "{out}");
    assert!(out.contains("settled main/default"), "{out}");
    assert_eq!(git(&repo, &["branch", "--list", "main/bundler/*"]), "");
}

#[test]
fn a_history_of_the_former_layout_is_refused_then_migrated() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = toy(tmp.path());
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "tmp"]);
    assert!(ok, "{err}");
    // A history of before 0499: a branch of its own name, no record.
    let tip = git(&repo, &["rev-parse", "tmp/default"]);
    git(&repo, &["update-ref", "refs/heads/old", &tip]);
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "old"]);
    assert!(!ok);
    assert!(
        err.contains("old is a history of before spec 0499"),
        "{err}"
    );
    assert!(err.contains("git seriate migrate old"), "{err}");

    // --into NEW: written beside, the branch left as it is.
    let (ok, out, err) = git_seriate(&repo, &["migrate", "old", "--into", "master"]);
    assert!(ok, "{err}");
    assert!(out.contains("wrote history old as master/default"), "{out}");
    assert!(out.contains("old left as it is"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "master/default"]), tip);
    assert_eq!(git(&repo, &["rev-parse", "refs/heads/old"]), tip);
    assert_eq!(
        record(&repo, "master", "lineages"),
        format!("default {tip}")
    );
    let (ok, _, err) = git_seriate(&repo, &["migrate", "old", "--into", "master"]);
    assert!(!ok);
    assert!(err.contains("master exists"), "{err}");

    // In place: the former branch kept under refs/seriate/migrated/.
    let (ok, out, err) = git_seriate(&repo, &["migrate", "old"]);
    assert!(ok, "{err}");
    assert!(
        out.contains("its former branch kept as refs/seriate/migrated/old"),
        "{out}"
    );
    assert_eq!(git(&repo, &["rev-parse", "old/default"]), tip);
    assert_eq!(git(&repo, &["rev-parse", "refs/seriate/migrated/old"]), tip);
    assert_eq!(git(&repo, &["branch", "--list", "old"]), "");
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "old"]);
    assert!(ok, "{err}");
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

/// Forgetting a lineage's only snapshot empties that lineage, its tip
/// saved; the history's other lineage is left as it is.
#[test]
fn forgetting_a_lineages_only_snapshot_empties_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = toy(tmp.path());
    let (ok, _, err) = git_seriate(&repo, &["lineage", "set", "bundler", "exe2!set@100"]);
    assert!(ok, "{err}");
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    let default = git(&repo, &["rev-parse", "main/default"]);
    let (ok, out, err) = git_seriate(
        &repo,
        &["forget", "--into", "main", "--reason", "x", "exe2!set@100"],
    );
    assert!(ok, "{err}");
    assert!(out.contains("forgot exe2!set@100"), "{out}");
    assert!(
        out.contains(
            "emptied main/bundler: every snapshot of it forgotten; its tip saved as main/bundler-1"
        ),
        "{out}"
    );
    assert_eq!(git(&repo, &["rev-parse", "main/default"]), default);
    assert_eq!(
        record(&repo, "main", "lineages"),
        format!("default {default}")
    );
}

/// Invariant 0002: the histories and the two records, pushed, carry no
/// snapshot commit (spec 0499 S5a), whatever the history went through: a
/// settle per lineage, a move, a forget. `refs/seriate/forgotten/` does,
/// and stays local.
#[test]
fn pushing_the_histories_and_records_carries_no_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = toy(tmp.path());
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    let (ok, _, err) = git_seriate(&repo, &["lineage", "set", "bundler", "exe2!set@100"]);
    assert!(ok, "{err}");
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    let (ok, _, err) = git_seriate(
        &repo,
        &["forget", "--into", "main", "--reason", "x", "exe1"],
    );
    assert!(ok, "{err}");
    // Every snapshot commit there is: in the store, and forgotten.
    let mut snapshots: Vec<String> = git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/seriate/snapshots/",
        ],
    )
    .lines()
    .map(str::to_string)
    .collect();
    snapshots.extend(
        git(
            &repo,
            &["log", "-1", "--format=%P", "refs/seriate/forgotten/1"],
        )
        .split_whitespace()
        .map(str::to_string),
    );
    assert_eq!(snapshots.len(), 5, "{snapshots:?}");

    let remote = tmp.path().join("remote.git");
    gix::init_bare(&remote).unwrap();
    git(
        &repo,
        &[
            "push",
            "-q",
            remote.to_str().unwrap(),
            "refs/heads/main/default:refs/heads/main/default",
            "refs/heads/main/bundler:refs/heads/main/bundler",
            "refs/seriate/histories/main:refs/seriate/histories/main",
            "refs/seriate/lineages:refs/seriate/lineages",
        ],
    );
    for id in &snapshots {
        let there = Command::new("git")
            .args(["-C", remote.to_str().unwrap(), "cat-file", "-e", id])
            .status()
            .unwrap();
        assert!(!there.success(), "snapshot commit {id} was pushed");
    }
    // And the remote is whole: what was pushed reaches nothing missing.
    git(&remote, &["fsck", "--full", "--strict", "--no-dangling"]);
}

/// A statement outlives its snapshot (spec 0499 S1): the snapshot gone
/// from the store and from every history, `lineage list` still names it,
/// `set` refuses it, and `unset` removes it; then `unset` refuses it too,
/// nothing left to remove.
#[test]
fn a_statement_whose_snapshot_is_gone_can_be_unset() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let (ok, _, err) = git_seriate(&repo, &["lineage", "set", "bundler", "s0"]);
    assert!(ok, "{err}");
    git(&repo, &["update-ref", "-d", "refs/seriate/snapshots/s0"]);

    let (ok, out, err) = git_seriate(&repo, &["lineage", "list"]);
    assert!(ok, "{err}");
    assert!(out.contains("s0 bundler"), "{out}");
    let (ok, _, err) = git_seriate(&repo, &["lineage", "set", "other", "s0"]);
    assert!(!ok);
    assert!(
        err.contains("is no snapshot of this repository (s0)"),
        "{err}"
    );

    let (ok, out, err) = git_seriate(&repo, &["lineage", "unset", "s0"]);
    assert!(ok, "{err}");
    assert!(out.contains("moved s0: bundler -> default"), "{out}");
    assert!(!record_file(&repo).contains("s0"), "{}", record_file(&repo));

    let (ok, _, err) = git_seriate(&repo, &["lineage", "unset", "s0"]);
    assert!(!ok);
    assert!(err.contains("nor named by a statement (s0)"), "{err}");
}
