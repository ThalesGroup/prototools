// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Pseudo-random protobuf instance generator for `prototext instantiate-schema`.
//!
//! Implements spec 0056 §"prototext instantiate-schema".

use prost::bytes::Bytes;
use prost::Message as ProstMessage;
use prost_reflect::{Cardinality, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, Value};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};

// ── Well-known types left empty in v1 ────────────────────────────────────────

const UNSUPPORTED_WKTS: &[&str] = &[
    "google.protobuf.Any",
    "google.protobuf.Struct",
    "google.protobuf.Value",
    "google.protobuf.ListValue",
];

// ── Public entry point ────────────────────────────────────────────────────────

/// Generation parameters.
pub struct InstantiateOpts {
    /// User-visible integer seed (recorded in `# seed:` comment).
    pub seed: i64,
    /// Maximum recursion depth for nested messages (default 4).
    pub max_depth: usize,
    /// Maximum number of elements for repeated fields (default 3).
    pub max_repeated: usize,
    /// Probability [0,1] of populating an optional field (default 0.7).
    pub p_optional: f64,
    /// Suppress warnings to stderr.
    pub quiet: bool,
}

impl Default for InstantiateOpts {
    fn default() -> Self {
        InstantiateOpts {
            seed: 0,
            max_depth: 4,
            max_repeated: 3,
            p_optional: 0.7,
            quiet: false,
        }
    }
}

/// Generate a pseudo-random binary protobuf for `descriptor`.
///
/// The effective PRNG seed is `SHA256("<seed>:<fqdn>")` → `StdRng::from_seed`.
/// Returns the raw wire bytes.
pub fn generate_message_bytes(descriptor: &MessageDescriptor, opts: &InstantiateOpts) -> Vec<u8> {
    let fqdn = format!(".{}", descriptor.full_name());
    let seed_input = format!("{}:{}", opts.seed, fqdn);
    let hash = Sha256::digest(seed_input.as_bytes());
    let mut seed_bytes = [0u8; 32];
    seed_bytes.copy_from_slice(&hash);
    let mut rng = StdRng::from_seed(seed_bytes);

    let msg = generate_message(descriptor, &mut rng, 0, opts);
    msg.encode_to_vec()
}

// ── Recursive message generator ───────────────────────────────────────────────

fn generate_message(
    descriptor: &MessageDescriptor,
    rng: &mut StdRng,
    depth: usize,
    opts: &InstantiateOpts,
) -> DynamicMessage {
    let mut msg = DynamicMessage::new(descriptor.clone());

    if UNSUPPORTED_WKTS.contains(&descriptor.full_name()) {
        if !opts.quiet {
            eprintln!(
                "warning: leaving {} empty (unsupported WKT)",
                descriptor.full_name()
            );
        }
        return msg;
    }

    // Process oneofs first: for each oneof, decide once whether to populate it
    // and which field to use.  Track which fields are covered by a oneof.
    let mut oneof_field_numbers: std::collections::HashSet<u32> = std::collections::HashSet::new();

    for oneof in descriptor.oneofs() {
        // Skip synthetic oneofs (proto3 optional — treat the field as optional below).
        if oneof.is_synthetic() {
            continue;
        }
        // Mark all fields in this oneof as handled here.
        for f in oneof.fields() {
            oneof_field_numbers.insert(f.number());
        }
        // Decide whether to populate the oneof at all.
        if rng.gen::<f64>() > opts.p_optional {
            continue;
        }
        // Pick one member uniformly at random.
        let fields: Vec<FieldDescriptor> = oneof.fields().collect();
        let chosen = &fields[rng.gen_range(0..fields.len())];
        if let Some(value) = generate_value(chosen, rng, depth, opts) {
            msg.set_field(chosen, value);
        }
    }

    // Process non-oneof fields.
    for field in descriptor.fields() {
        if oneof_field_numbers.contains(&field.number()) {
            continue;
        }

        match field.cardinality() {
            Cardinality::Required => {
                if let Some(value) = generate_value(&field, rng, depth, opts) {
                    msg.set_field(&field, value);
                }
            }
            Cardinality::Repeated => {
                let count = rng.gen_range(0..=opts.max_repeated);
                if count == 0 {
                    continue;
                }
                let values: Vec<Value> = (0..count)
                    .filter_map(|_| generate_value(&field, rng, depth, opts))
                    .collect();
                if !values.is_empty() {
                    msg.set_field(&field, Value::List(values));
                }
            }
            Cardinality::Optional => {
                if rng.gen::<f64>() <= opts.p_optional {
                    if let Some(value) = generate_value(&field, rng, depth, opts) {
                        msg.set_field(&field, value);
                    }
                }
            }
        }
    }

    msg
}

// ── Value generator ───────────────────────────────────────────────────────────

fn generate_value(
    field: &FieldDescriptor,
    rng: &mut StdRng,
    depth: usize,
    opts: &InstantiateOpts,
) -> Option<Value> {
    match field.kind() {
        Kind::Message(msg_desc) => {
            if depth >= opts.max_depth {
                return None;
            }
            // Map fields: treat the synthetic entry message as a single repeated
            // message (count already handled by the caller for Repeated fields;
            // here we just generate one entry message).
            let nested = generate_message(&msg_desc, rng, depth + 1, opts);
            Some(Value::Message(nested))
        }
        Kind::Enum(enum_desc) => {
            let values: Vec<i32> = enum_desc.values().map(|v| v.number()).collect();
            let idx = rng.gen_range(0..values.len());
            Some(Value::EnumNumber(values[idx]))
        }
        Kind::Bool => Some(Value::Bool(rng.gen())),
        Kind::String => Some(Value::String(format!("s{}", rng.gen_range(0u32..10000)))),
        Kind::Bytes => {
            let len = rng.gen_range(0..=8usize);
            let b: Vec<u8> = (0..len).map(|_| rng.gen()).collect();
            Some(Value::Bytes(Bytes::from(b)))
        }
        Kind::Float => Some(Value::F32(rng.gen_range(0.0f32..1000.0))),
        Kind::Double => Some(Value::F64(rng.gen_range(0.0f64..1000.0))),
        // All integer kinds.
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Some(Value::I32(rng.gen_range(0..=1000))),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Some(Value::I64(rng.gen_range(0..=1000))),
        Kind::Uint32 | Kind::Fixed32 => Some(Value::U32(rng.gen_range(0..=1000))),
        Kind::Uint64 | Kind::Fixed64 => Some(Value::U64(rng.gen_range(0..=1000))),
    }
}
