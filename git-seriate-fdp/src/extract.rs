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
    /// Look into zip and gzip members too (spec 0480 S8).
    pub inflate: Option<fdp_scan::InflateLimits>,
}

impl Extractor for FdpExtractor<'_> {
    fn extract(&self, bytes: &[u8]) -> Extracted {
        let mut out = Extracted::default();
        let Some(limits) = &self.inflate else {
            let explained = match &self.schema.graph {
                Some((graph, root)) => {
                    fdp_scan::scan_explained_with_options(bytes, graph, root, &self.options)
                }
                None => fdp_scan::scan_explained_options(bytes, &self.options),
            };
            collect(bytes, &explained, None, &mut out);
            return out;
        };
        let visit = |visit: fdp_scan::Visit<'_>| match visit {
            fdp_scan::Visit::Scanned {
                origin,
                data,
                explained,
            } => {
                let inner = (!origin.layers.is_empty()).then_some(origin);
                collect(data, explained, inner, &mut out);
            }
            fdp_scan::Visit::Skipped { origin, reason } => out.record.push((
                format!("skipped {}", reason.place(origin, None)),
                reason.code(),
            )),
        };
        match &self.schema.graph {
            Some((graph, root)) => {
                fdp_scan::scan_inflating_with(bytes, graph, root, &self.options, limits, visit)
            }
            None => fdp_scan::scan_inflating(bytes, &self.options, limits, visit),
        }
        out
    }
}

/// What the scan of `data` found, added to `out`. `origin` is that of a
/// member (spec 0480 S8): each descriptor found in it records it, and its
/// order cuts are told by origin. `None`, the input: as before.
fn collect(
    data: &[u8],
    explained: &fdp_scan::Explained,
    origin: Option<&fdp_scan::Origin>,
    out: &mut Extracted,
) {
    let path_of = |start: usize| name_at(data, start).map(|n| item_path(&n));
    // Every descriptor's origin (spec 0494 S1), the input's own included:
    // its range, after the members it was found in.
    let top = fdp_scan::Origin::default();
    let within = origin.unwrap_or(&top);
    let records: Vec<(usize, usize, String)> = explained
        .found
        .iter()
        .filter_map(|&(start, end)| Some((start, end, path_of(start)?)))
        .collect();
    // Each item's set (spec 0495 S2), labelled after the member it lies in.
    let chain = within.chain(None);
    let sets = framed_sets(data, &records);
    for ((start, end, path), set) in records.into_iter().zip(sets) {
        out.items.push(Item {
            path,
            bytes: data[start..end].to_vec(),
            record: Vec::new(),
            origin: Some(within.text(None, start, end)),
        });
        out.sets.push(set.map(|first| match chain.as_str() {
            "" => format!("set@{first}"),
            chain => format!("{chain}!set@{first}"),
        }));
    }
    out.rejected
        .extend(explained.rejected.iter().map(|r| Rejection {
            name: r.name.clone(),
            reason: reason_code(r.reason).into(),
        }));
    out.record
        .extend(explained.order_cuts.iter().filter_map(|c| {
            let range = match origin {
                Some(o) => o.text(None, c.start, c.end),
                None => format!("{}..{}", c.start, c.end),
            };
            Some((
                format!("order-cut {}", path_of(c.start)?),
                format!("{range}, out of order at {}", c.at),
            ))
        }));
    // A record its frame ended (spec 0496): where the walk would have read
    // on to, past the frame, into the enclosing message's fields.
    out.record
        .extend(explained.frame_cuts.iter().filter_map(|c| {
            Some((
                format!("frame-cut {}", path_of(c.start)?),
                format!(
                    "{}, read on to {}",
                    within.text(None, c.start, c.end),
                    c.read_to
                ),
            ))
        }));
}

/// The framed sets among a buffer's `records` (spec 0495 S2), in order:
/// for each record, the start of its set's first record, `None` for a
/// record in no set. A set is a chain of two records or more, each framed
/// where the previous one ends, the frame declaring its length (the frames
/// fdp-scan already reads, spec 0496); a path repeating within a chain
/// starts another set, a file appearing once in a set.
fn framed_sets(data: &[u8], records: &[(usize, usize, String)]) -> Vec<Option<usize>> {
    let mut sets = vec![None; records.len()];
    let mut mark = |from: usize, to: usize| {
        if to - from >= 2 {
            for set in &mut sets[from..to] {
                *set = Some(records[from].0);
            }
        }
    };
    let mut first = 0;
    while first < records.len() {
        let mut last = first;
        while last + 1 < records.len() {
            let (_, end, _) = records[last];
            let (start, next_end, _) = records[last + 1];
            if fdp_scan::frame_at(data, end, start) != Some(next_end - start) {
                break;
            }
            last += 1;
        }
        let mut from = first;
        let mut seen = std::collections::HashSet::new();
        for (offset, (_, _, path)) in records[first..=last].iter().enumerate() {
            if !seen.insert(path.as_str()) {
                let k = first + offset;
                mark(from, k);
                from = k;
                seen.clear();
                seen.insert(path.as_str());
            }
        }
        mark(from, last + 1);
        first = last + 1;
    }
    sets
}

