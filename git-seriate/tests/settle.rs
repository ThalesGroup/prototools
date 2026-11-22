// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `settle`, end to end (spec 0452 test plan 3–5, 7): repositories in
//! temporary directories, the binary run on them.

use std::path::{Path, PathBuf};
use std::process::Command;

use git_seriate::format::Source;
use git_seriate::store::{self, Item, NewSnapshot};
use seriation::generate::{generate, Params};

const ENV: [(&str, &str); 4] = [
    ("GIT_COMMITTER_NAME", "Tester"),
    ("GIT_COMMITTER_EMAIL", "tester@example.com"),
    ("GIT_AUTHOR_NAME", "Tester"),
    ("GIT_AUTHOR_EMAIL", "tester@example.com"),
];

/// No configuration but the repository's: not the user's (which may sign
/// with their key), not the system's.
fn isolated(command: &mut Command, repo: &Path) {
    let home = repo.parent().unwrap();
    command
        .envs(ENV)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG_GLOBAL");
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

/// A `seriation` corpus's snapshots, added as snapshot branches.
fn add_corpus(repo: &Path, name: &str, filter: impl Fn(usize) -> bool) {
    let args = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../seriation/tests/corpora/{name}.args")),
    )
    .unwrap();
    let corpus = generate(&Params::from_args(args.lines().map(str::to_string)).unwrap());
    let r = gix::open(repo).unwrap();
    for (i, s) in corpus.snapshots.iter().enumerate() {
        if !filter(i) {
            continue;
        }
        store::add(
            &r,
            NewSnapshot {
                source: Source {
                    identity: s.identity.clone(),
                    build_time: s.build_time,
                    order_hint: s.order_hint,
                    group: None,
                    metadata: Vec::new(),
                },
                items: s
                    .files
                    .iter()
                    .map(|(path, text)| Item {
                        path: path.clone(),
                        bytes: text.clone().into_bytes(),
                    })
                    .collect(),
                rejected: Vec::new(),
                record: Vec::new(),
            },
        )
        .unwrap();
    }
}

/// The history, oldest first, as `identity<TAB>tree` lines.
fn history(repo: &Path, branch: &str) -> String {
    let log = git(repo, &["log", "--reverse", "--format=%T %B%x00", branch]);
    log.split('\0')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| {
            let (tree, message) = c.split_once(' ').unwrap();
            let identity = message
                .lines()
                .next()
                .unwrap()
                .trim_start_matches("Snapshot ");
            format!("{identity}\t{tree}\n")
        })
        .collect()
}

fn build_times(repo: &Path) -> Vec<(i64, String)> {
    let r = gix::open(repo).unwrap();
    let mut times: Vec<(i64, String)> = store::list(&r)
        .unwrap()
        .into_iter()
        .map(|l| (l.block.source.build_time, l.branch))
        .collect();
    times.sort();
    times
}

#[test]
fn the_corpora_settle_as_the_existing_implementation_does() {
    for name in ["plain", "noisy", "builds"] {
        let expected = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/histories/{name}.history.tsv")),
        )
        .unwrap();

        // In one go.
        let tmp = tempfile::tempdir().unwrap();
        let repo = new_repo(tmp.path());
        add_corpus(&repo, name, |_| true);
        let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
        assert!(ok, "{name}: {err}");
        let onego = history(&repo, "main");
        assert_eq!(onego, expected, "{name}: one go");
        git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);

        // The newer half by build time first, then the rest.
        let tmp = tempfile::tempdir().unwrap();
        let repo = new_repo(tmp.path());
        add_corpus(&repo, name, |_| true);
        let times = build_times(&repo);
        let newer: Vec<&str> = times[times.len() / 2..]
            .iter()
            .map(|(_, b)| b.as_str())
            .collect();
        let mut args = vec!["settle", "--into", "main"];
        args.extend(&newer);
        let (ok, _, err) = git_seriate(&repo, &args);
        assert!(ok, "{name}: {err}");
        let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
        assert!(ok, "{name}: {err}");
        assert_eq!(history(&repo, "main"), expected, "{name}: in halves");
    }
}

