//! `refract` — command-line interface to refract-core.
//!
//! Exit codes: 0 success, 1 pattern rejected, 2 usage or analysis error.

use anyhow::Result;
use clap::{Parser, Subcommand};
use refract_core::Flavor;
use std::process::ExitCode;

mod commands;

#[derive(Parser)]
#[command(
    name = "refract",
    version,
    about = "Same pattern, different meaning: see exactly where.",
    long_about = "Refract parses regexes from several engines' flavours into one formal model, \
                  starting with an exact, span-preserving parser."
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
        Command::Flavors => commands::flavors(),
    }
}
