// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The variant: a YAML file and, beside it, a directory of `.pb`
//! fallbacks (reproto's `variant.py`). Resolution: `--proto-variant`, then
//! `REPROTO_VARIANT` (both through clap), then the built-in
//! `google-protobuf`, whose files are in `data/variants/`, embedded (spec
//! 0428 S7; reproto's package data until it was retired, spec 0487).

use prototext_upb::descriptor_access::FileDescriptorProto;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

const BUILTIN_YAML: &str = include_str!("../data/variants/google-protobuf.yaml");

/// The built-in variant's fallbacks, by root-relative `.pb` path.
const BUILTIN_PBS: &[(&str, &[u8])] = &[
    (
        "google/protobuf/any.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/any.pb"),
    ),
    (
        "google/protobuf/api.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/api.pb"),
    ),
    (
        "google/protobuf/descriptor.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/descriptor.pb"),
    ),
    (
        "google/protobuf/duration.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/duration.pb"),
    ),
    (
        "google/protobuf/empty.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/empty.pb"),
    ),
    (
        "google/protobuf/field_mask.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/field_mask.pb"),
    ),
    (
        "google/protobuf/source_context.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/source_context.pb"),
    ),
    (
        "google/protobuf/struct.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/struct.pb"),
    ),
    (
        "google/protobuf/timestamp.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/timestamp.pb"),
    ),
    (
        "google/protobuf/type.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/type.pb"),
    ),
    (
        "google/protobuf/wrappers.pb",
        include_bytes!("../data/variants/google-protobuf/google/protobuf/wrappers.pb"),
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
    /// Options rendered as orphans, by options message (`orphans`, after
    /// the edition orphans every variant has).
    pub orphans: std::collections::BTreeMap<String, Vec<String>>,
    pub root: Root,
    pub stem: String,
    /// The variant's name (its `name`, else the file's stem), and the keys
    /// of its file no tool reads, in the file's order (spec 0438 S5).
    pub name: String,
    pub unread_keys: Vec<String>,
    /// Spec 0447 S3, --target protoc: the rules derived from the descriptor
    /// in use; the descriptor's file and input package, once renamed.
    pub protoc_target: bool,
    pub renamed_descriptor: Option<(String, String)>,
    /// Spec 0447 S3, --target portable: what is mapped and redacted.
    pub portable: Option<Portable>,
}

/// --target portable (spec 0447 S3, S5): the descriptor in use kept as an
/// ordinary file; the options messages custom options extend mapped to the
/// stock ones; what the stock descriptor cannot represent redacted.
#[derive(Debug, Clone, Default)]
pub struct Portable {
    /// `.vendor.FieldOptions` -> `.google.protobuf.FieldOptions`, and the
    /// other options messages and FeatureSet, by whole name.
    extendee_rules: Vec<Rule>,
    /// The descriptor in use's package: a type of it named within it, as
    /// the two maps below name protoc's.
    pub package: String,
    /// Each message of protoc's descriptor, named within its package
    /// (`FieldOptions.EditionDefault`): its fields' numbers.
    pub stock_messages: std::collections::BTreeMap<String, std::collections::BTreeSet<u32>>,
    /// Each enum of protoc's descriptor, named within its package
    /// (`Edition`): its values' numbers.
    pub stock_enums: std::collections::BTreeMap<String, std::collections::BTreeSet<i32>>,
    /// Each message of protoc's descriptor: its extension ranges, `[start,
    /// end)`.
    pub stock_ranges: std::collections::BTreeMap<String, Vec<(u32, u32)>>,
}

/// The messages custom options and features extend.
pub const EXTENDEES: &[&str] = &[
    "FileOptions",
    "MessageOptions",
    "FieldOptions",
    "OneofOptions",
    "ExtensionRangeOptions",
    "EnumOptions",
    "EnumValueOptions",
    "ServiceOptions",
    "MethodOptions",
    "FeatureSet",
];

/// The keys a variant file may have, every one read (`variant._KEYS`).
const KEYS: &[&str] = &[
    "name",
    "description",
    "descriptor_proto",
    "well_known",
    "import_rewrites",
    "namespace_rewrites",
    "orphans",
];

