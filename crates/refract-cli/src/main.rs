//! `refract` — command-line interface to refract-core.
//!
//! Exit codes: 0 success / equivalent, 1 different or pattern rejected, 2 usage or analysis
//! error, 3 a prediction was contradicted by the real engine (a Refract bug: please report).

use anyhow::Result;
use clap::{Parser, Subcommand};
use refract_core::{Engine, Flavor, Op};
use std::process::ExitCode;

mod commands;
mod oracle;
mod redos;

#[derive(Parser)]
#[command(
    name = "refract",
    version,
    about = "Same pattern, different meaning: see exactly where.",
    long_about = "Refract parses regexes from several engines' flavours into one formal model, \
                  decides equivalence with automata, and reports the shortest string where two \
                  patterns disagree, replayed on the real engine."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a pattern and print its intermediate representation.
    Parse {
        /// Source flavour: rust, python, js, go, java.
        #[arg(long, short = 'f', default_value = "rust")]
        flavor: Flavor,
        /// Print the compact S-expression instead of the tree.
        #[arg(long)]
        compact: bool,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
        pattern: String,
    },
    /// Decide whether two patterns behave the same; print the shortest witness if not.
    ///
    /// Patterns are written FLAVOR:PATTERN (e.g. rust:a+). Without a known flavour prefix the
    /// whole argument is the pattern, in the --flavor flavour.
    Equiv {
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        /// Default flavour for arguments without a prefix.
        #[arg(long, short = 'f', default_value = "rust")]
        flavor: Flavor,
        /// Operation the verdict is about: full (whole haystack) or search (match anywhere).
        #[arg(long, default_value = "full")]
        op: Op,
        /// Automaton construction: deriv (default) or thompson (reference).
        #[arg(long, default_value = "deriv")]
        engine: Engine,
        /// Raise the state budget.
        #[arg(long)]
        max_states: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// One pattern, every engine: per engine SAME (proven), DIFFERS (shortest witness,
    /// replayed on both real engines) or REJECTS (confirmed on the engine).
    Refract {
        /// The flavour the pattern was written for.
        #[arg(long, short = 'f', default_value = "python")]
        from: Flavor,
        /// Engines to compare against (comma-separated).
        #[arg(long, value_delimiter = ',', default_value = "js,python,go,java,rust")]
        to: Vec<Flavor>,
        #[arg(long, default_value = "full")]
        op: Op,
        #[arg(long)]
        json: bool,
        pattern: String,
    },
    /// Build the minimal DFA of a pattern.
    Dfa {
        #[arg(long, short = 'f', default_value = "rust")]
        flavor: Flavor,
        #[arg(long, default_value = "full")]
        op: Op,
        #[arg(long, default_value = "deriv")]
        engine: Engine,
        /// Print Graphviz DOT.
        #[arg(long)]
        dot: bool,
        #[arg(long)]
        max_states: Option<usize>,
        pattern: String,
    },
    /// ReDoS lab: predict worst-case backtracking growth per engine (linear, polynomial,
    /// exponential) with an attack string, then measure it on the real engines.
    ///
    /// Exit 1 when some engine is confirmed super-linear, 3 when an engine contradicts the model.
    Redos {
        /// The flavour the pattern was written for.
        #[arg(long, short = 'f', default_value = "python")]
        from: Flavor,
        /// Engines (comma-separated); js-linear is V8 with its linear fallback enabled.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "python,js,js-linear,java,go,rust"
        )]
        engines: Vec<redos::Target>,
        #[arg(long, default_value = "full")]
        op: Op,
        /// Per-measurement timeout in seconds.
        #[arg(long, default_value_t = 3.0)]
        timeout: f64,
        /// Static prediction only.
        #[arg(long)]
        no_measure: bool,
        /// Measure this attack instead of the predicted one:
        /// '{"prefix": "", "pumps": ["a"], "suffix": "!"}'.
        #[arg(long, value_parser = redos::parse_attack)]
        attack: Option<refract_core::redos::Attack>,
        #[arg(long)]
        json: bool,
        pattern: String,
    },
    /// List flavours and their semantics profiles.
    Flavors,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Parse {
            flavor,
            compact,
            json,
            pattern,
        } => commands::parse(flavor, &pattern, compact, json),
        Command::Equiv {
            a,
            b,
            flavor,
            op,
            engine,
            max_states,
            json,
        } => commands::equiv(
            &commands::split_flavor(&a, flavor),
            &commands::split_flavor(&b, flavor),
            op,
            engine,
            max_states,
            json,
        ),
        Command::Refract {
            from,
            to,
            op,
            json,
            pattern,
        } => commands::refract(from, &pattern, &to, op, json),
        Command::Dfa {
            flavor,
            op,
            engine,
            dot,
            max_states,
            pattern,
        } => commands::dfa(flavor, &pattern, op, engine, dot, max_states),
        Command::Redos {
            from,
            engines,
            op,
            timeout,
            no_measure,
            attack,
            json,
            pattern,
        } => redos::redos(
            from,
            &pattern,
            &engines,
            op,
            std::time::Duration::from_secs_f64(timeout.max(0.05)),
            no_measure,
            attack,
            json,
        ),
        Command::Flavors => commands::flavors(),
    }
}
