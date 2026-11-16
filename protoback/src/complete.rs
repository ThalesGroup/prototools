// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Shell completion (spec 0433 S7), as reproto's completers answer
//! (`cli.py`; spec 0422): the descriptor files relative to the `-I` roots
//! already on the line, or members of an `-I` blob; `-O`'s directories,
//! `--proto-variant`'s `.yaml` files, the plugins' `.py` files, relative
//! to the current directory.
//!
//! A directory is a candidate ending in `/`, whether or not the shell can
//! see it from where it runs (an `-I` root elsewhere): bash
//! (`prototools-complete`), zsh and fish (clap_complete's scripts) then
//! add no space, so Tab descends into it.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use clap_complete::engine::CompletionCandidate;
use prototools_complete::args::flag_values;

/// `_complete_paths`: under each of `bases`, the entries of the
/// incomplete's directory whose name starts with its last component (all
/// of them when it ends in `/`): directories with a `/`, files unless
/// `dirs_only`, of one of `suffixes` if given; all of them sorted.
pub fn paths_under(
    incomplete: &str,
    bases: &[PathBuf],
    dirs_only: bool,
    suffixes: Option<&[&str]>,
) -> Vec<String> {
    let (prefix, stem) = if incomplete.ends_with('/') || incomplete.is_empty() {
        (incomplete.trim_end_matches('/').to_string(), String::new())
    } else {
        match incomplete.rsplit_once('/') {
            Some((dir, name)) => (dir.to_string(), name.to_string()),
            None => (String::new(), incomplete.to_string()),
        }
    };
    let mut out = Vec::new();
    for base in bases {
        let search = if prefix.is_empty() {
            base.clone()
        } else {
            base.join(&prefix)
        };
        let Ok(entries) = std::fs::read_dir(&search) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&stem) {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            // `Path.is_dir`: a link to a directory is one.
            if e.path().is_dir() {
                out.push(format!("{rel}/"));
            } else if !dirs_only && suffixes.is_none_or(|s| s.iter().any(|x| name.ends_with(x))) {
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

/// `complete_pb_files`: the descriptor files under the `-I` directories (the
/// current one when none is given), `.pb` and `.textpb`, then the members
/// of each `-I` blob whose path starts with the incomplete.
pub fn descriptor_files(incomplete: &str, roots: &[PathBuf]) -> Vec<String> {
    let roots: Vec<PathBuf> = if roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        roots.to_vec()
    };
    let (blobs, dirs): (Vec<PathBuf>, Vec<PathBuf>) = roots.into_iter().partition(|r| r.is_file());
    let mut out = paths_under(incomplete, &dirs, false, Some(&[".pb", ".textpb"]));
    let mut cache = HashMap::new();
    for blob in &blobs {
        let mut members: Vec<PathBuf> = crate::load::blob_members(&mut cache, blob)
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();
        members.sort();
        out.extend(
            members
                .iter()
                .map(|m| m.to_string_lossy().into_owned())
                .filter(|m| m.starts_with(incomplete)),
        );
    }
    out
}

fn candidates(values: Vec<String>) -> Vec<CompletionCandidate> {
    values.into_iter().map(CompletionCandidate::new).collect()
}

/// DESCRIPTOR_FILES, relative to the `-I` roots already on the line.
pub fn complete_descriptor_files(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let roots: Vec<PathBuf> = flag_values(Some("-I"), "--desc-root")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    candidates(descriptor_files(&incomplete.to_string_lossy(), &roots))
}

fn here() -> Vec<PathBuf> {
    vec![Path::new(".").to_path_buf()]
}

/// `-O`: directories, relative to the current one.
pub fn complete_dir_path(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    candidates(paths_under(
        &incomplete.to_string_lossy(),
        &here(),
        true,
        None,
    ))
}

/// `--proto-variant`: `.yaml` files and directories.
pub fn complete_yaml_path(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    candidates(paths_under(
        &incomplete.to_string_lossy(),
        &here(),
        false,
        Some(&[".yaml"]),
    ))
}

/// The plugins: `.py` files and directories.
pub fn complete_py_path(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    candidates(paths_under(
        &incomplete.to_string_lossy(),
        &here(),
        false,
        Some(&[".py"]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree: two roots, files and directories in each, a blob.
    /// `test` names the directory: the tests run in parallel.
    fn tree(test: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("protoback-complete-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["one/sub", "one/subtle", "two/sub", "here/vdir"] {
            std::fs::create_dir_all(d.join(p)).unwrap();
        }
        for f in [
            "one/a.pb",
            "one/b.textpb",
            "one/notes.txt",
            "one/sub/c.pb",
            "two/a2.pb",
            "here/v.yaml",
            "here/x.txt",
            "here/p.py",
        ] {
            std::fs::write(d.join(f), b"").unwrap();
        }
        d
    }

    #[test]
    fn descriptor_files_are_completed_under_each_root() {
        let d = tree("descriptor_files_are_completed_under_each_root");
        let roots = [d.join("one"), d.join("two")];
        // Both roots, sorted together; .pb and .textpb only; directories
        // with a slash.
        assert_eq!(
            descriptor_files("", &roots),
            ["a.pb", "a2.pb", "b.textpb", "sub/", "sub/", "subtle/"]
        );
        assert_eq!(descriptor_files("su", &roots), ["sub/", "sub/", "subtle/"]);
        // Into a directory.
        assert_eq!(descriptor_files("sub/", &roots[..1]), ["sub/c.pb"]);
        assert_eq!(descriptor_files("sub/c", &roots[..1]), ["sub/c.pb"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_blob_root_completes_to_its_members() {
        let d = tree("a_blob_root_completes_to_its_members");
        let blob = d.join("blob.bin");
        // Real-looking files among other bytes, as a blob in a binary.
        let member = |name: &str| {
            prototext_upb::text_format::parse_descriptor(
                &format!(
                    r#"name: "{name}" package: "p" message_type {{ name: "M"
                       field {{ name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 }} }}"#
                ),
                false,
            )
            .unwrap()
        };
        let mut bytes: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
        for name in ["extra/second.proto", "first.proto"] {
            bytes.extend(member(name));
            bytes.extend([0x00, 0xff].repeat(64));
        }
        std::fs::write(&blob, bytes).unwrap();
        assert_eq!(
            descriptor_files("", std::slice::from_ref(&blob)),
            ["extra/second.pb", "first.pb"]
        );
        assert_eq!(
            descriptor_files("ex", std::slice::from_ref(&blob)),
            ["extra/second.pb"]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn options_complete_relative_paths_by_suffix() {
        let d = tree("options_complete_relative_paths_by_suffix");
        let here = [d.join("here")];
        assert_eq!(
            paths_under("", &here, false, Some(&[".yaml"])),
            ["v.yaml", "vdir/"]
        );
        assert_eq!(
            paths_under("", &here, false, Some(&[".py"])),
            ["p.py", "vdir/"]
        );
        assert_eq!(paths_under("v", &here, true, None), ["vdir/"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
