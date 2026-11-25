//! `refract translate` (verified translation, ADR-0013) and `refract tests` (coverage test
//! generation). Both replay their claims on the real engines before calling them confirmed.

use crate::oracle::Oracles;
use anyhow::{Result, anyhow};
use refract_core::equiv::{Verdict, escape};
use refract_core::testgen::{Case, cover, sample};
use refract_core::translate::{Outcome, Scope, translate_for};
use refract_core::{Budget, Flavor, Op, parse as parse_pattern};
use serde_json::{Value, json};
use std::process::ExitCode;

/// Real-engine replay of a translation: both engines run the coverage cases of the source.
enum Replay {
    Confirmed(usize),
    Predicted(String),
    Contradicted(String),
}

fn replay(
    oracles: &mut Oracles,
    from: Flavor,
    src: &str,
    to: Flavor,
    dst: &str,
    cases: &[Case],
    op: Op,
) -> Replay {
    let inputs: Vec<&str> = cases.iter().map(|c| c.text.as_str()).collect();
    let (a, b) = (
        oracles.run_many(from, src, op, &inputs),
        oracles.run_many(to, dst, op, &inputs),
    );
    match (a, b) {
        (Ok(x), Ok(y)) => {
            for ((c, x), y) in cases.iter().zip(x).zip(y) {
                if x != c.matches || y != c.matches {
                    return Replay::Contradicted(format!(
                        "on {}: model {}, {from} {}, {to} {}",
                        escape(&c.text),
                        c.matches,
                        x,
                        y
                    ));
                }
            }
            Replay::Confirmed(cases.len())
        }
        (Err(e), _) | (_, Err(e)) => Replay::Predicted(e),
    }
}

fn verdict_text(v: &Verdict) -> String {
    match v {
        Verdict::Equivalent => "proven".into(),
        Verdict::NoDifferenceUpTo { max_len, .. } => {
            format!("no difference up to length {max_len} (bounded)")
        }
        Verdict::Different(w) => format!("DIFFERS on {}", escape(&w.text)),
    }
}

