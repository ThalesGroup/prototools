// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A canonical history taken as it is (spec 0464): a history commit written
//! with today's rule sets and renderer is not canonicalized again;
//! `--verify-canon` checks what it claims; `compact` marks the commits
//! written before the renderer's version was recorded.

use std::cell::Cell;
use std::path::Path;
use std::process::Command;

use git_seriate::compact;
use git_seriate::format::{Block, Source};
use git_seriate::history::{self, Cutoff, Options, Plugins, Report};
use git_seriate::plugins::{Canonicalizer, Pattern, Renderer};
use git_seriate::store::{self, Item, NewSnapshot};

/// Stores items as they are, counting the items it parses; its version is
/// `version`.
struct Counting {
    parses: Cell<usize>,
    version: u32,
}

impl Counting {
    fn new(version: u32) -> Self {
        Counting {
            parses: Cell::new(0),
            version,
        }
    }
}

impl Renderer for Counting {
    fn render(&self, _: &str, item: &[u8]) -> Result<Vec<u8>, String> {
        Ok(item.to_vec())
    }
    fn parse(&self, _: &str, stored: &[u8]) -> Result<Vec<u8>, String> {
        self.parses.set(self.parses.get() + 1);
        Ok(stored.to_vec())
    }
    fn version(&self) -> Vec<(String, u32)> {
        vec![("counting".into(), self.version)]
    }
}

/// One rule set: text in lowercase.
struct Lower;

impl Canonicalizer for Lower {
    fn rule_sets(&self) -> Vec<(String, u32)> {
        vec![("lower".into(), 1)]
    }
    fn canon(&self, _: &str, item: &[u8]) -> Vec<u8> {
        item.to_ascii_lowercase()
    }
}

const DAY: i64 = 86_400;
const T0: i64 = 1_700_000_000;

fn env() {
    std::env::set_var("GIT_COMMITTER_NAME", "Tester");
    std::env::set_var("GIT_COMMITTER_EMAIL", "tester@example.com");
}