/// A small history: three snapshots of one growing file.
fn small(repo: &Path) {
    let r = gix::open(repo).unwrap();
    for (i, text) in ["a\n", "a\nb\n", "a\nb\nc\n"].iter().enumerate() {
        store::add(
            &r,
            NewSnapshot {
                source: Source {
                    identity: format!("s{i}"),
                    build_time: 1_700_000_000 + i as i64 * 86_400,
                    order_hint: None,
                    group: None,
                    metadata: Vec::new(),
                },
                items: vec![Item {
                    path: "f.txt".into(),
                    bytes: text.as_bytes().to_vec(),
                }],
                rejected: Vec::new(),
                record: Vec::new(),
            },
        )
        .unwrap();
    }
}

/// A manual commit on `branch` writing `path`.
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
    let file = worktree.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    run(&["add", path]);
    let tree = run(&["write-tree"]);
    let parent = run(&["rev-parse", branch]);
    let commit = run(&["commit-tree", &tree, "-p", &parent, "-m", subject]);
    run(&["update-ref", &format!("refs/heads/{branch}"), &commit]);
}

#[test]
fn nothing_new_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    small(&repo);
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(out.contains("wrote 3 snapshot(s)"), "{out}");
    let before = git(&repo, &["rev-parse", "main"]);

    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok);
    assert!(err.contains("nothing new to settle"), "{err}");
    // A full re-sort finds the same order, and keeps every commit.
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert!(out.contains("kept 3, wrote 0"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), before);
    assert!(
        git(&repo, &["branch", "--list", "main-*"]).is_empty(),
        "no backup"
    );
}

#[test]
fn a_manual_commit_follows_its_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    git(&repo, &["config", "seriate.itemPattern", "*.txt"]);
    small(&repo);
    // s0 and s2 first; notes after s2; then s1, built between them.
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main", "s0", "s2"]);
    assert!(ok, "{err}");
    manual_commit(&repo, "main", "NOTES.md", "notes\n", "docs: notes");
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert!(out.contains("replay "), "{out}");
    assert!(out.contains("previous tip saved as main-1"), "{out}");
    let subjects = git(&repo, &["log", "--reverse", "--format=%s", "main"]);
    assert_eq!(
        subjects,
        "Snapshot s0\nSnapshot s1\nSnapshot s2\ndocs: notes\n"
    );
    let message = git(&repo, &["log", "-1", "--format=%B", "main"]);
    assert!(message.contains("Snapshot-Anchor: s2"), "{message}");
    let tip = git(&repo, &["rev-parse", "main"]);

    // Again: the recorded anchor keeps it where it is.
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert!(out.contains("wrote 0"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), tip);
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

#[test]
fn a_manual_commit_on_an_item_path_stops_the_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    small(&repo);
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main", "s1", "s2"]);
    assert!(ok, "{err}");
    manual_commit(&repo, "main", "README.md", "hi\n", "docs: readme");
    // No pattern: README.md counts as an item, and the warning says so.
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(err.contains("settle stopped at"), "{err}");
    assert!(err.contains("no seriate.itemPattern is set"), "{err}");
    let subjects = git(&repo, &["log", "--reverse", "--format=%s", "main"]);
    assert_eq!(
        subjects, "Snapshot s1\nSnapshot s2\ndocs: readme\nSnapshot s0\n",
        "below the stop nothing moves; s0 goes on top"
    );
}

#[test]
fn a_dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    small(&repo);
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--dry-run"]);
    assert!(ok, "{err}");
    assert!(out.contains("would settle main"), "{out}");
    assert!(git(&repo, &["branch", "--list", "main"]).is_empty());
    let objects = git(&repo, &["count-objects", "-v"]);
    let (ok, _, _) = git_seriate(&repo, &["settle", "--into", "main", "--dry-run"]);
    assert!(ok);
    assert_eq!(
        git(&repo, &["count-objects", "-v"]),
        objects,
        "no object written"
    );
}

