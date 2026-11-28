// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A snapshot met again (spec 0500), with the `fdp` rule set, whose R1
//! drops a `json_name` equal to protoc's default: a raw form and its
//! history's canonical form differ.

use std::path::{Path, PathBuf};

use git_seriate::cli::Kit;
use git_seriate::history::{self, Cutoff, Options, Plugins};
use git_seriate::plugins::{NoRules, Pattern};
use git_seriate::store::{self, Added};
use git_seriate_fdp::extract::{extract_files, Options as ExtractOptions};
use git_seriate_fdp::kit;
use prototext_upb::text_format::parse_descriptor;

const T0: i64 = 1_700_000_000;

fn env() {
    std::env::set_var("GIT_COMMITTER_NAME", "Tester");
    std::env::set_var("GIT_COMMITTER_EMAIL", "tester@example.com");
}

/// A descriptor whose field sets `json_name` to protoc's default, which
/// `fdp`'s R1 drops; `n` changes its message's name.
fn descriptor(n: u32) -> Vec<u8> {
    parse_descriptor(
        &format!(
            r#"name: "x/a.proto" package: "x" message_type {{ name: "M{n}" field {{ name: "foo_bar" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 json_name: "fooBar" }} }}"#
        ),
        false,
    )
    .unwrap()
}

/// An executable holding `fdp`, after 64 bytes of other data.
fn executable(dir: &Path, name: &str, fdp: &[u8]) -> PathBuf {
    let mut bytes = vec![0u8; 64];
    bytes.extend_from_slice(fdp);
    bytes.extend_from_slice(&[0u8; 16]);
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn extract(
    r: &gix::Repository,
    kit: &Kit,
    file: &Path,
    id: &str,
    day: i64,
    metadata: &[(&str, &str)],
) -> Result<Vec<Added>, git_seriate::Error> {
    extract_files(
        r,
        kit,
        &[file.to_path_buf()],
        &ExtractOptions {
            id: Some(id.into()),
            build_time: Some(T0 + day * 86_400),
            metadata: metadata
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..ExtractOptions::default()
        },
        &mut Vec::new(),
    )
    .map(|outcomes| outcomes.into_iter().map(|o| o.added).collect())
}

fn settle(r: &gix::Repository, kit: &Kit) -> Result<history::Settled, git_seriate::Error> {
    env();
    let pattern = Pattern(kit.item_pattern.clone());
    history::settle(
        r,
        &Options {
            into: Some("main".into()),
            cutoff: Cutoff::Off,
            settling_days: 60,
            dry_run: false,
            branches: Vec::new(),
            sort: seriation::Settings::default(),
            plugins: Plugins {
                renderer: kit.renderer.as_ref(),
                canonicalizer: kit.canonicalizer.as_ref(),
                item_paths: &pattern,
                comparator: kit.comparator.as_ref(),
                legacy: None,
            },
            verify_canon: false,
            verbose: false,
        },
    )
}

fn snapshot_tip(r: &gix::Repository, id: &str) -> Option<gix::ObjectId> {
    store::tip(r, &git_seriate::layout::snapshot_ref(id).unwrap()).unwrap()
}

/// A history settled from `a` (day 1), the store pruned: its raw form
/// lost; a snapshot built before it, settled, rebuilds `a` from the history.
fn pruned_and_rebuilt(tmp: &Path) -> (gix::Repository, PathBuf, gix::ObjectId) {
    let r = gix::init_bare(tmp.join("repo.git")).unwrap();
    let kit = kit(&r).unwrap();
    let file = executable(tmp, "a.bin", &descriptor(1));
    extract(&r, &kit, &file, "a", 1, &[]).unwrap();
    let original = snapshot_tip(&r, "a").unwrap();
    settle(&r, &kit).unwrap();
    let listed = store::to_prune(&r, None, false, None).unwrap();
    assert_eq!(
        git_seriate::meet::raw_forms_lost(&r, &listed, None).unwrap(),
        1,
        "its json_name is in the raw form only"
    );
    store::prune(&r, None, false, None).unwrap();
    let older = executable(tmp, "z.bin", &descriptor(0));
    extract(&r, &kit, &older, "z", 0, &[]).unwrap();
    let settled = settle(&r, &kit).unwrap();
    assert!(
        settled.lineages[0]
            .notes
            .iter()
            .any(|n| n == "recreated snapshot a"),
        "{settled:?}"
    );
    let rebuilt = snapshot_tip(&r, "a").unwrap();
    assert_ne!(rebuilt, original, "rebuilt from the canonical form");
    (r, file, original)
}

#[test]
fn an_original_met_again_replaces_its_rebuilt_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let (r, file, original) = pruned_and_rebuilt(tmp.path());
    let kit = kit(&r).unwrap();
    let history = store::tip(&r, &store::branch_ref("main/default").unwrap()).unwrap();
    let added = extract(&r, &kit, &file, "a", 1, &[]).unwrap();
    assert_eq!(
        added,
        [Added::Replaced {
            identity: "a".into()
        }]
    );
    assert_eq!(snapshot_tip(&r, "a"), Some(original), "the original back");
    // The history already holds it: nothing to settle, nothing rewritten.
    settle(&r, &kit).unwrap();
    assert_eq!(
        store::tip(&r, &store::branch_ref("main/default").unwrap()).unwrap(),
        history
    );
    // Met again, the original is kept.
    let added = extract(&r, &kit, &file, "a", 1, &[]).unwrap();
    assert_eq!(
        added,
        [Added::Unchanged {
            identity: "a".into()
        }]
    );
}

