// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The graph dump, `--dump-graph FILE` (spec 0428 S1–S6).
//!
//! What phases 1 to 6 leave, written as text, after which the run stops:
//! the instrument that compared protoback with reproto.
//!
//! Ported from `test_dump_graph.py`. A set without one of protoc's files is
//! protoc's own bytes for the others (`common::set_without`), where the
//! original rebuilt it with Python's protobuf. The tests that varied
//! `PYTHONHASHSEED` still set it, for each run in its own process: it was
//! the original's lever on a set's order, and protoback must not depend on
//! a process's state either.

use crate::common::{protoback, protoback_command, set_without, tmp, tools, write};
use std::path::{Path, PathBuf};

const OPTS: &str = r#"syntax = "proto2";

package dg;

import "google/protobuf/descriptor.proto";

extend google.protobuf.MessageOptions {
  optional string tag = 50001;
}
"#;

const MAIN: &str = r#"syntax = "proto2";

package dg;

import "opts.proto";
import "gone.proto";

message Holder {
  option (dg.tag) = "t";
  optional Inner inner = 1;
  optional gone.G lost = 2;

  message Inner {
    optional Kind kind = 1;
  }
}

enum Kind {
  KIND_A = 0;
}

service Svc {
  rpc Get(Holder) returns (Holder);
}
"#;

const GONE: &str = "syntax = \"proto2\";\npackage gone;\nmessage G {}\n";

const SECTIONS: &[&str] = &[
    "[files]",
    "[pool]",
    "[context]",
    "[nodes]",
    "[seeds]",
    "[warnings]",
];

fn protoc(args: &[String]) {
    let o = std::process::Command::new("protoc")
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

/// `main` compiled with its imports, then `drop` taken out of the set.
fn compiled_without(src: &Path, main: &str, drop: &[&str]) -> PathBuf {
    let full = src.join("full.pb");
    protoc(&[
        "--include_imports".into(),
        format!("--descriptor_set_out={}", full.display()),
        format!("-I{}", src.display()),
        src.join(main).display().to_string(),
    ]);
    let pb = src.join("set.pb");
    std::fs::write(&pb, set_without(&std::fs::read(&full).unwrap(), drop)).unwrap();
    pb
}

/// main.proto and opts.proto, without gone.proto: a missing import.
fn set(src: &Path) -> PathBuf {
    for (name, body) in [
        ("opts.proto", OPTS),
        ("main.proto", MAIN),
        ("gone.proto", GONE),
    ] {
        write(&src.join(name), body);
    }
    compiled_without(src, "main.proto", &["gone.proto"])
}

fn dump(dir: &Path, pb: &Path, seed: &str, args: &[&str]) -> (String, PathBuf) {
    let out = dir.join(format!("out-{seed}"));
    let file = dir.join(format!("graph-{seed}.txt"));
    let mut all: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("--proto-out={}", out.display()),
        format!("--dump-graph={}", file.display()),
    ];
    all.extend(args.iter().map(|a| a.to_string()));
    all.push(pb.display().to_string());
    let o = protoback_command(&all)
        .env("PYTHONHASHSEED", seed)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    (std::fs::read_to_string(&file).unwrap(), out)
}

/// The text between `[from]\n` and `[to]`.
fn section<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    text.split_once(&format!("{from}\n"))
        .unwrap()
        .1
        .split_once(to)
        .unwrap()
        .0
}

fn has_proto(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.filter_map(Result::ok).any(|e| {
        let p = e.path();
        if p.is_dir() {
            has_proto(&p)
        } else {
            p.extension().is_some_and(|x| x == "proto")
        }
    })
}

#[test]
fn test_every_section_in_order_and_nothing_rendered() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_every_section_in_order_and_nothing_rendered");
    let pb = set(&dir.join("src"));
    let (text, out) = dump(&dir, &pb, "1", &[]);
    let lines: Vec<&str> = text.lines().collect();
    let headers: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with('['))
        .collect();
    assert_eq!(headers, SECTIONS);
    // The run stops after phase 6: nothing rendered.
    assert!(!has_proto(&out));
    // The missing import: a ref in [files], stripped from main.proto, warned.
    assert!(
        lines.iter().any(|l| l.starts_with("gone.proto | ref")),
        "{text}"
    );
    let main = lines
        .iter()
        .find(|l| l.starts_with("main.proto | file"))
        .unwrap();
    assert!(main.contains("stripped imports [gone.proto]"), "{main}");
    assert!(text.contains("w1 | gone.proto"), "{text}");
    // Nodes of each kind, with their flags and links.
    assert!(text.contains("desc:.dg.Holder.Inner"));
    assert!(text.contains("serv:.dg.Svc"));
}

#[test]
fn test_two_hash_seeds_give_the_same_dump() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_two_hash_seeds_give_the_same_dump");
    let pb = set(&dir.join("src"));
    assert_eq!(dump(&dir, &pb, "1", &[]).0, dump(&dir, &pb, "2", &[]).0);
}

