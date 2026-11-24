//! Real-engine oracle harness (plan §5.10, protocol: `oracle/protocol.md`).
//!
//! Spawns one long-lived runner process per engine and talks JSON lines to it. Runners are the
//! scripts under `oracle/`, embedded here with `include_str!` so the CLI works without a repo
//! checkout. Two modes:
//!
//! - [`Mode::Native`]: run the engine installed on this machine (`python3`, `node`).
//! - [`Mode::Docker`]: run it in a pinned image (`docker run -i --rm refract-oracle-<engine>`),
//!   which is what CI and reproducible reports use.
//!
//! `REFRACT_ORACLE=native|docker` selects the mode; the default is native.

#![forbid(unsafe_code)]

use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const PYTHON_RUNNER: &str = include_str!("../../../oracle/python/runner.py");
const NODE_RUNNER: &str = include_str!("../../../oracle/node/runner.mjs");

/// Engines with an external runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EngineKind {
    Python,
    Node,
    /// Docker only (`oracle/go`): the runner is a compiled Go program.
    Go,
    /// Docker only (`oracle/java`).
    Java,
}

impl EngineKind {
    pub fn name(self) -> &'static str {
        match self {
            EngineKind::Python => "python",
            EngineKind::Node => "node",
            EngineKind::Go => "go",
            EngineKind::Java => "java",
        }
    }

    /// The Docker image built from `oracle/<engine>/Dockerfile`.
    pub fn image(self) -> String {
        format!("refract-oracle-{}", self.name())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Native,
    Docker,
}

impl Mode {
    /// From `REFRACT_ORACLE`, defaulting to native.
    pub fn from_env() -> Mode {
        match std::env::var("REFRACT_ORACLE").as_deref() {
            Ok("docker") => Mode::Docker,
            _ => Mode::Native,
        }
    }
}

/// The match operation, as the runners name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Full,
    Search,
    Compile,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Full => "full",
            Op::Search => "search",
            Op::Compile => "compile",
        }
    }
}

/// What the engine did with one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The engine rejected the pattern, with its own message.
    Rejected(String),
    /// One boolean per input, in order.
    Matched(Vec<bool>),
}

/// A running engine.
pub struct Engine {
    kind: EngineKind,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    version: String,
}

impl Engine {
    /// Starts the runner and asks it for its version.
    pub fn spawn(kind: EngineKind, mode: Mode) -> io::Result<Engine> {
        // Go and Java runners need a build step, so they always run in their pinned image.
        let mode = match kind {
            EngineKind::Go | EngineKind::Java => Mode::Docker,
            _ => mode,
        };
        let mut cmd = match mode {
            Mode::Native => match kind {
                EngineKind::Python => {
                    let mut c = Command::new("python3");
                    c.args(["-I", "-c", PYTHON_RUNNER]);
                    c
                }
                EngineKind::Node => {
                    let mut c = Command::new("node");
                    c.args(["--input-type=module", "-e", NODE_RUNNER]);
                    c
                }
                EngineKind::Go | EngineKind::Java => unreachable!("docker-only engines"),
            },
            Mode::Docker => {
                let mut c = Command::new("docker");
                c.args(["run", "-i", "--rm", "--network=none", &kind.image()]);
                c
            }
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no stdin"))?;
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("no stdout"))?,
        );
        let mut engine = Engine {
            kind,
            child,
            stdin,
            stdout,
            next_id: 1,
            version: String::new(),
        };
        let v = engine.request(json!({"id": 0, "op": "version"}))?;
        engine.version = v["version"].as_str().unwrap_or("?").to_string();
        Ok(engine)
    }

    pub fn kind(&self) -> EngineKind {
        self.kind
    }

    /// Engine version as reported by the runner (e.g. `3.15.2`, V8 `14.6.202.34`).
    pub fn version(&self) -> &str {
        &self.version
    }

    fn request(&mut self, req: Value) -> io::Result<Value> {
        writeln!(self.stdin, "{}", ascii_json(&req))?;
        self.stdin.flush()?;
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Err(io::Error::other(format!(
                "{} runner exited",
                self.kind.name()
            )));
        }
        serde_json::from_str(&line).map_err(io::Error::other)
    }

    /// Compiles `pattern` and runs it on every input under `op`.
    pub fn run(&mut self, pattern: &str, op: Op, inputs: &[&str]) -> io::Result<Outcome> {
        self.run_with_flags(pattern, "", op, inputs)
    }

    /// Like [`Engine::run`] with engine flags (JavaScript: `"u"`, `"v"`, `"i"`, ...).
    pub fn run_with_flags(
        &mut self,
        pattern: &str,
        flags: &str,
        op: Op,
        inputs: &[&str],
    ) -> io::Result<Outcome> {
        let id = self.next_id;
        self.next_id += 1;
        let resp = self.request(json!({
            "id": id, "pattern": pattern, "flags": flags, "op": op.name(), "inputs": inputs,
        }))?;
        if resp["id"].as_u64() != Some(id) {
            return Err(io::Error::other(format!("runner out of sync: {resp}")));
        }
        if resp["ok"].as_bool() == Some(true) {
            let results = resp["results"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_bool().unwrap_or(false)).collect())
                .unwrap_or_default();
            Ok(Outcome::Matched(results))
        } else {
            Ok(Outcome::Rejected(
                resp["error"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_string(),
            ))
        }
    }
}

/// Serialises as ASCII-only JSON: every non-ASCII char becomes `\uXXXX` (a surrogate pair for
/// astral chars). Runners read line by line, and some line readers (Node's `readline`) also
/// split on U+2028/U+2029, which plain JSON leaves unescaped.
fn ascii_json(v: &Value) -> String {
    let raw = v.to_string();
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skips (with a note) when the engine is not installed, so `cargo test` works anywhere.
    fn engine(kind: EngineKind) -> Option<Engine> {
        match Engine::spawn(kind, Mode::Native) {
            Ok(e) => Some(e),
            Err(e) => {
                eprintln!("skipping: {} not available natively ({e})", kind.name());
                None
            }
        }
    }

    #[test]
    fn python_runner_round_trip() {
        let Some(mut py) = engine(EngineKind::Python) else {
            return;
        };
        assert!(!py.version().is_empty());
        let out = py.run(r"^\w+$", Op::Full, &["abc", "abc\n", "é"]).unwrap();
        assert_eq!(out, Outcome::Matched(vec![true, false, true]));
        assert!(matches!(
            py.run("a**", Op::Compile, &[]).unwrap(),
            Outcome::Rejected(_)
        ));
    }

    #[test]
    fn line_separators_survive_the_protocol() {
        let Some(mut js) = engine(EngineKind::Node) else {
            return;
        };
        let out = js
            .run("x.y", Op::Full, &["x\u{2028}y", "x\u{2029}y", "x😀y"])
            .unwrap();
        assert_eq!(out, Outcome::Matched(vec![false, false, false]));
        let out = js.run("b", Op::Search, &["b"]).unwrap();
        assert_eq!(out, Outcome::Matched(vec![true]));
    }

    #[test]
    fn node_runner_round_trip() {
        let Some(mut js) = engine(EngineKind::Node) else {
            return;
        };
        let out = js.run(r"^\w+$", Op::Full, &["abc", "é"]).unwrap();
        assert_eq!(out, Outcome::Matched(vec![true, false]));
        let out = js.run_with_flags("^.$", "u", Op::Full, &["😀"]).unwrap();
        assert_eq!(out, Outcome::Matched(vec![true]));
    }
}
