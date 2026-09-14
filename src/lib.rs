// SPDX-License-Identifier: AGPL-3.0-or-later
//! evorule-wasm-demo — minimal WASM prototype.
//!
//! # Why the pure (non-async) path
//!
//! The full governance flow (`SessionManager` -> `Reactor::spawn` -> tokio
//! current-thread runtime) cannot be built/run on `wasm32-unknown-unknown` in
//! Node.js: `std::time::Instant::now()` is unimplemented on that target and
//! tokio's runtime builder calls it unconditionally at construction (verified
//! empirically: runtime panic at `build_current_thread_runtime_components`).
//!
//! Per the task's sanctioned fallback, this demo drives the **real** TCB pure
//! function + the **real** BLAKE3 audit chain, with no tokio and no clock:
//!
//!   load rules (constitution)  ->  `evorule_tcb::execute_transition`
//!   -> record the fact stream in `FactsLog`  ->  `Auditor` (BLAKE3 hash chain)
//!   -> `audit_new()` then `verify()`
//!
//! `run_core` is synchronous, so the exact same code compiles and runs both
//! natively and under wasm32-unknown-unknown — the strongest possible
//! determinism guarantee.

use evorule_governance::Auditor;
use evorule_reactor::{serde_to_tcb, tcb_to_serde, Fact, FactId, FactsLog, TraceHit};
use evorule_tcb::{execute_transition, JsonValue, TransitionResult};

/// Embedded constitution (the same `evorule-tcb/core_eval.json` the TCB tests
/// use). Compiled in via include_str! so it is byte-identical on native and
/// wasm32 (no filesystem / no network at runtime).
const CONSTITUTION: &str = include_str!("../../evorule-tcb/core_eval.json");

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

/// Run one rule-evaluation round and return a JSON string.
///
/// `rules_json`: either a JSON array of transform rules, or empty string to use
/// the embedded constitution. `command_json`: a single instruction object.
pub fn run_core(rules_json: &str, command_json: &str) -> String {
    // --- 1. Load rules (constitution transform list) ---
    let rules: Vec<serde_json::Value> = if rules_json.trim().is_empty() {
        let constitution: serde_json::Value =
            serde_json::from_str(CONSTITUTION).expect("parse embedded constitution");
        constitution["transform"]
            .as_array()
            .expect("constitution has transform array")
            .clone()
    } else {
        serde_json::from_str(rules_json).expect("parse rules_json")
    };
    let core_eval: Vec<JsonValue> = rules.iter().map(serde_to_tcb).collect();

    // --- 2. Parse the incoming command ---
    let cmd: serde_json::Value = serde_json::from_str(command_json).expect("parse command_json");
    let instruction = serde_to_tcb(&cmd);

    // --- 3. Pure TCB state transition (no tokio, no clock) ---
    let payload = JsonValue::empty_object();
    let queue: Vec<JsonValue> = Vec::new();
    let result = execute_transition(&core_eval, &instruction, &payload, &queue)
        .expect("execute_transition");
    let (new_payload, new_queue, rule_hits) = match result {
        TransitionResult::State {
            new_payload,
            new_queue,
            rule_hits,
        } => (new_payload, new_queue, rule_hits),
        other => panic!("expected TransitionResult::State, got {:?}", other),
    };

    // --- 4. Record the fact stream in a pure-memory FactsLog ---
    // Mirrors what the async reactor emits for one command:
    //   Command -> StateTransition -> TransitionTrace -> Stable
    let facts_log = FactsLog::new();
    facts_log
        .append(Fact::Command {
            id: FactId(1),
            instruction,
        })
        .expect("append Command");
    facts_log
        .append(Fact::StateTransition {
            id: FactId(2),
            cause: FactId(1),
            new_payload: new_payload.clone(),
            new_queue: new_queue.clone(),
        })
        .expect("append StateTransition");
    let trace_hits: Vec<TraceHit> = rule_hits
        .into_iter()
        .map(|h| TraceHit {
            index: h.index as u64,
            instr_type: h.instr_type,
            hit: h.hit,
        })
        .collect();
    facts_log
        .append(Fact::TransitionTrace {
            id: FactId(3),
            cause: FactId(2),
            rule_hits: trace_hits,
        })
        .expect("append TransitionTrace");
    facts_log
        .append(Fact::Stable { id: FactId(4), version: 1 })
        .expect("append Stable");

    // --- 5. Build + verify the BLAKE3 audit chain ---
    let mut auditor = Auditor::new(facts_log.clone());
    let audit_count = auditor.audit_new();
    let audit_verified = auditor.verify();

    // --- 6. Business payload snapshot ---
    let (payload_snap, _, payload_version) = facts_log.snapshot();

    serde_json::json!({
        "payload": tcb_to_serde(&payload_snap),
        "payload_version": payload_version,
        "audit_verified": audit_verified,
        "audit_count": audit_count,
    })
    .to_string()
}

