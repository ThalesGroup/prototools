# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from .prototext_codec_lib import (
    SchemaHandle,
    format_as_bytes,
    format_as_text,
    parse_descriptor_text,
    read_descriptors,
    register_schema,
    scan_descriptor_text,
    split_descriptor_set,
)
from . import prototext_codec_lib

__doc__ = prototext_codec_lib.__doc__
__all__ = [
    "SchemaHandle",
    "format_as_bytes",
    "format_as_text",
    "parse_descriptor_text",
    "read_descriptors",
    "register_schema",
    "scan_descriptor_text",
    "split_descriptor_set",
]
