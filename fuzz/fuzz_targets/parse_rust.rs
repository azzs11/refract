//! The Rust-flavour parser must never panic, hang or blow up memory on any input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use refract_core::{Flavor, parse};

fuzz_target!(|data: &[u8]| {
    if let Ok(pattern) = std::str::from_utf8(data) {
        if let Ok(re) = parse(Flavor::Rust, pattern) {
            // Exercise the printers too.
            let _ = re.root.to_string();
            let _ = re.root.pretty();
        }
    }
});
