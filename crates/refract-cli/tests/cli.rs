//! Golden reports (doc §9 "Golden reports"): CLI output is stable unless intentionally changed.
//! Update with `INSTA_UPDATE=always cargo test -p refract-cli` and review the diff.

use std::process::Command;

fn refract(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_refract"))
        .args(args)
        .output()
        .expect("run refract");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn parse_tree() {
    let (code, out) = refract(&["parse", "--flavor", "rust", "a(b|c)*"]);
    assert_eq!(code, 0);
    insta::assert_snapshot!(out);
}

#[test]
fn parse_error_points_at_the_problem() {
    let (code, out) = refract(&["parse", "x(?P<n>a)(?P<n>b)"]);
    assert_eq!(code, 1);
    insta::assert_snapshot!(out);
}

#[test]
fn equiv_different_confirmed() {
    let (code, out) = refract(&["equiv", "--a", "rust:\\d+", "--b", "rust:[0-9]+"]);
    assert_eq!(code, 1);
    insta::assert_snapshot!(out);
}

#[test]
fn equiv_equivalent() {
    let (code, out) = refract(&["equiv", "--a", "(a|b)*", "--b", "(a*b*)*"]);
    assert_eq!(code, 0);
    insta::assert_snapshot!(out);
}

#[test]
fn equiv_search_operation_is_named() {
    let (code, out) = refract(&["equiv", "--op", "search", "--a", "^abc", "--b", "abc"]);
    assert_eq!(code, 1);
    insta::assert_snapshot!(out);
}

#[test]
fn equiv_json() {
    let (code, out) = refract(&["equiv", "--json", "--a", "(?s).", "--b", "."]);
    assert_eq!(code, 1);
    insta::assert_snapshot!(out);
}

#[test]
fn equiv_both_engines_agree() {
    let a = ["equiv", "--a", "[a-z]+@[a-z]+", "--b", "\\w+@\\w+"];
    let (_, deriv) = refract(&a);
    let (_, thompson) = refract(&[&a[..], &["--engine", "thompson"]].concat());
    // Same verdict and witness; only the explored-state count may differ.
    let strip = |s: &str| {
        s.lines()
            .filter(|l| !l.contains("explored"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(strip(&deriv), strip(&thompson));
}

#[test]
fn word_boundaries_are_analysed() {
    // \b anywhere is exact since P3 (look-around via continuations and look-behind context).
    let (code, out) = refract(&["equiv", "--op", "search", "--a", "\\bfoo", "--b", "foo"]);
    assert_eq!(code, 1);
    insta::assert_snapshot!(out);
}

#[test]
fn unsupported_is_honest() {
    // Java \X (grapheme clusters) is valid but not analysed: refused with its location.
    let (code, out) = refract(&["equiv", "--flavor", "java", "--a", "a\\X", "--b", "a"]);
    assert_eq!(code, 2);
    insta::assert_snapshot!(out);
}

#[test]
fn backreferences_are_bounded() {
    let (code, out) = refract(&[
        "equiv", "--flavor", "python", "--a", "(a+)\\1", "--b", "(aa)+",
    ]);
    assert_eq!(code, 0);
    insta::assert_snapshot!(out);
}

/// Cross-engine report, Python source vs Rust (in-process), when python3 is installed.
#[test]
fn refract_python_vs_rust() {
    // Confirmation needs the pinned CPython (3.15): a native python3 of another version is
    // refused by the oracle, and without the Docker image the verdict is only "predicted".
    let pinned = std::process::Command::new("python3")
        .args([
            "-c",
            "import sys; sys.exit(sys.version_info[:2] != (3, 15))",
        ])
        .status()
        .is_ok_and(|s| s.success());
    if !pinned {
        eprintln!("skipping: CPython 3.15 (the pinned engine) not available");
        return;
    }
    let (code, out) = refract(&[
        "refract",
        "--from",
        "python",
        "--to",
        "rust",
        r"^\w+@\w+\.com$",
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("DIFFERS") && out.contains("confirmed"),
        "{out}"
    );
    assert!(out.contains("\"a@².com\""), "{out}");
}

#[test]
fn dfa_table() {
    let (code, out) = refract(&["dfa", "(a|b)*abb"]);
    assert_eq!(code, 0);
    insta::assert_snapshot!(out);
}

#[test]
fn redos_predicts_per_engine() {
    let (code, out) = refract(&[
        "redos",
        "--no-measure",
        "--engines",
        "python,js,js-linear,java,go,rust",
        "^(a+)+$",
    ]);
    assert_eq!(code, 0, "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn redos_polynomial_search() {
    let (code, out) = refract(&[
        "redos",
        "--no-measure",
        "--op",
        "search",
        "--engines",
        "python,java",
        r"\s+$",
    ]);
    assert_eq!(code, 0, "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn redos_safe_pattern() {
    let (code, out) = refract(&[
        "redos",
        "--no-measure",
        "--engines",
        "python",
        "^[a-z]+@[a-z]+\\.com$",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("no ambiguous loop"), "{out}");
}

/// Measured on CPython when it is installed: the exponential prediction is confirmed.
#[test]
fn redos_confirmed_on_python() {
    let (code, out) = refract(&["redos", "--engines", "python", "--timeout", "2", "^(a+)+$"]);
    if out.contains("unavailable") {
        eprintln!("skipping: no python engine");
        return;
    }
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("confirmed"), "{out}");
}