/// A rewrite rule: names starting with `prefix` are rewritten (`to`, the
/// prefix replaced) or kept (`to` none); the rules after it are tried too
/// only when `cont` (`continue: true`).
#[derive(Debug, Clone)]
pub struct Rule {
    pub prefix: String,
    pub to: Option<String>,
    pub cont: bool,
    /// A rule of a whole name (the rules --target protoc derives, spec
    /// 0447 S3): `.vendor.FileOptions` matches `.vendor.FileOptions.Kind`,
    /// not `.vendor.FileOptionsX`; a file's rule, the file alone.
    pub whole: bool,
}

/// The stock descriptor: the tools' embedded copy (the built-in
/// variant's), whatever variant is active (spec 0447 S4).
pub fn stock_descriptor_pb() -> &'static [u8] {
    BUILTIN_PBS
        .iter()
        .find(|(n, _)| *n == "google/protobuf/descriptor.pb")
        .map(|(_, b)| *b)
        .unwrap_or_default()
}

/// reproto's `rule_matches`.
fn rule_matches(rule: &Rule, name: &str, inside: bool) -> bool {
    let Some(rest) = name.strip_prefix(&rule.prefix) else {
        return false;
    };
    !rule.whole || rest.is_empty() || (inside && rest.starts_with('.'))
}

pub const STOCK_PACKAGE: &str = "google.protobuf";
pub const STOCK_DESCRIPTOR: &str = "google/protobuf/descriptor.proto";

