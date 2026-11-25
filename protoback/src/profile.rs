// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Profiles (spec 0447 S6): a saved set of command-line options, as
//! reproto's `profile.py` reads them.
//!
//! `--profile NAME`, repeatable. NAME is looked up as a path (`.yaml`
//! optional), then in each folder of `$PROTOBACK_PROFILE_PATH`, then among
//! the built-in profiles (`data/profiles/`, embedded). A key is an
//! option's long name; a later profile overrides an earlier one, the
//! command line overrides them all, and lists accumulate. A profile
//! `X.yaml` may have a folder `X/` beside it, laid out as a descriptor root:
//! every file in it replaces the input's copy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

pub const ENV: &str = "PROTOBACK_PROFILE_PATH";

/// The built-in profiles: `data/profiles/`, embedded (reproto's package
/// data until it was retired, spec 0487).
const BUILTIN: &[(&str, &str)] = &[("prost.yaml", include_str!("../data/profiles/prost.yaml"))];

/// The values a key takes, where the option limits them.
const CHOICES: &[(&str, &[&str])] = &[
    ("target", &["keep", "protoc", "portable"]),
    ("editions", &["keep", "proto2"]),
    (
        "builtin",
        &[
            "any",
            "empty",
            "timestamp",
            "duration",
            "struct",
            "wrappers",
            "descriptor",
            "source_context",
            "field_mask",
            "type",
            "api",
            "all",
        ],
    ),
];
const STRINGS: &[&str] = &["descriptor", "target", "editions"];
const BOOLS: &[&str] = &["import_descriptor", "emit_descriptor"];
const LISTS: &[&str] = &["builtin", "fail_on"];

/// The profiles named, merged in order.
#[derive(Debug, Default)]
pub struct Profiles {
    pub strings: BTreeMap<String, String>,
    pub bools: BTreeMap<String, bool>,
    pub lists: BTreeMap<String, Vec<String>>,
    /// The profiles' folders, in order: each file in one replaces the
    /// input's.
    pub folders: Vec<PathBuf>,
}

/// The profile's text, and the path of its file (`None` for a built-in
/// one).
fn locate(name: &str) -> Result<(String, Option<PathBuf>), String> {
    let stems: Vec<String> = if name.ends_with(".yaml") {
        vec![name.to_string()]
    } else {
        vec![name.to_string(), format!("{name}.yaml")]
    };
    for stem in &stems {
        let path = Path::new(stem);
        if path.is_file() {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{stem}: {e}"))?;
            return Ok((text, Some(path.canonicalize().unwrap_or(path.into()))));
        }
    }
    let folders: Vec<PathBuf> = std::env::var(ENV)
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .collect();
    for folder in &folders {
        for stem in &stems {
            let path = folder.join(stem);
            if path.is_file() {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                return Ok((text, Some(path.canonicalize().unwrap_or(path))));
            }
        }
    }
    for stem in &stems {
        if let Some((_, text)) = BUILTIN.iter().find(|(n, _)| n == stem) {
            return Ok((text.to_string(), None));
        }
    }
    let mut where_ = vec!["the current directory".to_string()];
    where_.extend(folders.iter().map(|f| f.display().to_string()));
    where_.push("the built-in profiles".to_string());
    Err(format!(
        "profile '{name}' not found (looked in: {})",
        where_.join(", ")
    ))
}

fn check_choice(name: &str, key: &str, value: &str) -> Result<(), String> {
    if let Some((_, choices)) = CHOICES.iter().find(|(k, _)| *k == key) {
        if !choices.contains(&value) {
            return Err(format!(
                "profile '{name}': '{key}': '{value}' is not one of {}",
                choices.join(", ")
            ));
        }
    }
    Ok(())
}

/// The profiles `names`, merged in order: an error on a profile not found,
/// a key no option has, or a value of the wrong type.
pub fn load(names: &[String]) -> Result<Profiles, String> {
    let mut out = Profiles::default();
    for name in names {
        let (text, path) = locate(name)?;
        let raw: Value = serde_yaml::from_str(&text)
            .map_err(|e| format!("profile '{name}': invalid YAML: {e}"))?;
        let map = match raw {
            Value::Null => serde_yaml::Mapping::new(),
            Value::Mapping(m) => m,
            _ => return Err(format!("profile '{name}': must be a mapping of options")),
        };
        let mut unknown: Vec<String> = map
            .keys()
            .map(|k| k.as_str().map_or_else(|| format!("{k:?}"), str::to_string))
            .filter(|k| {
                !STRINGS.contains(&k.as_str())
                    && !BOOLS.contains(&k.as_str())
                    && !LISTS.contains(&k.as_str())
            })
            .collect();
        if !unknown.is_empty() {
            unknown.sort();
            return Err(format!(
                "profile '{name}': unknown key(s): {}",
                unknown.join(", ")
            ));
        }
        for key in STRINGS {
            if let Some(v) = map.get(*key) {
                let v = v
                    .as_str()
                    .ok_or_else(|| format!("profile '{name}': '{key}' must be a str"))?;
                check_choice(name, key, v)?;
                out.strings.insert(key.to_string(), v.to_string());
            }
        }
        for key in BOOLS {
            if let Some(v) = map.get(*key) {
                let v = v
                    .as_bool()
                    .ok_or_else(|| format!("profile '{name}': '{key}' must be a bool"))?;
                out.bools.insert(key.to_string(), v);
            }
        }
        for key in LISTS {
            if let Some(v) = map.get(*key) {
                let bad = || format!("profile '{name}': '{key}' must be a list of strings");
                let values: Vec<String> = match v {
                    Value::String(s) => vec![s.clone()],
                    Value::Sequence(s) => s
                        .iter()
                        .map(|x| x.as_str().map(str::to_string).ok_or_else(bad))
                        .collect::<Result<_, _>>()?,
                    _ => return Err(bad()),
                };
                for v in &values {
                    check_choice(name, key, v)?;
                }
                out.lists.entry(key.to_string()).or_default().extend(values);
            }
        }
        if let Some(path) = path {
            let folder = path.with_extension("");
            if folder.is_dir() {
                out.folders.push(folder);
            }
        }
    }
    Ok(out)
}
