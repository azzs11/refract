//! `refract` — command-line interface to refract-core.
//!
//! Exit codes: 0 success, 1 pattern rejected, 2 usage or analysis error.

use anyhow::Result;
use clap::{Parser, Subcommand};
use refract_core::{Engine, Flavor, Op};
use std::process::ExitCode;

mod commands;

#[derive(Parser)]
#[command(
    name = "refract",
    version,
    about = "Same pattern, different meaning: see exactly where.",
    long_about = "Refract parses regexes from several engines' flavours into one formal model, \
                  and builds minimal automata for it."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a pattern and print its intermediate representation.
    Parse {
        /// Source flavour (rust; others arrive in P3).
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
        Command::Dfa {
            flavor,
            op,
            engine,
            dot,
            max_states,
            pattern,
        } => commands::dfa(flavor, &pattern, op, engine, dot, max_states),
        Command::Flavors => commands::flavors(),
    }
}
