// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;

use super::fixups::parse_stamp;
use super::Source;

/// How a generated file's stamp differs from the current sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Staleness {
    /// The file has no stamp.
    NoStamp,
    /// Sources extracted since (`new`), and sources gone (`gone`).
    Differs { new: Vec<String>, gone: Vec<String> },
}

/// `None` when `text`'s stamp names exactly the current sources.
pub fn staleness(text: &str, sources: &[Source]) -> Option<Staleness> {
    let Some(stamped) = parse_stamp(text) else {
        return Some(Staleness::NoStamp);
    };
    let current: BTreeSet<String> = sources.iter().map(|s| s.identity.clone()).collect();
    if stamped == current {
        return None;
    }
    Some(Staleness::Differs {
        new: current.difference(&stamped).cloned().collect(),
        gone: stamped.difference(&current).cloned().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advise::fixups::stamp;

    fn src(id: &str) -> Source {
        Source {
            identity: id.into(),
            build_time: 0,
            group: None,
            paths: Default::default(),
            metadata: vec![],
        }
    }

    #[test]
    fn current_new_and_gone() {
        let text = stamp(&[src("a"), src("b")]);
        assert_eq!(staleness(&text, &[src("b"), src("a")]), None);
        assert_eq!(
            staleness(&text, &[src("a"), src("c")]),
            Some(Staleness::Differs {
                new: vec!["c".into()],
                gone: vec!["b".into()],
            })
        );
        assert_eq!(staleness("x", &[]), Some(Staleness::NoStamp));
    }
}
