// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The wrapper against real descriptor sets (spec 0415, test plan 1, 2
//! and 4).

use prototext_upb::{Cardinality, Edition, Kind, Pool};

/// `tests/fixtures/editions2024.proto` and the 2023 file it imports,
/// compiled by the pinned protoc with `--include_imports`.
const EDITIONS: &[u8] = include_bytes!("fixtures/editions.pb");

// ── Feature resolution (test plan 1) ────────────────────────────────────────

/// What each fixture field resolves to, as four letters: packed (`P`),
/// tracks presence (`H`), group-framed (`G`), and cardinality (`O`ptional,
/// `Q` required, `R`epeated); `-` for false. The same table Python's
/// protobuf (upb) reports for the same set, recorded when the fixture was
/// written.
const RESOLVED: &[(&str, &str)] = &[
    ("ed2023.Fields.expanded_from_file", "---R"),
    ("ed2023.Fields.packed_on_field", "P--R"),
    ("ed2023.Fields.delimited", "-HGO"),
    ("ed2023.Fields.length_prefixed", "-H-O"),
    ("ed2023.Fields.legacy_required", "-H-Q"),
    ("ed2023.Fields.implicit_presence", "---O"),
    ("ed2023.Fields.explicit_presence", "-H-O"),
    ("ed2023.Fields.open_enum", "-H-O"),
    ("ed2023.Fields.closed_enum", "-H-O"),
    ("ed2023.PackedMessage.packed_from_message", "P--R"),
    ("ed2023.PackedMessage.expanded_on_field", "---R"),
    ("ed2024.Holder.delimited_from_file", "-HGO"),
    ("ed2024.Holder.length_prefixed_on_field", "-H-O"),
    ("ed2024.Holder.packed_default", "P--R"),
    ("ed2024.Holder.imported_closed_enum", "-H-O"),
];

fn resolution(f: &prototext_upb::Field) -> String {
    let flag = |on: bool, c: char| if on { c } else { '-' };
    let cardinality = match f.cardinality() {
        Cardinality::Optional => 'O',
        Cardinality::Required => 'Q',
        Cardinality::Repeated => 'R',
    };
    [
        flag(f.is_packed(), 'P'),
        flag(f.has_presence(), 'H'),
        flag(f.is_group(), 'G'),
        cardinality,
    ]
    .iter()
    .collect()
}

#[test]
fn editions_files_load_with_their_features_resolved() {
    let pool = Pool::new();
    let report = pool.load_set(EDITIONS);
    assert_eq!(report.loaded, ["editions2023.proto", "editions2024.proto"]);
    assert!(report.skipped.is_empty() && report.set_error.is_none());

    for &(path, expected) in RESOLVED {
        let (message, field) = path.rsplit_once('.').unwrap();
        let m = pool.get_message_by_name(message).expect(message);
        let f = m.get_field_by_name(field).expect(field);
        assert_eq!(resolution(&f), expected, "{path}");
    }
}

#[test]
fn files_report_their_edition() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    assert_eq!(
        pool.file("editions2023.proto").unwrap().edition(),
        Edition::Edition2023
    );
    assert_eq!(
        pool.file("editions2024.proto").unwrap().edition(),
        Edition::Edition2024
    );
}

#[test]
fn enums_are_open_or_closed_as_resolved() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    let fields = pool.get_message_by_name("ed2023.Fields").unwrap();
    let closed = |name: &str| match fields.get_field_by_name(name).unwrap().kind() {
        Kind::Enum(e) => e.is_closed(),
        other => panic!("{name}: {other:?}"),
    };
    assert!(!closed("open_enum"));
    assert!(closed("closed_enum"));
}

#[test]
fn a_delimited_field_is_a_message_of_group_kind() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    let holder = pool.get_message_by_name("ed2024.Holder").unwrap();
    let f = holder.get_field(1).unwrap();
    assert!(f.is_group());
    match f.kind() {
        Kind::Message(m) => assert_eq!(m.full_name(), "ed2023.Inner"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_extension_is_found_by_extendee_and_number() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    let inner = pool.get_message_by_name("ed2023.Inner").unwrap();
    let ext = inner.get_extension(100).expect("extension 100");
    assert_eq!(ext.full_name(), "ed2024.delimited_extension");
    assert!(ext.is_extension());
    assert!(ext.is_group(), "delimited by the 2024 file's default");
    assert_eq!(ext.containing_message(), inner);
    assert!(inner.get_extension(101).is_none());
    assert!(inner.get_extension(u32::MAX).is_none());
}

#[test]
fn names_and_scopes_are_upbs() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    let fields = pool.get_message_by_name("ed2023.Fields").unwrap();
    assert_eq!(fields.name(), "Fields");
    assert_eq!(fields.parent_file().name(), "editions2023.proto");
    assert_eq!(fields.parent_file().package_name(), "ed2023");
    let f = fields.get_field(7).unwrap();
    assert_eq!(
        (f.name(), f.full_name()),
        ("explicit_presence", "ed2023.Fields.explicit_presence")
    );
    let e = pool.get_enum_by_name("ed2023.ClosedEnum").unwrap();
    let v = e.get_value(1).unwrap();
    assert_eq!(
        (v.name(), v.number(), v.full_name()),
        ("CLOSED_ONE", 1, "ed2023.CLOSED_ONE")
    );
    assert_eq!(v.parent_enum(), e);
    assert!(e.get_value(2).is_none());
}

