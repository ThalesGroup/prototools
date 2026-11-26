# SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

# The quick world (spec 0490 S3): the development cycle's, every input
# built at cargoProfile = "quick", as ci-quick's.
(import ./default.nix { cargoProfile = "quick"; }).dev-shell
