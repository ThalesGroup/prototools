#!/usr/bin/env bash

# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

# reproto/patch/patch_reproto.sh
#
# Generates binary .pb descriptor files from the well-known-type .proto sources.
#
# Usage:
#   ./patch_reproto.sh <bare_reproto_path> <out>
#
# Arguments:
#   bare_reproto_path  Path to the installed reproto package (provides bin/reproto).
#   out                Path to a writable copy of the reproto/ package root.
#                      (i.e. the directory containing src/ and patch/)
#
# Requirements:
#   - "protoc" must be in PATH.
#   - $out/src/resources/google/protobuf/*.proto must already be present
#     (seeded from pkgs.protobuf by the Nix derivation before this script runs).

set -euo pipefail

source "$(dirname "$0")/lib_proto_helpers.sh"

if [ "$#" -ne 2 ]; then
    die "Usage: $0 <bare_reproto_path> <out>"
fi

bare_reproto="$1"
out="$2"

[ -d "$out" ] || die "Output directory not found: $out"

# =============================================================================
# SECTION 1: Compile google/protobuf/*.proto → *.pb (open-source variants)
# =============================================================================

log "Compiling google/protobuf proto files to .pb..."

resources_path="$out/src/resources"
variants_oss_pb="$out/src/reproto/variants/google-protobuf"
google_proto_files=(
    "google/protobuf/descriptor.proto"
    "google/protobuf/any.proto"
    "google/protobuf/empty.proto"
    "google/protobuf/timestamp.proto"
    "google/protobuf/duration.proto"
    "google/protobuf/struct.proto"
    "google/protobuf/wrappers.proto"
)

for proto_file in "${google_proto_files[@]}"; do
    base_name=$(basename "$proto_file" .proto)
    proto_dir=$(dirname "$proto_file")
    log "  Compiling $proto_file → ${base_name}.pb"
    protoc \
        --descriptor_set_out="${resources_path}/${proto_dir}/${base_name}.pb" \
        --proto_path="${resources_path}" \
        "$proto_file"
    mkdir -p "${variants_oss_pb}/${proto_dir}"
    cp "${resources_path}/${proto_dir}/${base_name}.pb" \
       "${variants_oss_pb}/${proto_dir}/${base_name}.pb"
done

log "Google protobuf variants compiled successfully."
