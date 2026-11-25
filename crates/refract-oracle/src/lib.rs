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
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

const PYTHON_RUNNER: &str = include_str!("../runners/runner.py");
const NODE_RUNNER: &str = include_str!("../runners/runner.mjs");

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

    /// Version prefix of the pinned engine the models were built from (`oracle/*/Dockerfile`):
    /// CPython 3.15, V8 14.6 (Node 26), Go 1.27, JDK 25. A native engine of another version is
    /// not ground truth: its regex features and Unicode tables differ.
    pub fn pinned(self) -> &'static str {
        match self {
            EngineKind::Python => "3.15.",
            EngineKind::Node => "14.6.",
            EngineKind::Go => "go1.27",
            EngineKind::Java => "25.",
        }
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

/// The result of timing one haystack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Timing {
    /// Wall-clock time of the match call inside the runner.
    Took(Duration),
    /// The runner did not answer in time; it was killed and must be respawned.
    TimedOut,
    /// The engine rejected the pattern.
    Rejected(String),
}

/// A running engine.
pub struct Engine {
    kind: EngineKind,
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<io::Result<String>>,
    next_id: u64,
    version: String,
    /// Container name in Docker mode (killed on timeout: killing the client is not enough).
    container: Option<String>,
    dead: bool,
}

static CONTAINERS: AtomicU64 = AtomicU64::new(0);

impl Engine {
    /// Starts the runner and asks it for its version.
    pub fn spawn(kind: EngineKind, mode: Mode) -> io::Result<Engine> {
        Engine::spawn_with(kind, mode, &[])
    }