#[test]
fn commits_are_signed_when_git_is_configured_to() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    let key = tmp.path().join("key");
    let Ok(made) = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .output()
    else {
        eprintln!("skipped: no ssh-keygen");
        return;
    };
    assert!(made.status.success());
    git(&repo, &["config", "commit.gpgSign", "true"]);
    git(&repo, &["config", "gpg.format", "ssh"]);
    git(&repo, &["config", "user.signingKey", key.to_str().unwrap()]);
    small(&repo);
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    let commit = git(&repo, &["cat-file", "commit", "main"]);
    assert!(
        commit.contains("gpgsig -----BEGIN SSH SIGNATURE-----"),
        "{commit}"
    );
    // The payload signed is the commit without its signature: git agrees.
    let allowed = tmp.path().join("allowed");
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    std::fs::write(&allowed, format!("tester@example.com {public}")).unwrap();
    git(
        &repo,
        &[
            "config",
            "gpg.ssh.allowedSignersFile",
            allowed.to_str().unwrap(),
        ],
    );
    git(&repo, &["verify-commit", "main"]);
}

/// Runs the `git ...` command a checked-out note names, in `repo`.
fn run_note(repo: &Path, err: &str) -> String {
    let command = err
        .split('\'')
        .nth(1)
        .unwrap_or_else(|| panic!("no command in: {err}"));
    let words: Vec<&str> = command.split_whitespace().collect();
    assert_eq!(words[0], "git", "{command}");
    git(repo, &words[1..]);
    command.to_string()
}

/// Settling the checked-out branch leaves the index and work tree alone;
/// the command the note names brings them to the new tip, keeping local
/// changes, whether the branch was unborn or had a tip.
#[test]
fn the_checked_out_note_names_a_command_that_works() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("work");
    git(
        tmp.path(),
        &["init", "-q", "-b", "main", repo.to_str().unwrap()],
    );
    std::fs::write(repo.join("local.txt"), "mine\n").unwrap();
    small(&repo);

    // Unborn: no previous tip.
    let (ok, _, err) = git_seriate(&repo, &["settle"]);
    assert!(ok, "{err}");
    assert_eq!(run_note(&repo, &err), "git read-tree -m -u main");
    assert_eq!(git(&repo, &["status", "--porcelain"]), "?? local.txt\n");
    assert_eq!(
        std::fs::read_to_string(repo.join("f.txt")).unwrap(),
        "a\nb\nc\n"
    );

    // A previous tip, and an uncommitted edit to a file settle doesn't touch.
    git(&repo, &["config", "seriate.itemPattern", "f.txt"]);
    std::fs::write(repo.join("notes.md"), "notes\n").unwrap();
    git(&repo, &["add", "notes.md"]);
    git(&repo, &["commit", "-q", "-m", "Add notes"]);
    std::fs::write(repo.join("notes.md"), "notes, edited\n").unwrap();
    let r = gix::open(&repo).unwrap();
    store::add(
        &r,
        NewSnapshot {
            source: Source {
                identity: "s3".into(),
                build_time: 1_700_000_000 + 3 * 86_400,
                order_hint: None,
                group: None,
                metadata: Vec::new(),
            },
            items: vec![Item {
                path: "f.txt".into(),
                bytes: b"a\nb\nc\nd\n".to_vec(),
            }],
            rejected: Vec::new(),
            record: Vec::new(),
        },
    )
    .unwrap();
    let (ok, _, err) = git_seriate(&repo, &["settle"]);
    assert!(ok, "{err}");
    assert_eq!(run_note(&repo, &err), "git read-tree -m -u main-1 main");
    assert_eq!(
        git(&repo, &["status", "--porcelain"]),
        " M notes.md\n?? local.txt\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("f.txt")).unwrap(),
        "a\nb\nc\nd\n"
    );
}
