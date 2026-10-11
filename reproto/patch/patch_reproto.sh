#!/usr/bin/env bash

# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

# reproto/patch/patch_reproto.sh
#
# Compiles the well-known types' .proto files to the binary .pb descriptors
# reproto ships in its built-in variant, for the descriptor sets that leave
# them implicit. The .pb are committed; the Nix check reproto-wkt-check runs
# this script with the pinned protobuf and compares (spec 0409 S4).
#
# Usage:
#   ./patch_reproto.sh <include> <out>
#
# Arguments:
#   include   protobuf's include directory, holding google/protobuf/*.proto
#   out       the variant directory to write to, normally
#             reproto/src/reproto/variants/google-protobuf
#
# Requirements:
#   - "protoc" must be in PATH.

set -euo pipefail

source "$(dirname "$0")/lib_proto_helpers.sh"

if [ "$#" -ne 2 ]; then
    die "Usage: $0 <include> <out>"
fi

include="$1"
out="$2"

[ -f "$include/google/protobuf/descriptor.proto" ] \
    || die "No google/protobuf/descriptor.proto under: $include"

# =============================================================================
# SECTION 1: Compile google/protobuf/*.proto → *.pb (open-source variants)
# =============================================================================

log "Compiling google/protobuf proto files to .pb..."

google_proto_files=(
    "google/protobuf/descriptor.proto"
    "google/protobuf/any.proto"
    "google/protobuf/empty.proto"
    "google/protobuf/timestamp.proto"
    "google/protobuf/duration.proto"
    "google/protobuf/struct.proto"
    "google/protobuf/wrappers.proto"
    "google/protobuf/source_context.proto"   # no imports
    "google/protobuf/field_mask.proto"       # no imports
    "google/protobuf/type.proto"             # imports any, source_context
    "google/protobuf/api.proto"              # imports timestamp, source_context
)

for proto_file in "${google_proto_files[@]}"; do
    base_name=$(basename "$proto_file" .proto)
    proto_dir=$(dirname "$proto_file")
    log "  Compiling $proto_file → ${base_name}.pb"
    mkdir -p "${out}/${proto_dir}"
    protoc \
        --descriptor_set_out="${out}/${proto_dir}/${base_name}.pb" \
        --proto_path="${include}" \
        "$proto_file"
done

log "Google protobuf variants compiled successfully."
