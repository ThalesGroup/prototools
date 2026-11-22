// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! git-seriate-fdp, end to end (spec 0453 test plan).

use std::path::{Path, PathBuf};
use std::process::Command;

use git_seriate::cli::Kit;
use git_seriate::history::{self, Cutoff, Options, Plugins};
use git_seriate::plugins::{Canonicalizer, Composed, Pattern, Renderer};
use git_seriate_fdp::extract::{extract_files, Options as ExtractOptions};
use git_seriate_fdp::{kit, rules, FdpRenderer, Schema};

fn repo(root: &Path) -> (PathBuf, gix::Repository) {
    let path = root.join("repo.git");
    let repo = gix::init_bare(&path).unwrap();
    (path, repo)
}

/// The stock descriptor set's files, as protoc wrote them.
fn stock_files() -> Vec<Vec<u8>> {
    files_of(prototext::EMBEDDED_DESCRIPTOR)
}

/// The `file` entries (field 1) of a FileDescriptorSet.
fn files_of(set: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < set.len() {
        assert_eq!(set[pos], 0x0a, "a FileDescriptorSet's field 1");
        pos += 1;
        let (len, n) = varint(&set[pos..]);
        pos += n;
        out.push(set[pos..pos + len as usize].to_vec());
        pos += len as usize;
    }
    out
}

fn varint(b: &[u8]) -> (u64, usize) {
    let mut v = 0;
    for (i, &x) in b.iter().enumerate() {
        v |= u64::from(x & 0x7f) << (7 * i);
        if x < 0x80 {
            return (v, i + 1);
        }
    }
    panic!("truncated varint")
}

#[test]
fn every_stock_descriptor_renders_and_reads_back() {
    let schema = Schema::stock().unwrap();
    let renderer = FdpRenderer { root: schema.root };
    for fdp in stock_files() {
        let text = renderer.render("x.pb", &fdp).unwrap();
        assert!(text.starts_with(b"#@ prototext:"), "annotated");
        assert_eq!(renderer.parse("x.pb", &text).unwrap(), fdp);
    }
}

#[test]
fn canonical_forms_are_canonical() {
    let fdp = rules::Fdp::new().unwrap();
    for file in stock_files() {
        let once = fdp.canon("x.pb", &file);
        assert_eq!(fdp.canon("x.pb", &once), once);
    }
}

/// protoc's descriptor set of a small file, or `None` without protoc.
fn protoc(dir: &Path, source: &str) -> Option<Vec<u8>> {
    std::fs::write(dir.join("t.proto"), source).unwrap();
    let out = Command::new("protoc")
        .current_dir(dir)
        .args(["--descriptor_set_out=t.pb", "t.proto"])
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(files_of(&std::fs::read(dir.join("t.pb")).unwrap()).remove(0))
}

const SOURCE: &str = r#"syntax = "proto3";
package t;
message M {
  string foo_bar = 1;
  string baz = 2 [json_name = "custom"];
}
"#;

#[test]
fn r1_keeps_only_the_json_names_protoc_would_not_derive() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(fdp) = protoc(tmp.path(), SOURCE) else {
        eprintln!("skipped: no protoc");
        return;
    };
    let canonical = rules::Fdp::new().unwrap().canon("t.pb", &fdp);
    let text = String::from_utf8(
        FdpRenderer {
            root: Schema::stock().unwrap().root,
        }
        .render("t.pb", &canonical)
        .unwrap(),
    )
    .unwrap();
    assert_eq!(text.matches("json_name").count(), 1, "{text}");
    assert!(text.contains("\"custom\""), "{text}");
}

/// A file holding `fdp` between runs of NULs, as a C++ binary embeds it.
fn executable(dir: &Path, name: &str, fdps: &[&[u8]], mtime: Option<u64>) -> PathBuf {
    let mut bytes = vec![0u8; 64];
    for fdp in fdps {
        bytes.extend_from_slice(fdp);
        bytes.extend_from_slice(&[0u8; 64]);
    }
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    if let Some(seconds) = mtime {
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
            .unwrap();
    }
    path
}

