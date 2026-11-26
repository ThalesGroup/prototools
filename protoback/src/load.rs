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
    // Its imports, then its `import option`s (edition 2024), each once:
    // reproto's `fdp_imports`.
    let mut imports: Vec<String> = f
        .dependency()
        .iter()
        .map(|d| String::from_utf8_lossy(d).into_owned())
        .collect();
    for d in f.option_dependency() {
        let d = String::from_utf8_lossy(d).into_owned();
        if !imports.contains(&d) {
            imports.push(d);
        }
    }
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
    /// Import discovery under way: a file not found is an import, which
    /// the report has from phase 2, with its importer.
    pub importing: bool,
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
        let missing = posix_str(&rel_path.to_string_lossy());
        if ctx.importing {
            ctx.events.w1_unreported(&missing);
        } else {
            ctx.events.w1(&missing);
        }
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

/// The unit tests of `test_split_fdps.py` (spec 0488 S2), on the function
/// that ports reproto's `split_fdps` (spec 0065). reproto took the data and
/// the file's extension, which settled text against binary; protoback
/// takes `Contents`, which `read` makes text or binary by the extension:
/// the tiebreaker tests are on `read`. A `ValueError` is `None`. The
/// original's binary inputs, sets and files of names alone, are their
/// bytes, built here.
#[cfg(test)]
mod test_split_fdps {
    use super::*;
    use crate::suite_fixtures::{fixture, protoc, tmp, tools};
    use prototext_upb::descriptor_access::FileDescriptorProto;
    use prototext_upb::{DynamicMessage, Pool};

