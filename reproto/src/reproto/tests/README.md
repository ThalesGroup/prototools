<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# reproto Tests

## Overview

This directory contains tests for the reproto tool, which regenerates `.proto` files from protobuf descriptor sets.

## Test Types

### Roundtrip Tests (`test_roundtrip.py`)

These tests verify that reproto can perfectly reconstruct proto files by:

1. **Compile** original `.proto` to descriptor set
2. **Regenerate** `.proto` from descriptor set using reproto
3. **Recompile** regenerated `.proto` to descriptor set
4. **Compare** both descriptor sets (must be byte-for-byte identical)

This ensures semantic equivalence: if the descriptor sets match, the regenerated proto file is functionally identical to the original.

## Running Tests

### Prerequisites

```bash
# Install dependencies
pip install pytest protobuf

# Ensure protoc is available
protoc --version
```

### Run all tests

```bash
# From the repository root
pytest src/reproto/tests/

# With verbose output
pytest src/reproto/tests/ -v

# Run specific test file
pytest src/reproto/tests/test_roundtrip.py -v
```

### Run specific fixtures

```bash
# Run only enum.proto roundtrip test
pytest src/reproto/tests/test_roundtrip.py::test_roundtrip[enum.proto] -v
```

### Skip roundtrip tests

```bash
# Skip roundtrip tests (useful for quick checks)
pytest src/reproto/tests/ -m "not roundtrip"
```

## Adding New Test Fixtures

To add a new test fixture:

1. Create a `.proto` file in `fixtures/` directory
2. The test will automatically discover and test it
3. Ensure the proto file is valid and self-contained (or includes only google/protobuf/* imports)

Example fixtures to add:
- `message.proto` - Message types with various field types
- `service.proto` - Service definitions with RPCs
- `proto3.proto` - Proto3 syntax examples
- `nested.proto` - Nested messages and enums
- `oneof.proto` - Oneof fields
- `map.proto` - Map fields

## Test Output

When tests pass:
```
test_roundtrip.py::test_roundtrip[enum.proto] PASSED
```

When tests fail:
```
AssertionError: Descriptor sets differ:
  Original: /tmp/reproto_orig_xxx/enum.pb
  Regenerated: /tmp/reproto_new_xxx/enum.pb
Run: diff <(xxd /tmp/...) <(xxd /tmp/...) to see differences
```

The temporary directories are preserved on failure for debugging.

## Directory Structure

```
tests/
├── __init__.py           # Package marker
├── conftest.py           # Pytest configuration
├── test_roundtrip.py     # Roundtrip tests
├── README.md             # This file
└── fixtures/             # Test proto files
    ├── __init__.py
    └── enum.proto        # Enum test fixture
```