#[test]
fn listing_walks_every_message_and_enum() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    let messages: Vec<String> = pool
        .all_messages()
        .iter()
        .map(|m| m.full_name().to_string())
        .collect();
    assert_eq!(
        messages,
        [
            "ed2023.Inner",
            "ed2023.Fields",
            "ed2023.PackedMessage",
            "ed2024.Holder"
        ]
    );
    let enums: Vec<String> = pool
        .all_enums()
        .iter()
        .map(|e| e.full_name().to_string())
        .collect();
    assert_eq!(enums, ["ed2023.OpenEnum", "ed2023.ClosedEnum"]);
}

#[test]
fn a_file_keeps_its_original_bytes() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    let bytes = pool.file_bytes("editions2023.proto").unwrap();
    assert!(bytes.windows(18).any(|w| w == b"editions2023.proto"));
    assert!(pool.file_bytes("absent.proto").is_none());
}

// ── Loading never panics (test plan 2) ──────────────────────────────────────

/// A serialized `FileDescriptorProto` with only a name and imports, and
/// optionally `syntax` and `edition`: enough for upb to build a file.
fn fdp(name: &str, imports: &[&str], edition: Option<(&str, u64)>) -> Vec<u8> {
    fn len_field(out: &mut Vec<u8>, number: u8, payload: &[u8]) {
        out.push(number << 3 | 2);
        out.push(payload.len() as u8);
        out.extend_from_slice(payload);
    }
    let mut out = Vec::new();
    len_field(&mut out, 1, name.as_bytes());
    for i in imports {
        len_field(&mut out, 3, i.as_bytes());
    }
    if let Some((syntax, edition)) = edition {
        len_field(&mut out, 12, syntax.as_bytes());
        out.push(14 << 3);
        let mut v = edition;
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
    out
}

fn set(files: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in files {
        out.push(1 << 3 | 2);
        let mut len = f.len();
        loop {
            let byte = (len & 0x7f) as u8;
            len >>= 7;
            if len == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out.extend_from_slice(f);
    }
    out
}

fn skipped(report: &prototext_upb::LoadReport) -> Vec<&str> {
    report.skipped.iter().map(|(f, _)| f.as_str()).collect()
}

#[test]
fn a_missing_import_skips_the_file_and_its_dependents_only() {
    let pool = Pool::new();
    let report = pool.load_set(&set(&[
        fdp("a.proto", &["missing.proto"], None),
        fdp("b.proto", &["a.proto"], None),
        fdp("c.proto", &[], None),
    ]));
    assert_eq!(report.loaded, ["c.proto"]);
    assert_eq!(skipped(&report), ["a.proto", "b.proto"]);
    assert!(report.skipped[0].1.contains("missing.proto"));
    assert!(report.skipped[1].1.contains("a.proto"));
}

#[test]
fn imports_are_added_first_whatever_the_set_order() {
    let pool = Pool::new();
    let report = pool.load_set(&set(&[
        fdp("top.proto", &["mid.proto"], None),
        fdp("mid.proto", &["base.proto"], None),
        fdp("base.proto", &[], None),
    ]));
    assert_eq!(report.loaded, ["top.proto", "mid.proto", "base.proto"]);
    let added: Vec<String> = pool.files().iter().map(|f| f.name().to_string()).collect();
    assert_eq!(added, ["base.proto", "mid.proto", "top.proto"]);
}

#[test]
fn an_import_cycle_skips_every_file_on_it() {
    let pool = Pool::new();
    let report = pool.load_set(&set(&[
        fdp("a.proto", &["b.proto"], None),
        fdp("b.proto", &["a.proto"], None),
        fdp("c.proto", &[], None),
    ]));
    assert_eq!(report.loaded, ["c.proto"]);
    assert_eq!(skipped(&report), ["a.proto", "b.proto"]);
}

#[test]
fn a_duplicate_file_name_keeps_the_first() {
    let pool = Pool::new();
    let report = pool.load_set(&set(&[
        fdp("a.proto", &[], None),
        fdp("a.proto", &[], None),
    ]));
    assert_eq!(report.loaded, ["a.proto"]);
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].1.contains("duplicate"));
}

#[test]
fn unknown_and_future_editions_are_skipped() {
    let pool = Pool::new();
    let report = pool.load_set(&set(&[
        fdp("unknown.proto", &[], Some(("editions", 0))),
        fdp("future.proto", &[], Some(("editions", 99_999))),
        fdp("ok.proto", &[], Some(("editions", 1000))),
    ]));
    assert_eq!(report.loaded, ["ok.proto"]);
    assert_eq!(skipped(&report), ["unknown.proto", "future.proto"]);
}

