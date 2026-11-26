// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! reproto's test suite, ported to Rust against protoback (spec 0488).
//!
//! One module per file of the suite at `4da0f2d6`
//! (`reproto/src/reproto/tests/`), under the same name, each test under its
//! original's name, with its inputs and expected values unchanged (S2,
//! S3). One test binary, so that the helpers of `common` are shared without
//! each binary compiling them for itself.

mod common;
#[path = "../../src/suite_fixtures.rs"]
mod suite_fixtures;

mod via_text;

mod test_aggregate_options;
mod test_anomalies;
mod test_blob_root_and_defaults;
mod test_comments;
mod test_completion_scripts;
mod test_console;
mod test_custom_features;
mod test_descriptor_and_fill_in;
mod test_descriptor_package_rewrite;
mod test_descriptor_proto_refused;
mod test_descriptor_rerender;
mod test_dump_graph;
mod test_edition_2024_rendering;
mod test_edition_2026;
mod test_edition_names;
mod test_editions_rendering;
mod test_emit_binary;
mod test_emit_extension_ranges;
mod test_emit_scoring_graphs;
mod test_entry_points;
mod test_extension_field_options;
mod test_fdp_plugin;
mod test_feature_resolution;
mod test_filter_config;
mod test_instantiate;
mod test_oneof_and_range_options;
mod test_open_enum_scoring;
mod test_option_names;
mod test_out_of_range_scoring;
mod test_path_patterns;
mod test_path_prune_through_import;
mod test_phases;
mod test_profiles;
mod test_proto3_optional_as_proto2;
mod test_prune_seed_patterns;
mod test_pruned_dependency;
mod test_refusal_said_once;
mod test_refused_file_options;
mod test_renaming;
mod test_render_failures;
mod test_render_order;
mod test_rewrite_collisions;
mod test_roundtrip;
mod test_schema_db;
mod test_schema_db_editions;
mod test_schema_db_proto_subdir;
mod test_schema_db_reproducibility;
mod test_scoring_graph_namespace_consistency;
mod test_source_info;
mod test_summoning;
mod test_targets;
mod test_text_input_options;
mod test_type_names;
mod test_variant_descriptor_package;
mod test_w23_stripped_import;
mod test_w3_duplicate_extension;
mod test_warning_codes;
mod test_warnings;
