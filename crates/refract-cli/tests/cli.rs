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
fn unsupported_is_honest() {
    let (code, out) = refract(&["equiv", "--a", "\\bfoo", "--b", "foo"]);
    assert_eq!(code, 2);
    insta::assert_snapshot!(out);
}

#[test]
fn dfa_table() {
    let (code, out) = refract(&["dfa", "(a|b)*abb"]);
    assert_eq!(code, 0);
    insta::assert_snapshot!(out);
}
