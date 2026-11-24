//! # refract-core
//!
//! The analysis core of Refract: regex flavour front-ends, a flavour-independent IR, a
//! character-set algebra over Unicode scalar values, and two independent automaton engines
//! (symbolic derivatives; Thompson + subset construction) with two minimisers.
//!
//! Pipeline: pattern text + flavour → [`flavor::parse`] → [`ir::Regex`] (all flavour semantics
//! resolved) → lowering to a regular term → automata → analyses.

#![forbid(unsafe_code)]

pub mod alphabet;
pub mod automata;
pub mod budget;
pub mod charset;
pub mod deriv;
pub mod dfa;
pub mod equiv;
pub mod error;
pub mod flavor;
pub mod interp;
pub mod ir;
pub mod minimize;
pub mod regular;
pub mod tables;
pub mod thompson;
pub mod unicode;
pub mod utf16;

pub use automata::Engine;
pub use budget::Budget;
pub use charset::CharSet;
pub use dfa::Dfa;
pub use error::{Error, ErrorKind, Result, Span};
pub use flavor::{Flavor, parse};
pub use ir::Regex;
pub use regular::{Op, Re};
