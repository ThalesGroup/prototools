// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Store undated snapshots of files in git, and rebuild one linear history
//! from them (specs 0450, 0452).
//!
//! A snapshot is a branch whose single, parentless commit holds the files
//! a source held, and a block saying where they come from. `settle` orders
//! the snapshots with the `seriation` crate and rebuilds the history from
//! them. What is specific to a kind of file comes through the traits of
//! [`plugins`].

pub mod format;
pub mod plugins;
pub mod store;

use std::fmt;

/// Why an operation failed.
#[derive(Debug)]
pub enum Error {
    /// Git refused, or the repository could not be read.
    Git(String),
    /// The operation would contradict what the repository holds.
    Refused(String),
    /// A check this crate makes on its own output failed.
    Bug(String),
    /// Reading files.
    Io(std::io::Error),
}

impl Error {
    pub(crate) fn git(e: impl fmt::Display) -> Self {
        Error::Git(e.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Git(m) => write!(f, "git: {m}"),
            Error::Refused(m) => f.write_str(m),
            Error::Bug(m) => write!(f, "internal error: {m}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
