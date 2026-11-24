//! # refract-core
//!
//! The analysis core of Refract: regex flavour front-ends, a flavour-independent IR, a
//! character-set algebra over Unicode scalar values. Automata arrive in P1.
//!
//! Pipeline: pattern text + flavour → [`flavor::parse`] → [`ir::Regex`] (all flavour semantics
//! resolved) → lowering to a regular term → automata → analyses.

#![forbid(unsafe_code)]

pub mod budget;
pub mod charset;
pub mod error;
pub mod flavor;
pub mod ir;
pub mod unicode;

pub use budget::Budget;
pub use charset::CharSet;
pub use error::{Error, ErrorKind, Result, Span};
pub use flavor::{Flavor, parse};
pub use ir::Regex;