/// `fdp`'s top-level fields out of number order, `message_type` (4) moved
/// before `package` (2), with every field's default `json_name` kept: the
/// shape of another writer's descriptor. `name` stays first, as every
/// writer puts it, and as the scanner finds a record by.
fn other_flavor(fdp: &[u8]) -> Vec<u8> {
    // protoc's output carries json_name already; the C++ flavor drops it.
    let mut fields: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut pos = 0;
    while pos < fdp.len() {
        let start = pos;
        let (tag, n) = varint(&fdp[pos..]);
        pos += n;
        match tag & 7 {
            0 => pos += varint(&fdp[pos..]).1,
            2 => {
                let (len, n) = varint(&fdp[pos..]);
                pos += n + len as usize;
            }
            w => panic!("wire type {w}"),
        }
        fields.push((tag >> 3, fdp[start..pos].to_vec()));
    }
    fields.sort_by_key(|(n, _)| match n {
        1 => 0,
        4 => 1,
        n => n + 1,
    });
    fields.into_iter().flat_map(|(_, b)| b).collect()
}

fn history_trees(repo: &Path) -> Vec<String> {
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(repo)
        .args(["log", "--reverse", "--format=%T", "main"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", repo.parent().unwrap())
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn settle(repo: &gix::Repository, kit: &Kit, canonicalizer: &dyn Canonicalizer) {
    std::env::set_var("GIT_COMMITTER_NAME", "Tester");
    std::env::set_var("GIT_COMMITTER_EMAIL", "tester@example.com");
    let pattern = Pattern(kit.item_pattern.clone());
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
                renderer: kit.renderer.as_ref(),
                canonicalizer,
                item_paths: &pattern,
                comparator: kit.comparator.as_ref(),
                legacy: None,
            },
        },
    )
    .unwrap();
}

/// A test rule set: drops `FileDescriptorProto.option_dependency` (15),
/// as a build system's own rule would drop what it adds.
struct DropOptionDependency;

impl Canonicalizer for DropOptionDependency {
    fn rule_sets(&self) -> Vec<(String, u32)> {
        vec![("test".into(), 1)]
    }
    fn canon(&self, _path: &str, item: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut pos = 0;
        while pos < item.len() {
            let start = pos;
            let (tag, n) = varint(&item[pos..]);
            pos += n;
            match tag & 7 {
                0 => pos += varint(&item[pos..]).1,
                2 => {
                    let (len, n) = varint(&item[pos..]);
                    pos += n + len as usize;
                }
                _ => return item.to_vec(),
            }
            if tag >> 3 != 15 {
                out.extend_from_slice(&item[start..pos]);
            }
        }
        out
    }
}

#[test]
fn flavors_of_one_file_are_one_version() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(protoc_fdp) = protoc(tmp.path(), SOURCE) else {
        eprintln!("skipped: no protoc");
        return;
    };
    let fdp = rules::Fdp::new().unwrap();
    let base = fdp.canon("t.pb", &protoc_fdp); // the C++ flavor: no json_name
    let go = other_flavor(&protoc_fdp); // reordered, json_name kept
    let mut python = base.clone(); // a module name added, field 15
    python.extend_from_slice(&[0x7a, 4]);
    python.extend_from_slice(b"t_pb");
    let flavors: [(&str, &[u8]); 3] = [("cpp", &base), ("go", &go), ("py", &python)];

    for order in [[0, 1, 2], [2, 1, 0]] {
        let dir = tmp.path().join(format!("o{}", order[0]));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, r) = repo(&dir);
        let kit = kit(&r).unwrap();
        for (k, &i) in order.iter().enumerate() {
            let (name, bytes) = flavors[i];
            let file = executable(&dir, name, &[bytes], None);
            let mut warnings = Vec::new();
            extract_files(
                &r,
                &kit,
                &[file],
                &ExtractOptions {
                    id: Some(name.into()),
                    build_time: Some(1_700_000_000 + k as i64 * 3_600),
                    ..ExtractOptions::default()
                },
                &mut warnings,
            )
            .unwrap();
        }
        // The rule set fdp collapses C++ and Go; the composition, Python too.
        let composed = Composed(vec![
            Box::new(rules::Fdp::new().unwrap()),
            Box::new(DropOptionDependency),
        ]);
        settle(&r, &kit, &composed);
        let trees = history_trees(&path);
        assert_eq!(trees.len(), 3);
        assert!(
            trees.windows(2).all(|w| w[0] == w[1]),
            "one version: {trees:?}"
        );
    }

    // Without the composition, the Python flavor stays a version of its own.
    let dir = tmp.path().join("plain");
    std::fs::create_dir_all(&dir).unwrap();
    let (path, r) = repo(&dir);
    let kit = kit(&r).unwrap();
    for (k, (name, bytes)) in flavors.iter().enumerate() {
        let file = executable(&dir, name, &[bytes], None);
        extract_files(
            &r,
            &kit,
            &[file],
            &ExtractOptions {
                id: Some(name.to_string()),
                build_time: Some(1_700_000_000 + k as i64 * 3_600),
                ..ExtractOptions::default()
            },
            &mut Vec::new(),
        )
        .unwrap();
    }
    settle(&r, &kit, kit.canonicalizer.as_ref());
    let mut trees = history_trees(&path);
    trees.dedup();
    assert_eq!(trees.len(), 2, "C++ and Go one version, Python another");
}

