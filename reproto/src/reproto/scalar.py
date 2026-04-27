# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

from json import dumps

from .text import render_bytes


class Scalar:
    def __init__(
        self,
        value: bool | int | str | bytes | float,
        is_enum: bool = False,
    ) -> None:
        if is_enum and not isinstance(value, str):
            raise ValueError('Enum value must be a str.')
        self.value = value
        self.is_enum = is_enum

    def __str__(self) -> str:
        match self.value:
            case bool():
                return str(self.value).lower()
            case int() | float():
                return str(self.value)
            case str():
                if self.is_enum:
                    return self.value
                else:
                    return dumps(self.value)
            case bytes():
                return render_bytes(self.value)
            case _:
                raise RuntimeError('Unexpected type for Scalar')