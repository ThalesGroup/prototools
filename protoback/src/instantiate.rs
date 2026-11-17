// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `protoback instantiate` (spec 0432): reproto-instantiate-schema's
//! pseudo-random instances, draw for draw, on upb's definitions.

mod pyrandom;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use prototext_upb::{
    Cardinality, DynamicMessage, Field, Kind, Message, MessageMut, Pool, Scalar, Value,
};
use sha2::{Digest, Sha256};

use pyrandom::Random;

#[derive(Parser, Debug)]
#[command(
    name = "protoback instantiate",
    version,
    about = "Generate pseudo-random binary protobuf instances from a .desc FileDescriptorSet.",
    long_about = "Generate pseudo-random binary protobuf instances from a .desc FileDescriptorSet.

FQDNS are fully-qualified message type names.  For each FQDN an output file
is written to <output-root>/<fqdn-with-dots-as-slashes>.pb."
)]
pub struct Cli {
    #[arg(
        long = "descriptor-set",
        help = "Path to the .desc FileDescriptorSet to load. [env: PROTOTEXT_DESCRIPTOR_SET]"
    )]
    descriptor_set: Option<PathBuf>,
    #[arg(
        long = "descriptor",
        hide = true,
        help = "Deprecated alias for --descriptor-set."
    )]
    descriptor_deprecated: Option<PathBuf>,
    #[arg(
        short = 'O',
        long = "output-root",
        help = "Root directory for output files (default: current directory)."
    )]
    output_root: Option<PathBuf>,
    #[arg(
        long = "seed",
        default_value_t = 0,
        allow_negative_numbers = true,
        help = "PRNG seed (default: 0)."
    )]
    seed: i64,
    #[arg(
        long = "max-depth",
        default_value_t = 4,
        allow_negative_numbers = true,
        help = "Maximum recursion depth for nested messages (default: 4)."
    )]
    max_depth: i64,
    #[arg(
        long = "max-repeated",
        default_value_t = 3,
        allow_negative_numbers = true,
        help = "Maximum number of elements for repeated fields (default: 3)."
    )]
    max_repeated: i64,
    #[arg(
        short = 'q',
        long = "quiet",
        help = "Suppress per-file progress messages."
    )]
    quiet: bool,
    #[arg(required = true)]
    fqdns: Vec<String>,
}

/// The command, for its man page.
pub fn command() -> clap::Command {
    <Cli as clap::CommandFactory>::command()
}

fn usage(message: &str) -> ExitCode {
    eprintln!("Usage: protoback instantiate [OPTIONS] FQDNS...");
    eprintln!("Try 'protoback instantiate --help' for help.");
    eprintln!();
    eprintln!("Error: {message}");
    ExitCode::from(2)
}