/// The scanner's refusal reasons, as the block records them.
pub fn reason_code(reason: fdp_scan::Reason) -> &'static str {
    match reason {
        fdp_scan::Reason::ImplausibleName => "implausible-name",
        fdp_scan::Reason::NameOnly => "name-only",
        fdp_scan::Reason::Defect => "defect",
        fdp_scan::Reason::DanglingDependency => "dangling-dependency",
        fdp_scan::Reason::ImplausibleEdition => "implausible-edition",
        fdp_scan::Reason::DeclaresNothing => "declares-nothing",
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
    /// Look into zip and gzip members (spec 0480 S8), within the default
    /// limits.
    pub inflate: bool,
    /// Keep each input whole: no set made a snapshot of its own (`--split
    /// none`, spec 0495 S2).
    pub no_split: bool,
    /// One snapshot of every file, under this identity (`--as-one`, spec
    /// 0495 S3): the caller's statement that they are one build.
    pub as_one: Option<String>,
}

/// What extracting did: one per snapshot written, an input's rest and its
/// sets each one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub file: PathBuf,
    pub added: Added,
}

/// An input's rendered items, by part (spec 0495 S2): the rest, and each
/// set, with the refusals and record entries that go with each.
#[derive(Default)]
struct Parts {
    rest: Vec<Item>,
    rejected: Vec<Rejection>,
    record: Vec<(String, String)>,
    /// Each set, in the order found.
    sets: Vec<Set>,
}

/// A framed set of an input (spec 0495 S2), a snapshot of its own.
struct Set {
    /// `set@<start>`, after the chain of the member it lies in.
    label: String,
    items: Vec<Item>,
    /// The record entries about its items.
    record: Vec<(String, String)>,
}

impl Parts {
    /// What `found` holds, rendered by `kit`: an item the renderer refuses
    /// is a refusal. A record entry about one item (`<key> <path>`) goes
    /// with that item's part; the others with the rest.
    fn of(kit: &Kit, found: Extracted, split: bool) -> Parts {
        let mut parts = Parts {
            rejected: found.rejected,
            ..Parts::default()
        };
        let labels = found.sets.into_iter().chain(std::iter::repeat(None));
        let mut path_set: std::collections::HashMap<String, usize> = Default::default();
        for (item, label) in found.items.into_iter().zip(labels) {
            let text = match kit.renderer.render(&item.path, &item.bytes) {
                Ok(text) => text,
                Err(e) => {
                    parts.rejected.push(Rejection {
                        name: item.path,
                        reason: format!("render: {e}"),
                    });
                    continue;
                }
            };
            let rendered = Item {
                path: item.path,
                bytes: text,
                record: item.record,
                origin: item.origin,
            };
            match label.filter(|_| split) {
                None => parts.rest.push(rendered),
                Some(label) => {
                    let at = match parts.sets.iter().position(|set| set.label == label) {
                        Some(at) => at,
                        None => {
                            parts.sets.push(Set {
                                label,
                                items: Vec::new(),
                                record: Vec::new(),
                            });
                            parts.sets.len() - 1
                        }
                    };
                    path_set.insert(rendered.path.clone(), at);
                    parts.sets[at].items.push(rendered);
                }
            }
        }
        for (key, value) in found.record {
            let about = key.split_once(' ').and_then(|(_, path)| path_set.get(path));
            match about {
                Some(&at) => parts.sets[at].record.push((key, value)),
                None => parts.record.push((key, value)),
            }
        }
        parts
    }

    /// These parts as one input of a group (spec 0495 S3): each origin,
    /// set label, record key and refusal after the input's path.
    fn within(mut self, input: &str) -> Parts {
        let prefix = |items: &mut Vec<Item>| {
            for item in items {
                item.origin = item.origin.take().map(|o| format!("{input}!{o}"));
            }
        };
        prefix(&mut self.rest);
        for set in &mut self.sets {
            set.label = format!("{input}!{}", set.label);
            prefix(&mut set.items);
            for (key, _) in &mut set.record {
                *key = format!("{input}: {key}");
            }
        }
        for (key, _) in &mut self.record {
            *key = format!("{input}: {key}");
        }
        for r in &mut self.rejected {
            r.name = format!("{input}: {}", r.name);
        }
        self
    }