#[test]
fn test_seeds_and_pruning_show() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_seeds_and_pruning_show");
    let pb = set(&dir.join("src"));
    let (text, _) = dump(
        &dir,
        &pb,
        "1",
        &["--seed", "desc:.dg.Holder.Inner", "--prune", "serv:.dg.Svc"],
    );
    let seeds: Vec<&str> = section(&text, "[seeds]", "[warnings]").lines().collect();
    assert_eq!(seeds, ["desc:.dg.Holder.Inner"]);
    // The pattern as kept, and the node it pruned.
    assert!(
        section(&text, "[context]", "[nodes]").contains("pruned fqdns | [serv:dg/Svc]"),
        "{text}"
    );
    let svc = text
        .lines()
        .find(|l| l.starts_with("ReServiceDescriptorProto | serv:.dg.Svc |"))
        .unwrap();
    assert!(svc.split(" | ").nth(3).unwrap().starts_with('P'), "{svc}");
}

/// Files under a directory input are found sorted by path, whatever order
/// the filesystem lists them in; so the first of two files holding the same
/// descriptor, which W7 keeps, is the first by path.
#[test]
fn test_a_directory_is_read_in_sorted_order() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a_directory_is_read_in_sorted_order");
    let (src, root) = (dir.join("src"), dir.join("root"));
    std::fs::create_dir_all(root.join("sub")).unwrap();
    // Created out of order: a directory's listing order is the
    // filesystem's, insertion order on a small ext4 or tmpfs directory.
    for name in ["zeta", "alpha", "sub/mu", "beta", "sub/kappa"] {
        let stem = name.replace('/', "_");
        let proto = src.join(format!("{stem}.proto"));
        write(
            &proto,
            &format!("syntax = \"proto2\";\npackage {stem};\nmessage M {{}}\n"),
        );
        protoc(&[
            format!("--descriptor_set_out={}.pb", root.join(name).display()),
            format!("-I{}", src.display()),
            proto.display().to_string(),
        ]);
    }
    // Two files holding the same descriptor, the later by path created
    // first: W7 keeps the first by path, b_second.pb, not y_first.pb.
    let omega = src.join("omega.proto");
    write(&omega, "syntax = \"proto2\";\npackage omega;\n");
    protoc(&[
        format!("--descriptor_set_out={}", root.join("y_first.pb").display()),
        format!("-I{}", src.display()),
        omega.display().to_string(),
    ]);
    std::fs::copy(root.join("y_first.pb"), root.join("b_second.pb")).unwrap();
    let file = dir.join("graph.txt");
    let r = protoback([
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", root.display()),
        format!("--dump-graph={}", file.display()),
        ".".into(),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let text = std::fs::read_to_string(&file).unwrap();
    let files = text.split_once("[pool]").unwrap().0;
    // Relative to their root, without their extension.
    let paths: Vec<PathBuf> = files
        .lines()
        .filter(|l| l.contains(" | file | "))
        .map(|l| l.split(" | ").nth(2).unwrap())
        .filter(|p| !p.starts_with("internal/"))
        .map(PathBuf::from)
        .collect();
    let mut common = paths[0].parent().unwrap().to_path_buf();
    while !paths.iter().all(|p| p.starts_with(&common)) {
        common = common.parent().unwrap().to_path_buf();
    }
    let rel: Vec<String> = paths
        .iter()
        .map(|p| {
            p.strip_prefix(&common)
                .unwrap()
                .with_extension("")
                .display()
                .to_string()
        })
        .collect();
    assert_eq!(
        rel,
        ["alpha", "b_second", "beta", "sub/kappa", "sub/mu", "zeta"]
    );
    let w7 = text.lines().any(|l| {
        let f: Vec<&str> = l.split(" | ").collect();
        f.len() == 4
            && f[0] == "w7"
            && f[1] == "omega.proto"
            && Path::new(f[2]).file_stem().is_some_and(|s| s == "y_first")
            && Path::new(f[3]).file_stem().is_some_and(|s| s == "b_second")
    });
    assert!(w7, "{text}");
}

/// Phase 2 adds files to the pool rank by rank, imports first; within a
/// rank, in the order phase 1 found them.
#[test]
fn test_files_of_one_rank_enter_the_pool_in_topology_order() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_files_of_one_rank_enter_the_pool_in_topology_order");
    let src = dir.join("src");
    // Leaves imported by a root, in an order neither sorted nor reversed:
    // one rank of eight files, then the root.
    let leaves = ["m", "c", "x", "a", "q", "f", "z", "k"];
    for name in leaves {
        write(
            &src.join(format!("{name}.proto")),
            &format!("syntax = \"proto2\";\npackage {name};\nmessage M {{}}\n"),
        );
    }
    let imports: String = leaves
        .iter()
        .map(|n| format!("import \"{n}.proto\";\n"))
        .collect();
    write(
        &src.join("root.proto"),
        &format!("syntax = \"proto2\";\npackage root;\n{imports}message R {{}}\n"),
    );
    let pb = compiled_without(&src, "root.proto", &[]);
    let (text, _) = dump(&dir, &pb, "1", &[]);
    let files: Vec<&str> = text
        .split_once("[pool]")
        .unwrap()
        .0
        .lines()
        .filter(|l| l.contains(" | file | "))
        .map(|l| l.split(" | ").next().unwrap())
        .collect();
    let pool: Vec<&str> = section(&text, "[pool]", "[context]")
        .lines()
        .map(|l| l.split(" | ").next().unwrap())
        .collect();
    // descriptor.proto and the eight leaves import nothing: one rank, in
    // topology order; then root.proto.
    let mut expected: Vec<&str> = files
        .iter()
        .copied()
        .filter(|f| *f != "root.proto")
        .collect();
    expected.push("root.proto");
    assert_eq!(pool, expected);
}

