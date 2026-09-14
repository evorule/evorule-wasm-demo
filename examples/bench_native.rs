// SPDX-License-Identifier: AGPL-3.0-or-later
//! Day5 native performance benchmark.
//!
//! Times the three workloads with std::time::Instant, repeats each measurement
//! 3 times and reports the median, then prints a single JSON line consumed by
//! the Day5 report.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::time::Instant;

fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn run_three<F: FnMut() -> String>(mut f: F) -> Vec<f64> {
    let mut out = Vec::new();
    for _ in 0..3 {
        let t0 = Instant::now();
        let _checksum = f();
        out.push(t0.elapsed().as_secs_f64() * 1e6); // microseconds
    }
    out
}

fn main() {
    // Warmup so allocator/code caches are hot before measured runs.
    let _ = evorule_wasm_demo::bench_load_rules(10, 50);
    let _ = evorule_wasm_demo::bench_execute(200);
    let _ = evorule_wasm_demo::bench_audit(50);

    // --- Dimension 1: rule loading ---
    let load_ns: [usize; 3] = [10, 100, 1000];
    let load_reps = 500usize;
    let mut load = serde_json::Map::new();
    for &n in &load_ns {
        let runs_total_us = run_three(|| evorule_wasm_demo::bench_load_rules(n, load_reps));
        let per_op: Vec<f64> = runs_total_us.iter().map(|t| t / load_reps as f64).collect();
        load.insert(
            n.to_string(),
            serde_json::json!({
                "runs_total_us": runs_total_us,
                "per_op_us": per_op,
                "per_op_us_median": median(&per_op),
            }),
        );
    }

    // --- Dimension 2: instruction execution ---
    let iters = 2000usize;
    let exec_runs_us = run_three(|| evorule_wasm_demo::bench_execute(iters));
    let exec_per_op: Vec<f64> = exec_runs_us.iter().map(|t| t / iters as f64).collect();

    // --- Dimension 3: audit chain ---
    let audit_n = 200usize;
    let audit_runs_us = run_three(|| evorule_wasm_demo::bench_audit(audit_n));

    let report = serde_json::json!({
        "binary": "native (cargo build --release)",
        "load_rules": { "n_values": load_ns, "reps_per_measurement": load_reps, "by_n": load },
        "execute": {
            "iterations_per_run": iters,
            "runs_total_us": exec_runs_us,
            "per_op_us": exec_per_op,
            "per_op_us_median": median(&exec_per_op),
        },
        "audit": {
            "facts_per_run": audit_n,
            "runs_us": audit_runs_us,
            "median_us": median(&audit_runs_us),
        },
    });
    println!("NATIVE_BENCH: {}", report);
}
