//! `refract check PATH...`: finds regex literals in source files (refract_core::extract) and
//! reports, per literal, what Refract can tell without running anything:
//!
//! - `syntax` (error): the file's own engine rejects the pattern.
//! - `redos` (warning): super-linear backtracking predicted for that engine (Python, JS, Java;
//!   Go and Rust are linear), with an attack; `refract redos` confirms it by measurement.
//! - `portability` (note, with `--targets`): the pattern means something else in a target
//!   engine, with the shortest witness.
//! - `unsupported` (note): a construct or call Refract does not analyse.
//!
//! Static: every finding is labelled "predicted" (plan §8); the commands that confirm it on the
//! real engines are printed with it.

use anyhow::Result;
use refract_core::equiv::{Verdict, compare, escape};
use refract_core::extract::{Found, Lang, extract};
use refract_core::redos::Growth;
use refract_core::{Budget, ErrorKind, Flavor, parse};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum Severity {
    Note,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Format {
    Text,
    Json,
    /// GitHub Actions workflow commands (annotations on the pull request).
    Github,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum FailOn {
    Error,
    Warning,
    Note,
    Never,
}

struct Finding {
    path: String,
    found: Found,
    severity: Severity,
    rule: &'static str,
    message: String,
    confirm: Option<String>,
}

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "__pycache__",
    "venv",
    ".venv",
];

fn walk(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_file() {
        out.push(p.to_path_buf());
        return;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for e in entries {
        let name = e.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if e.is_dir() && (name.starts_with('.') || SKIP_DIRS.contains(&name)) {
            continue;
        }
        if e.is_dir() {
            walk(&e, out);
        } else if Lang::from_path(name).is_some() {
            out.push(e);
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn budget() -> Budget {
    let mut b = Budget::small();
    b.time_limit = Some(Duration::from_secs(2));
    b.start()
}

fn analyse(path: &str, f: Found, targets: &[Flavor], redos: bool, out: &mut Vec<Finding>) {
    let mut add = |severity, rule, message: String, confirm: Option<String>, f: &Found| {
        out.push(Finding {
            path: path.to_string(),
            found: f.clone(),
            severity,
            rule,
            message,
            confirm,
        });
    };
    if let Some(why) = &f.unsupported {
        add(
            Severity::Note,
            "unsupported",
            format!("{}: {why}", f.call),
            None,
            &f,
        );
        return;
    }
    let re = match parse(f.flavor, &f.pattern) {
        Ok(re) => re,
        Err(e) if e.kind == ErrorKind::Syntax => {
            add(
                Severity::Error,
                "syntax",
                format!(
                    "{} rejects this pattern: {} (predicted)",
                    f.flavor, e.message
                ),
                Some(format!(
                    "refract parse -f {} {}",
                    f.flavor.name(),
                    shell_quote(&f.pattern)
                )),
                &f,
            );
            return;
        }
        Err(e) => {
            add(
                Severity::Note,
                "unsupported",
                format!("not analysed: {}", e.message),
                None,
                &f,
            );
            return;
        }
    };
    if redos && matches!(f.flavor, Flavor::Python | Flavor::JavaScript | Flavor::Java) {
        // `re.match`, `RegExp.test` and friends search; `fullmatch`/`matches` anchor both ends.
        if let Ok(p) = refract_core::redos::analyse(&re, f.op, &budget())
            && p.growth != Growth::Linear
        {
            let attack = p
                .attack
                .as_ref()
                .map(|a| {
                    format!(
                        "; attack {} + ({})×n + {}",
                        escape(&a.prefix),
                        a.pumps
                            .iter()
                            .map(|x| escape(x))
                            .collect::<Vec<_>>()
                            .join(" "),
                        escape(&a.suffix)
                    )
                })
                .unwrap_or_default();
            let sev = if p.growth == Growth::Exponential {
                Severity::Error
            } else {
                Severity::Warning
            };
            add(
                sev,
                "redos",
                format!(
                    "{} backtracking on {} ({}){attack} (predicted)",
                    p.growth.name(),
                    f.flavor,
                    p.reason
                ),
                Some(format!(
                    "refract redos -f {} --op {} {}",
                    f.flavor.name(),
                    f.op.name(),
                    shell_quote(&f.pattern)
                )),
                &f,
            );
        }
    }
    for &t in targets {
        if t == f.flavor {
            continue;
        }
        let Some(text) = port(&f.pattern, f.flavor, t) else {
            continue;
        };
        let msg = match parse(t, &text) {
            Err(e) if e.kind == ErrorKind::Syntax => Some(format!("{t} rejects it: {}", e.message)),
            Err(_) => None,
            Ok(other) => match compare(&re, &other, f.op, &budget()) {
                Ok(r) => match r.verdict {
                    Verdict::Different(w) => {
                        let src_m = w.accepted_by == refract_core::equiv::Side::Left;
                        Some(format!(
                            "means something else in {t}: on {} {} {}, {t} {} (predicted)",
                            escape(&w.text),
                            f.flavor,
                            if src_m { "matches" } else { "does not" },
                            if src_m { "does not" } else { "matches" }
                        ))
                    }
                    _ => None,
                },
                Err(_) => None,
            },
        };
        if let Some(m) = msg {
            add(
                Severity::Note,
                "portability",
                m,
                Some(format!(
                    "refract equiv --op {} --a {} --b {}",
                    f.op.name(),
                    shell_quote(&format!("{}:{}", f.flavor.name(), f.pattern)),
                    shell_quote(&format!("{}:{text}", t.name()))
                )),
                &f,
            );
        }
    }
}

/// The pattern as someone porting it would write it in `to`: JS literals lose their slashes
/// (`i m s` flags become inline flags; `u`/`v` have no inline form, so no port), and other
/// patterns become a JS literal without flags.
fn port(pattern: &str, from: Flavor, to: Flavor) -> Option<String> {
    if from == Flavor::JavaScript {
        let rest = pattern.strip_prefix('/')?;
        let end = rest.rfind('/')?;
        let (body, flags) = (&rest[..end], &rest[end + 1..]);
        if flags.contains(['u', 'v']) {
            return None;
        }
        let inline: String = flags.chars().filter(|c| "ims".contains(*c)).collect();
        let body = body.replace("\\/", "/");
        return Some(if inline.is_empty() {
            body
        } else {
            format!("(?{inline}){body}")
        });
    }
    if to == Flavor::JavaScript {
        let mut out = String::from("/");
        let mut escaped = false;
        for c in pattern.chars() {
            if c == '/' && !escaped {
                out.push('\\');
            }
            escaped = c == '\\' && !escaped;
            out.push(c);
        }
        out.push('/');
        return Some(out);
    }
    Some(pattern.to_string())
}

/// GitHub workflow-command escaping.
fn gh(s: &str, property: bool) -> String {
    let s = s
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    if property {
        s.replace(':', "%3A").replace(',', "%2C")
    } else {
        s
    }
}

pub fn check(
    paths: &[PathBuf],
    targets: &[Flavor],
    redos: bool,
    format: Format,
    fail_on: FailOn,
) -> Result<ExitCode> {
    let mut files = Vec::new();
    for p in paths {
        walk(p, &mut files);
    }
    let mut findings = Vec::new();
    let mut literals = 0;
    for file in &files {
        let name = file.to_string_lossy().to_string();
        // Annotations need repository-relative paths.
        let name = name.strip_prefix("./").map(str::to_string).unwrap_or(name);
        let Some(lang) = Lang::from_path(&name) else {
            continue;
        };
        let Ok(src) = std::fs::read_to_string(file) else {
            continue;
        };
        for f in extract(&src, lang) {
            literals += 1;
            analyse(&name, f, targets, redos, &mut findings);
        }
    }
    let word = |s: Severity| match s {
        Severity::Note => "note",
        Severity::Warning => "warning",
        Severity::Error => "error",
    };
    match format {
        Format::Json => {
            let v: Vec<_> = findings
                .iter()
                .map(|x| {
                    json!({"path": x.path, "line": x.found.line, "column": x.found.col, "severity": word(x.severity),
                           "rule": x.rule, "message": x.message, "pattern": x.found.pattern, "flavor": x.found.flavor.name(),
                           "operation": x.found.op.name(), "call": x.found.call, "confirm": x.confirm})
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"files": files.len(), "patterns": literals, "findings": v})
                )?
            );
        }
        Format::Github => {
            for x in &findings {
                let level = match x.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Note => "notice",
                };
                let mut msg = format!("{} — pattern {}", x.message, x.found.pattern);
                if let Some(c) = &x.confirm {
                    msg.push_str(&format!("\nconfirm on the real engine: {c}"));
                }
                println!(
                    "::{level} file={},line={},col={},title={}::{}",
                    gh(&x.path, true),
                    x.found.line,
                    x.found.col,
                    gh(&format!("refract {}", x.rule), true),
                    gh(&msg, false)
                );
            }
            println!(
                "refract check: {} findings in {} patterns, {} files",
                findings.len(),
                literals,
                files.len()
            );
        }
        Format::Text => {
            for x in &findings {
                println!(
                    "{}:{}:{}: {}[{}]: {}",
                    x.path,
                    x.found.line,
                    x.found.col,
                    word(x.severity),
                    x.rule,
                    x.message
                );
                println!("    {} {}", x.found.call, x.found.pattern);
                if let Some(c) = &x.confirm {
                    println!("    confirm: {c}");
                }
            }
            println!(
                "{} findings in {} patterns, {} files",
                findings.len(),
                literals,
                files.len()
            );
        }
    }
    let threshold = match fail_on {
        FailOn::Error => Some(Severity::Error),
        FailOn::Warning => Some(Severity::Warning),
        FailOn::Note => Some(Severity::Note),
        FailOn::Never => None,
    };
    let fail = threshold.is_some_and(|t| findings.iter().any(|x| x.severity >= t));
    Ok(ExitCode::from(u8::from(fail)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_redos_syntax_and_portability() {
        let mut out = Vec::new();
        for f in extract(
            "import re\nre.fullmatch(r'^(a+)+$', s)\nre.compile('a(b')\nre.search(r'^\\w+$', s)\n",
            Lang::Python,
        ) {
            analyse("x.py", f, &[Flavor::JavaScript], true, &mut out);
        }
        let rules: Vec<_> = out
            .iter()
            .map(|f| (f.found.line, f.rule, f.severity))
            .collect();
        assert!(rules.contains(&(2, "redos", Severity::Error)), "{rules:?}");
        assert!(rules.contains(&(3, "syntax", Severity::Error)), "{rules:?}");
        assert!(
            rules.contains(&(4, "portability", Severity::Note)),
            "{rules:?}"
        );
    }

    #[test]
    fn ports_js_literals() {
        assert_eq!(
            port("/a\\/b/gi", Flavor::JavaScript, Flavor::Python).as_deref(),
            Some("(?i)a/b")
        );
        assert_eq!(port("/a/u", Flavor::JavaScript, Flavor::Go), None);
        assert_eq!(
            port("a/b\\/c", Flavor::Python, Flavor::JavaScript).as_deref(),
            Some("/a\\/b\\/c/")
        );
    }

    #[test]
    fn github_escaping() {
        assert_eq!(gh("a,b:c%\n", true), "a%2Cb%3Ac%25%0A");
    }
}
