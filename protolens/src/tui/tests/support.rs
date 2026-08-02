// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The shared test fixtures, reached through one name.
//!
//! Each fixture lives in a `support_*` sibling grouped by subject; this
//! file is the door they are all opened through, so a test writes
//! `use super::support::*;` without having to know which sibling holds
//! what.

pub(super) use prototext_core::helpers::{WT_LEN, WT_VARINT};
pub(super) use ratatui::backend::TestBackend;

pub(super) use super::support_any::*;
pub(super) use super::support_basic::*;
pub(super) use super::support_build::*;
pub(super) use super::support_export::*;
pub(super) use super::support_inspect::*;
pub(super) use super::support_repeated::*;
pub(super) use super::support_typed::*;
