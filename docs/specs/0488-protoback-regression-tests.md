<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0488 — protoback's tests: reproto's suite, ported to Rust against protoback

Status: implemented
Implemented in: 2026-11-26
App: protoback, nix
Refs: docs/specs/0487-retire-reproto.md (what this reverses in part: D3,
        the suite deleted; N2, the gap accepted),
      docs/specs/0424-protoback-reproto-in-rust-on-upb.md (the port; S2:
        the suite's `REPROTO` command, which ran it against protoback),
      docs/specs/0485-a-cargo-profile-argument-and-bounded-pytest.md
        (the tests' `quick` profile; `scripts/ci`)

## Background

Spec 0487 deleted reproto's test suite (D3) and accepted the coverage it
took with it (N2, "to be filled case by case if a regression shows it").
At 0.4.0 (`4da0f2d6`), the suite was:

- **66 test files, 344 test functions, 84 fixtures,** under
  `reproto/src/reproto/tests/`, some fixtures with the expected output
  beside them (`*.golden.proto`, `*.golden.pb`): regression tests of the
  rendering.
- **Run against protoback as well as reproto:** its command-line tests
  ran the command in `REPROTO` (`conftest.py`, spec 0424 S2), and
  `reprotoTestsOnProtoback` ran the whole suite with
  `REPROTO=protoback`.
- **18 of the 66 files test reproto's Python internals** (108 tests that
  call functions imported from `reproto.*`): unit tests of reproto's
  functions, beside their command-line tests. At 0.4.0 they tested reproto
  even when the suite ran with `REPROTO=protoback`.

protoback is left with its own 74 tests (27 unit, 47 integration). The
maintainer judged the gap too wide (2026-11-25): the suite is to be
resurrected and applied to protoback, its unit tests included where
protoback has the function they test, and ported to Rust if it can be,
the Python suite having been slow.

protoback is a port: its modules mirror reproto's, function for
function where the behavior is the same, and say so in their comments
(`variant.rs`: "reproto's `variant.py`", "reproto's `rule_matches`";
`profile.rs`: "as reproto's `profile.py` reads them"). That mapping is
what lets a unit test of reproto's carry over.

## Goals

- **G1. Every test of the suite that checks a behavior protoback has**
  runs against protoback: the command-line tests as they are, the unit
  tests against the protoback function that ports the one they tested.
- **G2. The suite's fixtures and goldens** come back with it, as its
  regression tests of the rendering.
- **G3. In Rust,** as integration tests (`protoback/tests/`) and unit
  tests (in the modules), so that the suite runs in `rustTests`, at
  `quick`, in seconds, with no Python where the test doesn't need it
  (the plugin tests' Python plugins excepted).
- **G4. In `ci`,** as the suite was until 0487.

## Non-goals

- **N1. No comparison with reproto,** in any form: reproto doesn't run,
  and no output of it serves as a reference beyond the goldens the suite
  already held.
- **N2. No test of a behavior protoback doesn't have:** a reproto-only
  feature, a Python API. Such tests are listed (S1), not ported.
- **N3. No change to protoback's behavior,** but where a resurrected test
  shows protoback wrong, which is then a spec of its own, the test
  marked as failing until it lands.
- **N4. No new tests** beyond the suite's.

## Specification

- **S1. The inventory.** Each of the 66 files, and in the 18 with unit
  tests each test, classified: a command-line test (runs against protoback as
  is); a unit test whose function protoback ports (named, `module::fn`);
  a test of what protoback doesn't have (with the reason). Kept in an
  appendix to this spec, filled at implementation.
- **S2. The port to Rust, test by test,** from the test's code at
  `4da0f2d6`: what it runs, on what, and what it expects. A command-line
  test becomes a Rust integration test under `protoback/tests/`, driving
  `env!("CARGO_BIN_EXE_protoback")`; a unit test, a test in the module
  whose function ports the one it tested (S1). Each keeps its file's and
  its test's names, so that the old suite's history finds it. What
  `conftest.py` provided (compiling fixtures with `protoc`, a temporary run
  directory, comparing a rendered `.proto`) becomes a test helper module.
- **S3. Its goldens are the check:** a ported test keeps its original's
  inputs and expected values (its assertions, its `*.golden.*` files),
  unchanged. protoback has not changed since the suite last passed against
  it (`4da0f2d6`, `reprotoTestsOnProtoback` in `ci`; since then only its
  embedded data's paths, moved by 0487, and their formatting), so a ported
  command-line test that passes with its original's expected values
  reproduces it. A unit test's expected values come from reproto's
  function; its port passing against protoback's is the finding that the
  two agree, and its failing a finding (N3).
- **S4. The fixtures** move to where the Rust tests read them (in the
  crate's test tree, so in its Nix source), and `customDb` reads them
  there: one copy.
- **S5. In `ci`:** the ported tests in `rustTests`; nothing else. A test
  needing an external tool (protoc, buf, prototext) skips without it,
  saying so, so that a downstream `cargo test -p protoback` (nixpkgs'
  check phase, which has protoc alone) passes with no new check input, and
  a version bump needs no edit of the recipe. `rustTests` provides all
  three (prototext: the binary its own `cargo test --workspace` builds)
  and sets `PROTOBACK_SUITE_REQUIRE_TOOLS`, under which a missing tool
  fails the test: the repository's CI never passes by skipping.
  The suite runs twice there, as the original ran (spec 0427): on the
  inputs as they are, then with `PROTOBACK_SUITE_VIA_TEXT`, every binary
  set given to protoback turned into protoc's text form first
  (`suite/via_text.rs`, ported from `via_text.py`); a test the original
  skipped via text skips so too.

## Decisions

Decided by the maintainer, 2026-11-26:

- **D1. Straight to Rust, no Python** (revised 2026-11-26; "Python first,
  then Rust" was decided first, then dropped): no Python suite in the
  tree, not even as a local oracle, whose cost the check S3 makes
  unnecessary.
- **D2. A test keeps its behavior, identically:** the
  same inputs, the same fixtures, the same assertions, the same
  normalizations. Where a test normalized a rendered `.proto` with `buf
  format` (`proto_normalize.py`), its port does too, buf a check input;
  where it compared exactly, so does its port. A difference of behavior
  is a finding (N3), not a liberty of the port.
- **D3. The port's order is the implementer's,** all of it ported in the
  end. Chosen: the command-line-only files first, which port
  mechanically and settle the test helper module (S2) on simple cases;
  then the mixed files' command-line tests, the rendering, editions and
  options ones first; then the unit tests, against the functions S1
  names.
- **D4. prototext is a test tool, as protoc is:** a test may run the
  prototext binary on what protoback produced (a schema DB decoded, a
  payload scored against it), where the original did. It tests
  protoback, prototext reading its output, never prototext itself; one
  more check input, beside protoc and buf, skipped without it as they
  are (S5). A test of prototext alone, protoback's output incidental to
  it, is dropped, not ported: `test_out_of_range_scoring.py`'s R4 (that
  prototext rejects two removed flags, spec 0178 S3).

## Alternatives considered

- **Comparing protoback with reproto** (the differential harness, or
  reproto 0.4.0's outputs frozen as goldens). Rejected (N1): reproto is
  retired, and a reference outside the tests keeps it alive.
- **Leaving the gap (0487 N2).** Rejected: see Background.

## Test plan

1. Each ported test passes against protoback on `main`, with its
   original's inputs and expected values (S3); a failure is a finding
   (N3), the test marked as failing until its spec lands.
2. Each ported file's tests fail on a protoback with the behavior they
   check broken (one deliberate breakage per file, made and reverted
   locally): a port that passes whatever protoback does checks nothing.
3. `rustTests` timed before and after.

## Appendix: the inventory (S1)

`stash/0488/inventory.py` (from the syntax trees at `4da0f2d6`; a test is a
unit test when its body, or a helper of its module it calls, uses a name
imported from `reproto.*` other than the suite's own helpers
`reproto.tests.*`), 2026-11-26:

- **344 tests in 66 files:** 236 command-line tests, 108 unit tests.
- **48 files command line only, 10 mixed, 8 unit tests only.**
- **The internals the unit tests use,** by tests: `load.PathPatterns` 30,
  `context.Context` 26, `split_fdps.split_fdps` 17,
  `feature_resolution.ResolvedFeatures` 15, `lib.warnings.WarningCollector`
  13, `Fqdn` 9, `filter_config.load` 8, `phases.fqdn_match` 8,
  `phases._canonical_extension_ranges` 7,
  `feature_resolution.build_edition_defaults` 7, `load.QualFile` 7,
  `load.load_from_path` 7, and 22 more with five tests or fewer.
- **Their protoback counterparts, by name:** `PathPatterns`, `fqdn_match`
  (`pathmatch.rs`); `split_fdps`, `QualFile`, `load_from_path`
  (`load.rs`); `field_label`, `packed_option` (`render/message.rs`);
  `canonical_extension_ranges` (`scoring.rs`); `load_pool`,
  `generate_instance` (`instantiate.rs`); `filter_config` (`main.rs`).
  The others (feature resolution, `Context`, `WarningCollector`, `Fqdn`,
  `is_synthetic_oneof`, …) to be found test by test, in S3: under other
  names, or in other crates (upb resolves features).

- **Protoback unchanged since `4da0f2d6`:** `git diff 4da0f2d6 HEAD --
  protoback/src` touches `variant.rs` and `profile.rs` only, the embedded
  data's paths (`../data/…`) and their rustfmt layout (2026-11-26).

Each file, as ported (S1, S2): **ported**, with its inputs and expected
values; **adapted**, the same property checked another way (said how);
**twin**, a test of protoback's own already; **reproto-only**, what
protoback doesn't have (said why).

| file | tests | status | where / note |
|---|---|---|---|
| test_aggregate_options.py | 3 | ported | `suite/test_aggregate_options.rs` |
| test_anomalies.py | 12 CLI + 2 unit | CLI ported, unit reproto-only | `suite/test_anomalies.rs`; ten tests' sets (Python-built, refused by protoc at source level) `anomalies/`; D1, D2 passed `dump_option` a Python value of the wrong type (an int for a string field, a list): unrepresentable in protoback, its option values typed by upb |
| test_blob_root_and_defaults.py | 8 CLI + 1 unit | ported | CLI: `suite/test_blob_root_and_defaults.rs`; unit: `src/complete.rs`, `test_blob_root_and_defaults`, on `descriptor_files` (`_complete_blob_members`; `a_blob_root_completes_to_its_members` near it, not a twin: another blob, another word); the members (Python-built) `blob_root/`, the blob assembled in the tests as `_write_blob` did |
| test_comments.py | 4 | ported | `suite/test_comments.rs`; protoc's Locations read with `descriptor_access` |
| test_completion_scripts.py | 4 | adapted, 1 reproto-only | `suite/test_completion_scripts.rs`: reproto completed through Click, protoback through clap (its scripts and formats); S2 checks the bash script is protoback's (no committed `completions.sh` to compare); N2 (reproto-instantiate-schema kept Click's scripts) reproto-only, `protoback instantiate` a subcommand; the responses: the same candidates, `a.pb` and `sub/`, not `notes.txt` |
| test_console.py | 1 | adapted | `suite/test_console.rs`: called reproto's `rprint` by string; the same property (a message over 80 columns on one line, stderr not a terminal) checked on a W5 from the command line |
| test_custom_features.py | 4 | ported | `suite/test_custom_features.rs`; the last two tests' inputs (Python-built) the `#@prototext` fixtures `custom_features/` (generator in `protoback/tests/fixtures/gen/`), one descriptor.proto for both (the same bytes); the last test's expected value, the field's options as Python serialized them (`88b51807`), the generator's |
| test_descriptor_and_fill_in.py | 6 | ported | `suite/test_descriptor_and_fill_in.rs`; `needs_release_protobuf` dropped |
| test_descriptor_package_rewrite.py | 2 | ported | `suite/test_descriptor_package_rewrite.rs`; the pool its set must load into is protoc's (`--descriptor_set_in`), where it used Python's `DescriptorPool`, not protoback's own upb pool; names lower-cased (`test_G1_` is `test_g1_`), no lint pragma |
| test_descriptor_proto_refused.py | 1 | ported | `suite/test_descriptor_proto_refused.rs`; its inputs (protoc's output with descriptor.proto sabotaged by Python's protobuf) are the `#@prototext` fixtures `descriptor_proto_refused/` (the sabotaged descriptor.proto: 2 291 lines) |
| test_descriptor_rerender.py | 1 (3 parameters) | ported | `suite/test_descriptor_rerender.rs`; its input, built with Python's protobuf, is the `#@prototext` fixture `descriptor_rerender.pb` (the original's bytes, SHA-256 checked) |
| test_dump_graph.py | 8 | ported | `suite/test_dump_graph.rs`; a set without a file is protoc's own bytes for the others (`common::set_without`), where it rebuilt it with Python's protobuf; `PYTHONHASHSEED` still varied, for each run's process |
| test_edition_2024_rendering.py | 2 | ported | `suite/test_edition_2024_rendering.rs` |
| test_edition_2026.py | 1 | ported | `suite/test_edition_2026.rs`; its `needs_release_protobuf` marker dropped (a constraint of reproto's Python protobuf, which protoback doesn't have) |
| test_edition_names.py | 1 CLI + 1 unit | CLI ported, unit reproto-only | `suite/test_edition_names.rs`, skipped via text as it was; inputs (Python-built bare FDPs) `edition_names/`; `test_an_older_protobuf_is_said` (W37, Python protobuf older than reproto's descriptor.proto) reproto-only |
| test_editions_rendering.py | 2 CLI + 17 unit | ported | CLI: `suite/test_editions_rendering.rs` (T13, T14 goldens); unit: `src/render/syntax.rs`, `test_editions_rendering`: the five decisions (`field_label`, `packed_option`, `allow_groups`, `is_synthetic_oneof`, `should_render_default`) extracted from `render/message.rs` into `render/syntax.rs` (the maintainer, 2026-11-26), each on what it decides from, where they took the whole render context or were inline (`should_render_default` twice); behavior unchanged, protoback's tests and the goldens passing |
| test_emit_binary.py | 3 (the first over 22 fixtures) | ported | `suite/test_emit_binary.rs`; `roundtrip` marker dropped; the FDP taken as the set's own bytes (a wire walk), where it reserialized with Python's protobuf; the two goldens match the in-process rendering, which is thus `prototext decode`'s |
| test_emit_extension_ranges.py | 6 CLI + 7 unit | ported | CLI: `suite/test_emit_extension_ranges.rs`; unit: `src/scoring.rs`, `test_emit_extension_ranges`, on `canonical_ranges`, split out of `canonical_extension_ranges` (the maintainer, 2026-11-26): its inputs (overlapping, degenerate, past the largest number) are ones upb refuses to hold (W19 on a degenerate range), which the original gave a stand-in descriptor; behavior unchanged, the CLI tests passing |
| test_emit_scoring_graphs.py | 14 | ported | `suite/test_emit_scoring_graphs.rs`; the YAML read with `serde_yaml` (a parser; protoback's emitter is its own, `scoring/pyyaml.rs`), where it used PyYAML; TC-6's binaries with `descriptor_access` |
| test_entry_points.py | 1 | adapted | `suite/test_entry_points.rs`: `protoback` and `protoback instantiate` (reproto-instantiate-schema's port, a subcommand) answer `--version` with their name and the crate's version; the Python protobuf line (spec 0467 S2) reproto-only |
| test_extend_orphans.py | 1 | twin | `protoback/tests/extend.rs` (same input and text, said by the original's docstring) |
| test_extension_field_options.py | 2 | ported | `suite/test_extension_field_options.rs`; `conftest.compile_proto` as `common::compile_proto` |
| test_fd_is_packed.py | 3 unit | ported | `src/scoring.rs`, `test_fd_is_packed`, on `fd_is_packed`, extracted from `field` where it was inline (the pattern the maintainer approved for `canonical_ranges` and `syntax.rs`; behavior unchanged); its files written as text |
| test_fdp_plugin.py | 8 | ported | `suite/test_fdp_plugin.rs`; its plugins are Python given Python protobuf's descriptors: `PROTOBACK_PYTHON` (else `python3`) must import protobuf (`tools`' `python-protobuf`), `rustTests` names one; not a twin of `tests/plugin.rs` (`--fdp-plugin-cmd`) |
| test_feature_resolution.py | 2 CLI + 7 unit | ported | CLI: `suite/test_feature_resolution.rs` (YAML as data, `serde_yaml`); unit: `src/phase2.rs`, `test_feature_resolution`, on `edition_defaults` (`build_edition_defaults`) and `features::resolve` (`resolve_features`, per feature); "the built-in descriptor.proto" protoback's (`DESCRIPTOR_SET`), where it was Python protobuf's; T6's, T7's descriptors (Python-built) `feature_resolution/` |
| test_filter_config.py | 6 CLI + 8 unit | ported | CLI: `suite/test_filter_config.rs`; unit: `src/main.rs`, `test_filter_config`, on `filter_config` (`filter_config.load`), its `Err` matched where `pytest.raises(ValueError, match=…)` was |
| test_instantiate.py | 1 CLI + 3 unit | ported | CLI: `suite/test_instantiate.rs` (eight `protoback instantiate` runs, where it ran `generate_instance` in eight Python processes); unit: `src/instantiate.rs`, module `test_instantiate`, on `generate_instance` and `load_pool`; schemas (Python-built) `instantiate/inst.pb`, `own.pb` (fixtures stored `.pb`, `fixtureFilter` admitting no `.desc`; written `.desc` by the tests) |
| test_load.py | 7 unit | ported | `src/load.rs`, `test_load`, on `load_from_path`; warnings from `Events`' record; a text file's package from the text it keeps (spec 0427 S3), where reproto parsed it |
| test_oneof_and_range_options.py | 2 | ported | `suite/test_oneof_and_range_options.rs` |
| test_open_enum_scoring.py | 3 | ported | `suite/test_open_enum_scoring.rs`, prototext's binary reading protoback's DB (D4); E1's directory not named after the test: prototext echoes the payload's path, and `…_is_not_vetoed` held the word asserted absent (pytest's names, cut to 30 characters, did not) |
| test_option_names.py | 1 | ported | `suite/test_option_names.rs`; each element's options as protoc's bytes (a wire walk), where it reserialized them with Python's protobuf: the same comparison |
| test_out_of_range_scoring.py | 4 | 3 ported, 1 dropped | `suite/test_out_of_range_scoring.rs`, R1 to R3 (the ranges are in the scoring graph protoback builds); R4 (prototext rejects `--relax-ranges`, `--no-strict-ranges`, spec 0178 S3) tested prototext's command line alone: dropped (the maintainer, 2026-11-26) |
| test_path_patterns.py | 2 CLI + 6 unit | ported | CLI: `suite/test_path_patterns.rs`; unit: `src/pathmatch.rs`, `test_path_patterns`, on `PathPatterns` and `fqdn_match` |
| test_path_prune_through_import.py | 1 (2 parameters) | ported | `suite/test_path_prune_through_import.rs`, the parameters in turn |
| test_phases.py | 17 | ported | `suite/test_phases.rs`; names lower-cased; P1's, P5's and M2's inputs (Python-built; M2's printed by `text_format`) the fixtures `phases/` (generator in `protoback/tests/fixtures/gen/`); `compile_proto_multi` in `common.rs` |
| test_profiles.py | 9 | ported | `suite/test_profiles.rs` |
| test_proto3_optional_as_proto2.py | 1 | ported | `suite/test_proto3_optional_as_proto2.rs`; Python's `==` on two messages as their deterministic encodings' equality (`common::message_type`) |
| test_protos_summary.py | 3 (the last with 3 parameters) | twin | `protoback/tests/summary.rs`: the same three tests and parameters |
| test_prune_seed_patterns.py | 4 CLI + 7 unit | ported | CLI: `suite/test_prune_seed_patterns.rs`; unit: `src/pathmatch.rs`, `test_prune_seed_patterns`, on `fqdn_match`; `fqdn_matches_any` (an `any` over `fqdn_match`, inlined in protoback) tested as that `any` |
| test_pruned_dependency.py | 5 | ported | `suite/test_pruned_dependency.rs`; the last test's input (Python-built) `pruned_dependency/set.pb` (generator in `protoback/tests/fixtures/gen/`) |
| test_refusal_said_once.py | 2 | ported | `suite/test_refusal_said_once.rs` (its mention of `tests/repair.rs` is a pointer, not a twin) |
| test_refused_file_options.py | 1 | ported | `suite/test_refused_file_options.rs` |
| test_renaming.py | 2 | ported | `suite/test_renaming.rs` |
| test_render_failures.py | 4 | ported | `suite/test_render_failures.rs` |
| test_render_order.py | 1 (2 parameters) | ported | `suite/test_render_order.rs`; ord_ext.proto compiled by protoc, ord_use.proto's set (stripped of its import in Python) the fixture `render_order/ord_use.pb` |
| test_render_state_per_file.py | 1 unit | reproto-only | in-process state carried between files' renders; protoback renders each file with its own context, in parallel (spec 0429 S4), as its docstring says |
| test_rewrite_collisions.py | 2 | ported | `suite/test_rewrite_collisions.rs`; its inputs, made by its `_inputs` with Python's protobuf, are `fixtures/rewrite_collisions/` (generator: `protoback/tests/fixtures/gen/test_rewrite_collisions.py`, run once with protobuf 7.34.1 and protoc 36.2): three `#@prototext` sets of the original's bytes, `user.textpb` as protoc printed it. protobuf's own copy too: Python reserializes it by field number, protoc wrote one `edition_removed` out of order, so compiling it in the test would not give the same bytes |
| test_roundtrip.py | 8 (3 parametrized: 14, 1, 5 fixtures) | ported | `suite/test_roundtrip.rs`, `run_roundtrip` public for the three below; `proto_normalize.py` in `common.rs`: `uncomment` by a lexer where it used tree-sitter's grammar (the same comment lexemes), `buf format` as it was, `pb_diff`/`pb_diff_fields` rendering in-process with `prototext_core` and diffing by LCS where it used `prototext decode` and `difflib` |
| test_schema_db.py | 1 | ported | `suite/test_schema_db.rs`; the set's files read with `common::set_files` |
| test_schema_db_editions.py | 3 | ported | `suite/test_schema_db_editions.rs` (D4) |
| test_schema_db_proto_subdir.py | 6 | ported | `suite/test_schema_db_proto_subdir.rs`; names lower-cased |
| test_schema_db_reproducibility.py | 1 | ported | `suite/test_schema_db_reproducibility.rs`, four processes as it ran |
| test_scoring_graph_namespace_consistency.py | 3 | ported | `suite/test_scoring_graph_namespace_consistency.rs` (D4), on `test_descriptor_package_rewrite::run` |
| test_show.py | 4 unit | twin | `src/scoring/html.rs`'s tests: the same four, graph, patterns and assertions, on `page` (`render_scoring_graph`) |
| test_source_info.py | 3 (the second over 4 fixtures) | ported | `suite/test_source_info.rs`; protoback's FDP read with `descriptor_access` |
| test_split_fdps.py | 17 unit | ported | `src/load.rs`, `test_split_fdps`, on `split_fdps` (`Contents` text or binary, where reproto took the extension) and `read` (where protoback's extension decides: the tiebreaker tests); `ValueError` is `None`; the binary inputs (name-only sets and files) built as bytes; the binary class's `test_bare_fdp` is `test_binary_bare_fdp`, the text class's keeping the name |
| test_summoning.py | 8 | ported | `suite/test_summoning.rs`; names lower-cased |
| test_targets.py | 12 | 11 ported, 1 reproto-only | `suite/test_targets.rs`; `test_portable_is_a_usage_error_in_reproto` was skipped against protoback, which implements `--target portable` (`tests/portable.rs`, not a twin of this file) |
| test_text_input_options.py | 7 | ported | `suite/test_text_input_options.rs` |
| test_type_names.py | 2 (the first over 9 cases) | ported | `suite/test_type_names.rs`; references by a wire walk |
| test_variant.py | 3 unit | ported | `src/variant.rs`, `test_variant`, on `Variant::load(None)`; reproto's resource root is `Root::Builtin` |
| test_variant_descriptor_package.py | 2 | ported | `suite/test_variant_descriptor_package.rs`; its input (Python protobuf 7.34.1's own descriptor.proto, renamed) the `#@prototext` fixture `variant_descriptor_package/set.pb` (generator in `protoback/tests/fixtures/gen/`) |
| test_w23_stripped_import.py | 3 | ported | `suite/test_w23_stripped_import.rs` |
| test_w3_duplicate_extension.py | 3 | ported | `suite/test_w3_duplicate_extension.rs` |
| test_warning_codes.py | 6 | ported | `suite/test_warning_codes.rs`; inputs (Python-built) `warning_codes/a.textpb` (protoc's text, edited by the original) and `b.pb`; the `via_text` skip dropped with that mode |
| test_warnings.py | 13 unit | 11 ported, 2 adapted | `src/events.rs`, `test_warnings`, on `Events` (`WarningCollector`), read through its record of what it printed (`lines`), where it captured the log; the two W2 tests (flush's hint, printed unrecorded) on the command line, `suite/test_warnings.rs`, on `test_warning_codes`' inputs; reproto's `w5` had no caller but these tests, protoback has none: `w5` is `w1` in the ports (which gives the W5 line), and `w5`'s detailed line, given by nothing any more, reproto-only (one assertion of `test_w3_detailed_fires_immediately`) |

## Measured outcome

- **All 344 tests accounted for:** 232 command-line tests in
  `protoback/tests/suite/` (58 files, one binary), 97 unit tests in the
  modules whose functions port the ones they tested (`load.rs`,
  `pathmatch.rs`, `render/syntax.rs`, `events.rs`, `scoring.rs`,
  `main.rs`, `phase2.rs`, `instantiate.rs`, `variant.rs`, `complete.rs`);
  8 already protoback's own (twins: `extend.rs`, `summary.rs`,
  `scoring/html.rs`); 6 reproto-only; 1 dropped (D4).
- **Passing, at `quick`:** the suite in 2 s, again via text in 12 s
  (S5); with protoback's own, 403 tests. No test of a ported file's
  original failed against protoback: no finding (N3).
- **protoback's code, refactored where a unit test needed a function it
  inlined, behavior unchanged** (the maintainer, 2026-11-26):
  `scoring::canonical_ranges` and `fd_is_packed`, split out of their
  callers; `render/syntax.rs`, the five syntax decisions out of
  `render/message.rs` (`should_render_default` written twice before).
- **Fixtures:** the inputs the original built with Python's protobuf are
  `#@prototext` fixtures of its own bytes, made once by its own code, each
  encoded in the test and its SHA-256 checked; text the original wrote
  (`.textpb`) committed as text; none made by protoback.
- **`rustTests`:** buf, prototext and a Python with protobuf provided,
  `PROTOBACK_SUITE_REQUIRE_TOOLS` set, the suite run a second time via
  text; buf's allowance back in `scripts/protobuf-release-check.py`.
  Verified: `nix-build -A rust-tests` ran the suite as it is and via
  text, 232 tests each, and protoback's 124 unit tests, none failing, none
  skipped.
