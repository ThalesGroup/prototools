# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import logging

def cli_error(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_err", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_warning(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_warn", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_info(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_info", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_attention(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_attn", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

