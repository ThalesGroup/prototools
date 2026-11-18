// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 1's loading: the input's files, found under the roots and split
//! into one `QualFile` per `FileDescriptorProto` (reproto's `load.py` and
//! `split_fdps.py`).

use crate::hash::HashMap;
use std::path::{Path, PathBuf};

use prototext_upb::descriptor_access::{FileDescriptorProto, FileDescriptorSet};
use prototext_upb::{DynamicMessage, Pool};

use crate::events::Events;
use crate::pathmatch::{posix_str, PathPatterns};

pub const BINARY_EXTENSIONS: &[&str] = &[".pb", ".binpb", ".pbset", ".protoset", ".desc"];
pub const TEXT_EXTENSIONS: &[&str] = &[".textpb", ".pbtxt", ".prototxt", ".ascii_proto"];
pub const PROTO_EXTENSION: &str = ".proto";

/// A file's contents: bytes, or text a file with a text extension held.
#[derive(Debug, Clone)]
pub enum Contents {
    Binary(Vec<u8>),
    Text(String),
}

/// One `FileDescriptorProto` of the input, where it came from.
#[derive(Debug, Clone)]
pub struct QualFile {
    pub root: PathBuf,
    pub rel_path: PathBuf,
    pub name: String,
    /// The serialized file: empty for a text not read yet.
    pub contents: Vec<u8>,
    /// A text file keeps its text until the input is read (spec 0427 S3).
    pub text: Option<String>,
    pub from_text: bool,
    pub imports: Vec<String>,
}

/// `Path.suffix`: the last `.xxx` of the name, if the name is not only it.
pub fn suffix(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[i..].to_string(),
        _ => String::new(),
    }
}

fn with_suffix(path: &Path, ext: &str) -> PathBuf {
    let s = suffix(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = &name[..name.len() - s.len()];
    path.with_file_name(format!("{stem}{ext}"))
}

/// `str(root / rel)`, as Python joins paths: an absolute `rel` replaces
/// `root`, and `.` disappears.
pub fn py_join(root: &Path, rel: &Path) -> String {
    let rel_s = rel.to_string_lossy();
    if rel_s.starts_with('/') {
        return posix_str(&rel_s);
    }
    posix_str(&format!("{}/{}", root.to_string_lossy(), rel_s))
}

fn decode_fdp(bytes: &[u8]) -> Option<DynamicMessage> {
    let def = Pool::vendored().get_message_by_name("google.protobuf.FileDescriptorProto")?;
    DynamicMessage::decode(&def, bytes).ok()
}

/// A serialized file's name and imports, as Python's descriptor reads them.
pub fn name_and_imports(bytes: &[u8]) -> Option<(String, Vec<String>)> {
    let m = decode_fdp(bytes)?;
    let f = FileDescriptorProto(m.as_ref());
    let name = String::from_utf8_lossy(f.name()).into_owned();
    let imports = f
        .dependency()
        .iter()
        .map(|d| String::from_utf8_lossy(d).into_owned())
        .collect();
    Some((name, imports))
}

// ── split_fdps ──────────────────────────────────────────────────────────────

/// A varint as Python's `_scan_wire_tags` reads it: an integer of any
/// size, read to the end of the data when it is cut short. Not
/// `prototext_upb::wire::read_varint`, which refuses both: this one must
/// accept what Python's scan accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PyInt {
    /// Its low 128 bits, exact.
    low: u128,
    /// Whether a bit above them is set: then it equals no field number or
    /// length a message can hold.
    big: bool,
}

impl PyInt {
    fn read(b: &[u8], pos: &mut usize) -> PyInt {
        let (mut low, mut big, mut shift) = (0u128, false, 0u32);
        while *pos < b.len() {
            let byte = b[*pos];
            *pos += 1;
            let bits = u128::from(byte & 0x7f);
            if shift < 128 {
                low |= bits << shift;
                big |= shift > 121 && bits >> (128 - shift) != 0;
            } else {
                big |= bits != 0;
            }
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        PyInt { low, big }
    }

    /// As a length or an offset: past any data when it does not fit.
    fn as_len(self) -> usize {
        if self.big {
            usize::MAX
        } else {
            usize::try_from(self.low).unwrap_or(usize::MAX)
        }
    }
}

/// A field number as Python reads it from a tag: the tag shifted right by
/// three, of any size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PyFieldNumber {
    low: u128,
    big: bool,
}

