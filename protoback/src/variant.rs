// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The variant: a YAML file and, beside it, a directory of `.pb`
//! fallbacks (reproto's `variant.py`). Resolution: `--proto-variant`, then
//! `REPROTO_VARIANT` (both through clap), then the built-in
//! `google-protobuf`, whose files are reproto's, embedded (spec 0428 S7).

use std::path::{Path, PathBuf};

use serde_yaml::Value;

const BUILTIN_YAML: &str = include_str!("../../reproto/src/reproto/variants/google-protobuf.yaml");

/// The built-in variant's fallbacks, by root-relative `.pb` path.
const BUILTIN_PBS: &[(&str, &[u8])] = &[
    (
        "google/protobuf/any.pb",
        include_bytes!("../../reproto/src/reproto/variants/google-protobuf/google/protobuf/any.pb"),
    ),
    (
        "google/protobuf/api.pb",
        include_bytes!("../../reproto/src/reproto/variants/google-protobuf/google/protobuf/api.pb"),
    ),
    (
        "google/protobuf/descriptor.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/descriptor.pb"
        ),
    ),
    (
        "google/protobuf/duration.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/duration.pb"
        ),
    ),
    (
        "google/protobuf/empty.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/empty.pb"
        ),
    ),
    (
        "google/protobuf/field_mask.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/field_mask.pb"
        ),
    ),
    (
        "google/protobuf/source_context.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/source_context.pb"
        ),
    ),
    (
        "google/protobuf/struct.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/struct.pb"
        ),
    ),
    (
        "google/protobuf/timestamp.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/timestamp.pb"
        ),
    ),
    (
        "google/protobuf/type.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/type.pb"
        ),
    ),
    (
        "google/protobuf/wrappers.pb",
        include_bytes!(
            "../../reproto/src/reproto/variants/google-protobuf/google/protobuf/wrappers.pb"
        ),
    ),
];

/// Where a variant's fallbacks are.
#[derive(Debug, Clone)]
pub enum Root {
    Builtin,
    Dir(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub descriptor_proto: String,
    /// Canonical path → the variant's path, in the file's order.
    pub well_known: Vec<(String, String)>,
    /// `import_rewrites`, applied to file names.
    pub import_rules: Vec<Rule>,
    /// `namespace_rewrites`, applied to type names and packages.
    pub namespace_rules: Vec<Rule>,
    pub root: Root,
    pub stem: String,
}

/// A rewrite rule: names starting with `prefix` are rewritten (`to`, the
/// prefix replaced) or kept (`to` none); the rules after it are tried too
/// only when `cont` (`continue: true`).
#[derive(Debug, Clone)]
pub struct Rule {
    pub prefix: String,
    pub to: Option<String>,
    pub cont: bool,
}

/// reproto's `apply_variant_namespace` and `canonize_dependency`: the rules
/// in order, the first matching one ending the walk unless it continues.
fn apply_rules(rules: &[Rule], name: &str) -> String {
    let mut s = name.to_string();
    for rule in rules {
        if s.starts_with(&rule.prefix) {
            if let Some(to) = &rule.to {
                s = format!("{to}{}", &s[rule.prefix.len()..]);
            }
            if !rule.cont {
                return s;
            }
        }
    }
    s
}

fn rules(value: &Value) -> Vec<Rule> {
    let Some(seq) = value.as_sequence() else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|r| {
            let m = r.as_mapping()?;
            let field = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
            let prefix = field("match")?;
            let to = (field("action").as_deref() == Some("rewrite"))
                .then(|| field("to").unwrap_or_default());
            let cont = m.get("continue").and_then(Value::as_bool).unwrap_or(false);
            Some(Rule { prefix, to, cont })
        })
        .collect()
}

impl Variant {
    /// The variant named by `path`, or the built-in one.
    pub fn load(path: Option<&Path>) -> Result<Variant, String> {
        let (text, root, stem) = match path {
            None => (
                BUILTIN_YAML.to_string(),
                Root::Builtin,
                "google-protobuf".to_string(),
            ),
            Some(p) => {
                let abs = std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))?;
                let stem = abs
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let dir = abs.parent().map(Path::to_path_buf).unwrap_or_default();
                let file = dir.join(format!("{stem}.yaml"));
                let text = std::fs::read_to_string(&file)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                (text, Root::Dir(dir), stem)
            }
        };
        let raw: Value = serde_yaml::from_str(&text).map_err(|e| format!("variant: {e}"))?;
        let get = |k: &str| {
            raw.as_mapping()
                .and_then(|m| m.get(k))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let well_known = get("well_known")
            .as_mapping()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.as_str()?.to_string(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Variant {
            descriptor_proto: get("descriptor_proto")
                .as_str()
                .unwrap_or("google/protobuf/descriptor.proto")
                .to_string(),
            well_known,
            import_rules: rules(&get("import_rewrites")),
            namespace_rules: rules(&get("namespace_rewrites")),
            root,
            stem,
        })
    }

    /// `apply_variant_namespace`: a type reference (`.pkg.Type`).
    pub fn namespace(&self, name: &str) -> String {
        apply_rules(&self.namespace_rules, name)
    }

    /// `apply_variant_namespace_to_package`: a file's package, matched as
    /// `.package.`.
    pub fn namespace_package(&self, package: &str) -> String {
        if package.is_empty() {
            return String::new();
        }
        self.namespace(&format!(".{package}."))
            .trim_matches('.')
            .to_string()
    }

    /// `canonize_dependency`: a file name, the variant's descriptor.proto
    /// kept as it is under `--keep-descriptor-path`.
    pub fn canonize_dependency(&self, name: &str, keep_descriptor: bool) -> String {
        if keep_descriptor && name == self.descriptor_proto {
            return name.to_string();
        }
        apply_rules(&self.import_rules, name)
    }

    /// A well-known type's path in this variant.
    pub fn well_known(&self, canonical: &str) -> String {
        self.well_known
            .iter()
            .find(|(c, _)| c == canonical)
            .map_or_else(|| canonical.to_string(), |(_, v)| v.clone())
    }

    /// The fallback `.pb` for `pb_name` (a root-relative path), if any.
    pub fn resource(&self, pb_name: &str) -> Option<Vec<u8>> {
        match &self.root {
            Root::Builtin => BUILTIN_PBS
                .iter()
                .find(|(n, _)| *n == pb_name)
                .map(|(_, b)| b.to_vec()),
            Root::Dir(dir) => std::fs::read(dir.join(&self.stem).join(pb_name)).ok(),
        }
    }
}
