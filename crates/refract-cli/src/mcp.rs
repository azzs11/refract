//! `refract mcp`: a Model Context Protocol server on stdio (JSON-RPC 2.0, one message per line),
//! so an assistant can check the regexes it writes (plan §4 "LLM regex check").
//!
//! Each tool runs this same binary's subcommand with `--json` and returns its report, so tool
//! results carry exactly the CLI's verdicts, including real-engine replay. Nothing but protocol
//! messages is written to stdout.

use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::process::{Command, ExitCode};

const PROTOCOL: &str = "2025-06-18";
const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

fn flavor_schema() -> Value {
    json!({"type": "string", "enum": ["js", "python", "go", "java", "rust"]})
}

fn op_schema() -> Value {
    json!({"type": "string", "enum": ["full", "search"], "default": "full",
           "description": "full: the whole string must match (Python fullmatch, Java matches()); search: a match anywhere (JS test(), Python search)."})
}

fn tools() -> Value {
    json!([
        {
            "name": "refract",
            "title": "One regex across engines",
            "description": "Does this pattern mean the same in JavaScript, Python, Go, Java and Rust? Per engine: SAME (proven), DIFFERS with the shortest string where it differs (replayed on the real engines), or REJECTS.",
            "inputSchema": {"type": "object", "required": ["pattern", "from"], "properties": {
                "pattern": {"type": "string", "description": "The pattern; for JavaScript, a /literal/flags or the bare source."},
                "from": flavor_schema(),
                "to": {"type": "array", "items": flavor_schema(), "description": "Engines to compare against (default: all)."},
                "op": op_schema()}},
        },
        {
            "name": "equiv",
            "title": "Are two regexes equivalent?",
            "description": "Decides whether two patterns (possibly in different flavours) match exactly the same strings; if not, returns the shortest distinguishing string, replayed on the real engines.",
            "inputSchema": {"type": "object", "required": ["a", "b", "flavor_a", "flavor_b"], "properties": {
                "a": {"type": "string"}, "flavor_a": flavor_schema(),
                "b": {"type": "string"}, "flavor_b": flavor_schema(),
                "op": op_schema()}},
        },
        {
            "name": "translate",
            "title": "Translate a regex with a proof",
            "description": "Rewrites a pattern for other engines. A translation is 'verified' only when proven equivalent to the source (full match and search) and replayed on both real engines; constructs with no equivalent are named.",
            "inputSchema": {"type": "object", "required": ["pattern", "from", "to"], "properties": {
                "pattern": {"type": "string"}, "from": flavor_schema(),
                "to": {"type": "array", "items": flavor_schema()}}},
        },
        {
            "name": "redos",
            "title": "ReDoS risk per engine",
            "description": "Predicts worst-case matching time (linear, polynomial, exponential) per engine with an attack string. With measure=true the attack is timed on the real engines (slow).",
            "inputSchema": {"type": "object", "required": ["pattern", "from"], "properties": {
                "pattern": {"type": "string"}, "from": flavor_schema(), "op": op_schema(),
                "measure": {"type": "boolean", "default": false}}},
        },
        {
            "name": "tests",
            "title": "Test strings for a regex",
            "description": "Strings covering every state and transition of the pattern's minimal DFA, each labelled match / no match and confirmed on the real engine.",
            "inputSchema": {"type": "object", "required": ["pattern", "flavor"], "properties": {
                "pattern": {"type": "string"}, "flavor": flavor_schema(), "op": op_schema()}},
        },
    ])
}

fn s<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