impl PyFieldNumber {
    fn is(self, n: u128) -> bool {
        !self.big && self.low == n
    }
}

/// `_scan_wire_tags`: the top level's field numbers, how many field 1s,
/// and the first field 1's payload when it is length-delimited.
fn scan_wire_tags(data: &[u8]) -> (Vec<PyFieldNumber>, usize, Option<&[u8]>) {
    let (mut numbers, mut count, mut first) = (Vec::new(), 0, None);
    let mut pos = 0;
    while pos < data.len() {
        let tag = PyInt::read(data, &mut pos);
        let number = PyFieldNumber {
            low: tag.low >> 3,
            big: tag.big,
        };
        if !numbers.contains(&number) {
            numbers.push(number);
        }
        match tag.low & 7 {
            0 => {
                while pos < data.len() && data[pos] & 0x80 != 0 {
                    pos += 1;
                }
                pos = pos.saturating_add(1);
            }
            1 => pos = pos.saturating_add(8),
            2 => {
                let len = PyInt::read(data, &mut pos).as_len();
                let end = pos.saturating_add(len).min(data.len());
                if number.is(1) {
                    count += 1;
                    if first.is_none() {
                        first = Some(&data[pos.min(data.len())..end]);
                    }
                }
                pos = pos.saturating_add(len);
            }
            5 => pos = pos.saturating_add(4),
            _ => break,
        }
    }
    (numbers, count, first)
}

/// `_is_fdp`: a bare `FileDescriptorProto`, or a set.
fn is_fdp(data: &[u8]) -> bool {
    let (numbers, count, first) = scan_wire_tags(data);
    if numbers.iter().any(|n| !n.is(1)) {
        return true;
    }
    if count > 1 {
        return false;
    }
    let Some(payload) = first else { return false };
    if let Some((name, _)) = name_and_imports(payload) {
        if !name.is_empty() {
            return false;
        }
    }
    true
}

/// `_split_binary`: (name, bytes) for each named file.
fn split_binary(data: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    if is_fdp(data) {
        let (name, _) = name_and_imports(data)?;
        return (!name.is_empty()).then(|| vec![(name, data.to_vec())]);
    }
    let def = Pool::vendored().get_message_by_name("google.protobuf.FileDescriptorSet")?;
    let set = DynamicMessage::decode(&def, data).ok()?;
    // Each file encoded on its own, in parallel, in order (spec 0443 P8).
    use rayon::prelude::*;
    let entries = FileDescriptorSet(set.as_ref()).file();
    let files: Vec<(String, Vec<u8>)> = entries
        .par_iter()
        .filter_map(|f| {
            let name = String::from_utf8_lossy(f.name()).into_owned();
            (!name.is_empty()).then(|| Some((name, f.0.encode().ok()?)))?
        })
        .collect();
    (!files.is_empty()).then_some(files)
}

