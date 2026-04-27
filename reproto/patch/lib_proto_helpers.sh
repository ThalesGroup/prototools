#!/usr/bin/env bash

# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

# libreproto:/patch/lib_proto_helpers.sh
#
# Helper for generating protobuf artifacts conditionally based on requested extensions.
# Usage:
#   generate_proto_artifacts <proto_path> <proto_file> <ext1> [<ext2> ...]

set -euo pipefail

log() {
    printf '[proto_helper] %s\n' "$*"
}

die() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}

generate_proto_artifacts() {
    local proto_path=$1
    local proto_file=$2
    shift 2  # remaining args are the requested extensions
    local exts=("$@")

    [ -d "$proto_path" ] || die "Directory not found: $proto_path"
    [ -f "$proto_path/$proto_file" ] || die "File not found: $proto_path/$proto_file"

    local base_name
    base_name=$(basename "$proto_file" .proto)

    for ext in "${exts[@]}"; do
        case "$ext" in
            .pb)
                log "Generating protobuf descriptor (.pb) for $proto_file..."
                protoc \
                    --proto_path="$proto_path" \
                    --descriptor_set_out="$proto_path/$base_name$ext" \
                    "$proto_file"
                ;;
            .desc|.protoset)
                log "Generating protobuf descriptor (.desc) for $proto_file..."
                protoc \
                    --proto_path="$proto_path" \
                    --include_imports \
                    --descriptor_set_out="$proto_path/$base_name$ext" \
                    "$proto_file"
                ;;
            _pb2.py)
                log "Generating Python bindings (.py) for $proto_file..."
                protoc \
                    --proto_path="$proto_path" \
                    --python_out="$proto_path" \
                    "$proto_file"
                log "Applying pylance type-ignore fix..."
                sed -i \
                    's/^\(from google\.protobuf import runtime_version as _runtime_version\)$/\1 # type: ignore/' \
                    "$proto_path/${base_name}_pb2.py"
                ;;
            _pb2.pyi)
                log "Generating Python type hints (.pyi) for $proto_file..."
                protoc \
                    --proto_path="$proto_path" \
                    --pyi_out="$proto_path" \
                    "$proto_file"
                ;;
            *)
                die "Unknown extension requested: $ext"
                ;;
        esac
    done

    log "Requested artifacts generated successfully for $proto_file"
}
