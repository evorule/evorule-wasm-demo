// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build script: extracts the evorule-tcb dependency version from Cargo.toml
//! at compile time, so `engine_version()` can self-report the TCB version this
//! demo artifact was built against. Surfaced in the live demo footer — engine
//! staleness is always self-evident, no repo cross-checking needed.

use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    let manifest = fs::read_to_string("Cargo.toml").expect("read Cargo.toml");
    let tcb_version = manifest
        .lines()
        .find_map(|line| {
            let line = line.trim();
            if !line.starts_with("evorule-tcb = ") {
                return None;
            }
            line.split("version = \"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
        })
        .expect("evorule-tcb dependency with explicit version in Cargo.toml");
    println!("cargo:rustc-env=EVORULE_TCB_DEP_VERSION={tcb_version}");
}
