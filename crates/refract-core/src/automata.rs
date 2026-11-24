//! One entry point for building minimal DFAs with either engine.

use crate::alphabet::Alphabet;
use crate::budget::Budget;
use crate::dfa::Dfa;
use crate::error::Result;
use crate::minimize;
use crate::regular::Re;
use crate::{deriv, thompson};
use std::sync::Arc;

/// Which construction to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Engine {
    /// Brzozowski derivatives (primary).
    Derivative,
    /// Thompson NFA + subset construction (reference).
    Thompson,
}

impl std::str::FromStr for Engine {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "deriv" | "derivative" | "derivatives" => Ok(Engine::Derivative),
            "thompson" | "subset" => Ok(Engine::Thompson),
            other => Err(format!("unknown engine `{other}` (use deriv or thompson)")),
        }
    }
}

/// The shared minterm alphabet of several terms.
pub fn alphabet_for(terms: &[&Re]) -> Arc<Alphabet> {
    let mut sets = Vec::new();
    for t in terms {
        t.collect_sets(&mut sets);
    }
    Arc::new(Alphabet::new(&sets))
}

/// A (not necessarily minimal) complete DFA for `re`.
pub fn dfa(re: &Re, alphabet: &Arc<Alphabet>, engine: Engine, budget: &Budget) -> Result<Dfa> {
    match engine {
        Engine::Derivative => deriv::to_dfa(re, alphabet, budget),
        Engine::Thompson => thompson::to_dfa(re, alphabet, budget),
    }
}

/// The minimal complete DFA for `re` (Hopcroft), canonically numbered.
pub fn minimal_dfa(
    re: &Re,
    alphabet: &Arc<Alphabet>,
    engine: Engine,
    budget: &Budget,
) -> Result<Dfa> {
    Ok(minimize::hopcroft(&dfa(re, alphabet, engine, budget)?))
}

/// DFAs for several lowered regexes over one shared alphabet of scalar values. Code-unit
/// languages (JS without `u`, `Domain::Utf16`) are lifted exactly (`utf16`).
pub fn shared_dfas(
    langs: &[(&Re, crate::ir::Domain)],
    budget: &Budget,
) -> Result<(Arc<Alphabet>, Vec<Dfa>)> {
    use crate::ir::Domain;
    let mut sets = Vec::new();
    let mut units = Vec::new();
    for (re, d) in langs {
        match d {
            Domain::Scalars => {
                re.collect_sets(&mut sets);
                units.push(None);
            }
            Domain::Utf16 => {
                let u = crate::utf16::UnitDfa::new(re, budget)?;
                sets.extend(u.cells());
                units.push(Some(u));
            }
        }
    }
    let alphabet = Arc::new(Alphabet::new(&sets));
    let mut out = Vec::new();
    for ((re, _), u) in langs.iter().zip(&units) {
        out.push(match u {
            Some(u) => u.lift(&alphabet),
            None => crate::deriv::to_dfa(re, &alphabet, budget)?,
        });
    }
    Ok((alphabet, out))
}

/// The DFA (over scalar values) of a parsed regex under an operation, in any domain.
pub fn regex_dfa(re: &crate::ir::Regex, op: crate::regular::Op, budget: &Budget) -> Result<Dfa> {
    let lang = crate::regular::lower(re, op, budget)?;
    let (_, mut d) = shared_dfas(&[(&lang, re.domain)], budget)?;
    Ok(d.remove(0))
}