/// wasm-bindgen entry point (only compiled for wasm32).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn run_demo(rules_json: &str, command_json: &str) -> String {
    run_core(rules_json, command_json)
}

// ============================================================================
// Day5 performance benchmarks.
//
// Each function does real work and returns a checksum string, so the compiler
// cannot elide the loops. Native times these internally with std::time::Instant;
// wasm32 has no Instant, so JS wraps the call with performance.now().
// ============================================================================

fn load_constitution_rules() -> Vec<JsonValue> {
    let c: serde_json::Value =
        serde_json::from_str(CONSTITUTION).expect("parse embedded constitution");
    c["transform"]
        .as_array()
        .expect("constitution has transform array")
        .iter()
        .map(serde_to_tcb)
        .collect()
}

/// Dimension 1: load `n` rules (JSON parse + serde_to_tcb), repeated `reps`
/// times. Returns a checksum so the loop is not optimized away.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn bench_load_rules(n: usize, reps: usize) -> String {
    let rule = r#"{"type":"increment","params":{"attr":"x","delta":1}}"#;
    let json = format!("[{}]", (0..n).map(|_| rule).collect::<Vec<_>>().join(","));

    let mut checksum: u64 = 0;
    for _ in 0..reps {
        let v: Vec<serde_json::Value> =
            serde_json::from_str(&json).expect("parse rules array");
        let t: Vec<JsonValue> = v.iter().map(serde_to_tcb).collect();
        // Touch every element so neither parse nor convert is elided.
        checksum = checksum.wrapping_add(t.len() as u64);
    }
    format!("{}", checksum)
}

/// Dimension 2: one `increment` instruction through `execute_transition`,
/// called `iterations` times. Returns a checksum derived from new_payload.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn bench_execute(iterations: usize) -> String {
    let core_eval = load_constitution_rules();
    let instruction = serde_to_tcb(&serde_json::json!({
        "type": "increment",
        "params": { "attr": "x", "delta": 1 }
    }));
    let queue: Vec<JsonValue> = Vec::new();

    let mut checksum: u64 = 0;
    for _ in 0..iterations {
        let payload = JsonValue::empty_object();
        match execute_transition(&core_eval, &instruction, &payload, &queue) {
            Ok(TransitionResult::State { new_payload, .. }) => {
                checksum = checksum.wrapping_add(new_payload.to_string().len() as u64);
            }
            _ => {
                checksum = checksum.wrapping_add(0xDEAD);
            }
        }
    }
    format!("{}", checksum)
}

/// Dimension 3: append `n` facts to a pure-memory FactsLog, then
/// Auditor.audit_new() + verify() (BLAKE3 content hash + chain verify).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn bench_audit(n: usize) -> String {
    let facts_log = FactsLog::new();
    for i in 0..n {
        facts_log
            .append(Fact::StateTransition {
                id: FactId(i as u64 + 1),
                cause: FactId(0),
                new_payload: JsonValue::Integer(i as i64),
                new_queue: Vec::new(),
            })
            .expect("append fact");
    }
    let mut auditor = Auditor::new(facts_log.clone());
    let count = auditor.audit_new();
    let ok = auditor.verify();
    format!("{}:{}", count, ok)
}
