// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Every history commit has an author (spec 0471): a commit whose author
//! email is empty has none, and gets the identity of the user who writes
//! it, its date kept; any other author is kept, through rewrites too.

use std::process::Command;

use git_seriate::compact;
use git_seriate::format::Source;
use git_seriate::history::{self, Cutoff, Options, Plugins};
use git_seriate::plugins::{AsIs, NoRules, Pattern};
use git_seriate::store::{self, Item, NewSnapshot};

const DAY: i64 = 86_400;
const T0: i64 = 1_700_000_000;

fn identity() {
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "Settler"),
        ("GIT_AUTHOR_EMAIL", "settler@example.com"),
        ("GIT_COMMITTER_NAME", "Settler"),
        ("GIT_COMMITTER_EMAIL", "settler@example.com"),
    ] {
        std::env::set_var(k, v);
    }
}

fn add(repo: &gix::Repository, id: &str, day: i64, hint: u64, text: &str) {
    store::add(
        repo,
        NewSnapshot {
            source: Source {
                identity: id.into(),
                build_time: T0 + day * DAY,
                order_hint: Some(hint),
                group: None,
                metadata: Vec::new(),
            },
            items: vec![Item {
                path: "f.txt".into(),
                bytes: text.as_bytes().to_vec(),
                record: Vec::new(),
                origin: None,
            }],
            rejected: Vec::new(),
            record: Vec::new(),
        },
    )
    .unwrap();
}

fn settle(repo: &gix::Repository) {
    identity();
    let pattern = Pattern(None);
    history::settle(
        repo,
        &Options {
            into: Some("main".into()),
            cutoff: Cutoff::Off,
            settling_days: 60,
            dry_run: false,
            branches: Vec::new(),
            sort: seriation::Settings::default(),
            plugins: Plugins {
                renderer: &AsIs,
                canonicalizer: &NoRules,
                item_paths: &pattern,
                comparator: &seriation::LinesChars,
                legacy: None,
            },
            verify_canon: false,
        },
    )
    .unwrap();
}

