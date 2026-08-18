//! Calcium — a symbolic calculation engine for Markdown documents in which
//! any line can be a calculation.
//!
//! The design rests on one observation: a *unit is just a definition*
//! (`ft = 3048 m / 10000`), so dimensional analysis needs no dedicated engine —
//! it falls out of ordinary symbolic algebra. Complex numbers work the same
//! way: `i` is a symbol with the rewrite `i^2 -> -1`.

pub mod ast;
pub mod builtins;
pub mod check;
pub mod doc;
pub mod eval;
pub mod format;
pub mod lexer;
pub mod num;
pub mod parser;
pub mod plot;
pub mod simplify;
pub mod solve;
pub mod typst;
