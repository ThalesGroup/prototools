// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `extract --tar-out` (spec 0466): the snapshots as an archive of a bare
//! repository, with no repository; the same bytes whatever the order of the
//! files; the same refs and objects as `extract` into a repository.

use std::path::{Path, PathBuf};
use std::process::Command;

const T0: u64 = 1_700_000_000;

/// The stock descriptor set's files.
fn stock_files() -> Vec<Vec<u8>> {
    let set = prototext::EMBEDDED_DESCRIPTOR;
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < set.len() {
        assert_eq!(set[pos], 0x0a);
        pos += 1;
        let (mut len, mut shift) = (0usize, 0);
        loop {
            let b = set[pos];
            pos += 1;
            len |= usize::from(b & 0x7f) << shift;
            shift += 7;
            if b < 0x80 {
                break;
            }
        }
        out.push(set[pos..pos + len].to_vec());
        pos += len;
    }
    out
}

/// Three "executables" embedding some of the stock descriptors each.
fn executables(dir: &Path) -> Vec<PathBuf> {
    let files = stock_files();
    let mut out = Vec::new();
    for (i, range) in [(0..3), (1..5), (2..7)].into_iter().enumerate() {
        let mut bytes = vec![0u8; 64];
        for f in &files[range] {
            bytes.extend_from_slice(f);
            bytes.extend_from_slice(&[0u8; 16]);
        }
        let path = dir.join(format!("bin{i}"));
        std::fs::write(&path, bytes).unwrap();
        out.push(path);
    }
    out
}

/// `git seriate-fdp` in `dir`, with no repository above it.
fn seriate_fdp(dir: &Path, args: &[String]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_git-seriate-fdp"))
        .current_dir(dir)
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
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
        .arg("--git-dir")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", repo)
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `extract`, the files named as in `dir`, then `extra`.
fn extract_args(files: &[PathBuf], extra: &[&str]) -> Vec<String> {
    let mut args = vec!["extract".to_string(), "--build-time".into(), T0.to_string()];
    args.extend(extra.iter().map(|a| a.to_string()));
    args.extend(
        files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned()),
    );
    args
}

fn unpack(archive: &Path, into: &Path) {
    std::fs::create_dir_all(into).unwrap();
    let ok = Command::new("tar")
        .arg("xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

#[test]
fn tar_out_is_deterministic_and_needs_no_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let files = executables(&work);
    let (ok, out, err) = seriate_fdp(&work, &extract_args(&files, &["--tar-out", "a.tar"]));
    assert!(ok, "{err}");
    assert_eq!(
        out.lines().filter(|l| l.starts_with("added ")).count(),
        3,
        "{out}"
    );
    let reversed: Vec<PathBuf> = files.iter().rev().cloned().collect();
    let (ok, _, err) = seriate_fdp(&work, &extract_args(&reversed, &["--tar-out", "b.tar"]));
    assert!(ok, "{err}");
    let a = std::fs::read(work.join("a.tar")).unwrap();
    assert!(
        a == std::fs::read(work.join("b.tar")).unwrap(),
        "the order changed the archive"
    );

    // To standard output: the same bytes, the messages on standard error.
    let out = Command::new(env!("CARGO_BIN_EXE_git-seriate-fdp"))
        .current_dir(&work)
        .args(extract_args(&files, &["--tar-out", "-"]))
        .env("GIT_CEILING_DIRECTORIES", tmp.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &work)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("added "), "{err}");
    assert!(out.stdout == a, "standard output is not the archive");

    // No repository was needed, nor made.
    assert!(!work.join(".git").exists() && !work.join("HEAD").exists());
}

#[test]
fn tar_out_is_the_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let files = executables(&work);
    let (ok, _, err) = seriate_fdp(&work, &extract_args(&files, &["--tar-out", "a.tar"]));
    assert!(ok, "{err}");
    let unpacked = tmp.path().join("unpacked.git");
    unpack(&work.join("a.tar"), &unpacked);
    git(&unpacked, &["fsck", "--full", "--strict", "--no-dangling"]);

    // The same extraction into a repository, the files named alike (their
    // identities are their paths as given).
    let repo = tmp.path().join("repo.git");
    gix::init_bare(&repo).unwrap();
    let mut args = vec!["-C".to_string(), repo.display().to_string()];
    args.extend(extract_args(&files, &[]));
    let (ok, _, err) = seriate_fdp(&work, &args);
    assert!(ok, "{err}");
    let refs = |r: &Path| git(r, &["for-each-ref", "--format=%(objectname) %(refname)"]);
    let objects = |r: &Path| {
        let mut list: Vec<String> = git(r, &["cat-file", "--batch-all-objects", "--batch-check"])
            .lines()
            .map(str::to_string)
            .collect();
        list.sort();
        list
    };
    assert_eq!(refs(&unpacked), refs(&repo));
    assert_eq!(objects(&unpacked), objects(&repo));
    // No branch yet: HEAD names one to be, as `git init --bare` leaves it.
    assert_eq!(
        git(&unpacked, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/main"
    );

    // And settled, the same history.
    for r in [&unpacked, &repo] {
        let out = Command::new(env!("CARGO_BIN_EXE_git-seriate-fdp"))
            .args(["-C", r.to_str().unwrap(), "settle", "--into", "main"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &work)
            .env("GIT_COMMITTER_NAME", "Tester")
            .env("GIT_COMMITTER_EMAIL", "tester@example.com")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let trees = |r: &Path| git(r, &["log", "--format=%T", "main"]);
    assert_eq!(trees(&unpacked), trees(&repo));
}
