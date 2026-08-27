# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Unit tests for fdp_scan_lib.scan().

Fixtures are synthesised in-process using protobuf's Python API so that
the test suite has no dependency on protoc and no committed binary files.
"""

from __future__ import annotations

from google.protobuf.descriptor_pb2 import FileDescriptorProto

import fdp_scan_lib


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_fdp_bytes(name: str) -> bytes:
    """Return a minimal serialised FileDescriptorProto with the given name.

    Sets ``package`` as well as ``name`` so that the record passes the
    structural floor in ``declares_more_than_a_name`` (spec 0313 S4):
    a descriptor with only its name is indistinguishable from a Java
    package option and is deliberately rejected.
    """
    fdp = FileDescriptorProto()
    fdp.name = name
    fdp.package = "test"
    return fdp.SerializeToString()


# ---------------------------------------------------------------------------
# TC-1  Empty buffer → empty list
# ---------------------------------------------------------------------------

def test_empty_buffer() -> None:
    """scan() on an empty buffer returns an empty list."""
    assert fdp_scan_lib.scan(b"") == []


# ---------------------------------------------------------------------------
# TC-2  Buffer of zero bytes → empty list
# ---------------------------------------------------------------------------

def test_zero_bytes() -> None:
    """scan() on a buffer of NUL bytes returns an empty list."""
    assert fdp_scan_lib.scan(b"\x00" * 64) == []


# ---------------------------------------------------------------------------
# TC-3  Single FDP blob → one (start, end) pair
# ---------------------------------------------------------------------------

def test_single_fdp_one_result() -> None:
    """scan() on a single FDP blob returns exactly one (start, end) pair."""
    payload = _make_fdp_bytes("google/protobuf/descriptor.proto")
    results = fdp_scan_lib.scan(payload)
    assert len(results) == 1
    start, end = results[0]
    assert start >= 0
    assert end <= len(payload)
    assert start < end


# ---------------------------------------------------------------------------
# TC-4  Two concatenated FDP blobs → two pairs with correct byte ranges
# ---------------------------------------------------------------------------

def test_two_fdps_two_results() -> None:
    """scan() on two concatenated FDP blobs returns two non-overlapping pairs."""
    blob_a = _make_fdp_bytes("foo/bar.proto")
    blob_b = _make_fdp_bytes("baz/qux.proto")
    buf = blob_a + blob_b
    results = fdp_scan_lib.scan(buf)
    assert len(results) == 2
    # Pairs must be non-overlapping and within bounds.
    for start, end in results:
        assert 0 <= start < end <= len(buf)
    (s0, e0), (s1, e1) = results
    assert e0 <= s1 or e1 <= s0, "pairs must not overlap"


# ---------------------------------------------------------------------------
# TC-5  FDP preceded by noise bytes + 0x00 terminator → detected
# ---------------------------------------------------------------------------

def test_fdp_preceded_by_noise() -> None:
    """scan() finds an FDP embedded after noise bytes when followed by 0x00."""
    payload = _make_fdp_bytes("embedded/thing.proto")
    noise = b"\xDE\xAD\xBE\xEF" * 16
    buf = noise + payload + b"\x00"
    results = fdp_scan_lib.scan(buf)
    assert len(results) >= 1


# ---------------------------------------------------------------------------
# TC-6  Extracted slice deserialises to a FileDescriptorProto with the expected name
# ---------------------------------------------------------------------------

def test_extracted_slice_roundtrips() -> None:
    """The bytes slice indicated by scan() deserialises to the original FDP."""
    name = "roundtrip/check.proto"
    payload = _make_fdp_bytes(name)
    results = fdp_scan_lib.scan(payload)
    assert len(results) == 1
    start, end = results[0]
    recovered = FileDescriptorProto()
    recovered.ParseFromString(payload[start:end])
    assert recovered.name == name
