// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `--dump-resolved-features FILE` (spec 0433 G4): reproto's
//! `_dump_resolved_features_yaml`. After phase 3, the named file's resolved
//! features, for the file and each message, field and enum in it, with
//! the overrides each sets itself, as YAML on stdout.

use prototext_upb::descriptor_access::{
    DescriptorProto, EnumDescriptorProto, FeatureSet, FieldDescriptorProto, FileDescriptorProto,
};

use crate::features::{resolve, value_name, Feature};
use crate::run::Run;
use crate::scoring::pyyaml::{dump_map, Y};

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn s(v: &str) -> Y {
    Y::Str(v.to_string())
}

/// Print the dump of file `name`; `Err`, reproto's warning (W14), when it
/// is not one of the files loaded.
pub fn print(run: &Run, name: &str) -> Result<(), String> {
    let graph = &run.graph;
    let file = graph
        .find(&format!("file:{name}"))
        .filter(|&i| graph.nodes[i].present)
        .and_then(|i| graph.nodes[i].def.as_ref())
        .and_then(|d| run.db.files.get(d.file));
    let Some(fdp) = file else {
        return Err(format!(
            "--dump-resolved-features: file '{name}' not found in loaded set"
        ));
    };
    let f = FileDescriptorProto(fdp.as_ref());
    let dump = Dump {
        run,
        edition: f.edition(),
    };
    print!("{}", dump_map(&dump.file(&f)));
    Ok(())
}

struct Dump<'r> {
    run: &'r Run,
    edition: i32,
}

impl Dump<'_> {
    /// `_feat_dict(_resolve(*sets))`: each feature, resolved, by name.
    fn resolved(&self, sets: &[Option<FeatureSet<'_>>]) -> Y {
        let table = &self.run.edition_defaults;
        Y::Map(
            Feature::ALL
                .iter()
                .map(|&f| {
                    let v = resolve(table, f, self.edition, sets);
                    (f.name().to_string(), s(&value_name(table, f, v)))
                })
                .collect(),
        )
    }

    /// `_overrides_dict`: the features `fs` sets itself.
    fn overrides(&self, fs: &Option<FeatureSet<'_>>) -> Y {
        let table = &self.run.edition_defaults;
        let Some(fs) = fs else {
            return Y::Map(Vec::new());
        };
        Y::Map(
            Feature::ALL
                .iter()
                .filter_map(|&f| {
                    f.get(fs)
                        .map(|v| (f.name().to_string(), s(&value_name(table, f, v))))
                })
                .collect(),
        )
    }

    fn file(&self, f: &FileDescriptorProto<'_>) -> Y {
        let file_fs = f.options().and_then(|o| o.features());
        let mut doc = vec![
            ("file".to_string(), s(&text(f.name()))),
            ("edition".to_string(), Y::Int(i64::from(self.edition))),
            ("edition_defaults".to_string(), self.resolved(&[])),
            ("file_features".to_string(), self.overrides(&file_fs)),
            (
                "file_resolved".to_string(),
                self.resolved(std::slice::from_ref(&file_fs)),
            ),
        ];
        let chain = [file_fs];
        let messages = f.message_type();
        if !messages.is_empty() {
            doc.push((
                "messages".to_string(),
                Y::Map(
                    messages
                        .iter()
                        .map(|m| (text(m.name()), self.message(m, &chain)))
                        .collect(),
                ),
            ));
        }
        let enums = f.enum_type();
        if !enums.is_empty() {
            doc.push((
                "enums".to_string(),
                Y::Map(
                    enums
                        .iter()
                        .map(|e| (text(e.name()), self.enumeration(e, &chain)))
                        .collect(),
                ),
            ));
        }
        Y::Map(doc)
    }

    /// An element's entry: its features resolved through `outer` (the
    /// file's, then each enclosing message's) and its own; its overrides.
    fn entry<'f>(
        &self,
        outer: &[Option<FeatureSet<'f>>],
        own: Option<FeatureSet<'f>>,
    ) -> Vec<(String, Y)> {
        let overrides = self.overrides(&own);
        let mut chain = outer.to_vec();
        chain.push(own);
        vec![
            ("resolved".to_string(), self.resolved(&chain)),
            ("overrides".to_string(), overrides),
        ]
    }

    fn message<'f>(&self, m: &DescriptorProto<'f>, outer: &[Option<FeatureSet<'f>>]) -> Y {
        let own = m.options().and_then(|o| o.features());
        let mut entry = self.entry(outer, own.clone());
        let mut chain = outer.to_vec();
        chain.push(own);
        let fields = m.field();
        if !fields.is_empty() {
            entry.push((
                "fields".to_string(),
                Y::Map(
                    fields
                        .iter()
                        .map(|f| (text(f.name()), self.field(f, &chain)))
                        .collect(),
                ),
            ));
        }
        let nested = m.nested_type();
        if !nested.is_empty() {
            entry.push((
                "messages".to_string(),
                Y::Map(
                    nested
                        .iter()
                        .map(|n| (text(n.name()), self.message(n, &chain)))
                        .collect(),
                ),
            ));
        }
        let enums = m.enum_type();
        if !enums.is_empty() {
            entry.push((
                "enums".to_string(),
                Y::Map(
                    enums
                        .iter()
                        .map(|e| (text(e.name()), self.enumeration(e, &chain)))
                        .collect(),
                ),
            ));
        }
        Y::Map(entry)
    }

    fn field<'f>(&self, f: &FieldDescriptorProto<'f>, outer: &[Option<FeatureSet<'f>>]) -> Y {
        Y::Map(self.entry(outer, f.options().and_then(|o| o.features())))
    }

    fn enumeration<'f>(&self, e: &EnumDescriptorProto<'f>, outer: &[Option<FeatureSet<'f>>]) -> Y {
        Y::Map(self.entry(outer, e.options().and_then(|o| o.features())))
    }
}