#[test]
fn a_gate_refusal_skips_the_file() {
    let pool = Pool::with_gate(|bytes| {
        if bytes.windows(4).any(|w| w == b"evil") {
            Err("refused by the gate".into())
        } else {
            Ok(())
        }
    });
    let report = pool.load_set(&set(&[
        fdp("evil.proto", &[], None),
        fdp("good.proto", &[], None),
    ]));
    assert_eq!(report.loaded, ["good.proto"]);
    assert_eq!(
        report.skipped,
        [("evil.proto".to_string(), "refused by the gate".to_string())]
    );
}

#[test]
fn a_file_with_no_readable_name_is_reported_by_index() {
    let pool = Pool::new();
    let report = pool.load_set(&set(&[vec![], fdp("ok.proto", &[], None)]));
    assert_eq!(report.loaded, ["ok.proto"]);
    assert_eq!(skipped(&report), ["#0"]);
}

#[test]
fn bytes_that_are_not_a_set_report_it() {
    let pool = Pool::new();
    let report = pool.load_set(b"\x0a\x05abc");
    assert!(report.set_error.is_some());
    assert!(report.loaded.is_empty() && pool.files().is_empty());
}

#[test]
fn random_bytes_never_panic() {
    // A fixed xorshift stream: reproducible, and wide enough to reach
    // upb's parser with well-framed garbage.
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..2000 {
        let len = (next() % 200) as usize;
        let payload: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let pool = Pool::new();
        // As a whole set, and as one file framed in a set.
        pool.load_set(&payload);
        pool.load_set(&set(&[payload]));
    }
}

#[test]
fn a_failed_add_leaves_the_pool_unchanged() {
    let pool = Pool::new();
    pool.add_file(&fdp("a.proto", &[], None)).unwrap();
    assert!(pool.add_file(&fdp("a.proto", &[], None)).is_err());
    assert!(pool
        .add_file(&fdp("b.proto", &["missing.proto"], None))
        .is_err());
    assert_eq!(pool.files().len(), 1);
    assert!(pool.file("b.proto").is_none());
}

// ── Handles (test plan 4) ───────────────────────────────────────────────────

#[test]
fn handles_survive_later_adds_and_outlive_the_pool_value() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    let inner = pool.get_message_by_name("ed2023.Inner").unwrap();
    for i in 0..200 {
        pool.add_file(&fdp(&format!("f{i}.proto"), &[], None))
            .unwrap();
    }
    assert_eq!(inner.full_name(), "ed2023.Inner");
    assert_eq!(pool.get_message_by_name("ed2023.Inner").unwrap(), inner);
    drop(pool);
    // The handle keeps the pool alive.
    assert_eq!(inner.get_field(1).unwrap().name(), "a");
}

#[test]
fn lookups_and_adds_run_from_several_threads() {
    let pool = Pool::new();
    pool.load_set(EDITIONS);
    let inner = pool.get_message_by_name("ed2023.Inner").unwrap();
    std::thread::scope(|s| {
        for t in 0..4 {
            let pool = pool.clone();
            let inner = inner.clone();
            s.spawn(move || {
                for i in 0..200 {
                    if t == 0 {
                        pool.add_file(&fdp(&format!("t{i}.proto"), &[], None))
                            .unwrap();
                    }
                    assert!(pool.get_message_by_name("ed2024.Holder").is_some());
                    assert_eq!(inner.get_extension(100).unwrap().number(), 100);
                }
            });
        }
    });
    assert_eq!(pool.files().len(), 2 + 200);
}

// ── Mutations of a real set (test plan 3, on stable Rust) ───────────────────

/// Walk everything a pool loaded, asking each definition every question
/// the wrapper exposes, so a definition upb built from damaged bytes is
/// exercised and not merely created.
fn walk(pool: &Pool) -> usize {
    let mut seen = 0;
    for m in pool.all_messages() {
        seen += m.full_name().len() + m.oneofs().count();
        for f in m.fields() {
            let _ = (f.kind(), f.cardinality(), f.is_packed(), f.has_presence());
            let _ = (f.is_group(), f.containing_oneof(), f.json_name());
            seen += f.number() as usize & 1;
        }
        for n in [1, 2, 100, 536_870_911] {
            let _ = m.get_extension(n);
        }
    }
    for e in pool.all_enums() {
        seen += e.values().map(|v| v.number() as usize & 1).sum::<usize>();
        let _ = e.is_closed();
    }
    seen
}

#[test]
fn mutated_sets_load_or_report_and_never_crash() {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut loaded = 0;
    for _ in 0..20_000 {
        let mut bytes = EDITIONS.to_vec();
        for _ in 0..1 + next() % 4 {
            if bytes.is_empty() {
                break;
            }
            let at = (next() as usize) % bytes.len();
            match next() % 4 {
                0 => bytes[at] ^= 1 << (next() % 8),
                1 => bytes.insert(at, next() as u8),
                2 if bytes.len() > 1 => {
                    bytes.remove(at);
                }
                _ => bytes.truncate(at),
            }
        }
        let pool = Pool::new();
        let report = pool.load_set(&bytes);
        loaded += report.loaded.len();
        walk(&pool);
    }
    // Most mutations damage something, but not all: some files load, so
    // the walk above exercised definitions upb built from damaged sets.
    assert!(loaded > 0);
}