/// `_split_text`: (name, text) for each named file, a set's `file` entries
/// (inside an `entry` when it is the only top-level field), else the whole
/// text when it names a file itself.
fn split_text(src: &str) -> Option<Vec<(String, String)>> {
    use prototext_upb::text_format::{field_bodies, scan_imports};
    let entries = field_bodies(src, "entry").ok()?;
    let level = if entries.len() == 1 { entries[0] } else { src };
    let files = field_bodies(level, "file").ok()?;
    let mut out = Vec::new();
    for f in &files {
        if let Ok((name, _)) = scan_imports(f) {
            if !name.is_empty() {
                out.push((name, f.to_string()));
            }
        }
    }
    if out.is_empty() && files.is_empty() {
        if let Ok((name, _)) = scan_imports(level) {
            if !name.is_empty() {
                out.push((name, src.to_string()));
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A fragment of the input: a binary file, or a text one.
pub enum Fragment {
    Binary(Vec<u8>),
    Text(String),
}

/// `split_fdps`: the input file's `FileDescriptorProto`s, or `None` when it
/// is not a set or a file of descriptors.
pub fn split_fdps(contents: &Contents) -> Option<Vec<(String, Fragment)>> {
    use prototext_core::serialize::encode_text::encode_text_to_binary;
    use prototext_core::serialize::render_text::is_prototext_text;
    match contents {
        Contents::Binary(b) => {
            let decoded;
            let data: &[u8] = if is_prototext_text(b) {
                decoded = encode_text_to_binary(b);
                &decoded
            } else {
                b
            };
            split_binary(data).map(|v| {
                v.into_iter()
                    .map(|(n, b)| (n, Fragment::Binary(b)))
                    .collect()
            })
        }
        Contents::Text(t) => {
            split_text(t).map(|v| v.into_iter().map(|(n, t)| (n, Fragment::Text(t))).collect())
        }
    }
}

// ── Loading ─────────────────────────────────────────────────────────────────

/// A raw file of the input, before splitting.
pub struct RawFile {
    pub root: PathBuf,
    pub rel_path: PathBuf,
    /// The physical file read, under `root`: `rel_path` but for an import,
    /// found under another extension (spec 0446's inputs).
    pub found: PathBuf,
    pub contents: Contents,
}

/// What `load_from_path` needs of the run.
pub struct LoadCtx<'a> {
    pub pruned_paths: &'a PathPatterns,
    pub path_seeds: &'a PathPatterns,
    pub events: &'a mut Events,
    pub blobs: &'a mut HashMap<PathBuf, Vec<(PathBuf, Vec<u8>)>>,
    /// `file:<name>` for each file seeded by path.
    pub path_seed_fqdns: &'a mut std::collections::BTreeSet<String>,
    /// The plugin, which sees each file as it is read (spec 0431 S1).
    pub plugin: Option<&'a crate::plugin::Plugin>,
}

/// `blob_members`: the descriptors fdp-scan finds in a file-shaped root,
/// named after themselves, `.proto` written `.pb`.
pub fn blob_members(
    blobs: &mut HashMap<PathBuf, Vec<(PathBuf, Vec<u8>)>>,
    blob: &Path,
) -> Vec<(PathBuf, Vec<u8>)> {
    if let Some(m) = blobs.get(blob) {
        return m.clone();
    }
    let data = std::fs::read(blob).unwrap_or_default();
    let mut members = Vec::new();
    for (start, end) in fdp_scan::scan(&data) {
        let fragment = data[start..end].to_vec();
        let Some((name, _)) = name_and_imports(&fragment) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let mut rel = PathBuf::from(&name);
        if suffix(&rel) == PROTO_EXTENSION {
            rel = with_suffix(&rel, ".pb");
        }
        members.push((rel, fragment));
    }
    blobs.insert(blob.to_path_buf(), members.clone());
    members
}

fn read(path: &Path) -> Option<Contents> {
    if TEXT_EXTENSIONS.contains(&suffix(path).as_str()) {
        std::fs::read_to_string(path).ok().map(Contents::Text)
    } else {
        std::fs::read(path).ok().map(Contents::Binary)
    }
}

/// Every path under `dir`, files and directories, sorted by path as Python
/// sorts `Path`s (component by component), as `sorted(dir.rglob('*'))`.
fn walk_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            // rglob does not follow a symlink to a directory.
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out.sort();
    out
}

/// `_load_files`: the raw files `rel_path` names under `roots`.
fn load_files(ctx: &mut LoadCtx, roots: &[PathBuf], rel_path: &Path) -> Vec<RawFile> {
    let mut loaded = Vec::new();
    for root in roots {
        if root.is_file() {
            // A file-shaped root is a blob (spec 0243 S5).
            let members = blob_members(ctx.blobs, root);
            let wanted = if suffix(rel_path) == PROTO_EXTENSION {
                with_suffix(rel_path, ".pb")
            } else {
                rel_path.to_path_buf()
            };
            let whole = posix_str(&rel_path.to_string_lossy()) == ".";
            for (member, fragment) in members {
                if !whole && member != wanted && !member.ancestors().skip(1).any(|a| a == wanted) {
                    continue;
                }
                if path_pruned(ctx, &member, &member) {
                    continue;
                }
                loaded.push(RawFile {
                    root: root.clone(),
                    found: member.clone(),
                    rel_path: member,
                    contents: Contents::Binary(fragment),
                });
            }
            continue;
        }
        let res_path = root.join(rel_path);
        if res_path.is_dir() {
            let all_ext: Vec<&str> = BINARY_EXTENSIONS
                .iter()
                .chain(TEXT_EXTENSIONS)
                .copied()
                .collect();
            for f in walk_sorted(&res_path) {
                if !all_ext.contains(&suffix(&f).as_str()) {
                    continue;
                }
                let Ok(f_rel) = f.strip_prefix(root) else {
                    continue;
                };
                if path_pruned(ctx, f_rel, f_rel) {
                    continue;
                }
                if let Some(contents) = read(&f) {
                    loaded.push(RawFile {
                        root: root.clone(),
                        rel_path: f_rel.to_path_buf(),
                        found: f_rel.to_path_buf(),
                        contents,
                    });
                }
            }
            continue;
        }
        let ext = suffix(rel_path);
        if TEXT_EXTENSIONS.contains(&ext.as_str()) || BINARY_EXTENSIONS.contains(&ext.as_str()) {
            if !res_path.is_file() || path_pruned(ctx, rel_path, rel_path) {
                continue;
            }
            if let Some(contents) = read(&res_path) {
                loaded.push(RawFile {
                    root: root.clone(),
                    rel_path: rel_path.to_path_buf(),
                    found: rel_path.to_path_buf(),
                    contents,
                });
            }
            return loaded;
        } else if ext == PROTO_EXTENSION {
            for e in TEXT_EXTENSIONS.iter().chain(BINARY_EXTENSIONS) {
                let found = with_suffix(rel_path, e);
                let candidate = root.join(&found);
                if candidate.is_file() {
                    // The file found, as everywhere else a path pattern
                    // applies; the import's `.proto` name too, as before.
                    if path_pruned(ctx, &found, &found) || path_pruned(ctx, rel_path, &found) {
                        continue;
                    }
                    if let Some(contents) = read(&candidate) {
                        loaded.push(RawFile {
                            root: root.clone(),
                            rel_path: rel_path.to_path_buf(),
                            found: found.clone(),
                            contents,
                        });
                    }
                    return loaded;
                }
            }
        }
    }
    if loaded.is_empty() {
        ctx.events.w1(&posix_str(&rel_path.to_string_lossy()));
    }
    loaded
}

/// Whether a path pattern matches `matched`, keeping physical file `file`
/// from loading; recorded in the report when so (spec 0446 S4).
fn path_pruned(ctx: &LoadCtx, matched: &Path, file: &Path) -> bool {
    match ctx.pruned_paths.matching(&matched.to_string_lossy()) {
        Some(pattern) => {
            crate::report::pruned_path(file, pattern);
            true
        }
        None => false,
    }
}

/// `parse_qfile`: one `QualFile` per file of the raw file.
fn parse_qfile(ctx: &mut LoadCtx, file: &RawFile) -> Vec<QualFile> {
    let rel = posix_str(&file.rel_path.to_string_lossy());
    let Some(fragments) = split_fdps(&file.contents) else {
        ctx.events.warn("W15", &format!("Cannot parse '{rel}'"));
        return Vec::new();
    };
    // Without a plugin, each binary fragment's imports read on their own,
    // in parallel (spec 0443 P8); a plugin's calls keep their order.
    let read: Vec<Option<Vec<String>>> = if ctx.plugin.is_none() {
        use rayon::prelude::*;
        fragments
            .par_iter()
            .map(|(_, fragment)| match fragment {
                Fragment::Binary(b) => {
                    Some(name_and_imports(b).map(|(_, i)| i).unwrap_or_default())
                }
                Fragment::Text(_) => None,
            })
            .collect()
    } else {
        vec![None; fragments.len()]
    };
    let mut out = Vec::new();
    for ((name, fragment), read) in fragments.into_iter().zip(read) {
        // With a plugin, a text file is parsed now, with descriptor.proto
        // alone, and its imports are the plugin's answer's (spec 0431 S1).
        let patched_imports = |bytes: &[u8]| -> Vec<String> {
            if let Some(imports) = &read {
                return imports.clone();
            }
            let answer = ctx.plugin.map(|p| p.apply(&name, bytes));
            name_and_imports(answer.as_deref().unwrap_or(bytes))
                .map(|(_, i)| i)
                .unwrap_or_default()
        };
        let (contents, text, imports) = match fragment {
            Fragment::Text(t) if ctx.plugin.is_some() => {
                match prototext_upb::text_format::parse_descriptor(&t, false) {
                    Ok(bytes) => {
                        let imports = patched_imports(&bytes);
                        (bytes, Some(t), imports)
                    }
                    Err(e) => {
                        ctx.events.warn(
                            "W15",
                            &format!(
                                "Cannot parse '{rel}' (file '{name}'): {}:{}: {}",
                                e.line, e.column, e.message
                            ),
                        );
                        continue;
                    }
                }
            }
            Fragment::Text(t) => match prototext_upb::text_format::scan_imports(&t) {
                Ok((_, imports)) => (Vec::new(), Some(t), imports),
                Err(e) => {
                    ctx.events
                        .warn("W15", &format!("Cannot parse '{rel}' (file '{name}'): {e}"));
                    continue;
                }
            },
            Fragment::Binary(b) => {
                let imports = patched_imports(&b);
                (b, None, imports)
            }
        };
        out.push(QualFile {
            root: file.root.clone(),
            rel_path: file.rel_path.clone(),
            name,
            contents,
            from_text: text.is_some(),
            text,
            imports,
        });
    }
    out
}

/// `load_from_path`: the input's files under `roots` that `rel_path`
/// names, one per descriptor name (the first; the others W7).
pub fn load_from_path(ctx: &mut LoadCtx, roots: &[PathBuf], rel_path: &Path) -> Vec<QualFile> {
    let raw = load_files(ctx, roots, rel_path);
    let mut qual_files = Vec::new();
    for file in &raw {
        let parsed = parse_qfile(ctx, file);
        crate::report::input(
            &file.root,
            &file.found,
            parsed.iter().map(|qf| qf.name.clone()).collect(),
        );
        if ctx.path_seeds.matches(&file.rel_path.to_string_lossy()) {
            for qf in &parsed {
                ctx.path_seed_fqdns.insert(format!("file:{}", qf.name));
            }
        }
        qual_files.extend(parsed);
    }
    let mut seen: HashMap<String, (PathBuf, PathBuf)> = HashMap::default();
    let mut deduped = Vec::new();
    for qf in qual_files {
        match seen.get(&qf.name) {
            None => {
                seen.insert(qf.name.clone(), (qf.root.clone(), qf.rel_path.clone()));
                deduped.push(qf);
            }
            Some((root, rel)) => {
                ctx.events.w7(
                    &qf.name,
                    &py_join(&qf.root, &qf.rel_path),
                    &py_join(root, rel),
                );
            }
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    mod cases {
        include!("load/is_fdp_cases.rs");
    }

    #[test]
    fn a_bare_file_or_a_set_as_python_decides() {
        let wrong: Vec<&str> = cases::CASES
            .iter()
            .filter(|(_, data, want)| is_fdp(data) != *want)
            .map(|(name, _, _)| *name)
            .collect();
        assert!(
            wrong.is_empty(),
            "{} of {} differ: {wrong:?}",
            wrong.len(),
            cases::CASES.len()
        );
    }
}
