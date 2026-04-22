//! LogQL language intelligence: tokenizer, parser, AST, analyzer, and completion engine.
//!
//! This crate is dependency-free (no runtime, no HTTP, no framework) and can be
//! used by both the embedded TUI and as the backend of the `logql-lsp` language
//! server binary.

pub mod analyzer;
pub mod ast;
pub mod completions;
pub mod keywords;
pub mod parser;
pub mod tokenizer;
pub mod util;