    /// A length-delimited field 1 holding `payload`.
    fn field1(payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 0x80);
        [&[0x0a, payload.len() as u8][..], payload].concat()
    }

    /// `_make_binary_fdp`: a FileDescriptorProto of `name` alone.
    fn binary_fdp(name: &str) -> Vec<u8> {
        field1(name.as_bytes())
    }

    /// `_make_binary_fds`: a set of such files.
    fn binary_fds(names: &[&str]) -> Vec<u8> {
        names.iter().flat_map(|n| field1(&binary_fdp(n))).collect()
    }

    fn text(src: &str) -> Vec<(String, Fragment)> {
        split_fdps(&Contents::Text(src.to_string())).unwrap()
    }

    fn names(result: &[(String, Fragment)]) -> Vec<&str> {
        result.iter().map(|(n, _)| n.as_str()).collect()
    }

    fn interior(f: &Fragment) -> &str {
        match f {
            Fragment::Text(t) => t,
            Fragment::Binary(_) => panic!("a binary fragment"),
        }
    }

    #[test]
    fn test_two_file_blocks() {
        let r = text(r#"file { name: "a.proto" } file { name: "b.proto" }"#);
        assert_eq!(names(&r), ["a.proto", "b.proto"]);
    }

    #[test]
    fn test_three_file_blocks() {
        let r =
            text(r#"file { name: "a.proto" } file { name: "b.proto" } file { name: "c.proto" }"#);
        assert_eq!(names(&r), ["a.proto", "b.proto", "c.proto"]);
    }

    #[test]
    fn test_entry_wrapper() {
        let r = text(r#"entry { file { name: "a.proto" } file { name: "b.proto" } }"#);
        assert_eq!(names(&r), ["a.proto", "b.proto"]);
    }

    #[test]
    fn test_colon_syntax() {
        let r = text(r#"file: { name: "a.proto" } file: { name: "b.proto" }"#);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "a.proto");
    }

    #[test]
    fn test_angle_delimiters() {
        let r = text(r#"file < name: "a.proto" > file < name: "b.proto" >"#);
        assert_eq!(names(&r), ["a.proto", "b.proto"]);
    }

    /// The interior is exactly what was between the braces.
    #[test]
    fn test_interior_verbatim() {
        let r = text("file { name: \"a.proto\"\n  package: \"foo\" # a comment\n}");
        assert_eq!(r.len(), 1);
        let i = interior(&r[0].1);
        for s in [r#"package: "foo""#, "# a comment", r#"name: "a.proto""#] {
            assert!(i.contains(s), "{s}: {i}");
        }
    }

    #[test]
    fn test_nested_message_verbatim() {
        let r = text(
            r#"file { name: "a.proto" message_type { name: "Msg" field { name: "f" number: 1 } } }"#,
        );
        assert_eq!(r.len(), 1);
        let i = interior(&r[0].1);
        assert!(i.contains("message_type") && i.contains("field"), "{i}");
    }

    #[test]
    fn test_string_with_braces() {
        let r = text(r#"file { name: "a.proto" options { java_package: "com.{example}" } }"#);
        assert_eq!(r.len(), 1);
        assert!(interior(&r[0].1).contains("com.{example}"));
    }

    /// A bare FileDescriptorProto: one fragment, the whole source.
    #[test]
    fn test_bare_fdp() {
        let r = text("name: \"a.proto\"\npackage: \"foo\"");
        assert_eq!(names(&r), ["a.proto"]);
        interior(&r[0].1);
    }

    #[test]
    fn test_single_file_block() {
        let r = text(r#"file { name: "a.proto" }"#);
        assert_eq!(names(&r), ["a.proto"]);
    }

    #[test]
    fn test_two_fdps() {
        let r = split_fdps(&Contents::Binary(binary_fds(&["a.proto", "b.proto"]))).unwrap();
        let ns = names(&r);
        assert!(ns.contains(&"a.proto") && ns.contains(&"b.proto"), "{ns:?}");
        let def = Pool::vendored()
            .get_message_by_name("google.protobuf.FileDescriptorProto")
            .unwrap();
        for (_, f) in &r {
            let Fragment::Binary(b) = f else {
                panic!("a text fragment")
            };
            let m = DynamicMessage::decode(&def, b).unwrap();
            let name = String::from_utf8_lossy(FileDescriptorProto(m.as_ref()).name()).into_owned();
            assert!(ns.contains(&name.as_str()), "{name}");
        }
    }

    #[test]
    fn test_binary_bare_fdp() {
        let r = split_fdps(&Contents::Binary(binary_fdp("a.proto"))).unwrap();
        assert_eq!(names(&r), ["a.proto"]);
        assert!(matches!(r[0].1, Fragment::Binary(_)));
    }

    /// `content` written as `name` and read as protoback reads it.
    fn read_as(test: &str, name: &str, content: &[u8]) -> Contents {
        let p = tmp(test).join(name);
        std::fs::write(&p, content).unwrap();
        read(&p).unwrap()
    }

    #[test]
    fn test_text_extension_prefers_text() {
        let c = read_as(
            "test_text_extension_prefers_text",
            "a.textpb",
            br#"file { name: "a.proto" }"#,
        );
        assert!(matches!(split_fdps(&c).unwrap()[0].1, Fragment::Text(_)));
    }

    #[test]
    fn test_binary_extension_prefers_binary() {
        let c = read_as(
            "test_binary_extension_prefers_binary",
            "a.pb",
            &binary_fds(&["a.proto"]),
        );
        assert!(matches!(split_fdps(&c).unwrap()[0].1, Fragment::Binary(_)));
    }

    #[test]
    fn test_unrecognisable_raises() {
        assert!(split_fdps(&Contents::Binary(b"\x00\x01\x02\x03garbage".to_vec())).is_none());
    }

    #[test]
    fn test_empty_bytes_raises() {
        assert!(split_fdps(&Contents::Binary(Vec::new())).is_none());
    }

    /// A real fixture, compiled by protoc.
    #[test]
    fn test_address_book_binary() {
        if !tools(&["protoc"]) {
            return;
        }
        let pb = tmp("test_address_book_binary").join("address_book.pb");
        protoc([
            format!("-I{}", fixture("").display()),
            format!("--descriptor_set_out={}", pb.display()),
            fixture("address_book.proto").display().to_string(),
        ]);
        let r = split_fdps(&Contents::Binary(std::fs::read(&pb).unwrap())).unwrap();
        assert!(!r.is_empty());
        assert!(r.iter().all(|(n, _)| n.ends_with(".proto")));
    }
}

/// The unit tests of `test_load.py` (spec 0488 S2), on the function that
/// ports reproto's `load_from_path` (specs 0148, 0149). The warnings it
/// logged are read from the record of what `Events` printed; a text file's
/// package, which reproto parsed, from the text it keeps until the input is
/// read (spec 0427 S3).
#[cfg(test)]
mod test_load {
    use super::*;
    use crate::suite_fixtures::tmp;

    /// `load_from_path` under `roots` for `rel`, `pruned` and `seeds` its
    /// path patterns: the files, the warnings printed, the path-seeded
    /// files.
    fn load(
        roots: &[PathBuf],
        rel: &str,
        pruned: &[&str],
        seeds: &[&str],
    ) -> (
        Vec<QualFile>,
        Vec<String>,
        std::collections::BTreeSet<String>,
    ) {
        let strings = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (pruned, seeds) = (
            PathPatterns::new(&strings(pruned)),
            PathPatterns::new(&strings(seeds)),
        );
        let mut events = Events::new(false);
        events.record();
        let (mut blobs, mut seeded) = (HashMap::default(), std::collections::BTreeSet::new());
        let mut ctx = LoadCtx {
            pruned_paths: &pruned,
            path_seeds: &seeds,
            events: &mut events,
            blobs: &mut blobs,
            path_seed_fqdns: &mut seeded,
            plugin: None,
            importing: false,
        };
        let files = load_from_path(&mut ctx, roots, Path::new(rel));
        let warnings = events
            .lines
            .iter()
            .filter_map(|l| l.strip_prefix("warning | "))
            .map(String::from)
            .collect();
        (files, warnings, seeded)
    }

    /// Roots `a` and `b` under the test's directory, `files` written in
    /// them as (root, path, text).
    fn roots(test: &str, files: &[(&str, &str, &str)]) -> (PathBuf, PathBuf) {
        let dir = tmp(test);
        for (root, path, text) in files {
            let p = dir.join(root).join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        (a, b)
    }

    fn package(qf: &QualFile) -> &str {
        let t = qf.text.as_deref().unwrap_or("");
        ["pkg_a", "pkg_b", "pkg_1", "pkg_2"]
            .into_iter()
            .find(|p| t.contains(&format!("package: \"{p}\"")))
            .unwrap_or("")
    }

    /// G1: every -I root is scanned for a directory-shaped argument.
    #[test]
    fn test_directory_seed_arg_loads_from_every_root() {
        let (a, b) = roots(
            "test_directory_seed_arg_loads_from_every_root",
            &[
                ("a", "foo.textpb", "name: \"foo.proto\"\n"),
                ("b", "bar.textpb", "name: \"bar.proto\"\n"),
            ],
        );
        let (files, warnings, _) = load(&[a, b], ".", &[], &[]);
        let mut names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["bar.proto", "foo.proto"]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// G2, G3: a name across roots: the first wins, W7.
    #[test]
    fn test_same_name_across_roots_shadows_and_warns() {
        let (a, b) = roots(
            "test_same_name_across_roots_shadows_and_warns",
            &[
                (
                    "a",
                    "foo.textpb",
                    "name: \"foo.proto\"\npackage: \"pkg_a\"\n",
                ),
                (
                    "b",
                    "foo.textpb",
                    "name: \"foo.proto\"\npackage: \"pkg_b\"\n",
                ),
            ],
        );
        let (files, warnings, _) = load(&[a.clone(), b.clone()], ".", &[], &[]);
        assert_eq!(files.len(), 1);
        assert_eq!(
            package(&files[0]),
            "pkg_a",
            "first root (-I order) must win"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let w = &warnings[0];
        assert!(w.contains("foo.proto"), "{w}");
        assert!(
            w.contains(&a.join("foo.textpb").display().to_string()),
            "{w}"
        );
        assert!(
            w.contains(&b.join("foo.textpb").display().to_string()),
            "{w}"
        );
        assert!(w.starts_with("Warning [W7]:"), "{w}");
    }

    /// G4: within one root too.
    #[test]
    fn test_same_name_within_one_root_shadows_and_warns() {
        let (a, _) = roots(
            "test_same_name_within_one_root_shadows_and_warns",
            &[
                (
                    "a",
                    "foo.textpb",
                    "name: \"foo.proto\"\npackage: \"pkg_1\"\n",
                ),
                (
                    "a",
                    "foo_copy.textpb",
                    "name: \"foo.proto\"\npackage: \"pkg_2\"\n",
                ),
            ],
        );
        let (files, warnings, _) = load(&[a], ".", &[], &[]);
        assert_eq!(files.len(), 1);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    /// N2: a single file stops at the first root, no other probed, no W7.
    #[test]
    fn test_single_file_seed_arg_stops_at_first_root_no_warning() {
        let (a, b) = roots(
            "test_single_file_seed_arg_stops_at_first_root_no_warning",
            &[
                (
                    "a",
                    "foo.textpb",
                    "name: \"foo.proto\"\npackage: \"pkg_a\"\n",
                ),
                (
                    "b",
                    "foo.textpb",
                    "name: \"foo.proto\"\npackage: \"pkg_b\"\n",
                ),
            ],
        );
        let (files, warnings, _) = load(&[a, b], "foo.textpb", &[], &[]);
        assert_eq!(files.len(), 1);
        assert_eq!(package(&files[0]), "pkg_a");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    const NESTED: &[(&str, &str, &str)] = &[
        (
            "a",
            "nested/foo.textpb",
            "name: \"foo.proto\"\npackage: \"pkg_a\"\n",
        ),
        (
            "b",
            "other/foo.textpb",
            "name: \"foo.proto\"\npackage: \"pkg_b\"\n",
        ),
    ];

    /// G3: prune-by-path settles a name two paths share: root_a's pruned,
    /// root_b's survives, no W7.
    #[test]
    fn test_g3_prune_by_path_disambiguates_duplicate_name() {
        let (a, b) = roots("test_g3_prune_by_path_disambiguates_duplicate_name", NESTED);
        let (files, warnings, _) = load(&[a, b], ".", &["nested/foo.textpb"], &[]);
        assert_eq!(files.len(), 1);
        assert_eq!(
            package(&files[0]),
            "pkg_b",
            "root_a's candidate was pruned by path; root_b's survives"
        );
        assert!(
            warnings.is_empty(),
            "only one physical candidate remains: no W7"
        );
    }

    /// The same, the roots reversed.
    #[test]
    fn test_g3_prune_by_path_disambiguation_independent_of_root_order() {
        let (a, b) = roots(
            "test_g3_prune_by_path_disambiguation_independent_of_root_order",
            NESTED,
        );
        let (files, warnings, _) = load(&[b, a], ".", &["nested/foo.textpb"], &[]);
        assert_eq!(files.len(), 1);
        assert_eq!(package(&files[0]), "pkg_b");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// G6: a bare path pattern matches alike under every root.
    #[test]
    fn test_g6_bare_path_pattern_matches_under_every_root() {
        let (a, b) = roots(
            "test_g6_bare_path_pattern_matches_under_every_root",
            &[
                ("a", "pkg/thing_a.textpb", "name: \"thing_a.proto\"\n"),
                ("b", "pkg/thing_b.textpb", "name: \"thing_b.proto\"\n"),
            ],
        );
        let (_, _, seeded) = load(&[a, b], ".", &[], &["pkg/*.textpb"]);
        let want: std::collections::BTreeSet<String> = ["file:thing_a.proto", "file:thing_b.proto"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            seeded, want,
            "a bare glob must match whichever -I root a candidate is under"
        );
    }
}
