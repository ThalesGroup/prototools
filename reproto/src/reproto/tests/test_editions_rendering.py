# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Unit tests for editions rendering phase 2 (spec 0027).

Tests T1-T12 exercise each modified syntax.py helper directly with synthetic
ResolvedFeatures values (both features=None and features=<value>).
Test T13 is a golden regression test: compiles editions_rendering.proto,
runs reproto, compares output against the checked-in golden file.
"""

from __future__ import annotations

import importlib
import importlib.resources
import os
import subprocess
import sys
from pathlib import Path

import pytest

from google.protobuf.descriptor_pb2 import FieldDescriptorProto

from reproto.context import Context

from reproto.feature_resolution import (
    FIELD_PRESENCE_EXPLICIT,
    FIELD_PRESENCE_IMPLICIT,
    FIELD_PRESENCE_LEGACY_REQUIRED,
    MESSAGE_ENCODING_DELIMITED,
    MESSAGE_ENCODING_LENGTH_PREFIXED,
    REPEATED_FIELD_ENCODING_EXPANDED,
    REPEATED_FIELD_ENCODING_PACKED,
    ResolvedFeatures,
)
from reproto.syntax import (
    allow_groups,
    field_label,
    is_synthetic_oneof,
    packed_option,
    should_render_default,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _ctx(target_syntax: str = "proto2", syntax: str = "proto2") -> Context:
    """Return a minimal Context sufficient for the syntax helpers."""
    ctx = Context(pruned_fqdns=set())
    ctx.target_syntax = target_syntax
    ctx.syntax = syntax
    return ctx


def _field(
    label: FieldDescriptorProto.Label.ValueType,
    proto3_optional: bool = False,
    has_default: bool = False,
) -> FieldDescriptorProto:
    """Return a FieldDescriptorProto stub with the given label."""
    f = FieldDescriptorProto()
    f.name = "x"
    f.number = 1
    f.label = label
    f.type = FieldDescriptorProto.TYPE_INT32
    if proto3_optional:
        f.proto3_optional = True
    if has_default:
        f.default_value = "42"
    return f


def _features(**kwargs: int) -> ResolvedFeatures:
    """Return a ResolvedFeatures with the given field overrides."""
    return ResolvedFeatures(**kwargs)


# ---------------------------------------------------------------------------
# T1 — field_label proto2/proto3 unchanged (features=None)
# ---------------------------------------------------------------------------

def test_T1_field_label_proto2_unchanged():
    """features=None: proto2 labels unchanged."""
    ctx = _ctx("proto2")
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_OPTIONAL), False) == "optional "
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_REQUIRED), False) == "required "
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_REPEATED), False) == "repeated "
    # is_oneof always yields ''
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_OPTIONAL), True) == ""


def test_T1_field_label_proto3_unchanged():
    """features=None: proto3 labels unchanged."""
    ctx = _ctx("proto3")
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_OPTIONAL), False) == ""
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_OPTIONAL, proto3_optional=True), False) == "optional "
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_REPEATED), False) == "repeated "


# ---------------------------------------------------------------------------
# T2 — field_label editions EXPLICIT
# ---------------------------------------------------------------------------

def test_T2_field_label_editions_explicit():
    """features.field_presence = EXPLICIT → 'optional '."""
    ctx = _ctx()
    f = _features(field_presence=FIELD_PRESENCE_EXPLICIT)
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_OPTIONAL), False, features=f) == "optional "


# ---------------------------------------------------------------------------
# T3 — field_label editions IMPLICIT
# ---------------------------------------------------------------------------

def test_T3_field_label_editions_implicit():
    """features.field_presence = IMPLICIT → ''."""
    ctx = _ctx()
    f = _features(field_presence=FIELD_PRESENCE_IMPLICIT)
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_OPTIONAL), False, features=f) == ""


# ---------------------------------------------------------------------------
# T4 — field_label editions LEGACY_REQUIRED
# ---------------------------------------------------------------------------

def test_T4_field_label_editions_legacy_required():
    """features.field_presence = LEGACY_REQUIRED → 'required '."""
    ctx = _ctx()
    f = _features(field_presence=FIELD_PRESENCE_LEGACY_REQUIRED)
    assert field_label(ctx, _field(FieldDescriptorProto.LABEL_OPTIONAL), False, features=f) == "required "


# ---------------------------------------------------------------------------
# T5 — packed_option editions PACKED default
# ---------------------------------------------------------------------------

def test_T5_packed_option_editions_packed_default():
    """features.repeated_field_encoding = PACKED, has_field=False → None."""
    ctx = _ctx()
    f = _features(repeated_field_encoding=REPEATED_FIELD_ENCODING_PACKED)
    assert packed_option(ctx, has_field=False, effective_packed=True, features=f) is None


# ---------------------------------------------------------------------------
# T6 — packed_option editions EXPANDED
# ---------------------------------------------------------------------------

def test_T6_packed_option_editions_expanded():
    """features.repeated_field_encoding = EXPANDED, has_field=False → None (deferred to phase 3)."""
    ctx = _ctx()
    f = _features(repeated_field_encoding=REPEATED_FIELD_ENCODING_EXPANDED)
    assert packed_option(ctx, has_field=False, effective_packed=False, features=f) is None


def test_T6_packed_option_editions_has_field_emits():
    """has_field=True in editions path → emit the explicit value (legacy round-trip)."""
    ctx = _ctx()
    f = _features(repeated_field_encoding=REPEATED_FIELD_ENCODING_PACKED)
    assert packed_option(ctx, has_field=True, effective_packed=True, features=f) == "true"
    assert packed_option(ctx, has_field=True, effective_packed=False, features=f) == "false"


# ---------------------------------------------------------------------------
# T7 — allow_groups editions DELIMITED
# ---------------------------------------------------------------------------

def test_T7_allow_groups_editions_delimited():
    """features.message_encoding = DELIMITED → True."""
    ctx = _ctx()
    f = _features(message_encoding=MESSAGE_ENCODING_DELIMITED)
    assert allow_groups(ctx, features=f) is True


# ---------------------------------------------------------------------------
# T8 — allow_groups editions LENGTH_PREFIXED
# ---------------------------------------------------------------------------

def test_T8_allow_groups_editions_length_prefixed():
    """features.message_encoding = LENGTH_PREFIXED → False."""
    ctx = _ctx()
    f = _features(message_encoding=MESSAGE_ENCODING_LENGTH_PREFIXED)
    assert allow_groups(ctx, features=f) is False


# ---------------------------------------------------------------------------
# T9 — is_synthetic_oneof editions IMPLICIT
# ---------------------------------------------------------------------------

def test_T9_is_synthetic_oneof_editions_implicit():
    """Single member with IMPLICIT presence → True (synthetic oneof)."""
    ctx = _ctx()
    member = _field(FieldDescriptorProto.LABEL_OPTIONAL)
    f = _features(field_presence=FIELD_PRESENCE_IMPLICIT)
    assert is_synthetic_oneof(ctx, "_x", [member], features=f) is True


def test_T9_is_synthetic_oneof_editions_implicit_no_underscore():
    """IMPLICIT member but name lacks '_' prefix → False."""
    ctx = _ctx()
    member = _field(FieldDescriptorProto.LABEL_OPTIONAL)
    f = _features(field_presence=FIELD_PRESENCE_IMPLICIT)
    assert is_synthetic_oneof(ctx, "real_oneof", [member], features=f) is False


def test_T9_is_synthetic_oneof_editions_implicit_multi_member():
    """IMPLICIT features but 2 members → False (real oneof)."""
    ctx = _ctx()
    members = [_field(FieldDescriptorProto.LABEL_OPTIONAL),
               _field(FieldDescriptorProto.LABEL_OPTIONAL)]
    f = _features(field_presence=FIELD_PRESENCE_IMPLICIT)
    assert is_synthetic_oneof(ctx, "_x", members, features=f) is False


# ---------------------------------------------------------------------------
# T10 — is_synthetic_oneof editions EXPLICIT
# ---------------------------------------------------------------------------

def test_T10_is_synthetic_oneof_editions_explicit():
    """Single member with EXPLICIT presence → False (real oneof)."""
    ctx = _ctx()
    member = _field(FieldDescriptorProto.LABEL_OPTIONAL)
    f = _features(field_presence=FIELD_PRESENCE_EXPLICIT)
    assert is_synthetic_oneof(ctx, "_x", [member], features=f) is False


# ---------------------------------------------------------------------------
# T11 — should_render_default editions IMPLICIT
# ---------------------------------------------------------------------------

def test_T11_should_render_default_editions_implicit():
    """field_presence = IMPLICIT → False even if default_value is set."""
    ctx = _ctx()
    f_implicit = _field(FieldDescriptorProto.LABEL_OPTIONAL, has_default=True)
    feats = _features(field_presence=FIELD_PRESENCE_IMPLICIT)
    assert should_render_default(ctx, f_implicit, features=feats) is False


# ---------------------------------------------------------------------------
# T12 — should_render_default editions EXPLICIT
# ---------------------------------------------------------------------------

def test_T12_should_render_default_editions_explicit():
    """field_presence = EXPLICIT, default_value set → True."""
    ctx = _ctx()
    f_explicit = _field(FieldDescriptorProto.LABEL_OPTIONAL, has_default=True)
    feats = _features(field_presence=FIELD_PRESENCE_EXPLICIT)
    assert should_render_default(ctx, f_explicit, features=feats) is True


def test_T12_should_render_default_editions_explicit_no_default():
    """field_presence = EXPLICIT but no default_value → False."""
    ctx = _ctx()
    f_no_default = _field(FieldDescriptorProto.LABEL_OPTIONAL, has_default=False)
    feats = _features(field_presence=FIELD_PRESENCE_EXPLICIT)
    assert should_render_default(ctx, f_no_default, features=feats) is False


# ---------------------------------------------------------------------------
# T13 — golden regression test
# ---------------------------------------------------------------------------

def _get_fixture_path(name: str) -> Path:
    pkg = importlib.import_module('reproto.tests.fixtures')
    files = importlib.resources.files(pkg)
    return Path(str(files.joinpath(name)))


@pytest.mark.roundtrip
def test_T13_editions_rendering_golden(tmp_path: Path) -> None:
    """Compile editions_rendering.proto, run reproto, compare against golden."""
    proto_src = _get_fixture_path("editions_rendering.proto")
    golden_src = _get_fixture_path("editions_rendering.golden.proto")

    orig_dir = tmp_path / "orig"
    orig_dir.mkdir()

    proto_path = orig_dir / "editions_rendering.proto"
    proto_path.write_text(proto_src.read_text(encoding="utf-8"), encoding="utf-8")

    pb_path = orig_dir / "editions_rendering.pb"
    result = subprocess.run(
        ["protoc",
         f"--descriptor_set_out={pb_path}",
         "--include_imports",
         f"-I{orig_dir}",
         str(proto_path)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed: {result.stderr}"

    out_dir = tmp_path / "out"
    out_dir.mkdir()

    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)

    reproto_cmd = [
        sys.executable, "-m", "reproto.cli",
        "--use-variant", "descriptor",
        f"-I{orig_dir}",
        f"--proto-out={out_dir}",
        str(pb_path),
    ]
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    result = subprocess.run(reproto_cmd, capture_output=True, text=True, env=env)
    assert result.returncode == 0, f"reproto failed: {result.stderr}\n{result.stdout}"

    # Locate generated file (reproto places it at the package path).
    generated_files = list(out_dir.rglob("editions_rendering.proto"))
    assert generated_files, f"No editions_rendering.proto in {out_dir}"
    actual = generated_files[0].read_text(encoding="utf-8")

    golden = golden_src.read_text(encoding="utf-8")

    # Normalise: strip trailing whitespace per line.
    def _norm(text: str) -> str:
        return "\n".join(line.rstrip() for line in text.splitlines())

    assert _norm(actual) == _norm(golden), (
        f"Output differs from golden.\n"
        f"--- golden ---\n{golden}\n--- actual ---\n{actual}"
    )
