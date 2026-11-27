// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Where git-seriate keeps what (spec 0499 S2): snapshots under
//! `refs/seriate/snapshots/`, a history as a branch per lineage,
//! `<history>/<lineage>`, the history's record under
//! `refs/seriate/histories/`, and the lineage statements' record,
//! `refs/seriate/lineages`.

use crate::Error;

/// The namespace of snapshots: `refs/seriate/snapshots/<identity>`.
pub const SNAPSHOTS: &str = "refs/seriate/snapshots/";
/// The namespace of the histories' records.
pub const HISTORIES: &str = "refs/seriate/histories/";
/// The lineage statements' record (S1b).
pub const LINEAGES: &str = "refs/seriate/lineages";
/// Where `migrate` keeps a history's former branch, until pruned (S4).
pub const MIGRATED: &str = "refs/seriate/migrated/";
/// The lineage of every snapshot no statement names (S1).
pub const DEFAULT_LINEAGE: &str = "default";

/// `name` as a full ref name, refused, saying what it names, when git would
/// refuse it.
pub fn full_name(name: &str, what: &str) -> Result<gix::refs::FullName, Error> {
    name.try_into()
        .map_err(|e| Error::Refused(format!("{what} {name:?}: not a valid ref name: {e}")))
}

/// A snapshot's ref, `refs/seriate/snapshots/<identity>`.
pub fn snapshot_ref(identity: &str) -> Result<gix::refs::FullName, Error> {
    full_name(&format!("{SNAPSHOTS}{identity}"), "snapshot")
}

/// A lineage's branch name, `<history>/<lineage>`.
pub fn lineage_branch(history: &str, lineage: &str) -> String {
    format!("{history}/{lineage}")
}

/// A history's record, `refs/seriate/histories/<history>`.
pub fn record_ref(history: &str) -> Result<gix::refs::FullName, Error> {
    full_name(&format!("{HISTORIES}{history}"), "history")
}

/// The lineage statements' record.
pub fn lineages_ref() -> gix::refs::FullName {
    LINEAGES.try_into().expect("a valid ref name")
}

/// A lineage name a statement may give: one ref component, not `default`
/// (every snapshot no statement names is in it).
pub fn check_lineage_name(name: &str) -> Result<(), Error> {
    if name == DEFAULT_LINEAGE {
        return Err(Error::Refused(format!(
            "{DEFAULT_LINEAGE:?} is the lineage of every snapshot no statement names: \
             use `lineage unset`"
        )));
    }
    if name.is_empty() || name.contains('/') {
        return Err(Error::Refused(format!(
            "lineage {name:?}: one ref component, with no '/'"
        )));
    }
    full_name(&format!("refs/heads/h/{name}"), "lineage").map(|_| ())
}

/// Move the snapshot branches of before spec 0499 under
/// `refs/seriate/snapshots/` (S4): each set at its new ref, then its branch
/// deleted. Returns the identities moved, or to move with `dry_run`.
pub fn migrate_snapshots(
    repo: &gix::Repository,
    dry_run: bool,
    legacy: Option<&dyn crate::plugins::LegacyReader>,
) -> Result<Vec<String>, Error> {
    use gix::refs::transaction::PreviousValue;
    let former = crate::store::former_snapshots(repo, legacy)?;
    let mut moved = Vec::with_capacity(former.len());
    for listed in former {
        if !dry_run {
            let new = snapshot_ref(&listed.identity)?;
            match crate::store::tip(repo, &new)? {
                Some(there) if there == listed.commit => {}
                Some(there) => {
                    return Err(Error::Refused(format!(
                        "snapshot {:?} is a branch as {}, and under {SNAPSHOTS} as {there}",
                        listed.identity, listed.commit
                    )))
                }
                None => {
                    crate::store::set_ref(repo, &new, PreviousValue::MustNotExist, listed.commit)?
                }
            }
            crate::store::delete_ref(
                repo,
                crate::store::branch_ref(&listed.identity)?,
                listed.commit,
            )?;
        }
        moved.push(listed.identity);
    }
    Ok(moved)
}
