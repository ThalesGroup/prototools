// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `extract`: executables as snapshots of the descriptors they embed (spec
//! 0453 S2, S5).

use std::path::{Component, Path, PathBuf};

use git_seriate::cli::Kit;
use git_seriate::format::{Rejection, Source};
use git_seriate::plugins::{Extracted, Extractor};
use git_seriate::store::{self, Added, Item, NewSnapshot};
use git_seriate::Error;

use crate::Schema;

/// Finds the descriptors in an executable's bytes (spec 0453 S2).
pub struct FdpExtractor<'a> {
    pub schema: &'a Schema,
    pub options: fdp_scan::ScanOptions,
}

impl Extractor for FdpExtractor<'_> {
    fn extract(&self, bytes: &[u8]) -> Extracted {
        let explained = match &self.schema.graph {
            Some((graph, root)) => {
                fdp_scan::scan_explained_with_options(bytes, graph, root, &self.options)
            }
            None => fdp_scan::scan_explained_options(bytes, &self.options),
        };
        let path_of = |start: usize| name_at(bytes, start).map(|n| item_path(&n));
        let items = explained
            .found
            .iter()
            .filter_map(|&(start, end)| {
                Some(Item {
                    path: path_of(start)?,
                    bytes: bytes[start..end].to_vec(),
                })
            })
            .collect();
        let rejected = explained
            .rejected
            .iter()
            .map(|r| Rejection {
                name: r.name.clone(),
                reason: reason_code(r.reason).into(),
            })
            .collect();
        let record = explained
            .order_cuts
            .iter()
            .filter_map(|c| {
                Some((
                    format!("order-cut {}", path_of(c.start)?),
                    format!("{}..{}, out of order at {}", c.start, c.end, c.at),
                ))
            })
            .collect();
        Extracted {
            items,
            rejected,
            record,
        }
    }
}

/// The scanner's refusal reasons, as the block records them.
pub fn reason_code(reason: fdp_scan::Reason) -> &'static str {
    match reason {
        fdp_scan::Reason::ImplausibleName => "implausible-name",
        fdp_scan::Reason::NameOnly => "name-only",
        fdp_scan::Reason::Defect => "defect",
        fdp_scan::Reason::DanglingDependency => "dangling-dependency",
    }
}

/// `a/b.proto` → `a/b.pb`.
fn item_path(name: &str) -> String {
    match name.strip_suffix(".proto") {
        Some(stem) => format!("{stem}.pb"),
        None => name.to_string(),
    }
}

/// The `name` a record found at `start` begins with: field 1, a string.
fn name_at(bytes: &[u8], start: usize) -> Option<String> {
    let rest = bytes.get(start..)?;
    if rest.first() != Some(&0x0a) {
        return None;
    }
    let mut len = 0usize;
    let mut i = 1;
    for shift in (0..35).step_by(7) {
        let b = *rest.get(i)?;
        i += 1;
        len |= usize::from(b & 0x7f) << shift;
        if b < 0x80 {
            break;
        }
    }
    let name = rest.get(i..i.checked_add(len)?)?;
    String::from_utf8(name.to_vec()).ok()
}

/// How to extract.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The identity, for a single file.
    pub id: Option<String>,
    pub build_time: Option<i64>,
    pub order_hint: Option<u64>,
    pub group: Option<String>,
    pub metadata: Vec<(String, String)>,
    pub increasing_field_numbers: bool,
}

/// What extracting one file did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub file: PathBuf,
    pub added: Added,
}

