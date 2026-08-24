#!/usr/bin/env bash

# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# mint-fixtures.sh — re-mint grpconf2026/fixtures/boblog and bobshark.
#
# Requires:
#   - BOBAPP_API_KEY in environment (or reads from ~/.config/bobapp/api-key)
#   - nix-build in PATH (nix-shell dev-shell.nix provides it)
#
# Usage:
#   cd <repo-root>
#   BOBAPP_API_KEY=$(cat ~/.config/bobapp/api-key) grpconf2026/mint-fixtures.sh
#
# What it does:
#   1. Builds bobapp and bobapp-extra-desc via nix.
#   2. Runs bobapp (SearchText first, then ComputeRoutes) writing a log.pb.
#   3. Extracts the first entry's places_request bytes as bobshark.
#   4. Copies both files into grpconf2026/fixtures/.
#
# Log entry order (spec 0350 G4):
#   1. SearchText "coffee in Grenoble"    (whole)
#   2. SearchText "bouchon lyonnais"      (whole)
#   3. ComputeRoutes Grenoble → Lyon      (whole)
#   4. ComputeRoutes Lyon → Grenoble      (truncated — anomaly 4)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

# ── API key ──────────────────────────────────────────────────────────────────

if [[ -z "${BOBAPP_API_KEY:-}" ]]; then
    if [[ -f "$HOME/.config/bobapp/api-key" ]]; then
        BOBAPP_API_KEY="$(cat "$HOME/.config/bobapp/api-key")"
        export BOBAPP_API_KEY
    else
        echo "error: BOBAPP_API_KEY is not set and ~/.config/bobapp/api-key does not exist" >&2
        exit 1
    fi
fi

# ── Build ─────────────────────────────────────────────────────────────────────

echo "Building bobapp..."
BOBAPP_BIN="$(nix-build "$REPO_ROOT" -A bobapp --no-out-link \
    2> >(tee /tmp/bobapp-build.log >&2))/bin/bobapp"

echo "Building bobapp-extra-desc..."
EXTRA_DESC_DIR="$(nix-build "$REPO_ROOT" -A bobapp-extra-desc --no-out-link \
    2> >(tee /tmp/bobapp-extra-desc-build.log >&2))"
EXTRA_DESC="$EXTRA_DESC_DIR/bobapp-extra.desc"

echo "bobapp:     $BOBAPP_BIN"
echo "extra-desc: $EXTRA_DESC"

# ── Run ──────────────────────────────────────────────────────────────────────

echo ""
echo "Running bobapp..."
BOBAPP_EXTRA_DESCRIPTOR_SET="$EXTRA_DESC" \
    "$BOBAPP_BIN" \
    --origin "45.188529, 5.724524" \
    --destination "45.764043, 4.835659" \
    --look-up "coffee in Grenoble" \
    --look-up "bouchon lyonnais" \
    --log-dir "$WORKDIR" \
    > /dev/null

LOG_PB="$WORKDIR/log.pb"
if [[ ! -f "$LOG_PB" ]]; then
    echo "error: bobapp did not write $LOG_PB" >&2
    exit 1
fi
echo "log.pb: $(wc -c < "$LOG_PB") bytes"

# ── Extract bobshark ─────────────────────────────────────────────────────────
#
# The log is a bobapp.v1.Log message:
#   field 1 (repeated) = Entry
#     field 2 = method (string)
#     field 4 = places_request (SearchTextRequest bytes)
#
# bobshark is the places_request bytes from the first entry (which must be a
# SearchText entry, since lookups run before ComputeRoutes).
#
# The log tail is truncated (anomaly 4), so we parse leniently.

python3 - "$LOG_PB" "$WORKDIR/bobshark" << 'PYEOF'
import sys, struct

def read_varint(data, pos):
    result = 0; shift = 0
    while pos < len(data):
        b = data[pos]; pos += 1
        result |= (b & 0x7f) << shift
        if not (b & 0x80):
            break
        shift += 7
    else:
        raise EOFError("varint at EOF")
    return result, pos

def read_tag(data, pos):
    v, pos = read_varint(data, pos)
    return v >> 3, v & 7, pos

def read_ld(data, pos):
    n, pos = read_varint(data, pos)
    end = pos + n
    return data[pos:min(end, len(data))], end  # lenient: return what is there

log_path, sharkpath = sys.argv[1], sys.argv[2]

with open(log_path, "rb") as f:
    data = f.read()

# Parse Log (field 1 = repeated Entry), lenient on truncation.
entries = []
pos = 0
while pos < len(data):
    try:
        field, wire, pos = read_tag(data, pos)
    except EOFError:
        break
    if wire == 2 and field == 1:
        try:
            payload, pos = read_ld(data, pos)
            entries.append(payload)
        except EOFError:
            break
    else:
        break

if not entries:
    print("error: no entries found in log", file=sys.stderr)
    sys.exit(1)

# Parse the first entry to find method and places_request (field 4).
first = entries[0]
ep = 0
method = None
places_request = None
while ep < len(first):
    try:
        f, w, ep = read_tag(first, ep)
    except EOFError:
        break
    if w == 2:
        try:
            p, ep = read_ld(first, ep)
        except EOFError:
            break
        if f == 2:
            method = p.decode("utf-8", errors="replace")
        elif f == 4:
            places_request = p
    elif w == 0:
        try:
            _, ep = read_varint(first, ep)
        except EOFError:
            break
    else:
        break

if method != "SearchText":
    print(f"error: first entry method is {method!r}, expected 'SearchText'", file=sys.stderr)
    print("  (check that --look-up runs before ComputeRoutes)", file=sys.stderr)
    sys.exit(1)

if places_request is None:
    print("error: first entry has no places_request (field 4)", file=sys.stderr)
    sys.exit(1)

with open(sharkpath, "wb") as f:
    f.write(places_request)

print(f"bobshark: {len(places_request)} bytes  (method={method!r})")
PYEOF

# ── Copy fixtures ─────────────────────────────────────────────────────────────

FIXTURES="$REPO_ROOT/grpconf2026/fixtures"
cp "$LOG_PB"          "$FIXTURES/boblog"
cp "$WORKDIR/bobshark" "$FIXTURES/bobshark"

echo ""
echo "Updated fixtures:"
echo "  $FIXTURES/boblog   ($(wc -c < "$FIXTURES/boblog") bytes)"
echo "  $FIXTURES/bobshark ($(wc -c < "$FIXTURES/bobshark") bytes)"
echo ""
echo "Verify:"
echo "  protoc --decode_raw < grpconf2026/fixtures/boblog  # should fail (truncated)"
