// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build script: extracts the evorule-tcb dependency pin (git tag) from
//! Cargo.toml at compile time, so `engine_version()` can self-report the
//! kernel tag this demo artifact was built against. Surfaced in the live demo
//! footer — engine staleness is always self-evident, no repo cross-check needed.
//!
//! The dependency is declared as `{ git = "...", tag = "vX.Y.Z" }`; the tag is
//! the single source of truth for the pin (no separate version field).

use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=constitution/core_eval.json");
    let manifest = fs::read_to_string("Cargo.toml").expect("read Cargo.toml");
    let tag = manifest
        .lines()
        .find_map(|line| {
            let line = line.trim();
            if !line.starts_with("evorule-tcb = ") {
                return None;
            }
            line.split("tag = \"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
        })
        .expect("evorule-tcb dependency with git tag pin in Cargo.toml");
    println!("cargo:rustc-env=EVORULE_TCB_DEP_VERSION={tag}");
}
