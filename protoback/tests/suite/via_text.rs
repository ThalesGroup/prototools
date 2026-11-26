// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoback, every binary descriptor set it is given turned into text
//! first (spec 0427): the text path checked against the binary one.
//!
//! Ported from `via_text.py`, the suite's other command. With
//! `PROTOBACK_SUITE_VIA_TEXT` set, `common::protoback` runs every test's
//! command so, and the test must pass as it does on the binary sets. Each
//! set is read as protoc's text form, `protoc --decode` with descriptor.proto
//! alone. Explicit binary arguments become `.textpb` files in a directory of
//! their own; each `-I` directory is mirrored, its binary sets as `.textpb`;
//! a root-relative argument, a `--filter-config` and a `path:` pattern of
//! `-s`/`-p` follow the renaming. A blob root, a file protoc cannot decode,
//! and everything else are passed as they are.
//!
//! A directory for a source's text form is named after the source and the
//! running test, where the original named it after the source and its
//! process (one pytest worker, running tests one at a time): two runs of
//! one test on the same inputs see the same paths, which the graph dump
//! writes (spec 0428 S2), and tests running at once, threads of one
//! process here, do not share one.

use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The variable that turns the mode on.
pub const VIA_TEXT: &str = "PROTOBACK_SUITE_VIA_TEXT";

/// Whether the suite runs via text.
pub fn on() -> bool {
    std::env::var_os(VIA_TEXT).is_some()
}

const BINARY: &[&str] = &["pb", "binpb", "pbset", "protoset", "desc"];

fn binary(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| BINARY.contains(&e))
}

/// A fresh directory for `source`'s text form (`_fresh`).
fn fresh(prefix: &str, source: &Path) -> PathBuf {
    let test = std::thread::current().name().unwrap_or("main").to_string();
    let source = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let key = format!("{test}:{}", source.display());
    let hash = format!("{:x}", Sha256::digest(key.as_bytes()));
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("via-text")
        .join(format!("{prefix}-{}", &hash[..16]));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// `src`'s text form into `dst`, when protoc can decode it.
fn to_text(src: &Path, dst: &Path) -> bool {
    let o = Command::new("protoc")
        .args([
            "--decode=google.protobuf.FileDescriptorSet",
            "google/protobuf/descriptor.proto",
        ])
        .stdin(Stdio::from(std::fs::File::open(src).unwrap()))
        .output()
        .unwrap_or_else(|e| panic!("{VIA_TEXT} needs protoc: {e}"));
    if !o.status.success() || o.stdout.iter().all(u8::is_ascii_whitespace) {
        return false;
    }
    std::fs::write(dst, o.stdout).unwrap();
    true
}

#[derive(Default)]
struct Converter {
    mirrors: Vec<PathBuf>,
}

impl Converter {
    fn file(&mut self, path: &Path) -> String {
        let d = fresh("f", path);
        std::fs::create_dir_all(&d).unwrap();
        let dst = d.join(format!(
            "{}.textpb",
            path.file_stem().unwrap().to_string_lossy()
        ));
        if to_text(path, &dst) {
            dst.display().to_string()
        } else {
            path.display().to_string()
        }
    }

    fn directory(&mut self, path: &Path) -> String {
        let root = fresh("d", path);
        std::fs::create_dir_all(&root).unwrap();
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.filter_map(Result::ok) {
                let f = e.path();
                if f.is_dir() {
                    stack.push(f);
                    continue;
                }
                let out = root.join(f.strip_prefix(path).unwrap());
                std::fs::create_dir_all(out.parent().unwrap()).unwrap();
                if binary(&f) && to_text(&f, &out.with_extension("textpb")) {
                    continue;
                }
                std::fs::copy(&f, &out).unwrap();
            }
        }
        self.mirrors.push(root.clone());
        root.display().to_string()
    }

    /// Whether a root-relative path has a text form in a mirror.
    fn mirrored(&self, rel: &str) -> bool {
        self.mirrors
            .iter()
            .any(|m| m.join(rel).with_extension("textpb").is_file())
    }

    fn filter_config(&mut self, path: &Path) -> String {
        if !path.is_file() {
            return path.display().to_string();
        }
        let d = fresh("filter", path);
        std::fs::create_dir_all(&d).unwrap();
        let copy = d.join(path.file_name().unwrap());
        std::fs::write(
            &copy,
            std::fs::read_to_string(path)
                .unwrap()
                .replace(".pb", ".textpb"),
        )
        .unwrap();
        copy.display().to_string()
    }
}

enum Arg {
    Ready(String),
    /// Relative to the roots: renamed once every mirror is made.
    Relative(String),
}

/// `argv` with its binary sets as text (`_arguments`).
pub fn arguments(argv: Vec<OsString>) -> Vec<OsString> {
    let mut c = Converter::default();
    let mut args = Vec::new();
    let mut it = argv.into_iter().map(|a| a.to_string_lossy().into_owned());
    while let Some(a) = it.next() {
        let is_dir = |p: &str| Path::new(p).is_dir();
        if a == "-I" || a == "--desc-root" {
            let v = it.next().unwrap();
            args.push(Arg::Ready(a));
            args.push(Arg::Ready(c.directory(Path::new(&v))));
        } else if let Some(d) = a.strip_prefix("-I").filter(|d| is_dir(d)) {
            args.push(Arg::Ready(format!("-I{}", c.directory(Path::new(d)))));
        } else if let Some(d) = a.strip_prefix("--desc-root=").filter(|d| is_dir(d)) {
            args.push(Arg::Ready(format!(
                "--desc-root={}",
                c.directory(Path::new(d))
            )));
        } else if a == "--filter-config" {
            let v = it.next().unwrap();
            args.push(Arg::Ready(a));
            args.push(Arg::Ready(c.filter_config(Path::new(&v))));
        } else if ["-s", "--seed", "-p", "--prune"].contains(&a.as_str()) {
            // A path pattern names a physical file: renamed as the files.
            let v = it.next().unwrap();
            args.push(Arg::Ready(a));
            args.push(Arg::Ready(if v.starts_with("path:") {
                v.replace(".pb", ".textpb")
            } else {
                v
            }));
        } else if binary(Path::new(&a)) && Path::new(&a).is_file() {
            args.push(Arg::Ready(c.file(Path::new(&a))));
        } else if binary(Path::new(&a)) && !a.starts_with('-') {
            args.push(Arg::Relative(a));
        } else {
            args.push(Arg::Ready(a));
        }
    }
    args.into_iter()
        .map(|a| match a {
            Arg::Ready(s) => s.into(),
            Arg::Relative(s) if c.mirrored(&s) => {
                Path::new(&s).with_extension("textpb").into_os_string()
            }
            Arg::Relative(s) => s.into(),
        })
        .collect()
}