pub fn translate(
    from: Flavor,
    pattern: &str,
    targets: &[Flavor],
    op: Option<Op>,
    as_json: bool,
) -> Result<ExitCode> {
    let budget = Budget::default().start();
    let src = parse_pattern(from, pattern)
        .map_err(|e| anyhow!("{from} pattern:\n{}", e.render(pattern)))?;
    let mut oracles = Oracles::default();
    let mut worst = 0u8;
    let mut rows: Vec<Value> = Vec::new();
    for &to in targets {
        if to == from {
            continue;
        }
        let run = |scope| {
            translate_for(&src, to, scope, &budget).map_err(|e| anyhow!("{to}: {}", e.message))
        };
        let mut scope = op.map_or(Scope::Both, Scope::Only);
        let mut out = run(scope)?;
        if op.is_none() && matches!(out, Outcome::Unsupported { .. }) {
            // No form that preserves search; one for full match may still exist.
            let full = run(Scope::Only(Op::FullMatch))?;
            if full.verified() {
                scope = Scope::Only(Op::FullMatch);
                out = full;
            }
        }
        let scope_name = match scope {
            Scope::Both => "full match and search".to_string(),
            Scope::Only(o) => format!("{} only", o.name()),
        };
        let row = match &out {
            Outcome::Unsupported { construct, span } => {
                worst = worst.max(1);
                json!({"target": to.name(), "status": "untranslatable", "construct": construct,
                       "at": pattern.get(span.start..span.end).unwrap_or("")})
            }
            Outcome::PrinterBug { text, error } => {
                worst = 3;
                json!({"target": to.name(), "status": "printer-bug", "text": text, "error": error})
            }
            Outcome::Printed { text, checks } => {
                let verified = out.verified();
                if !verified {
                    worst = worst.max(1);
                }
                let mut evidence = Vec::new();
                for c in checks {
                    // Confirm on the real engines: the coverage cases of the source (plus the
                    // witness, when there is one) must get the model's answer on both.
                    let mut cases = cover(&src, c.op, &budget)
                        .or_else(|_| sample(&src, c.op, 20, &budget))
                        .unwrap_or_default();
                    if let Verdict::Different(w) = &c.verdict {
                        let m = refract_core::automata::regex_dfa(&src, c.op, &budget)
                            .map(|d| d.accepts(&w.text))
                            .unwrap_or(w.accepted_by == refract_core::equiv::Side::Left);
                        // Only the source is expected to give `m`; check it on its own.
                        let r = (
                            oracles.run(from, pattern, c.op, &w.text),
                            oracles.run(to, text, c.op, &w.text),
                        );
                        let status = match r {
                            (Ok(x), Ok(y)) if x == m && y != m => "confirmed".to_string(),
                            (Ok(_), Ok(_)) => {
                                worst = 3;
                                "CONTRADICTED by the engines".to_string()
                            }
                            (Err(e), _) | (_, Err(e)) => format!("predicted ({e})"),
                        };
                        evidence.push(json!({"op": c.op.name(), "verdict": verdict_text(&c.verdict), "replay": status}));
                        continue;
                    }
                    cases.truncate(200);
                    let status = if cases.is_empty() {
                        "no cases".to_string()
                    } else {
                        match replay(&mut oracles, from, pattern, to, text, &cases, c.op) {
                            Replay::Confirmed(n) => format!("confirmed on {n} test strings"),
                            Replay::Predicted(e) => format!("predicted ({e})"),
                            Replay::Contradicted(e) => {
                                worst = 3;
                                format!("CONTRADICTED {e}")
                            }
                        }
                    };
                    evidence.push(
                        json!({"op": c.op.name(), "verdict": verdict_text(&c.verdict),
                                         "product_states": c.product_states, "replay": status}),
                    );
                }
                json!({"target": to.name(), "engine": oracles.label(to),
                       "status": if verified { "verified" } else { "not-verified" }, "scope": scope_name,
                       "text": text, "checks": evidence})
            }
        };
        rows.push(row);
    }
    if as_json {
        let out = json!({"pattern": pattern, "source": from.name(), "source_engine": oracles.label(from), "targets": rows});
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("pattern  {pattern}");
        println!("written  {}  ({})", from.name(), oracles.label(from));
        for r in &rows {
            let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
            println!();
            match s("status").as_str() {
                "untranslatable" => {
                    let at = if s("at").is_empty() {
                        String::new()
                    } else {
                        format!("  (at `{}`)", s("at"))
                    };
                    println!("{:<6} UNTRANSLATABLE  {}{at}", s("target"), s("construct"));
                }
                "printer-bug" => println!(
                    "{:<6} PRINTER BUG  {} ({})",
                    s("target"),
                    s("text"),
                    s("error")
                ),
                st => {
                    let seal = if st == "verified" {
                        "VERIFIED"
                    } else {
                        "NOT VERIFIED"
                    };
                    println!("{:<6} {seal} ({})  {}", s("target"), s("scope"), s("text"));
                    for c in r["checks"].as_array().into_iter().flatten() {
                        let g = |k: &str| c[k].as_str().unwrap_or("").to_string();
                        println!(
                            "{:<6}   {:<10} {:<44} {}",
                            "",
                            g("op"),
                            g("verdict"),
                            g("replay")
                        );
                    }
                }
            }
        }
    }
    Ok(ExitCode::from(worst))
}

/// Output formats of `refract tests`.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum TestFormat {
    Json,
    Pytest,
    Jest,
    Junit,
    Go,
}