/// `instantiate_cli.main`.
pub fn main(args: Vec<String>) -> ExitCode {
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(if e.use_stderr() { 2 } else { 0 });
        }
    };
    let mut path = cli.descriptor_set.clone();
    if let Some(old) = &cli.descriptor_deprecated {
        if !cli.quiet {
            eprintln!("Warning [W18]: --descriptor is deprecated; use --descriptor-set");
        }
        if path.is_none() {
            path = Some(old.clone());
        }
    }
    for (option, p) in [
        ("--descriptor-set", &cli.descriptor_set),
        ("--descriptor", &cli.descriptor_deprecated),
    ] {
        if let Some(p) = p {
            if !p.exists() {
                return usage(&format!(
                    "Invalid value for '{option}': File '{}' does not exist.",
                    p.display()
                ));
            }
            if p.is_dir() {
                return usage(&format!(
                    "Invalid value for '{option}': File '{}' is a directory.",
                    p.display()
                ));
            }
        }
    }
    let path = match path {
        Some(p) => p,
        None => {
            let mut env = std::env::var("PROTOTEXT_DESCRIPTOR_SET").ok();
            if env.is_none() {
                if let Ok(old) = std::env::var("PROTOTEXT_DEFAULT_DESCRIPTOR") {
                    if !cli.quiet {
                        eprintln!(
                            "Warning [W18]: PROTOTEXT_DEFAULT_DESCRIPTOR is deprecated; \
                             use PROTOTEXT_DESCRIPTOR_SET"
                        );
                    }
                    env = Some(old);
                }
            }
            match env {
                Some(p) => PathBuf::from(p),
                None => {
                    return usage(
                        "No descriptor specified. Use --descriptor-set or set \
                         PROTOTEXT_DESCRIPTOR_SET.",
                    )
                }
            }
        }
    };
    let pool = match load_pool(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}: {e}", path.display());
            return ExitCode::from(1);
        }
    };
    let options = Options {
        seed: cli.seed,
        max_depth: cli.max_depth,
        max_repeated: cli.max_repeated,
    };
    for fqdn in &cli.fqdns {
        let Some(wire) = generate_instance(fqdn, &pool, &options) else {
            eprintln!("Error: type not found in descriptor: \"Couldn't find message {fqdn}\"");
            return ExitCode::from(1);
        };
        if wire.is_empty() {
            continue;
        }
        let out = output_path(cli.output_root.as_deref(), fqdn);
        if let Some(parent) = out.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("Error: {}: {e}", parent.display());
                return ExitCode::from(1);
            }
        }
        if let Err(e) = std::fs::write(&out, wire) {
            eprintln!("Error: {}: {e}", out.display());
            return ExitCode::from(1);
        }
        if !cli.quiet {
            println!("{}", out.display());
        }
    }
    ExitCode::SUCCESS
}

/// `root / Path(*fqdn.split('.'))`, `.pb` its last part's suffix, as
/// Python's `Path` writes it: empty parts and a `.` root dropped.
fn output_path(root: Option<&Path>, fqdn: &str) -> PathBuf {
    let mut out = PathBuf::new();
    if let Some(r) = root {
        out.push(r);
    }
    for part in fqdn.split('.').filter(|p| !p.is_empty()) {
        out.push(part);
    }
    out.set_extension("pb");
    out
}

/// `load_pool`: the set into a fresh pool, its own descriptor.proto used
/// (spec 0432 S3), the vendored one added only for a set without one; a
/// file the pool refuses skipped.
fn load_pool(path: &Path) -> Result<Pool, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .expect("the vendored descriptor.proto declares it");
    let set = DynamicMessage::decode(&def, &data).map_err(|e| e.to_string())?;
    let files = prototext_upb::descriptor_access::FileDescriptorSet(set.as_ref()).file();
    let pool = if files
        .iter()
        .any(|f| f.name() == b"google/protobuf/descriptor.proto")
    {
        Pool::new()
    } else {
        Pool::with_descriptor_proto()
    };
    for f in files {
        if let Ok(bytes) = f.0.encode() {
            let _ = pool.add_file(&bytes);
        }
    }
    Ok(pool)
}

pub struct Options {
    pub seed: i64,
    pub max_depth: i64,
    pub max_repeated: i64,
}

/// `_make_rng`: `random.Random` seeded with the SHA-256 of
/// `"SEED:.FQDN"`, read as a big-endian integer.
fn make_rng(seed: i64, fqdn: &str) -> Random {
    let dotted = if fqdn.starts_with('.') {
        fqdn.to_string()
    } else {
        format!(".{fqdn}")
    };
    let mut digest: Vec<u8> = Sha256::digest(format!("{seed}:{dotted}").as_bytes()).to_vec();
    digest.reverse();
    Random::from_int_le(&digest)
}

/// `generate_instance`: the instance's bytes, serialized deterministically
/// (spec 0432 S5); `None` when the pool has no such type.
pub fn generate_instance(fqdn: &str, pool: &Pool, o: &Options) -> Option<Vec<u8>> {
    let desc = pool.get_message_by_name(fqdn)?;
    let mut rng = make_rng(o.seed, fqdn);
    let msg = Walk { rng: &mut rng, o }.message(&desc, 0);
    msg.encode_deterministic().ok()
}

