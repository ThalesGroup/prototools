// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `protoback instantiate`'s instances (spec 0432 S5, S6): serialized
//! deterministically, the same from one process to the next, though upb
//! seeds its string hash per process.
//!
//! Ported from `test_instantiate.py`, its command-line test: the original
//! drove `generate_instance` in eight fresh Python processes, the port runs
//! `protoback instantiate` eight times, with the same seed and bound. Its
//! unit tests are in `protoback/src/instantiate.rs`, on the functions that
//! port reproto's. The schema its code built with Python's protobuf is the
//! `#@prototext` fixture `instantiate/inst.pb`.

use crate::common::{encode_fixture, protoback, tmp};
use std::collections::BTreeSet;

#[test]
fn test_instances_are_reproducible() {
    let dir = tmp("test_instances_are_reproducible");
    let desc = dir.join("inst.desc");
    encode_fixture(
        "instantiate/inst.pb",
        "b6c5a886f00832d0b529633f476019d5a2ceae106d9f2187b7a27a7b8bc6f9e8",
        &desc,
    );
    let mut runs = BTreeSet::new();
    for i in 0..8 {
        let out = dir.join(format!("out-{i}"));
        let r = protoback([
            "instantiate".into(),
            "-q".into(),
            format!("--descriptor-set={}", desc.display()),
            format!("--output-root={}", out.display()),
            "--seed=3".into(),
            "--max-repeated=8".into(),
            "inst.M".into(),
        ]);
        assert_eq!(r.code, Some(0), "{}", r.stderr);
        runs.insert(std::fs::read(out.join("inst/M.pb")).unwrap());
    }
    assert_eq!(
        runs.len(),
        1,
        "{} different instances over 8 runs",
        runs.len()
    );
}
