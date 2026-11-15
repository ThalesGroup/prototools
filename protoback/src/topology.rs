// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The input's files and their imports (reproto's `topology.py`).
//!
//! reproto keeps `files` and `new_files`, two dicts, merged after each
//! round of import discovery; a file keeps the position it was first
//! created at. Here, one arena in creation order, and how many of its
//! files were merged: the same order, and the same files visible to a
//! round.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::load::QualFile;

/// A field or method phase 2 stripped (spec 0087): its record, as the dump
/// writes it.
pub type Stripped = Vec<String>;

#[derive(Debug, Default)]
pub struct File {
    pub name: String,
    pub qfile: Option<QualFile>,
    pub is_seed: bool,
    pub is_pruned: bool,
    pub targets: BTreeSet<usize>,
    pub stripped_dependencies: Vec<String>,
    pub stripped_public_dependencies: Vec<String>,
    /// Message FQDN → stripped fields, in their order.
    pub stripped_field_types: BTreeMap<String, Vec<Stripped>>,
    /// Service FQDN → stripped methods, in their order.
    pub stripped_method_types: BTreeMap<String, Vec<Stripped>>,
}

impl File {
    pub fn is_ref(&self) -> bool {
        self.qfile.is_none()
    }
}

#[derive(Debug, Default)]
pub struct Topology {
    pub files: Vec<File>,
    by_name: HashMap<String, usize>,
    /// Files `0..merged` are in reproto's `files`; the rest in `new_files`.
    pub merged: usize,
}

impl Topology {
    pub fn find(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    pub fn has_new(&self) -> bool {
        self.merged < self.files.len()
    }

    pub fn merge(&mut self) {
        self.merged = self.files.len();
    }

    /// `ReFile(topo, name)`: the file of that name, created as a reference
    /// when there is none.
    pub fn reference(&mut self, name: &str) -> usize {
        if let Some(i) = self.find(name) {
            return i;
        }
        self.files.push(File {
            name: name.to_string(),
            ..File::default()
        });
        let i = self.files.len() - 1;
        self.by_name.insert(name.to_string(), i);
        i
    }

    /// `ReFile(topo, qfile)`: the file of its name, given its qfile and its
    /// imports when it was a reference; unchanged when it was loaded.
    pub fn load(&mut self, qf: QualFile) -> usize {
        let i = self.reference(&qf.name);
        if !self.files[i].is_ref() {
            return i;
        }
        let imports = qf.imports.clone();
        self.files[i].qfile = Some(qf);
        self.files[i].is_seed = false;
        for dep in imports {
            let d = self.reference(&dep);
            self.files[i].targets.insert(d);
        }
        i
    }
}