/// Extract each of `files` as a snapshot (spec 0453 S5). Warnings go to
/// `warnings`.
pub fn extract_files(
    repo: &gix::Repository,
    kit: &Kit,
    files: &[PathBuf],
    options: &Options,
    warnings: &mut Vec<String>,
) -> Result<Vec<Outcome>, Error> {
    if options.id.is_some() && files.len() != 1 {
        return Err(Error::Refused(
            "--id names one file's snapshot: give one file".into(),
        ));
    }
    let schema = Schema::of(repo)?;
    let extractor = FdpExtractor {
        schema: &schema,
        options: fdp_scan::ScanOptions {
            increasing_field_numbers: options.increasing_field_numbers,
        },
    };
    let mut times = Vec::new();
    for file in files {
        times.push(modified(file)?);
    }
    if options.build_time.is_none() {
        for (file, &t) in files.iter().zip(&times) {
            if t <= 1 {
                warnings.push(format!(
                    "{}: modification time {t} used as its build time; give --build-time",
                    file.display()
                ));
            }
        }
        if files.len() > 1 && times.iter().all(|&t| t == times[0] && t > 1) {
            warnings.push(format!(
                "every file has the modification time {}, used as its build time; give \
                 --build-time if they were built apart",
                times[0]
            ));
        }
    }
    let mut outcomes = Vec::new();
    for (file, &time) in files.iter().zip(&times) {
        let bytes = std::fs::read(file)?;
        let found = extractor.extract(&bytes);
        let mut rejected = found.rejected;
        let mut items = Vec::new();
        for item in found.items {
            match kit.renderer.render(&item.path, &item.bytes) {
                Ok(text) => items.push(Item {
                    path: item.path,
                    bytes: text,
                }),
                Err(e) => rejected.push(Rejection {
                    name: item.path,
                    reason: format!("render: {e}"),
                }),
            }
        }
        // Refusals alone are kept, as a record-only snapshot.
        let mut record = found.record;
        if items.is_empty() && record.is_empty() && !rejected.is_empty() {
            record.push(("refused".into(), rejected.len().to_string()));
        }
        let identity = match &options.id {
            Some(id) => id.clone(),
            None => identity_of(file),
        };
        let added = store::add(
            repo,
            NewSnapshot {
                source: Source {
                    identity,
                    build_time: options.build_time.unwrap_or(time),
                    order_hint: options.order_hint,
                    group: options.group.clone(),
                    metadata: options.metadata.clone(),
                },
                items,
                rejected,
                record,
            },
        )?;
        outcomes.push(Outcome {
            file: file.clone(),
            added,
        });
    }
    Ok(outcomes)
}

/// A file's path as an identity, made a valid branch name (spec 0453 S5):
/// its components joined by `/`, a root, `.` and `..` dropped, and in each
/// component what git refuses in a ref name replaced by `_` (a leading
/// `.`, a `.lock` ending, `..`, `@{`, control characters, space and
/// `~^:?*[\`).
fn identity_of(file: &Path) -> String {
    file.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(ref_component(&s.to_string_lossy())),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn ref_component(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            c if c.is_control() => '_',
            ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\' => '_',
            c => c,
        })
        .collect();
    while out.contains("..") {
        out = out.replace("..", "._");
    }
    out = out.replace("@{", "@_");
    if out.starts_with('.') {
        out.replace_range(..1, "_");
    }
    if let Some(stem) = out.strip_suffix(".lock") {
        out = format!("{stem}_lock");
    }
    if out.ends_with('.') {
        out.pop();
        out.push('_');
    }
    out
}

fn modified(file: &Path) -> Result<i64, Error> {
    let t = std::fs::metadata(file)?.modified()?;
    Ok(t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_identities() {
        assert_eq!(item_path("a/b.proto"), "a/b.pb");
        assert_eq!(identity_of(Path::new("/usr/bin/x")), "usr/bin/x");
        assert_eq!(identity_of(Path::new("./bin/x")), "bin/x");
        assert_eq!(
            identity_of(Path::new("/tmp/.tmpAB/a b.lock")),
            "tmp/_tmpAB/a_b_lock"
        );
        assert_eq!(identity_of(Path::new("../x/y..z")), "x/y._z");
        let rec = [0x0a, 3, b'a', b'.', b'p'];
        assert_eq!(name_at(&rec, 0).as_deref(), Some("a.p"));
    }
}
