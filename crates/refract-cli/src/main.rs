//! `refract` — command-line interface to refract-core.
//!
//! Exit codes: 0 success / equivalent, 1 different or pattern rejected, 2 usage or analysis
//! error, 3 a prediction was contradicted by the real engine (a Refract bug: please report).

use anyhow::Result;
use clap::{Parser, Subcommand};
use refract_core::{Engine, Flavor, Op};
use std::process::ExitCode;

mod check;
mod commands;
mod mcp;
mod oracle;
mod redos;
mod translate;

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
    /// Translate a pattern into other flavours, prove each translation equivalent (full match
    /// and search) and replay coverage strings on both real engines.
    ///
    /// Exit 0 when every translation is verified, 1 when one is not, 3 when an engine
    /// contradicts the model.
    Translate {
        #[arg(long, short = 'f', default_value = "python")]
        from: Flavor,
        #[arg(long, value_delimiter = ',', default_value = "js,python,go,java,rust")]
        to: Vec<Flavor>,
        /// Preserve only this operation (default: both; falls back to full match when only
        /// that has a form in the target).
        #[arg(long)]
        op: Option<Op>,
        #[arg(long)]
        json: bool,
        pattern: String,
    },
    /// Generate test cases covering every state and transition of the minimal DFA, labelled by
    /// the model and confirmed on the real engine.
    Tests {
        #[arg(long, short = 'f', default_value = "python")]
        flavor: Flavor,
        #[arg(long, default_value = "full")]
        op: Op,
        /// json, pytest, jest, junit or go.
        #[arg(long, default_value = "json")]
        format: translate::TestFormat,
        /// Skip the real-engine replay.
        #[arg(long)]
        no_replay: bool,
        pattern: String,
    },
    /// Scan source files (Python, JS/TS, Go, Java, Rust) for regex literals: patterns the engine
    /// rejects, predicted ReDoS, and (with --targets) meaning changes in other engines.
    ///
    /// Exit 1 when a finding reaches --fail-on (default: warning).
    Check {
        /// Files or directories (default: the current directory).
        paths: Vec<std::path::PathBuf>,
        /// Also report where each pattern means something else in these engines.
        #[arg(long, value_delimiter = ',')]
        targets: Vec<Flavor>,
        /// Skip the ReDoS prediction.
        #[arg(long)]
        no_redos: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: check::Format,
        #[arg(long, value_enum, default_value = "warning")]
        fail_on: check::FailOn,
    },
    /// Model Context Protocol server on stdio: lets an assistant check its regexes with these
    /// commands (tools: refract, equiv, translate, redos, tests).
    Mcp,
    /// Many patterns: one `refract refract --json` report per JSON line of input
    /// (`{"from": "python", "pattern": "...", "op": "full", "to": [...], "id": ...}`).
    Batch {
        /// Input file (default: stdin).
        input: Option<std::path::PathBuf>,
        /// Seconds per pattern.
        #[arg(long, default_value_t = 5.0)]
        time_limit: f64,
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
        Command::Translate {
            from,
            to,
            op,
            json,
            pattern,
        } => translate::translate(from, &pattern, &to, op, json),
        Command::Tests {
            flavor,
            op,
            format,
            no_replay,
            pattern,
        } => translate::tests(flavor, &pattern, op, format, no_replay),
        Command::Check {
            paths,
            targets,
            no_redos,
            format,
            fail_on,
        } => {
            let paths = if paths.is_empty() {
                vec![".".into()]
            } else {
                paths
            };
            check::check(&paths, &targets, !no_redos, format, fail_on)
        }
        Command::Mcp => mcp::serve(),
        Command::Batch { input, time_limit } => commands::batch(input.as_deref(), time_limit),
        Command::Flavors => commands::flavors(),
    }
}