#[test]
fn the_corpus_records_are_items_and_its_decoys_refusals() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fdp-scan/tests/corpus");
    let tmp = tempfile::tempdir().unwrap();
    let (_, r) = repo(tmp.path());
    let kit = kit(&r).unwrap();
    let mut warnings = Vec::new();
    let outcomes = extract_files(
        &r,
        &kit,
        &[corpus.join("isolated.bin"), corpus.join("decoys.bin")],
        &ExtractOptions {
            build_time: Some(1_700_000_000),
            ..ExtractOptions::default()
        },
        &mut warnings,
    )
    .unwrap();
    let listed = git_seriate::store::list_with(&r, None).unwrap();
    let truth: Vec<(String, usize, usize)> = serde_json_lite(&corpus.join("isolated.truth.json"));
    let isolated = listed
        .iter()
        .find(|l| l.branch.ends_with("isolated.bin"))
        .unwrap();
    let contents = isolated.block.contents.as_ref().unwrap();
    // Every true record is an item; duplicates of one name become `~N`.
    assert!(contents.len() <= truth.len());
    assert!(contents.len() >= 12, "{contents:?}");
    assert!(contents.iter().all(|p| p.ends_with(".pb")));
    let decoys = git_seriate::store::list_with(&r, None)
        .unwrap()
        .into_iter()
        .find(|l| l.branch.ends_with("decoys.bin"))
        .expect("the decoys' refusals kept, as a record-only branch");
    assert!(decoys.block.contents.is_none());
    let reasons: Vec<&str> = decoys
        .block
        .rejected
        .iter()
        .map(|r| r.reason.as_str())
        .collect();
    assert!(reasons.contains(&"implausible-name"), "{reasons:?}");
    assert!(reasons.contains(&"dangling-dependency"), "{reasons:?}");
    assert_eq!(outcomes.len(), 2);
}

/// `[[name, start, end], …]`, without a JSON dependency.
fn serde_json_lite(path: &Path) -> Vec<(String, usize, usize)> {
    let text = std::fs::read_to_string(path).unwrap();
    text.split('[')
        .skip(2)
        .filter_map(|t| {
            let t = t.trim_end_matches([']', ',', ' ', '\n']);
            let mut parts = t.rsplitn(3, ',');
            let end = parts.next()?.trim().parse().ok()?;
            let start = parts.next()?.trim().parse().ok()?;
            let name = parts.next()?.trim().trim_matches('"').to_string();
            Some((name, start, end))
        })
        .collect()
}

#[test]
fn modification_times_that_say_nothing_are_warned_about() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, r) = repo(tmp.path());
    let kit = kit(&r).unwrap();
    let fdps = stock_files();
    let a = executable(tmp.path(), "a", &[&fdps[0]], Some(1));
    let mut warnings = Vec::new();
    extract_files(&r, &kit, &[a], &ExtractOptions::default(), &mut warnings).unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("modification time 1")),
        "{warnings:?}"
    );

    let b = executable(tmp.path(), "b", &[&fdps[0]], Some(1_600_000_000));
    let c = executable(tmp.path(), "c", &[&fdps[0]], Some(1_600_000_000));
    let mut warnings = Vec::new();
    extract_files(&r, &kit, &[b, c], &ExtractOptions::default(), &mut warnings).unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("every file has")),
        "{warnings:?}"
    );
}
