# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import sys

from google.protobuf.message_factory import GetMessageClass

from lib.warnings import cli_error
from .context import Context


def create_option_message_classes(ctx: Context) -> None:
  if ctx.variant_descriptor_proto.startswith('google/'):
    pkg = 'google.protobuf'
  else:
    pkg = 'proto2'
  try:
    # EnumOptions message class
    ctx.eno_desc = ctx.pool.FindMessageTypeByName(pkg + ".EnumOptions")
    ctx.eno_cls = GetMessageClass(ctx.eno_desc)

    # EnumValueOptions message class
    ctx.evo_desc = ctx.pool.FindMessageTypeByName(pkg + ".EnumValueOptions")
    ctx.evo_cls = GetMessageClass(ctx.evo_desc)

    # FieldOptions message class
    ctx.fdo_desc = ctx.pool.FindMessageTypeByName(pkg + ".FieldOptions")
    ctx.fdo_cls = GetMessageClass(ctx.fdo_desc)

    # FileOptions message class
    ctx.fio_desc = ctx.pool.FindMessageTypeByName(pkg + ".FileOptions")
    ctx.fio_cls = GetMessageClass(ctx.fio_desc)

    # MessageOptions message class
    ctx.mso_desc = ctx.pool.FindMessageTypeByName(pkg + ".MessageOptions")
    ctx.mso_cls = GetMessageClass(ctx.mso_desc)

    # MethodOptions message class
    ctx.meo_desc = ctx.pool.FindMessageTypeByName(pkg + ".MethodOptions")
    ctx.meo_cls = GetMessageClass(ctx.meo_desc)

    # OneofOptions message class
    ctx.ooo_desc = ctx.pool.FindMessageTypeByName(pkg + ".OneofOptions")
    ctx.ooo_cls = GetMessageClass(ctx.ooo_desc)

    # ServiceOptions message class
    ctx.svo_desc = ctx.pool.FindMessageTypeByName(pkg + ".ServiceOptions")
    ctx.svo_cls = GetMessageClass(ctx.svo_desc)

  except KeyError as e:
    cli_error(f"Failed to initialize option message classes: {e}")
    cli_error(f"The descriptor pool is missing required types from '{pkg}' package.")
    cli_error("Ensure descriptor.proto is properly loaded in the pool.")
    sys.exit(1)
