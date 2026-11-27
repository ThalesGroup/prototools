// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! git-seriate for the `FileDescriptorProto` records embedded in executables
//! (spec 0453): `fdp-scan` as the extractor, prototext's lossless text as
//! the stored form, and the rule set `fdp` as the canonicalizer.

pub mod advise;
pub mod extract;
pub mod hints;
pub mod rules;

use std::path::{Path, PathBuf};

use git_seriate::cli::Kit;
use git_seriate::plugins::{Checked, Renderer};
use git_seriate::Error;
use prototext_core::schema::Message;
use prototext_core::RenderOpts;
use prototext_graph::score::load::LoadedGraph;

/// The stock root.
pub const FDP: &str = "google.protobuf.FileDescriptorProto";

/// The descriptor schema a repository scans and renders with: the stock
/// `descriptor.proto`, or the one its git configuration names
/// (`seriate.fdpSchema`, a schema database written by `protoback
/// --schema-db-out DB.desc --emit-extension-ranges`, and `seriate.fdpRoot`,
/// its `FileDescriptorProto`). A repository setting, not a flag: every
/// command must render alike, or `settle` would see each item anew.
pub struct Schema {
    /// The root message, for rendering.
    pub root: Message,
    /// The scoring graph and root's name, for scanning; `None`: the stock
    /// one `fdp-scan` embeds.
    pub graph: Option<(LoadedGraph, String)>,
    /// The descriptor set and root rendered with, fingerprinted: part of the
    /// renderer's version (spec 0464 S1).
    pub fingerprint: u32,
}

impl Schema {
    pub fn of(repo: &gix::Repository) -> Result<Self, Error> {
        let config = repo.config_snapshot();
        let Some(db) = config.string("seriate.fdpSchema").map(|s| s.to_string()) else {
            return Self::stock();
        };
        let root = config
            .string("seriate.fdpRoot")
            .map_or_else(|| FDP.to_string(), |s| s.to_string());
        let base = repo.workdir().unwrap_or_else(|| repo.git_dir());
        let db = base.join(db);
        Self::from_db(&db, &root)
    }

    /// The stock `descriptor.proto`.
    pub fn stock() -> Result<Self, Error> {
        Self::from_descriptor_set(prototext::EMBEDDED_DESCRIPTOR, FDP)
    }

    /// A descriptor set and its root, with the stock scoring graph.
    pub fn from_descriptor_set(descriptor_set: &[u8], root: &str) -> Result<Self, Error> {
        Ok(Schema {
            root: root_message(descriptor_set, root)?,
            graph: None,
            fingerprint: fingerprint(descriptor_set, root),
        })
    }

    /// A schema database: `DB.desc` beside its directory `DB/`, holding
    /// `hopcroft.rkyv`.
    pub fn from_db(db: &Path, root: &str) -> Result<Self, Error> {
        let bytes = std::fs::read(db)?;
        let graph_path: PathBuf = db.with_extension("").join("hopcroft.rkyv");
        let graph = prototext_graph::score::load::load_graph(&graph_path)
            .map_err(|e| Error::Refused(format!("{}: {e}", graph_path.display())))?;
        Ok(Schema {
            root: root_message(&bytes, root)?,
            graph: Some((graph, root.to_string())),
            fingerprint: fingerprint(&bytes, root),
        })
    }
}

/// FNV-1a, 32 bits, over the descriptor set and the root's name: stable
/// across builds and platforms, as a version recorded in a history must be.
fn fingerprint(descriptor_set: &[u8], root: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in descriptor_set
        .iter()
        .chain([0u8].iter())
        .chain(root.as_bytes())
    {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn root_message(descriptor_set: &[u8], root: &str) -> Result<Message, Error> {
    let schema = prototext_core::parse_schema(descriptor_set, root)
        .map_err(|e| Error::Refused(format!("the descriptor schema: {e}")))?;
    schema
        .root_descriptor()
        .ok_or_else(|| Error::Refused(format!("the descriptor schema has no {root}")))
}

/// Descriptors stored as prototext's text, annotated, so that they read
/// back without a schema (spec 0453 S3).
pub struct FdpRenderer {
    pub root: Message,
    /// [`Schema::fingerprint`].
    pub schema: u32,
}

impl FdpRenderer {
    /// Rendering with `schema`, its fingerprint in the renderer's version.
    pub fn new(schema: Schema) -> Self {
        FdpRenderer {
            root: schema.root,
            schema: schema.fingerprint,
        }
    }
}

impl Renderer for FdpRenderer {
    fn render(&self, _path: &str, item: &[u8]) -> Result<Vec<u8>, String> {
        // Not read back: prototext's text is lossless, checked by the tests
        // (`tests/fdp.rs`, prototext's round trips), and at runtime with
        // `seriate.checkRendering` (spec 0465).
        prototext_core::render_as_text(
            item,
            Some(&self.root),
            RenderOpts {
                assume_binary: true,
                include_annotations: true,
                ..RenderOpts::default()
            },
        )
        .map_err(|e| e.to_string())
    }

    fn parse(&self, _path: &str, stored: &[u8]) -> Result<Vec<u8>, String> {
        prototext_core::render_as_bytes(stored, RenderOpts::default())
            .map(|b| b.into_owned())
            .map_err(|e| e.to_string())
    }

    /// `prototext`, raised with prototext's annotated text
    /// (`tests/rendering.rs` holds it, spec 0464 S5); and the schema's
    /// fingerprint.
    fn version(&self) -> Vec<(String, u32)> {
        vec![("prototext".into(), 1), ("schema".into(), self.schema)]
    }
}

/// The plugins for a repository of descriptors.
pub fn kit(repo: &gix::Repository) -> Result<Kit, Error> {
    Ok(Kit {
        renderer: Checked::configured(repo, Box::new(FdpRenderer::new(Schema::of(repo)?))),
        canonicalizer: Box::new(rules::Fdp::new()?),
        comparator: Box::new(seriation::LinesChars),
        legacy: None,
        item_pattern: Some("*.pb".into()),
    })
}