/// A generated value: a scalar, or a message.
enum Generated {
    Scalar(OwnedScalar),
    Message(DynamicMessage),
}

enum OwnedScalar {
    Bool(bool),
    I32(i32),
    I64(i64),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Enum(i32),
    Bytes(Vec<u8>),
}

impl OwnedScalar {
    fn as_scalar(&self) -> Scalar<'_> {
        match self {
            OwnedScalar::Bool(v) => Scalar::Bool(*v),
            OwnedScalar::I32(v) => Scalar::I32(*v),
            OwnedScalar::I64(v) => Scalar::I64(*v),
            OwnedScalar::U32(v) => Scalar::U32(*v),
            OwnedScalar::U64(v) => Scalar::U64(*v),
            OwnedScalar::F32(v) => Scalar::F32(*v),
            OwnedScalar::F64(v) => Scalar::F64(*v),
            OwnedScalar::Enum(v) => Scalar::Enum(*v),
            OwnedScalar::Bytes(v) => Scalar::Bytes(v),
        }
    }

    /// A map entry's key or value as read back: what is set, or the default.
    fn from_value(v: Value<'_>) -> Option<OwnedScalar> {
        Some(match v {
            Value::Bool(b) => OwnedScalar::Bool(b),
            Value::I32(x) => OwnedScalar::I32(x),
            Value::I64(x) => OwnedScalar::I64(x),
            Value::U32(x) => OwnedScalar::U32(x),
            Value::U64(x) => OwnedScalar::U64(x),
            Value::F32(x) => OwnedScalar::F32(x),
            Value::F64(x) => OwnedScalar::F64(x),
            Value::Enum(x) => OwnedScalar::Enum(x),
            Value::Bytes(b) => OwnedScalar::Bytes(b.to_vec()),
            _ => return None,
        })
    }
}

struct Walk<'r> {
    rng: &'r mut Random,
    o: &'r Options,
}