/// A message named by a file whose import is missing is a stub node, with
/// no parent: the dump writes `-`, and does not fail on it.
#[test]
fn test_a_stub_node_is_dumped_without_a_parent() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a_stub_node_is_dumped_without_a_parent");
    let src = dir.join("src");
    write(
        &src.join("config.proto"),
        "syntax = \"proto2\";\npackage ext_test;\nmessage Config { extensions 100 to 200; }\n",
    );
    write(
        &src.join("ext.proto"),
        "syntax = \"proto2\";\npackage ext_test;\nimport \"config.proto\";\nextend Config { optional int32 x = 101; }\n",
    );
    let pb = compiled_without(&src, "ext.proto", &["config.proto"]);
    let (text, _) = dump(&dir, &pb, "1", &[]);
    let stub = text
        .lines()
        .find(|l| l.starts_with("ReDescriptorProto | desc:.ext_test.Config |"))
        .unwrap();
    assert_eq!(
        stub.split(" | ").collect::<Vec<_>>()[2..5],
        ["stub", "---", "-"]
    );
}

/// Two --prune patterns, a message and the field it contains: both pruned,
/// the field not a second time by pruning the message, which reaches it.
#[test]
fn test_a_node_pruned_with_one_of_its_children() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a_node_pruned_with_one_of_its_children");
    let pb = set(&dir.join("src"));
    let (text, _) = dump(
        &dir,
        &pb,
        "1",
        &[
            "--prune",
            "desc:dg.Holder",
            "--prune",
            "fdsc:dg.Holder.inner",
        ],
    );
    let nodes = section(&text, "[nodes]", "[seeds]");
    for fqdn in ["desc:.dg.Holder", "fdsc:.dg.Holder.inner"] {
        let line = nodes
            .lines()
            .filter(|l| !l.starts_with(' '))
            .find(|l| l.split(" | ").nth(1) == Some(fqdn))
            .unwrap_or_else(|| panic!("{fqdn}"));
        assert!(line.split(" | ").nth(3).unwrap().starts_with('P'), "{fqdn}");
    }
}

/// A path seed stands for the files it matched; phase 5 tries them in the
/// order of their names. Each is pruned here, so each is warned about, in
/// that order, whatever the process.
#[test]
fn test_path_seeds_are_tried_in_sorted_order() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_path_seeds_are_tried_in_sorted_order");
    let (src, root) = (dir.join("src"), dir.join("root"));
    std::fs::create_dir_all(&root).unwrap();
    let names = ["m", "c", "x", "a", "q", "f"];
    for name in names {
        let proto = src.join(format!("{name}.proto"));
        write(
            &proto,
            &format!("syntax = \"proto2\";\npackage {name};\nmessage M {{}}\n"),
        );
        protoc(&[
            format!("--descriptor_set_out={}.pb", root.join(name).display()),
            format!("-I{}", src.display()),
            proto.display().to_string(),
        ]);
    }
    let mut sorted = names;
    sorted.sort();
    let expected: Vec<String> = sorted
        .iter()
        .map(|n| format!("warning | Warning [W12]: Seed not found: file:{n}.proto"))
        .collect();
    for seed in ["1", "2", "3"] {
        let file = dir.join(format!("graph-{seed}.txt"));
        let mut args: Vec<String> = vec![
            "--use-variant".into(),
            "descriptor".into(),
            format!("-I{}", root.display()),
            format!("--dump-graph={}", file.display()),
            "--seed".into(),
            "*".into(),
        ];
        for n in names {
            args.extend(["--prune".into(), format!("file:{n}.proto")]);
        }
        let o = protoback_command(&args)
            .env("PYTHONHASHSEED", seed)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let text = std::fs::read_to_string(&file).unwrap();
        let warnings: Vec<&str> = text
            .split_once("[warnings]\n")
            .unwrap()
            .1
            .lines()
            .filter(|l| l.contains("Seed not found"))
            .collect();
        assert_eq!(warnings, expected, "seed {seed}");
    }
}
