# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Pytest configuration for reproto tests."""

import shutil
import subprocess

import pytest


def pytest_configure(config):
    """Configure pytest with custom markers."""
    config.addinivalue_line(
        "markers", "roundtrip: marks tests as roundtrip tests (deselect with '-m \"not roundtrip\"')"
    )


@pytest.fixture(scope="session", autouse=True)
def check_dependencies():
    """Verify that required external dependencies are available."""
    # Check for protoc
    if not shutil.which("protoc"):
        pytest.skip(
            "protoc not found - install protocol buffers compiler",
            allow_module_level=True
        )

    # Verify protoc version
    result = subprocess.run(
        ["protoc", "--version"], capture_output=True, text=True
    )
    if result.returncode != 0:
        pytest.skip("protoc version check failed", allow_module_level=True)
