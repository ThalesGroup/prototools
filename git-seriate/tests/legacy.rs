// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The legacy reader (spec 0452 S2): a history in an older format, settled
//! with new snapshots on top, then converted by `compact`.

use gix::object::tree::EntryKind;
use gix::ObjectId;

use git_seriate::compact;
use git_seriate::format::{Block, Source};
use git_seriate::history::{self, Cutoff, Options, Plugins};
use git_seriate::plugins::{AsIs, LegacyReader, NoRules, Pattern};
use git_seriate::store::{self, Item, NewSnapshot};

/// A toy older format: `Old snapshot\n\nid: …\ntime: …\npaths: a,b`.
struct Old;

impl LegacyReader for Old {
    fn read(&self, message: &str) -> Option<Block> {
        let body = message.strip_prefix("Old snapshot\n\n")?;
        let field = |name: &str| {
            body.lines()
                .find_map(|l| l.strip_prefix(&format!("{name}: ")))
                .map(str::to_string)
        };
        Some(Block {
            source: Source {
                identity: field("id")?,
                build_time: field("time")?.parse().ok()?,
                order_hint: None,
                group: None,
                metadata: Vec::new(),
            },
            contents: Some(field("paths")?.split(',').map(str::to_string).collect()),
            evidence: Default::default(),
            rejected: Vec::new(),
            record: Vec::new(),
            canon_rules: Vec::new(),
            renderer: Vec::new(),
            reduced: false,
        })
    }
}

fn signature(seconds: i64) -> gix::actor::Signature {
    gix::actor::Signature {
        name: "old-tool".into(),
        email: "".into(),
        time: gix::date::Time::new(seconds, 0),
    }
}

/// A history in the old format: each commit the previous tree with one
/// file's new version.
fn old_history(repo: &gix::Repository, versions: &[&str]) -> ObjectId {
    let mut parent: Option<ObjectId> = None;
    let mut tree = ObjectId::empty_tree(repo.object_hash());
    for (i, text) in versions.iter().enumerate() {
        let blob = repo.write_blob(text.as_bytes()).unwrap().detach();
        let mut editor = repo.edit_tree(tree).unwrap();
        editor.upsert("f.txt", EntryKind::Blob, blob).unwrap();
        tree = editor.write().unwrap().detach();
        let time = 1_700_000_000 + i as i64 * 86_400;
        let commit = gix::objs::Commit {
            tree,
            parents: parent.into_iter().collect(),
            author: signature(time),
            committer: signature(time),
            encoding: None,
            message: format!("Old snapshot\n\nid: old{i}\ntime: {time}\npaths: f.txt\n").into(),
            extra_headers: Vec::new(),
        };
        parent = Some(repo.write_object(&commit).unwrap().detach());
    }
    let id = parent.unwrap();
    store::set_ref(
        repo,
        &store::branch_ref("main").unwrap(),
        gix::refs::transaction::PreviousValue::MustNotExist,
        id,
    )
    .unwrap();
    id
}

fn messages(repo: &gix::Repository) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = repo
        .find_reference("refs/heads/main")
        .ok()
        .map(|mut r| r.peel_to_id().unwrap().detach());
    while let Some(id) = current {
        let commit = repo.find_commit(id).unwrap();
        let decoded = commit.decode().unwrap();
        out.push(decoded.message.to_string());
        current = decoded.parents().next();
    }
    out.reverse();
    out
}

fn options<'a>(item_paths: &'a Pattern, legacy: &'a Old) -> Options<'a> {
    Options {
        into: Some("main".into()),
        cutoff: Cutoff::Off,
        settling_days: 60,
        dry_run: false,
        branches: Vec::new(),
        sort: seriation::Settings::default(),
        plugins: Plugins {
            renderer: &AsIs,
            canonicalizer: &NoRules,
            item_paths,
            comparator: &seriation::LinesChars,
            legacy: Some(legacy),
        },
        verify_canon: false,
    }
}