impl Walk<'_> {
    /// `_p_at_depth`.
    fn p(&self, depth: i64) -> f64 {
        if self.o.max_depth == 0 {
            return 0.0;
        }
        (self.o.max_depth - depth) as f64 / self.o.max_depth as f64
    }

    /// `_random_value`: `None` for a message at the maximum depth.
    fn value(&mut self, field: &Field, depth: i64) -> Option<Generated> {
        let r = &mut *self.rng;
        let int = |r: &mut Random| r.randint(1, 1000);
        Some(Generated::Scalar(match field.kind() {
            Kind::Message(sub) => {
                if depth >= self.o.max_depth {
                    return None;
                }
                return Some(Generated::Message(self.message(&sub, depth + 1)));
            }
            Kind::Enum(e) => {
                let values: Vec<i32> = e.values().map(|v| v.number()).collect();
                let non_zero: Vec<i32> = values.iter().copied().filter(|&v| v != 0).collect();
                let pick = if non_zero.is_empty() {
                    &values
                } else {
                    &non_zero
                };
                OwnedScalar::Enum(pick[r.choice(pick.len())])
            }
            Kind::Bool => OwnedScalar::Bool(true),
            Kind::String => OwnedScalar::Bytes(format!("s{}", r.randint(1, 9999)).into_bytes()),
            Kind::Bytes => {
                let n = r.randint(1, 8);
                OwnedScalar::Bytes((0..n).map(|_| r.randint(0, 255) as u8).collect())
            }
            Kind::Float => OwnedScalar::F32(r.uniform(1.0, 1000.0) as f32),
            Kind::Double => OwnedScalar::F64(r.uniform(1.0, 1000.0)),
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => OwnedScalar::I32(int(r) as i32),
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => OwnedScalar::I64(int(r)),
            Kind::Uint32 | Kind::Fixed32 => OwnedScalar::U32(int(r) as u32),
            Kind::Uint64 | Kind::Fixed64 => OwnedScalar::U64(int(r) as u64),
        }))
    }

    /// A singular value set: a message (a group included, spec 0432 S6)
    /// by `CopyFrom`, a scalar by assignment.
    fn set(w: &mut MessageMut<'_>, field: &Field, v: &Generated) {
        match v {
            Generated::Message(m) => {
                if let (Ok(mut sub), Ok(bytes)) = (w.message_mut(field), m.encode()) {
                    let _ = sub.merge_from(&bytes);
                }
            }
            Generated::Scalar(s) => {
                let _ = w.set(field, s.as_scalar());
            }
        }
    }

    /// `_generate_message`.
    fn message(&mut self, desc: &Message, depth: i64) -> DynamicMessage {
        let mut msg = DynamicMessage::new(desc);
        let p = self.p(depth);
        let mut in_oneof = crate::hash::HashSet::default();
        {
            let mut w = msg.as_mut();
            for oneof in desc.oneofs() {
                let fields: Vec<Field> = oneof.fields().collect();
                // reproto takes a oneof as synthetic by its shape and name.
                if fields.len() == 1 && oneof.name() == format!("_{}", fields[0].name()) {
                    continue;
                }
                in_oneof.extend(fields.iter().map(Field::number));
                if self.rng.random() >= p {
                    continue;
                }
                let chosen = &fields[self.rng.choice(fields.len())];
                if let Some(v) = self.value(chosen, depth) {
                    Self::set(&mut w, chosen, &v);
                }
            }
            for field in desc.fields() {
                if in_oneof.contains(&field.number()) {
                    continue;
                }
                match field.cardinality() {
                    Cardinality::Repeated => {
                        let max_count =
                            ((p * self.o.max_repeated as f64).round_ties_even() as i64).max(1);
                        let min_count = i64::from(depth == 0);
                        let count = self.rng.randint(min_count, max_count);
                        for _ in 0..count {
                            let Some(v) = self.value(&field, depth) else {
                                continue;
                            };
                            Self::push(&mut w, &field, v);
                        }
                    }
                    Cardinality::Required => {
                        if let Some(v) = self.value(&field, depth) {
                            Self::set(&mut w, &field, &v);
                        }
                    }
                    Cardinality::Optional => {
                        if self.rng.random() < p {
                            if let Some(v) = self.value(&field, depth) {
                                Self::set(&mut w, &field, &v);
                            }
                        }
                    }
                }
            }
        }
        msg
    }

    /// One element of a repeated field: a map entry assigned by its key (a
    /// key not generated is the default; a key again replaces its value),
    /// a message appended by `CopyFrom`, a scalar appended.
    fn push(w: &mut MessageMut<'_>, field: &Field, v: Generated) {
        match v {
            Generated::Message(entry) if field.is_map() => {
                let r = entry.as_ref();
                let def = entry.descriptor();
                let (Some(kf), Some(vf)) = (def.get_field(1), def.get_field(2)) else {
                    return;
                };
                let Some(key) = OwnedScalar::from_value(r.get(&kf)) else {
                    return;
                };
                match r.get(&vf) {
                    Value::Message(m) => {
                        let bytes = m.and_then(|m| m.encode().ok()).unwrap_or_default();
                        if let Ok(mut sub) = w.insert_message(field, key.as_scalar()) {
                            let _ = sub.merge_from(&bytes);
                        }
                    }
                    other => {
                        if let Some(value) = OwnedScalar::from_value(other) {
                            let _ = w.insert(field, key.as_scalar(), value.as_scalar());
                        }
                    }
                }
            }
            Generated::Message(m) => {
                if let (Ok(mut sub), Ok(bytes)) = (w.push_message(field), m.encode()) {
                    let _ = sub.merge_from(&bytes);
                }
            }
            Generated::Scalar(s) => {
                let _ = w.push(field, s.as_scalar());
            }
        }
    }
}