    fn absorb(&mut self, other: Parts) {
        self.rest.extend(other.rest);
        self.rejected.extend(other.rejected);
        self.record.extend(other.record);
        self.sets.extend(other.sets);
    }

    /// Each part written as a snapshot: the rest under `identity`, as an
    /// input with no set is written today; each set under
    /// `<identity>!<label>`. When there are sets, every part takes the
    /// input as its group, so that what to extract again stays known.
    fn write(
        self,
        sink: &mut store::Sink,
        identity: &str,
        build_time: i64,
        options: &Options,
        file: &Path,
    ) -> Result<Vec<Outcome>, Error> {
        let group = if self.sets.is_empty() {
            options.group.clone()
        } else {
            options.group.clone().or_else(|| Some(identity.to_string()))
        };
        let source = |identity: String| Source {
            identity,
            build_time,
            order_hint: options.order_hint,
            group: group.clone(),
            metadata: options.metadata.clone(),
        };
        // Refusals alone are kept, as a record-only snapshot.
        let mut record = self.record;
        if self.rest.is_empty() && record.is_empty() && !self.rejected.is_empty() {
            record.push(("refused".into(), self.rejected.len().to_string()));
        }
        let mut outcomes = vec![Outcome {
            file: file.to_path_buf(),
            added: store::add_to(
                sink,
                NewSnapshot {
                    source: source(identity.to_string()),
                    items: self.rest,
                    rejected: self.rejected,
                    record,
                },
            )?,
        }];
        for Set {
            label,
            items,
            record,
        } in self.sets
        {
            let label: Vec<String> = label.split('/').map(ref_component).collect();
            outcomes.push(Outcome {
                file: file.to_path_buf(),
                added: store::add_to(
                    sink,
                    NewSnapshot {
                        source: source(format!("{identity}!{}", label.join("/"))),
                        items,
                        rejected: Vec::new(),
                        record,
                    },
                )?,
            });
        }
        Ok(outcomes)
    }
}

/// A file's path as it prefixes origins in a group: `%`, `!` and `;`
/// escaped, as fdp-scan escapes a member's name (spec 0480 S6).
fn input_label(file: &Path) -> String {
    file.display()
        .to_string()
        .replace('%', "%25")
        .replace('!', "%21")
        .replace(';', "%3B")
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
    extract_files_into(
        repo,
        &mut store::Sink::Repo(repo),
        kit,
        files,
        options,
        warnings,
    )
}

/// [`extract_files`], the snapshots written into `sink`: `repo`'s, or an
/// archive (spec 0466). `repo` gives the settings (`seriate.fdpSchema`).
pub fn extract_files_into(
    repo: &gix::Repository,
    sink: &mut store::Sink,
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
    if options.id.is_some() && options.as_one.is_some() {
        return Err(Error::Refused(
            "--id and --as-one both name the snapshot: give one".into(),
        ));
    }
    let schema = Schema::of(repo)?;
    let extractor = FdpExtractor {
        schema: &schema,
        options: fdp_scan::ScanOptions {
            increasing_field_numbers: options.increasing_field_numbers,
        },
        inflate: options.inflate.then(fdp_scan::InflateLimits::default),
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
    // `--as-one`: every input's parts gathered, written once (spec 0495 S3).
    let mut group = options.as_one.as_ref().map(|_| Parts::default());
    for (file, &time) in files.iter().zip(&times) {
        let bytes = std::fs::read(file)?;
        let parts = Parts::of(kit, extractor.extract(&bytes), !options.no_split);
        match &mut group {
            Some(group) => group.absorb(parts.within(&input_label(file))),
            None => {
                let identity = match &options.id {
                    Some(id) => id.clone(),
                    None => identity_of(file),
                };
                let build_time = options.build_time.unwrap_or(time);
                outcomes.extend(parts.write(sink, &identity, build_time, options, file)?);
            }
        }
    }
    if let (Some(group), Some(identity)) = (group, &options.as_one) {
        // One build: its time the latest of its inputs', unless given.
        let build_time = options
            .build_time
            .or_else(|| times.iter().copied().max())
            .unwrap_or(0);
        outcomes.extend(group.write(sink, identity, build_time, options, Path::new(identity))?);
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
