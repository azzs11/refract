//! Command implementations. Exit codes: 0 success, 1 a pattern was rejected or a check failed,
//! 2 usage or internal error.

use anyhow::Result;
use refract_core::{Flavor, parse as parse_pattern};
use serde_json::json;
use std::process::ExitCode;

pub fn parse(flavor: Flavor, pattern: &str, compact: bool, as_json: bool) -> Result<ExitCode> {
    match parse_pattern(flavor, pattern) {
        Ok(re) => {
            if as_json {
                let out = json!({
                    "flavor": flavor.name(),
                    "pattern": pattern,
                    "ir": re.root.to_string(),
                    "captures": re.captures,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else if compact {
                println!("{}", re.root);
            } else {
                print!("{}", re.root.pretty());
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            if as_json {
                let out = json!({
                    "flavor": flavor.name(),
                    "pattern": pattern,
                    "error": {
                        "kind": format!("{:?}", e.kind),
                        "message": e.message,
                        "span": [e.span.start, e.span.end],
                    }
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                eprintln!("{}", e.render(pattern));
            }
            Ok(ExitCode::from(1))
        }
    }
}

pub fn flavors() -> Result<ExitCode> {
    for f in Flavor::ALL {
        match f.profile() {
            Some(p) => {
                println!("{:<7} implemented  {}", f.name(), p.engine);
                println!("        .  excludes {}", p.dot_excludes);
                println!("        $  {}", p.dollar);
                println!(
                    "        \\w \\d \\s Unicode by default: {}",
                    p.unicode_classes
                );
                println!("        (?i) {}", p.case_folding);
                println!("        spec: {}", p.spec);
            }
            None => println!("{:<7} planned", f.name()),
        }
    }
    Ok(ExitCode::SUCCESS)
}
