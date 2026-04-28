#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
# SPDX-License-Identifier: MIT
#
# Recompile all .proto files in this directory.
# Run from docs/mockup/:  ./compile_all.sh
set -euo pipefail

P="protoc --descriptor_set_out"

$P=f01.pb  -I. f01_syntax_proto2_explicit.proto
$P=f02.pb  -I. f02_syntax_proto2_implicit.proto
$P=f03.pb  -I. f03_syntax_proto3.proto
$P=f04.pb  -I. f04_syntax_edition2023.proto
$P=f05.pb  -I. f05_field_labels_proto2.proto
$P=f06.pb  -I. f06_field_labels_proto3.proto
$P=f07.pb  -I. f07_packed_proto2.proto
$P=f08.pb  -I. f08_packed_proto3.proto
$P=f09.pb  -I. f09_json_name.proto
$P=f10.pb  -I. f10_synthetic_oneof.proto
$P=f11.pb  -I. f11_default_values_proto2.proto
$P=f12.pb  -I. f12_extensions_proto2.proto
$P=f13.pb  -I. f13_groups_proto2.proto
$P=f14.pb  -I. f14_weak_import_proto2.proto
protoc --descriptor_set_out=f14_full.pb --include_imports \
    -I. f14_weak_import_proto2.proto f14_weak_import_proto2_dep.proto
$P=f15.pb  -I. f15_enums_closed_open.proto
$P=f15b.pb -I. f15b_enums_proto3.proto
$P=f16.pb  -I. f16_field_options_proto2.proto
$P=f16b.pb -I. f16b_field_options_proto3.proto
$P=f17.pb  -I. f17_message_options.proto
$P=f17b.pb -I. f17b_message_options_proto3.proto
$P=f18.pb  -I. f18_extension_range_options.proto
$P=f19.pb  -I. f19_edition2023_features.proto
protoc --descriptor_set_out=f20.pb --include_imports \
    -I. f20_weak_import_editions.proto f20_weak_import_editions_dep.proto
$P=f21.pb  -I. f21_message_set_editions.proto
$P=f22.pb  -I. f22_deprecated_legacy_json.proto
$P=f22b.pb -I. f22b_deprecated_legacy_json_proto3.proto
$P=f23.pb  -I. f23_extension_range_end.proto
$P=f24.pb  -I. f24_packed_wire_test.proto
protoc --python_out=. -I. f24_packed_wire_test.proto

echo "All descriptors compiled successfully."