    /// Like [`Engine::spawn`], passing `engine_args` to the engine itself (Node only, e.g.
    /// `--enable-experimental-regexp-engine-on-excessive-backtracks`).
    pub fn spawn_with(kind: EngineKind, mode: Mode, engine_args: &[&str]) -> io::Result<Engine> {
        if !engine_args.is_empty() && kind != EngineKind::Node {
            return Err(io::Error::other(
                "engine arguments are supported for node only",
            ));
        }
        // Go and Java runners need a build step, so they always run in their pinned image.
        let mode = match kind {
            EngineKind::Go | EngineKind::Java => Mode::Docker,
            _ => mode,
        };
        let mut container = None;
        let mut cmd = match mode {
            Mode::Native => match kind {
                EngineKind::Python => {
                    let mut c = Command::new("python3");
                    c.args(["-I", "-c", PYTHON_RUNNER]);
                    c
                }
                EngineKind::Node => {
                    let mut c = Command::new("node");
                    c.args(engine_args);
                    c.args(["--input-type=module", "-e", NODE_RUNNER]);
                    c
                }
                EngineKind::Go | EngineKind::Java => unreachable!("docker-only engines"),
            },
            Mode::Docker => {
                // Only a locally built image: a missing one would make `docker run` try a
                // registry pull (slow, or a hang on Windows runners) for every spawn.
                let present = Command::new("docker")
                    .args(["image", "inspect", "--format", "{{.Id}}", &kind.image()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
                if !present {
                    return Err(io::Error::other(format!(
                        "docker image {} is not built (docker build -t {} oracle/{})",
                        kind.image(),
                        kind.image(),
                        kind.name()
                    )));
                }
                let name = format!(
                    "refract-oracle-{}-{}-{}",
                    kind.name(),
                    std::process::id(),
                    CONTAINERS.fetch_add(1, Ordering::Relaxed)
                );
                let mut c = Command::new("docker");
                c.args([
                    "run",
                    "-i",
                    "--rm",
                    "--pull=never",
                    "--network=none",
                    "--name",
                    &name,
                ]);
                if engine_args.is_empty() {
                    c.arg(kind.image());
                } else {
                    c.args(["--entrypoint", "node", &kind.image()]);
                    c.args(engine_args);
                    c.arg("/runner.mjs");
                }
                container = Some(name);
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
        let mut stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("no stdout"))?,
        );
        // A reader thread lets requests wait with a timeout.
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let mut line = String::new();
                let r = stdout.read_line(&mut line);
                let stop = !matches!(r, Ok(n) if n > 0);
                if tx.send(r.map(|_| line)).is_err() || stop {
                    break;
                }
            }
        });
        let mut engine = Engine {
            kind,
            child,
            stdin,
            lines,
            next_id: 1,
            version: String::new(),
            container,
            dead: false,
        };
        let v = engine.request(json!({"id": 0, "op": "version"}))?;
        engine.version = v["version"].as_str().unwrap_or("?").to_string();
        if mode == Mode::Native && !engine.version.starts_with(kind.pinned()) {
            // Dropping the engine stops the runner.
            return Err(io::Error::other(format!(
                "native {} reports version {}, but Refract's model is pinned to {}*; use the pinned \
                 image (REFRACT_ORACLE=docker, docker build -t {} oracle/{})",
                kind.name(),
                engine.version,
                kind.pinned(),
                kind.image(),
                kind.name()
            )));
        }
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
        self.request_within(req, None)?
            .ok_or_else(|| io::Error::other("unreachable: no timeout"))
    }

    /// Sends a request and waits at most `timeout` (forever with `None`). `Ok(None)` means it
    /// timed out: the runner is killed and this engine refuses further requests.
    fn request_within(
        &mut self,
        req: Value,
        timeout: Option<Duration>,
    ) -> io::Result<Option<Value>> {
        if self.dead {
            return Err(io::Error::other(format!(
                "{} runner was killed after a timeout",
                self.kind.name()
            )));
        }
        writeln!(self.stdin, "{}", ascii_json(&req))?;
        self.stdin.flush()?;
        let got = match timeout {
            None => self
                .lines
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
            Some(t) => self.lines.recv_timeout(t),
        };
        let line = match got {
            Ok(line) => line?,
            Err(RecvTimeoutError::Timeout) => {
                self.kill();
                return Ok(None);
            }
            Err(RecvTimeoutError::Disconnected) => String::new(),
        };
        if line.is_empty() {
            self.dead = true;
            return Err(io::Error::other(format!(
                "{} runner exited",
                self.kind.name()
            )));
        }
        serde_json::from_str(&line)
            .map(Some)
            .map_err(io::Error::other)
    }

    fn kill(&mut self) {
        self.dead = true;
        if let Some(name) = &self.container {
            let _ = Command::new("docker")
                .args(["kill", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// `false` once the runner was killed (after a timeout) or exited.
    pub fn is_alive(&self) -> bool {
        !self.dead
    }

    /// Times one match of `pattern` on `input` inside the runner (full match or search, with
    /// engine `flags`), waiting at most `timeout`.
    pub fn time(
        &mut self,
        pattern: &str,
        flags: &str,
        full: bool,
        input: &str,
        timeout: Duration,
    ) -> io::Result<Timing> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "id": id, "pattern": pattern, "flags": flags, "op": "time",
            "mode": if full { "full" } else { "search" }, "inputs": [input],
        });
        let Some(resp) = self.request_within(req, Some(timeout))? else {
            return Ok(Timing::TimedOut);
        };
        if resp["ok"].as_bool() != Some(true) {
            return Ok(Timing::Rejected(
                resp["error"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_string(),
            ));
        }
        let ns = resp["times"][0]
            .as_u64()
            .ok_or_else(|| io::Error::other(format!("runner sent no time: {resp}")))?;
        Ok(Timing::Took(Duration::from_nanos(ns)))
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
        if !self.dead {
            // Kill the client; in Docker mode also the named container, which survives it.
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(name) = &self.container {
                let _ = Command::new("docker")
                    .args(["kill", name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
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
    fn timing_and_timeouts() {
        let Some(mut py) = engine(EngineKind::Python) else {
            return;
        };
        let t = py
            .time("^(a+)+$", "", true, "aaaa!", Duration::from_secs(5))
            .unwrap();
        assert!(matches!(t, Timing::Took(_)), "{t:?}");
        let slow = format!("{}!", "a".repeat(40));
        let t = py
            .time("^(a+)+$", "", true, &slow, Duration::from_millis(300))
            .unwrap();
        assert_eq!(t, Timing::TimedOut);
        assert!(!py.is_alive());
        assert!(py.run("a", Op::Full, &["a"]).is_err());
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