pub fn tests(
    flavor: Flavor,
    pattern: &str,
    op: Op,
    format: TestFormat,
    no_replay: bool,
) -> Result<ExitCode> {
    let budget = Budget::default().start();
    let re = parse_pattern(flavor, pattern)
        .map_err(|e| anyhow!("{flavor} pattern:\n{}", e.render(pattern)))?;
    let (cases, how) = match cover(&re, op, &budget) {
        Ok(c) => (c, "cover every state and transition of the minimal DFA"),
        Err(e) if e.kind == refract_core::ErrorKind::Unsupported => (
            sample(&re, op, 30, &budget).map_err(|e| anyhow!("{}", e.message))?,
            "are sampled up to length 4 (not regular: no coverage guarantee)",
        ),
        Err(e) => return Err(anyhow!("{}", e.message)),
    };
    let mut status = String::from("not replayed (--no-replay)");
    let mut code = 0u8;
    if !no_replay {
        let mut oracles = Oracles::default();
        let inputs: Vec<&str> = cases.iter().map(|c| c.text.as_str()).collect();
        status = match oracles.run_many(flavor, pattern, op, &inputs) {
            Ok(got) => match cases.iter().zip(&got).find(|(c, g)| c.matches != **g) {
                None => format!("all {} confirmed on {}", cases.len(), oracles.label(flavor)),
                Some((c, _)) => {
                    code = 3;
                    format!(
                        "CONTRADICTED on {} by {}",
                        escape(&c.text),
                        oracles.label(flavor)
                    )
                }
            },
            Err(e) => format!("predicted ({e})"),
        };
    }
    eprintln!("{} cases {how}; {status}", cases.len());
    let text = match format {
        TestFormat::Json => serde_json::to_string_pretty(&json!({
            "pattern": pattern, "flavor": flavor.name(), "operation": op.name(), "replay": status,
            "cases": cases.iter().map(|c| json!({"text": c.text, "matches": c.matches})).collect::<Vec<_>>(),
        }))?,
        TestFormat::Pytest => emit_pytest(pattern, op, &cases),
        TestFormat::Jest => emit_jest(pattern, op, &cases),
        TestFormat::Junit => emit_junit(pattern, op, &cases),
        TestFormat::Go => emit_go(pattern, op, &cases),
    };
    println!("{text}");
    Ok(ExitCode::from(code))
}

/// A double-quoted string literal with `\u` escapes for anything outside printable ASCII.
/// `astral` spells a code point above U+FFFF.
fn quote(s: &str, astral: impl Fn(u32) -> String) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            // Java turns `\u000a` into a raw newline before lexing: use the short escapes.
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(c),
            c if (c as u32) <= 0xFFFF => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&astral(c as u32)),
        }
    }
    out.push('"');
    out
}

fn py(s: &str) -> String {
    quote(s, |c| format!("\\U{c:08x}"))
}

fn js(s: &str) -> String {
    quote(s, |c| format!("\\u{{{c:x}}}"))
}

