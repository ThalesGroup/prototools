# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from .fdp_scan_lib import scan
from . import fdp_scan_lib

__doc__ = fdp_scan_lib.__doc__
__all__ = ["scan"]