fn add(repo: &gix::Repository, id: &str, day: i64, text: &str) {
    store::add(
        repo,
        NewSnapshot {
            source: Source {
                identity: id.into(),
                build_time: T0 + day * DAY,
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

/// A repository with three snapshots, in uppercase.
fn repo(root: &Path) -> gix::Repository {
    env();
    let repo = gix::init_bare(root.join("repo.git")).unwrap();
    for (i, text) in ["A\n", "A\nB\n", "A\nB\nC\n"].iter().enumerate() {
        add(&repo, &format!("s{i}"), i as i64, text);
    }
    repo
}

fn settle(repo: &gix::Repository, renderer: &Counting, dry_run: bool, verify: bool) -> Report {
    env();
    let pattern = Pattern(None);
    history::settle(
        repo,
        &Options {
            into: Some("main".into()),
            cutoff: Cutoff::Off,
            settling_days: 60,
            dry_run,
            branches: Vec::new(),
            sort: seriation::Settings::default(),
            plugins: Plugins {
                renderer,
                canonicalizer: &Lower,
                item_paths: &pattern,
                comparator: &seriation::LinesChars,
                legacy: None,
            },
            verify_canon: verify,
        },
    )
    .unwrap()
}

fn tip(repo: &gix::Repository) -> gix::ObjectId {
    repo.find_reference("refs/heads/main")
        .unwrap()
        .peel_to_id()
        .unwrap()
        .detach()
}

/// The blocks of `main`, newest first.
fn blocks(repo: &gix::Repository) -> Vec<Block> {
    let mut out = Vec::new();
    let mut id = Some(tip(repo));
    while let Some(i) = id {
        let commit = repo.find_commit(i).unwrap();
        let decoded = commit.decode().unwrap();
        out.push(Block::read(&decoded.message.to_string()).unwrap().unwrap());
        id = decoded.parents().next();
    }
    out
}

fn git(repo: &gix::Repository, args: &[&str], stdin: Option<&str>) -> String {
    use std::io::Write;
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(repo.git_dir())
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG_GLOBAL")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.unwrap_or("").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `main`'s tip replaced by a commit of `tree` and `message`, on the same
/// parent.
fn replace_tip(repo: &gix::Repository, tree: &str, message: &str) {
    let parent = git(repo, &["rev-parse", "main^"], None);
    let commit = git(repo, &["commit-tree", tree, "-p", &parent], Some(message));
    git(repo, &["update-ref", "refs/heads/main", &commit], None);
}

fn tip_message(repo: &gix::Repository) -> String {
    git(repo, &["log", "-1", "--format=%B", "main"], None)
}

#[test]
fn a_trusted_history_is_not_canonicalized() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let first = Counting::new(1);
    settle(&repo, &first, false, false);
    assert!(first.parses.get() > 0);
    assert!(blocks(&repo)
        .iter()
        .all(|b| b.renderer == [("counting".to_string(), 1)]));
    let before = tip(&repo);

    // Nothing new, the whole history re-sorted: no item parsed, nothing
    // rewritten.
    let again = Counting::new(1);
    let report = settle(&repo, &again, false, false);
    assert_eq!(again.parses.get(), 0, "{report:?}");
    assert_eq!(tip(&repo), before);
}

#[test]
fn another_renderer_version_is_canonicalized() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    settle(&repo, &Counting::new(1), false, false);
    let before = tip(&repo);
    let v2 = Counting::new(2);
    settle(&repo, &v2, false, false);
    assert!(v2.parses.get() > 0);
    assert_ne!(tip(&repo), before, "rewritten with the new version");
    assert!(blocks(&repo)
        .iter()
        .all(|b| b.renderer == [("counting".to_string(), 2)]));
}

#[test]
fn verify_canon_finds_a_false_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    settle(&repo, &Counting::new(1), false, false);
    // The tip's item replaced by an uppercase one, the marker unchanged.
    let blob = git(&repo, &["hash-object", "-w", "--stdin"], Some("A\nB\nC\n"));
    let tree = git(
        &repo,
        &["mktree"],
        Some(&format!("100644 blob {blob}\tf.txt\n")),
    );
    replace_tip(&repo, &tree, &tip_message(&repo));

    let plain = settle(&repo, &Counting::new(1), true, false);
    assert!(plain.false_markers.is_empty(), "trusted, not looked at");
    let verified = settle(&repo, &Counting::new(1), true, true);
    assert_eq!(verified.false_markers.len(), 1, "{verified:?}");
    assert!(
        verified.false_markers[0].starts_with("s2: f.txt: "),
        "{verified:?}"
    );
}

#[test]
fn legacy_histories_are_canonicalized_and_compact_marks_them() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    settle(&repo, &Counting::new(1), false, false);
    // The tip as written before spec 0464: no renderer's version.
    let mut block = blocks(&repo).remove(0);
    block.renderer.clear();
    let tree = git(&repo, &["rev-parse", "main^{tree}"], None);
    let message = tip_message(&repo);
    let subject = message.split("\n\n").next().unwrap();
    replace_tip(&repo, &tree, &format!("{subject}\n\n{}", block.message()));
    let before = tip(&repo);

    // Not trusted: canonicalized; but kept, not rewritten.
    let r = Counting::new(1);
    settle(&repo, &r, false, false);
    assert!(r.parses.get() > 0);
    assert_eq!(tip(&repo), before);

    // compact marks it; the next settle parses nothing.
    let marker = Counting::new(1);
    let report = compact::compact(
        &repo,
        &compact::Options {
            into: Some("main".into()),
            cutoff: T0,
            level: 1,
            dry_run: false,
        },
        &compact::Plugins {
            legacy: None,
            renderer: &marker,
            canonicalizer: &Lower,
        },
    )
    .unwrap();
    assert_eq!(report.marked, 1, "{report:?}");
    assert!(blocks(&repo)
        .iter()
        .all(|b| b.renderer == [("counting".to_string(), 1)]));
    let after = Counting::new(1);
    settle(&repo, &after, false, false);
    assert_eq!(after.parses.get(), 0);
}

// ── The read-back, on demand (spec 0465) ─────────────────────────────────────

/// Drops its item's last byte: no text of it reads back.
struct Lossy;

impl Renderer for Lossy {
    fn render(&self, _: &str, item: &[u8]) -> Result<Vec<u8>, String> {
        Ok(item[..item.len().saturating_sub(1)].to_vec())
    }
    fn parse(&self, _: &str, stored: &[u8]) -> Result<Vec<u8>, String> {
        Ok(stored.to_vec())
    }
    fn version(&self) -> Vec<(String, u32)> {
        vec![("lossy".into(), 1)]
    }
}

#[test]
fn render_does_not_read_back_unless_configured() {
    use git_seriate::plugins::Checked;
    use std::rc::Rc;

    /// A `Counting` shared, so that its count stays readable once boxed.
    struct Shared(Rc<Counting>);
    impl Renderer for Shared {
        fn render(&self, p: &str, i: &[u8]) -> Result<Vec<u8>, String> {
            self.0.render(p, i)
        }
        fn parse(&self, p: &str, s: &[u8]) -> Result<Vec<u8>, String> {
            self.0.parse(p, s)
        }
        fn version(&self) -> Vec<(String, u32)> {
            self.0.version()
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let repo = gix::init_bare(tmp.path().join("repo.git")).unwrap();
    for (setting, parses) in [(None, 0), (Some("true"), 1)] {
        if let Some(value) = setting {
            git(&repo, &["config", "seriate.checkRendering", value], None);
        }
        let repo = gix::open(repo.git_dir()).unwrap();
        let counting = Rc::new(Counting::new(1));
        let renderer = Checked::configured(&repo, Box::new(Shared(counting.clone())));
        renderer.render("f.txt", b"a\n").unwrap();
        assert_eq!(
            counting.parses.get(),
            parses,
            "seriate.checkRendering = {setting:?}"
        );
    }
}

#[test]
fn check_rendering_refuses() {
    use git_seriate::plugins::Checked;
    assert!(
        Lossy.render("f.txt", b"a\n").is_ok(),
        "unchecked, it passes"
    );
    let err = Checked(Box::new(Lossy))
        .render("f.txt", b"a\n")
        .unwrap_err();
    assert!(err.contains("does not read back"), "{err}");
}
