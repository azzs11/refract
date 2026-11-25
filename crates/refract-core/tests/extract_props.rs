//! The literal extractor never panics, and finds a planted call in every language whatever
//! surrounds it (plan §8 totality).

use proptest::prelude::*;
use refract_regex::extract::{Lang, extract};

const LANGS: [Lang; 5] = [
    Lang::Python,
    Lang::JavaScript,
    Lang::Go,
    Lang::Java,
    Lang::Rust,
];

fn noise() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            "\"",
            "'",
            "`",
            "/",
            "*",
            "\\",
            "#",
            "\n",
            "r",
            "#\"",
            "$",
            "{",
            "}",
            "(",
            ")",
            "re.compile(",
            "//",
            "/*",
            "*/",
            "\"\"\"",
            "a",
            " ",
            "😀",
            "x / y",
            "refract: js",
        ]),
        0..40,
    )
    .prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20_000))]

    #[test]
    fn never_panics(src in noise(), lang in 0usize..5) {
        let _ = extract(&src, LANGS[lang]);
    }

    #[test]
    fn finds_a_planted_call_after_complete_lines(lines in prop::collection::vec("[a-z =+;]{0,20}", 0..5), lang in 0usize..5) {
        let lang = LANGS[lang];
        let call = match lang {
            Lang::Python => "re.search(r\"ab+c\", s)",
            Lang::JavaScript => "const r = /ab+c/;",
            Lang::Go => "regexp.MustCompile(`ab+c`)",
            Lang::Java => "Pattern.compile(\"ab+c\");",
            Lang::Rust => "Regex::new(r\"ab+c\")",
        };
        let src = format!("{}\n{call}\n", lines.join("\n"));
        let found = extract(&src, lang);
        prop_assert!(found.iter().any(|f| f.pattern.contains("ab+c") && f.line == lines.len().max(1) + 1), "{:?} in {:?}", found, src);
    }
}
