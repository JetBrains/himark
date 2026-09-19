//! Vendored diff core of difftastic (MIT, (c) Wilfred Hughes).
//!
//! Upstream: https://github.com/Wilfred/difftastic, tag 0.71.0,
//! commit b7d119e90ac9f972f03f69508da765dacd302c0c. See README.md for the
//! list of local modifications.
//!
//! This crate contains only the language-agnostic structural diff engine:
//! the `Syntax` tree representation and the graph-based diff algorithm.
//! Trees are built by the caller (himark feeds its own tree-sitter trees);
//! difftastic's parsers, display code and CLI are not included.

pub mod diff;
pub mod hash;
pub mod lines;
pub mod parse;
pub mod words;

pub use parse::syntax;

pub mod options {
    /// Default limit on the size of the graph explored by the shortest-path
    /// algorithm before falling back (upstream src/options.rs).
    pub const DEFAULT_GRAPH_LIMIT: usize = 3_000_000;
}