fn java(s: &str) -> String {
    quote(s, |c| {
        let mut buf = [0u16; 2];
        char::from_u32(c)
            .map(|ch| {
                ch.encode_utf16(&mut buf)
                    .iter()
                    .map(|u| format!("\\u{u:04x}"))
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn go(s: &str) -> String {
    quote(s, |c| format!("\\U{c:08x}"))
}

fn header(pattern: &str, op: Op) -> String {
    format!(
        "Generated by `refract tests` for {pattern:?} ({}): covers every state and transition of the minimal DFA.",
        op.name()
    )
}

fn emit_pytest(pattern: &str, op: Op, cases: &[Case]) -> String {
    let f = if op == Op::FullMatch {
        "fullmatch"
    } else {
        "search"
    };
    let mut out = format!(
        "# {}\nimport re\n\nimport pytest\n\nPATTERN = re.compile({})\n\nCASES = [\n",
        header(pattern, op),
        py(pattern)
    );
    for c in cases {
        out.push_str(&format!(
            "    ({}, {}),\n",
            py(&c.text),
            if c.matches { "True" } else { "False" }
        ));
    }
    out.push_str(&format!(
        "]\n\n\n@pytest.mark.parametrize((\"text\", \"expected\"), CASES)\ndef test_pattern(text: str, expected: bool) -> None:\n    assert (PATTERN.{f}(text) is not None) == expected\n"
    ));
    out
}

fn emit_jest(pattern: &str, op: Op, cases: &[Case]) -> String {
    let (src, flags) = match pattern
        .strip_prefix('/')
        .and_then(|r| r.rfind('/').map(|e| (&r[..e], &r[e + 1..])))
    {
        Some((s, f)) => (s.to_string(), f.to_string()),
        None => (pattern.to_string(), String::new()),
    };
    let build = if op == Op::FullMatch {
        // Sticky + look-ahead: whole-string match independent of the m flag.
        format!(
            "new RegExp(`(?:${{{}}})(?![\\\\s\\\\S])`, {} + \"y\")",
            js(&src),
            js(&flags.replace('y', ""))
        )
    } else {
        format!("new RegExp({}, {})", js(&src), js(&flags))
    };
    let mut out = format!("// {}\nconst cases = [\n", header(pattern, op));
    for c in cases {
        out.push_str(&format!("  [{}, {}],\n", js(&c.text), c.matches));
    }
    out.push_str(&format!(
        "];\n\ntest.each(cases)(\"%j -> %s\", (text, expected) => {{\n  const re = {build};\n  expect(re.test(text)).toBe(expected);\n}});\n"
    ));
    out
}

fn emit_junit(pattern: &str, op: Op, cases: &[Case]) -> String {
    let m = if op == Op::FullMatch {
        "matches"
    } else {
        "find"
    };
    let mut out = format!(
        "// {}\nimport static org.junit.jupiter.api.Assertions.assertEquals;\n\nimport java.util.regex.Pattern;\nimport java.util.stream.Stream;\nimport org.junit.jupiter.params.ParameterizedTest;\nimport org.junit.jupiter.params.provider.Arguments;\nimport org.junit.jupiter.params.provider.MethodSource;\n\nclass RefractPatternTest {{\n  static final Pattern PATTERN = Pattern.compile({});\n\n  static Stream<Arguments> cases() {{\n    return Stream.of(\n",
        header(pattern, op),
        java(pattern)
    );
    let rows: Vec<String> = cases
        .iter()
        .map(|c| format!("        Arguments.of({}, {})", java(&c.text), c.matches))
        .collect();
    out.push_str(&rows.join(",\n"));
    out.push_str(&format!(
        ");\n  }}\n\n  @ParameterizedTest\n  @MethodSource(\"cases\")\n  void pattern(String text, boolean expected) {{\n    assertEquals(expected, PATTERN.matcher(text).{m}());\n  }}\n}}\n"
    ));
    out
}

fn emit_go(pattern: &str, op: Op, cases: &[Case]) -> String {
    let compile = if op == Op::FullMatch {
        format!("regexp.MustCompile(`\\A(?:` + {} + `)\\z`)", go(pattern))
    } else {
        format!("regexp.MustCompile({})", go(pattern))
    };
    let mut out = format!(
        "// {}\npackage refract_test\n\nimport (\n\t\"regexp\"\n\t\"testing\"\n)\n\nvar pattern = {compile}\n\nfunc TestPattern(t *testing.T) {{\n\tcases := []struct {{\n\t\ttext string\n\t\twant bool\n\t}}{{\n",
        header(pattern, op)
    );
    for c in cases {
        out.push_str(&format!("\t\t{{{}, {}}},\n", go(&c.text), c.matches));
    }
    out.push_str("\t}\n\tfor _, c := range cases {\n\t\tif got := pattern.MatchString(c.text); got != c.want {\n\t\t\tt.Errorf(\"%q: got %v, want %v\", c.text, got, c.want)\n\t\t}\n\t}\n}\n");
    out
}