/// reproto's `apply_variant_namespace` and `canonize_dependency`: the rules
/// in order, the first matching one ending the walk unless it continues.
fn apply_rules(rules: &[Rule], name: &str, inside: bool) -> String {
    let mut s = name.to_string();
    for rule in rules {
        if rule_matches(rule, &s, inside) {
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

/// `variant._EDITION_ORPHANS`: options every variant renders as orphans.
const EDITION_ORPHANS: &[(&str, &[&str])] = &[
    ("EnumOptions", &["features", "feature_support"]),
    ("EnumValueOptions", &["features", "feature_support"]),
    (
        "ExtensionRangeOptions",
        &["features", "feature_support", "verification"],
    ),
    ("FieldOptions", &["features", "feature_support"]),
    ("FileOptions", &["features", "feature_support"]),
    ("MessageOptions", &["features", "feature_support"]),
    ("MethodOptions", &["features", "feature_support"]),
    ("OneofOptions", &["features", "feature_support"]),
    ("ServiceOptions", &["features", "feature_support"]),
];

/// `variant._merge_orphans`: the edition orphans, then the file's, each
/// list without repeats, in order.
fn orphans(value: &Value) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out: std::collections::BTreeMap<String, Vec<String>> = EDITION_ORPHANS
        .iter()
        .map(|(k, v)| {
            (
                (*k).to_string(),
                v.iter().map(|s| (*s).to_string()).collect(),
            )
        })
        .collect();
    if let Some(m) = value.as_mapping() {
        for (k, v) in m {
            let Some(k) = k.as_str() else { continue };
            let list = out.entry(k.to_string()).or_default();
            for name in v
                .as_sequence()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !list.iter().any(|n| n == name) {
                    list.push(name.to_string());
                }
            }
        }
    }
    out
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
            Some(Rule {
                prefix,
                to,
                cont,
                whole: false,
            })
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
            orphans: orphans(&get("orphans")),
            name: get("name")
                .as_str()
                .map_or_else(|| stem.clone(), str::to_string),
            unread_keys: raw
                .as_mapping()
                .into_iter()
                .flat_map(|m| m.keys())
                .filter_map(|k| k.as_str())
                .filter(|k| !KEYS.contains(k))
                .map(str::to_string)
                .collect(),
            root,
            stem,
            protoc_target: false,
            renamed_descriptor: None,
            portable: None,
        })
    }

    /// `apply_variant_namespace`: a type reference (`.pkg.Type`).
    pub fn namespace(&self, name: &str) -> String {
        apply_rules(&self.namespace_rules, name, true)
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

    /// Whether any namespace rule renames (spec 0440 S2): without one, two
    /// different input names never become one output name.
    pub fn renames_symbols(&self, keep_descriptor: bool) -> bool {
        !keep_descriptor && self.namespace_rules.iter().any(|r| r.to.is_some())
    }

    /// `output_symbol` (spec 0440 S1): a symbol's output full name, its
    /// leading dot kept as given; the input's under
    /// `--keep-descriptor-path`. With `output_package` and
    /// `canonize_dependency`, the only appliers of the rules but for
    /// predicates on what they give.
    pub fn output_symbol(&self, name: &str, keep_descriptor: bool) -> String {
        if keep_descriptor {
            return name.to_string();
        }
        match name.strip_prefix('.') {
            Some(_) => self.namespace(name),
            None => {
                let out = self.namespace(&format!(".{name}"));
                out.get(1..).unwrap_or_default().to_string()
            }
        }
    }

    /// `output_package`: a package's output name, or a scope's (a package
    /// or a message's full name, no leading dot); `file`, the file it is
    /// the package or a scope of. Under --target protoc, the descriptor's
    /// package is the stock one in the descriptor's file alone (spec 0447
    /// S3): its other files keep theirs.
    pub fn output_package(&self, package: &str, file: &str, keep_descriptor: bool) -> String {
        if keep_descriptor {
            return package.to_string();
        }
        if let Some((f, p)) = &self.renamed_descriptor {
            if f == file && p == package {
                return STOCK_PACKAGE.to_string();
            }
        }
        self.namespace_package(package)
    }

    /// Spec 0447 S3, --target portable: `desc`, the descriptor in use,
    /// against `stock`, protoc's (the tools' embedded copy); nothing for
    /// the stock descriptor.
    pub fn derive_portable(
        &mut self,
        desc: &FileDescriptorProto<'_>,
        stock: &FileDescriptorProto<'_>,
    ) {
        use std::collections::{BTreeMap, BTreeSet};
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let package = text(desc.package());
        if package == STOCK_PACKAGE {
            return;
        }
        let prefix = if package.is_empty() {
            ".".to_string()
        } else {
            format!(".{package}.")
        };
        let mut portable = Portable::default();
        let mine: BTreeMap<String, _> = desc
            .message_type()
            .into_iter()
            .map(|m| (text(m.name()), m))
            .collect();
        let theirs: BTreeMap<String, _> = stock
            .message_type()
            .into_iter()
            .map(|m| (text(m.name()), m))
            .collect();
        for name in EXTENDEES {
            if mine.contains_key(*name) && theirs.contains_key(*name) {
                portable.extendee_rules.push(Rule {
                    prefix: format!("{prefix}{name}"),
                    to: Some(format!(".{STOCK_PACKAGE}.{name}")),
                    cont: false,
                    whole: true,
                });
            }
        }
        portable.package = package;
        let mut todo: Vec<(
            String,
            prototext_upb::descriptor_access::DescriptorProto<'_>,
        )> = stock
            .message_type()
            .into_iter()
            .map(|m| (String::new(), m))
            .collect();
        let values = |e: &prototext_upb::descriptor_access::EnumDescriptorProto<'_>| {
            e.value()
                .iter()
                .map(|v| v.number())
                .collect::<BTreeSet<i32>>()
        };
        for e in stock.enum_type() {
            portable.stock_enums.insert(text(e.name()), values(&e));
        }
        while let Some((scope, m)) = todo.pop() {
            let name = if scope.is_empty() {
                text(m.name())
            } else {
                format!("{scope}.{}", text(m.name()))
            };
            portable.stock_messages.insert(
                name.clone(),
                m.field()
                    .iter()
                    .filter_map(|f| u32::try_from(f.number()).ok())
                    .collect(),
            );
            portable.stock_ranges.insert(
                name.clone(),
                m.extension_range()
                    .iter()
                    .filter_map(|r| {
                        Some((u32::try_from(r.start()).ok()?, u32::try_from(r.end()).ok()?))
                    })
                    .collect(),
            );
            for e in m.enum_type() {
                portable
                    .stock_enums
                    .insert(format!("{name}.{}", text(e.name())), values(&e));
            }
            todo.extend(m.nested_type().into_iter().map(|n| (name.clone(), n)));
        }
        self.portable = Some(portable);
    }

    /// An extendee's output name, for an extension numbered `number`:
    /// under --target portable, an options message of the descriptor in
    /// use mapped to the stock one (spec 0447 S3), unless the stock one has
    /// no extension range for that number (the extension is then redacted,
    /// S5, and keeps its extendee); else `output_symbol`.
    pub fn output_extendee(&self, name: &str, number: u32, keep_descriptor: bool) -> String {
        if self.redacts_extension(name, number) {
            return self.output_symbol(name, keep_descriptor);
        }
        if let Some(p) = &self.portable {
            if p.extendee_rules
                .iter()
                .any(|r| rule_matches(r, name, false))
            {
                return apply_rules(&p.extendee_rules, name, false);
            }
        }
        self.output_symbol(name, keep_descriptor)
    }

    /// Spec 0447 S5, --target portable: whether an extension of `extendee`
    /// (`.vendor.FileOptions`) numbered `number` is one protoc's options
    /// message cannot declare (the vendor's range `990 to 999`, say): a
    /// mapped extendee whose stock message has no range for it.
    pub fn redacts_extension(&self, extendee: &str, number: u32) -> bool {
        let Some(p) = &self.portable else {
            return false;
        };
        if !self.maps_extendee(extendee) {
            return false;
        }
        let name = extendee.trim_start_matches('.');
        let name = match name.strip_prefix(&format!("{}.", p.package)) {
            Some(rest) if !p.package.is_empty() => rest,
            _ => name,
        };
        !p.stock_ranges
            .get(name)
            .is_some_and(|ranges| ranges.iter().any(|&(s, e)| (s..e).contains(&number)))
    }

    /// Whether an extendee is one --target portable maps.
    pub fn maps_extendee(&self, name: &str) -> bool {
        self.portable.as_ref().is_some_and(|p| {
            p.extendee_rules
                .iter()
                .any(|r| rule_matches(r, name, false))
        })
    }

    /// Spec 0447 S3, --target portable: the stock descriptor, imported for
    /// the mapped extends, is protoc's own: not written to disk.
    pub fn kept_off_disk(&self, name: &str) -> bool {
        self.portable.is_some()
            && name == STOCK_DESCRIPTOR
            && self.descriptor_proto != STOCK_DESCRIPTOR
    }

    /// Spec 0447 S3, --target protoc: the descriptor in use (`fdp`, its
    /// file) written as google/protobuf/descriptor.proto, its definitions
    /// renamed to the stock package, and every reference to them; nothing
    /// for the stock one. reproto's `_derive_protoc_renaming`.
    pub fn derive_protoc_renaming(&mut self, fdp: &FileDescriptorProto<'_>) {
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let name = self.descriptor_proto.clone();
        self.protoc_target = true;
        if name != STOCK_DESCRIPTOR {
            self.import_rules = vec![Rule {
                prefix: name.clone(),
                to: Some(STOCK_DESCRIPTOR.to_string()),
                cont: false,
                whole: true,
            }];
        }
        let package = text(fdp.package());
        if package != STOCK_PACKAGE {
            let prefix = if package.is_empty() {
                ".".to_string()
            } else {
                format!(".{package}.")
            };
            let top: Vec<String> = fdp
                .message_type()
                .iter()
                .map(|m| text(m.name()))
                .chain(fdp.enum_type().iter().map(|e| text(e.name())))
                .chain(fdp.extension().iter().map(|x| text(x.name())))
                .chain(fdp.service().iter().map(|s| text(s.name())))
                .collect();
            self.namespace_rules = top
                .into_iter()
                .map(|n| Rule {
                    prefix: format!("{prefix}{n}"),
                    to: Some(format!(".{STOCK_PACKAGE}.{n}")),
                    cont: false,
                    whole: true,
                })
                .collect();
            self.renamed_descriptor = Some((name, package));
        }
    }

    /// `canonize_dependency`: a file name, the variant's descriptor.proto
    /// kept as it is under `--keep-descriptor-path`.
    pub fn canonize_dependency(&self, name: &str, keep_descriptor: bool) -> String {
        if keep_descriptor && name == self.descriptor_proto {
            return name.to_string();
        }
        apply_rules(&self.import_rules, name, false)
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