#[test]
fn an_old_history_is_settled_and_converted() {
    std::env::set_var("GIT_COMMITTER_NAME", "Tester");
    std::env::set_var("GIT_COMMITTER_EMAIL", "tester@example.com");
    let tmp = tempfile::tempdir().unwrap();
    let repo = gix::init_bare(tmp.path().join("repo.git")).unwrap();
    old_history(&repo, &["a\n", "a\nb\n", "a\nb\nc\n"]);

    // A new snapshot, built before the last old one: settle reads the old
    // history through the legacy reader and puts it between.
    store::add(
        &repo,
        NewSnapshot {
            source: Source {
                identity: "new".into(),
                build_time: 1_700_000_000 + 86_400 + 3_600,
                order_hint: None,
                group: None,
                metadata: Vec::new(),
            },
            items: vec![Item {
                path: "f.txt".into(),
                bytes: b"a\nb\nx\n".to_vec(),
                record: Vec::new(),
                origin: None,
            }],
            rejected: Vec::new(),
            record: Vec::new(),
        },
    )
    .unwrap();
    let pattern = Pattern(None);
    let report = history::settle(&repo, &options(&pattern, &Old)).unwrap();
    assert!(report.moved, "{report:?}");
    let identities: Vec<String> = messages(&repo)
        .iter()
        .map(|m| {
            Block::read_with(m, Some(&Old))
                .unwrap()
                .unwrap()
                .source
                .identity
        })
        .collect();
    assert_eq!(identities, ["old0", "old1", "new", "old2"]);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("recreated snapshot branch old")),
        "the old snapshots had no branch: rebuilt from the history, {report:?}"
    );

    // compact, dropping nothing, writes what remains old in format 1.
    let report = compact::compact(
        &repo,
        &compact::Options {
            into: Some("main".into()),
            cutoff: 1_700_000_000 + 10 * 86_400,
            level: 1,
            dry_run: false,
        },
        &compact::Plugins {
            legacy: Some(&Old),
            renderer: &AsIs,
            canonicalizer: &NoRules,
        },
    )
    .unwrap();
    assert_eq!((report.dropped, report.converted), (0, 2), "{report:?}");
    for m in messages(&repo) {
        assert!(Block::read(&m).unwrap().is_some(), "not converted: {m}");
    }
}

/// A snapshot branch whose commit carries the old format: parentless,
/// named as its identity.
fn old_branch(repo: &gix::Repository, id: &str) -> ObjectId {
    let blob = repo.write_blob(id.as_bytes()).unwrap().detach();
    let mut editor = repo
        .edit_tree(ObjectId::empty_tree(repo.object_hash()))
        .unwrap();
    editor.upsert("f.txt", EntryKind::Blob, blob).unwrap();
    let tree = editor.write().unwrap().detach();
    let commit = gix::objs::Commit {
        tree,
        parents: Default::default(),
        author: signature(1_700_000_000),
        committer: signature(1_700_000_000),
        encoding: None,
        message: format!("Old snapshot\n\nid: {id}\ntime: 1700000000\npaths: f.txt\n").into(),
        extra_headers: Vec::new(),
    };
    let commit_id = repo.write_object(&commit).unwrap().detach();
    store::set_ref(
        repo,
        &store::branch_ref(id).unwrap(),
        gix::refs::transaction::PreviousValue::MustNotExist,
        commit_id,
    )
    .unwrap();
    commit_id
}

#[test]
fn old_snapshot_branches_are_listed_imported_and_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let theirs = gix::init_bare(tmp.path().join("theirs.git")).unwrap();
    let commit = old_branch(&theirs, "pkg/v1/0/bin");

    // Without the reader, not a snapshot branch; with it, one.
    assert!(store::list(&theirs).unwrap().is_empty());
    let listed = store::list_with(&theirs, Some(&Old)).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].branch, "pkg/v1/0/bin");

    let ours = gix::init_bare(tmp.path().join("ours.git")).unwrap();
    let imported = store::import(&ours, &tmp.path().join("theirs.git"), Some(&Old)).unwrap();
    assert_eq!(imported.copied, ["pkg/v1/0/bin"]);
    assert_eq!(
        store::tip(&ours, &store::branch_ref("pkg/v1/0/bin").unwrap()).unwrap(),
        Some(commit)
    );

    let deleted = store::drop(&ours, None, false, Some(&Old)).unwrap();
    assert_eq!(deleted, ["pkg/v1/0/bin"]);
    assert!(store::list_with(&ours, Some(&Old)).unwrap().is_empty());
}
