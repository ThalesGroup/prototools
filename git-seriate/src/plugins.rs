// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Plugin points (spec 0452 S2), and their generic defaults.

use crate::format::Rejection;
use crate::store::Item;

/// Finds the items in a source's bytes.
pub trait Extractor {
    fn extract(&self, bytes: &[u8]) -> Extracted;
}

/// What an [`Extractor`] found, and what it refused.
#[derive(Debug, Clone, Default)]
pub struct Extracted {
    pub items: Vec<Item>,
    pub rejected: Vec<Rejection>,
    /// What the extractor wants kept beside them, shown, never interpreted.
    pub record: Vec<(String, String)>,
}

/// How an item is stored in the tree, and back.
pub trait Renderer {
    fn render(&self, path: &str, item: &[u8]) -> Result<Vec<u8>, String>;
    fn parse(&self, path: &str, stored: &[u8]) -> Result<Vec<u8>, String>;
}

/// The form versions are identified and compared by (spec 0450 S2).
pub trait Canonicalizer {
    /// The rule sets composed, each with its version.
    fn rule_sets(&self) -> Vec<(String, u32)>;
    fn canon(&self, path: &str, item: &[u8]) -> Vec<u8>;
}

/// Which paths hold items. A manual commit changing none of them is set
/// aside by `settle`; one changing any stops it.
pub trait ItemPaths {
    fn is_item(&self, path: &str) -> bool;
    /// Whether this is the default of "every path", for `settle`'s warning.
    fn is_default(&self) -> bool {
        false
    }
}

/// Reads a commit block of an older format (spec 0452 S2), so that a
/// repository written by another tool can be settled and compacted;
/// `compact` writes what it reads in `format: 1`.
pub trait LegacyReader {
    /// The block `message` carries, or `None` if it carries none.
    fn read(&self, message: &str) -> Option<crate::format::Block>;
}

/// Stores items as they are.
#[derive(Debug, Clone, Copy, Default)]
pub struct AsIs;

impl Renderer for AsIs {
    fn render(&self, _: &str, item: &[u8]) -> Result<Vec<u8>, String> {
        Ok(item.to_vec())
    }
    fn parse(&self, _: &str, stored: &[u8]) -> Result<Vec<u8>, String> {
        Ok(stored.to_vec())
    }
}

/// No rule set: every item is its own canonical form.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRules;

impl Canonicalizer for NoRules {
    fn rule_sets(&self) -> Vec<(String, u32)> {
        Vec::new()
    }
    fn canon(&self, _: &str, item: &[u8]) -> Vec<u8> {
        item.to_vec()
    }
}

/// Canonicalizers applied in order, their rule sets recorded together
/// (spec 0453 S4): a tool composes the rule sets it ships with its own.
pub struct Composed(pub Vec<Box<dyn Canonicalizer>>);

impl Canonicalizer for Composed {
    fn rule_sets(&self) -> Vec<(String, u32)> {
        self.0.iter().flat_map(|c| c.rule_sets()).collect()
    }
    fn canon(&self, path: &str, item: &[u8]) -> Vec<u8> {
        let mut out = item.to_vec();
        for c in &self.0 {
            out = c.canon(path, &out);
        }
        out
    }
}

/// Item paths by a glob (`seriate.itemPattern`); every path without one.
#[derive(Debug, Clone, Default)]
pub struct Pattern(pub Option<String>);

impl ItemPaths for Pattern {
    fn is_item(&self, path: &str) -> bool {
        self.0.as_deref().is_none_or(|g| glob_matches(g, path))
    }
    fn is_default(&self) -> bool {
        self.0.is_none()
    }
}

/// Whether `text` matches `glob`: `*` any run of characters, `/` included,
/// `?` any one character, anything else itself.
pub fn glob_matches(glob: &str, text: &str) -> bool {
    let (g, t): (Vec<char>, Vec<char>) = (glob.chars().collect(), text.chars().collect());
    // Positions to resume from after the last `*`.
    let (mut gi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if gi < g.len() && (g[gi] == '?' || g[gi] == t[ti]) {
            gi += 1;
            ti += 1;
        } else if gi < g.len() && g[gi] == '*' {
            star = Some(gi);
            mark = ti;
            gi += 1;
        } else if let Some(s) = star {
            gi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    g[gi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::glob_matches;

    #[test]
    fn globs() {
        assert!(glob_matches("*.txt", "a.txt"));
        assert!(glob_matches("*.txt", "dir/a.txt"));
        assert!(!glob_matches("*.txt", "a.txt.license"));
        assert!(glob_matches("build*/part?", "build0001/part3"));
        assert!(!glob_matches("build*/part?", "build0001/part10"));
        assert!(glob_matches("*", ""));
        assert!(!glob_matches("a", ""));
    }
}
