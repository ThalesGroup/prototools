// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The textproto (protobuf text format) grammar for tree-sitter, laid out as
//! tree-sitter's own grammar crates are (spec 0414 S1).
//!
//! It is a modified copy of PorterAtGoogle/tree-sitter-textproto that also
//! parses a bare field number as a field name, which prototools' output
//! uses (spec 0121).
//!
//! ```
//! let mut parser = tree_sitter::Parser::new();
//! parser
//!     .set_language(&tree_sitter_textproto::LANGUAGE.into())
//!     .expect("the grammar loads");
//! let tree = parser.parse("name: 1", None).expect("it parses");
//! assert!(!tree.root_node().has_error());
//! ```

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_textproto() -> *const ();
}

/// The grammar, for `tree_sitter::Parser::set_language`.
// SAFETY: `tree_sitter_textproto` is the parser `build.rs` compiles, which
// returns a `TSLanguage` pointer valid for the life of the program.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_textproto) };

/// The syntax highlighting query (spec 0116).
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../highlights.scm");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_parses_without_error() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&LANGUAGE.into()).unwrap();
        let tree = parser
            .parse("name: 1\nnested { 2: \"x\" }\n", None)
            .unwrap();
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn the_highlights_query_compiles_against_the_grammar() {
        tree_sitter::Query::new(&LANGUAGE.into(), HIGHLIGHTS_QUERY).unwrap();
    }
}