#[test]
fn another_snapshot_under_the_identity_is_refused_naming_forget() {
    let tmp = tempfile::tempdir().unwrap();
    let (r, _, _) = pruned_and_rebuilt(tmp.path());
    let kit = kit(&r).unwrap();
    let other = executable(tmp.path(), "b.bin", &descriptor(9));
    let err = extract(&r, &kit, &other, "a", 1, &[])
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("1 snapshot(s) met their history and were refused"),
        "{err}"
    );
    assert!(err.contains("a: not the snapshot main/default"), "{err}");
    assert!(
        err.contains("forget the history's, then extract again"),
        "{err}"
    );
    // A different build time: another source.
    let file = executable(tmp.path(), "a2.bin", &descriptor(1));
    let err = extract(&r, &kit, &file, "a", 2, &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("its source differs"), "{err}");
}

/// The history written under `fdp` 1, today's rule sets none: the original
/// holds the json_name the history's canonical form dropped, and nothing
/// today can tell that from other bytes. Undecided, listed; accepted, the
/// next settle re-canonicalizes from it.
#[test]
fn an_undecided_meeting_is_listed_then_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let (r, file, original) = pruned_and_rebuilt(tmp.path());
    let mut today = kit(&r).unwrap();
    today.canonicalizer = Box::new(NoRules);
    // The rebuilt copy goes too: the original then meets the history alone.
    store::prune(&r, None, false, None).unwrap();

    let err = extract(&r, &today, &file, "a", 1, &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("a: undecided"), "{err}");
    assert!(
        err.contains("written under fdp 1, today's rule sets none"),
        "{err}"
    );
    assert!(err.contains("x/a.pb"), "{err}");
    let line = format!("a {original}");
    assert!(err.contains(&line), "the line accept --from takes: {err}");
    assert_eq!(snapshot_tip(&r, "a"), None, "refused: not written");

    let meeter = git_seriate::cli::meeter(&r, &today).unwrap();
    let chosen = [("a".to_string(), original)];
    // A dry run writes nothing.
    git_seriate::meet::accept(&r, &meeter, &chosen, "R1 dropped", true, None).unwrap();
    assert_eq!(
        store::tip(
            &r,
            &git_seriate::layout::full_name("refs/seriate/accepted", "r").unwrap()
        )
        .unwrap(),
        None
    );
    env();
    let lines = git_seriate::meet::accept(&r, &meeter, &chosen, "R1 dropped", false, None).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].starts_with(&format!("a\t{original}\t")),
        "{lines:?}"
    );
    let accepted = git_seriate::meet::accepted(&r).unwrap();
    assert_eq!(accepted[0].commit, original);

    // Accepted: written, then settled, from it, under today's rule sets.
    let added = extract(&r, &today, &file, "a", 1, &[]).unwrap();
    assert_eq!(
        added,
        [Added::Written {
            identity: "a".into(),
            rejected: Vec::new()
        }]
    );
    let settled = settle(&r, &today).unwrap();
    assert!(!settled.lineages.is_empty(), "{settled:?}");
    // The history now holds the original's form, under today's rule sets
    // (none): the json_name `fdp`'s R1 had dropped is back.
    let tip = store::tip(&r, &store::branch_ref("main/default").unwrap())
        .unwrap()
        .unwrap();
    let tree = r.find_commit(tip).unwrap().tree().unwrap();
    let entry = tree.lookup_entry_by_path("x/a.pb").unwrap().unwrap();
    let text = String::from_utf8(r.find_blob(entry.object_id()).unwrap().data.clone()).unwrap();
    assert!(text.contains("fooBar"), "{text}");
}

/// The same snapshot with one label more: it is the history's, the labels
/// reported, nothing rewritten for them (spec 0500 S1a).
#[test]
fn labels_differing_are_reported_and_rewrite_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let r = gix::init_bare(tmp.path().join("repo.git")).unwrap();
    let kit = kit(&r).unwrap();
    let file = executable(tmp.path(), "a.bin", &descriptor(1));
    extract(&r, &kit, &file, "a", 1, &[]).unwrap();
    let original = snapshot_tip(&r, "a").unwrap();
    settle(&r, &kit).unwrap();
    let history = store::tip(&r, &store::branch_ref("main/default").unwrap()).unwrap();

    let meeter = git_seriate::cli::meeter(&r, &kit).unwrap();
    let mut warnings = Vec::new();
    let outcomes = git_seriate_fdp::extract::extract_files_into(
        &r,
        &mut store::Sink::Repo(&r),
        Some(&meeter),
        &kit,
        std::slice::from_ref(&file),
        &ExtractOptions {
            id: Some("a".into()),
            build_time: Some(T0 + 86_400),
            metadata: vec![("arch".into(), "amd64".into())],
            ..ExtractOptions::default()
        },
        &mut warnings,
    )
    .unwrap();
    assert_eq!(
        outcomes[0].added,
        Added::Unchanged {
            identity: "a".into()
        }
    );
    assert!(meeter.refusal().is_none());
    assert_eq!(
        meeter.labels_report(false),
        ["labels differ on 1 snapshot(s), not counted (spec 0500 S1a): + arch (1)"]
    );
    assert_eq!(meeter.labels_report(true)[1], "  a: + arch");
    assert_eq!(
        snapshot_tip(&r, "a"),
        Some(original),
        "the store's copy kept"
    );
    settle(&r, &kit).unwrap();
    assert_eq!(
        store::tip(&r, &store::branch_ref("main/default").unwrap()).unwrap(),
        history,
        "no rewrite for labels alone"
    );
}