fn list(args: &Value, k: &str) -> Option<String> {
    let v = args.get(k)?.as_array()?;
    Some(
        v.iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// The CLI arguments for a tool call, or an error message for the model.
fn argv(name: &str, a: &Value) -> Result<Vec<String>, String> {
    let need = |k: &str| {
        s(a, k)
            .map(str::to_string)
            .ok_or(format!("missing argument `{k}`"))
    };
    let op = s(a, "op").unwrap_or("full").to_string();
    let mut v: Vec<String> = match name {
        "refract" => {
            let mut v = vec![
                "refract".into(),
                "--json".into(),
                "--from".into(),
                need("from")?,
                "--op".into(),
                op,
            ];
            if let Some(to) = list(a, "to") {
                v.extend(["--to".into(), to]);
            }
            v.push(need("pattern")?);
            v
        }
        "equiv" => vec![
            "equiv".into(),
            "--json".into(),
            "--op".into(),
            op,
            format!("--a={}:{}", need("flavor_a")?, need("a")?),
            format!("--b={}:{}", need("flavor_b")?, need("b")?),
        ],
        "translate" => {
            let mut v = vec![
                "translate".into(),
                "--json".into(),
                "--from".into(),
                need("from")?,
            ];
            if let Some(to) = list(a, "to") {
                v.extend(["--to".into(), to]);
            }
            v.push(need("pattern")?);
            v
        }
        "redos" => {
            let mut v = vec![
                "redos".into(),
                "--json".into(),
                "--from".into(),
                need("from")?,
                "--op".into(),
                op,
            ];
            if !a.get("measure").and_then(Value::as_bool).unwrap_or(false) {
                v.push("--no-measure".into());
            }
            v.push(need("pattern")?);
            v
        }
        "tests" => vec![
            "tests".into(),
            "--format".into(),
            "json".into(),
            "--flavor".into(),
            need("flavor")?,
            "--op".into(),
            op,
            need("pattern")?,
        ],
        _ => return Err(format!("unknown tool `{name}`")),
    };
    // A pattern starting with `-` must not be read as an option.
    if let Some(last) = v.last()
        && last.starts_with('-')
        && name != "equiv"
    {
        let p = v.pop().unwrap_or_default();
        v.push("--".into());
        v.push(p);
    }
    Ok(v)
}

fn call_tool(name: &str, args: &Value) -> Value {
    let argv = match argv(name, args) {
        Ok(v) => v,
        Err(e) => return json!({"content": [{"type": "text", "text": e}], "isError": true}),
    };
    let exe = std::env::current_exe().unwrap_or_else(|_| "refract".into());
    match Command::new(exe).args(&argv).output() {
        Err(e) => {
            json!({"content": [{"type": "text", "text": format!("could not run refract: {e}")}], "isError": true})
        }
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            // Exit 1 is a verdict ("different", "not verified", "super-linear"), not a failure.
            let code = out.status.code().unwrap_or(2);
            let structured: Option<Value> = serde_json::from_str(&stdout).ok();
            if code == 2 || structured.is_none() {
                let text = if stderr.trim().is_empty() {
                    stdout
                } else {
                    stderr
                };
                return json!({"content": [{"type": "text", "text": text.trim()}], "isError": true});
            }
            let mut text = stdout.trim().to_string();
            if code == 3 {
                text.push_str("\n\nA real engine contradicted Refract's model (exit 3): treat the verdict as unreliable and report it.");
            }
            json!({"content": [{"type": "text", "text": text}], "structuredContent": structured, "isError": false})
        }
    }
}

fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL);
            let version = if SUPPORTED.contains(&asked) {
                asked
            } else {
                PROTOCOL
            };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "refract", "title": "Refract", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Use these tools to check regexes: whether a pattern means the same in another engine (refract, equiv), to translate it with a proof (translate), to assess ReDoS risk (redos) and to get test strings (tests). Verdicts name the operation (full match or search). DIFFERS verdicts come with the shortest witness, replayed on the real engines when they are installed; 'predicted' means not replayed.",
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(call_tool(name, &args))
        }
        m if m.starts_with("notifications/") => return None,
        _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
    };
    let id = id?; // notifications get no response
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

pub fn serve() -> Result<ExitCode> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch.iter().filter_map(handle).collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            Ok(msg) => handle(&msg),
            Err(e) => Some(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}}),
            ),
        };
        if let Some(r) = reply {
            writeln!(stdout, "{r}")?;
            stdout.flush()?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_list() {
        let r = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        let r = handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 5);
        assert!(
            handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none()
        );
        let r = handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "nope"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn arguments_become_cli_calls() {
        let v = argv(
            "equiv",
            &json!({"a": "-a", "flavor_a": "js", "b": "a", "flavor_b": "python", "op": "search"}),
        )
        .unwrap();
        assert_eq!(
            v,
            [
                "equiv",
                "--json",
                "--op",
                "search",
                "--a=js:-a",
                "--b=python:a"
            ]
        );
        let v = argv("refract", &json!({"pattern": "-x", "from": "python"})).unwrap();
        assert_eq!(v[v.len() - 2..], ["--".to_string(), "-x".to_string()]);
        assert!(argv("refract", &json!({"from": "python"})).is_err());
    }
}
