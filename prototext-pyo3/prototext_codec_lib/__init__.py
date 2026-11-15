# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from .prototext_codec_lib import (
    DescriptorPool,
    SchemaHandle,
    format_as_bytes,
    format_as_text,
    parse_descriptor_text,
    register_schema,
)
from . import prototext_codec_lib

__doc__ = prototext_codec_lib.__doc__
__all__ = [
    "DescriptorPool",
    "SchemaHandle",
    "format_as_bytes",
    "format_as_text",
    "parse_descriptor_text",
    "register_schema",
]