/// `git` on `repo`, with `env` added; its trimmed output.
fn git(repo: &gix::Repository, args: &[&str], env: &[(&str, &str)], stdin: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(repo.git_dir())
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_COMMITTER_NAME", "Settler")
        .env("GIT_COMMITTER_EMAIL", "settler@example.com")
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `branch`'s commits, newest first: (author, author date, subject).
fn authors(repo: &gix::Repository, branch: &str) -> Vec<(String, i64, String)> {
    git(repo, &["log", "--format=%an <%ae>|%at|%s", branch], &[], "")
        .lines()
        .map(|l| {
            let mut p = l.splitn(3, '|');
            (
                p.next().unwrap().to_string(),
                p.next().unwrap().parse().unwrap(),
                p.next().unwrap().to_string(),
            )
        })
        .collect()
}

/// The tip of the ref `branch` (a full name) rewritten with the author
/// `author` (name, email) and the same tree, parents, message and author
/// date.
fn reauthor_tip(repo: &gix::Repository, branch: &str, name: &str, email: &str) {
    let tree = git(repo, &["rev-parse", &format!("{branch}^{{tree}}")], &[], "");
    let parents: Vec<String> = git(repo, &["log", "-1", "--format=%P", branch], &[], "")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let message = git(repo, &["log", "-1", "--format=%B", branch], &[], "");
    let date = git(repo, &["log", "-1", "--format=@%at +0000", branch], &[], "");
    let mut args = vec!["commit-tree".to_string(), tree];
    for p in &parents {
        args.push("-p".into());
        args.push(p.clone());
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let commit = git(
        repo,
        &args,
        &[
            ("GIT_AUTHOR_NAME", name),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_AUTHOR_DATE", &date),
        ],
        &message,
    );
    git(repo, &["update-ref", branch, &commit], &[], "");
}

fn repo() -> (tempfile::TempDir, gix::Repository) {
    identity();
    let tmp = tempfile::tempdir().unwrap();
    let repo = gix::init_bare(tmp.path().join("r.git")).unwrap();
    (tmp, repo)
}

#[test]
fn a_tool_authored_snapshot_is_authored_by_the_user() {
    let (_tmp, repo) = repo();
    add(&repo, "s0", 0, 10, "a\n");
    add(&repo, "s1", 1, 11, "a\nb\n");
    settle(&repo);
    let history = authors(&repo, "main/default");
    assert_eq!(history.len(), 2);
    for (author, date, _) in &history {
        assert_eq!(author, "Settler <settler@example.com>");
        assert!(
            [10, 11].contains(date),
            "the order hint as its date: {date}"
        );
    }
    assert_eq!(
        authors(&repo, "refs/seriate/snapshots/s0")[0].0,
        "git-seriate <>"
    );
}

#[test]
fn a_snapshots_author_is_kept() {
    let (_tmp, repo) = repo();
    add(&repo, "s0", 0, 10, "a\n");
    reauthor_tip(
        &repo,
        "refs/seriate/snapshots/s0",
        "Alice",
        "alice@example.com",
    );
    settle(&repo);
    assert_eq!(
        authors(&repo, "main/default")[0].0,
        "Alice <alice@example.com>",
        "a snapshot's own author"
    );
}

#[test]
fn a_rewrite_re_authors_only_the_authorless() {
    let (_tmp, repo) = repo();
    add(&repo, "s1", 1, 11, "a\n");
    add(&repo, "s2", 2, 12, "a\nb\n");
    settle(&repo);
    // The tip as settled before 0471: no author; a manual commit by Bob on
    // top.
    reauthor_tip(&repo, "refs/heads/main/default", "git-seriate", "");
    let tree = git(&repo, &["rev-parse", "main/default^{tree}"], &[], "");
    let tip = git(&repo, &["rev-parse", "main/default"], &[], "");
    let bob = git(
        &repo,
        &["commit-tree", &tree, "-p", &tip],
        &[
            ("GIT_AUTHOR_NAME", "Bob"),
            ("GIT_AUTHOR_EMAIL", "bob@example.com"),
        ],
        "docs: a note\n",
    );
    git(
        &repo,
        &["update-ref", "refs/heads/main/default", &bob],
        &[],
        "",
    );
    assert_eq!(authors(&repo, "main/default")[1].0, "git-seriate <>");

    // A snapshot built before them all: every commit re-sorted, replayed.
    add(&repo, "s0", 0, 10, "");
    settle(&repo);
    let history = authors(&repo, "main/default");
    let by_subject = |s: &str| history.iter().find(|h| h.2.contains(s)).unwrap().clone();
    assert_eq!(by_subject("docs: a note").0, "Bob <bob@example.com>");
    for id in ["s0", "s1", "s2"] {
        let (author, date, _) = by_subject(id);
        assert_eq!(author, "Settler <settler@example.com>", "{id}");
        assert!((10..=12).contains(&date), "{id}: {date}");
    }

    // compact, rewriting: s2's commit as written before 0464 and 0471 (no
    // renderer, no author), under Bob's note; compact marks it, so rewrites
    // it: the user its author, Bob kept.
    let s2 = git(&repo, &["rev-parse", "main/default~1"], &[], "");
    let message: String = git(&repo, &["log", "-1", "--format=%B", &s2], &[], "")
        .lines()
        .filter(|l| !l.starts_with("renderer:") && !l.starts_with("  \"generic\""))
        .map(|l| format!("{l}\n"))
        .collect();
    let date = git(&repo, &["log", "-1", "--format=@%at +0000", &s2], &[], "");
    let old = git(
        &repo,
        &[
            "commit-tree",
            &git(&repo, &["rev-parse", "main/default~1^{tree}"], &[], ""),
            "-p",
            &git(&repo, &["rev-parse", "main/default~2"], &[], ""),
        ],
        &[
            ("GIT_AUTHOR_NAME", "git-seriate"),
            ("GIT_AUTHOR_EMAIL", ""),
            ("GIT_AUTHOR_DATE", &date),
        ],
        &message,
    );
    let note = git(
        &repo,
        &[
            "commit-tree",
            &git(&repo, &["rev-parse", "main/default^{tree}"], &[], ""),
            "-p",
            &old,
        ],
        &[
            ("GIT_AUTHOR_NAME", "Bob"),
            ("GIT_AUTHOR_EMAIL", "bob@example.com"),
        ],
        "docs: a note\n",
    );
    git(
        &repo,
        &["update-ref", "refs/heads/main/default", &note],
        &[],
        "",
    );
    assert_eq!(authors(&repo, "main/default")[1].0, "git-seriate <>");
    identity();
    let report = compact::compact(
        &repo,
        &compact::Options {
            into: Some("main".into()),
            cutoff: T0 + 10 * DAY,
            level: 1,
            dry_run: false,
        },
        &compact::Plugins {
            legacy: None,
            renderer: &AsIs,
            canonicalizer: &NoRules,
        },
    )
    .unwrap()
    .remove(0);
    assert_eq!(report.marked, 1, "{report:?}");
    let history = authors(&repo, "main/default");
    assert_eq!(history[0].0, "Bob <bob@example.com>", "{history:?}");
    assert_eq!(history[1].0, "Settler <settler@example.com>", "{history:?}");
    assert_eq!(history[1].1, 12, "its date kept: {history:?}");
}

#[test]
fn a_recreated_branch_is_the_original_commit() {
    let (_tmp, repo) = repo();
    add(&repo, "s0", 0, 10, "a\n");
    add(&repo, "s1", 1, 11, "a\nb\n");
    settle(&repo);
    let original = git(&repo, &["rev-parse", "refs/seriate/snapshots/s0"], &[], "");
    git(
        &repo,
        &["update-ref", "-d", "refs/seriate/snapshots/s0"],
        &[],
        "",
    );
    // A snapshot before them: the history re-sorted, s0 recreated from it.
    add(&repo, "s", 0, 9, "");
    settle(&repo);
    assert_eq!(
        git(&repo, &["rev-parse", "refs/seriate/snapshots/s0"], &[], ""),
        original
    );
}
