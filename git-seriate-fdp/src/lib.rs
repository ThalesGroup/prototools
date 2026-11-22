// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! git-seriate for the `FileDescriptorProto` records embedded in executables
//! (spec 0453): `fdp-scan` as the extractor, prototext's lossless text as
//! the stored form, and the rule set `fdp` as the canonicalizer.

pub mod extract;
pub mod rules;

use std::path::{Path, PathBuf};

use git_seriate::cli::Kit;
use git_seriate::plugins::Renderer;
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
        Ok(Schema {
            root: root_message(prototext::EMBEDDED_DESCRIPTOR, FDP)?,
            graph: None,
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
        })
    }
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
}

impl Renderer for FdpRenderer {
    fn render(&self, _path: &str, item: &[u8]) -> Result<Vec<u8>, String> {
        let text = prototext_core::render_as_text(
            item,
            Some(&self.root),
            RenderOpts {
                assume_binary: true,
                include_annotations: true,
                ..RenderOpts::default()
            },
        )
        .map_err(|e| e.to_string())?;
        // Every rendering is read back before it is kept.
        if self.parse("", &text)? != item {
            return Err("render: the text does not read back to the same bytes".into());
        }
        Ok(text)
    }

    fn parse(&self, _path: &str, stored: &[u8]) -> Result<Vec<u8>, String> {
        prototext_core::render_as_bytes(stored, RenderOpts::default())
            .map(|b| b.into_owned())
            .map_err(|e| e.to_string())
    }
}

/// The plugins for a repository of descriptors.
pub fn kit(repo: &gix::Repository) -> Result<Kit, Error> {
    let schema = Schema::of(repo)?;
    Ok(Kit {
        renderer: Box::new(FdpRenderer { root: schema.root }),
        canonicalizer: Box::new(rules::Fdp::new()?),
        comparator: Box::new(seriation::LinesChars),
        legacy: None,
        item_pattern: Some("*.pb".into()),
    })
}
