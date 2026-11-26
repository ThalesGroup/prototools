// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! --emit-extension-ranges and its canonical form (spec 0238 S2, S3).
//!
//! A message declaring `extensions 1000 to max` tells the scorer that an
//! unrecognized field number in that band is legal, not evidence against
//! the type. The emitted form is unique for a given set of field numbers
//! (spec 0238 S5 interns range sets): the fixture spells one set,
//! 1000..2999, four ways, which must land on one canonical list.
//!
//! Ported from `test_emit_extension_ranges.py`, its command-line tests; the
//! YAML read with `serde_yaml`, where it used PyYAML.

use crate::common::{compile_proto, fixture, protoback, tmp, tools};
use serde_yaml::Value;

/// 2**29 - 1, the largest legal field number, which protoc has already
/// materialized `max` into.
const MAX_FIELD_NUMBER: i64 = 536_870_911;

/// extension_ranges.proto compiled and emitted with `extra`; its YAML's
/// messages.
fn messages(test: &str, extra: &[&str]) -> Option<Value> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let pbs = compile_proto(&pb, &["extension_ranges.proto"]);
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", fixture("").display()),
        format!("--proto-out={}", out.display()),
        "--emit-scoring-yaml".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args.push(pbs[0].display().to_string());
    let r = protoback(&args);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let data: Value =
        serde_yaml::from_str(&std::fs::read_to_string(out.join("extension_ranges.yaml")).unwrap())
            .unwrap();
    assert!(data["messages"].is_mapping());
    Some(data["messages"].clone())
}

fn ranges(v: &[[i64; 2]]) -> Value {
    serde_yaml::to_value(v).unwrap()
}

/// Without the flag the YAML is what it was before spec 0238.
#[test]
fn test_flag_off_emits_no_ext_ranges_key() {
    let Some(m) = messages("test_flag_off_emits_no_ext_ranges_key", &[]) else {
        return;
    };
    let m = m.as_mapping().unwrap();
    assert!(!m.is_empty(), "fixture should have produced messages");
    for (name, body) in m {
        assert!(
            body.get("ext_ranges").is_none(),
            "{name:?} carries ext_ranges without the flag"
        );
    }
}

#[test]
fn test_flag_on_records_the_declared_band() {
    let Some(m) = messages(
        "test_flag_on_records_the_declared_band",
        &["--emit-extension-ranges"],
    ) else {
        return;
    };
    assert_eq!(
        m["extranges.OneClause"]["ext_ranges"],
        ranges(&[[1000, 2999]])
    );
}

/// A message with no `extensions` clause stays closed by absence: never an
/// empty list (spec 0238 S5).
#[test]
fn test_closed_message_gets_no_key() {
    let Some(m) = messages(
        "test_closed_message_gets_no_key",
        &["--emit-extension-ranges"],
    ) else {
        return;
    };
    assert!(m["extranges.Closed"].get("ext_ranges").is_none());
}

/// Four spellings of 1000..2999, one list; adjacency the interesting one.
#[test]
fn test_every_spelling_of_one_set_canonicalizes_alike() {
    let Some(m) = messages(
        "test_every_spelling_of_one_set_canonicalizes_alike",
        &["--emit-extension-ranges"],
    ) else {
        return;
    };
    for name in [
        "extranges.OneClause",
        "extranges.TwoAdjacentClauses",
        "extranges.OutOfOrderClauses",
        "extranges.SingleNumberClause",
    ] {
        assert_eq!(m[name]["ext_ranges"], ranges(&[[1000, 2999]]), "{name}");
    }
}

/// `to max` comes out as the number, never a sentinel.
#[test]
fn test_to_max_is_materialized_not_symbolic() {
    let Some(m) = messages(
        "test_to_max_is_materialized_not_symbolic",
        &["--emit-extension-ranges"],
    ) else {
        return;
    };
    assert_eq!(
        m["extranges.Unbounded"]["ext_ranges"],
        ranges(&[[1000, MAX_FIELD_NUMBER]])
    );
}

/// A message set's `max` arrives as 2147483647 and leaves as 2**29 - 1.
#[test]
fn test_a_message_sets_max_is_clamped() {
    let Some(m) = messages(
        "test_a_message_sets_max_is_clamped",
        &["--emit-extension-ranges"],
    ) else {
        return;
    };
    assert_eq!(
        m["extranges.Set"]["ext_ranges"],
        ranges(&[[4, MAX_FIELD_NUMBER]])
    );
}
