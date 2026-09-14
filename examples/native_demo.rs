// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native comparison program — exercises the exact same [`run_core`] as the
//! WASM build, with byte-identical inputs, so the printed JSON can be diffed
//! against the Node.js / wasm32 output field-by-field.
#![allow(clippy::unwrap_used, clippy::expect_used)]

fn main() {
    // Empty rules -> embedded constitution; one `increment x by 1` command.
    let command = serde_json::json!({
        "type": "increment",
        "params": { "attr": "x", "delta": 1 }
    })
    .to_string();

    let result = evorule_wasm_demo::run_core("", &command);
    println!("NATIVE RESULT: {}", result);
}
