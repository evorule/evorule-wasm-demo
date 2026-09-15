// SPDX-License-Identifier: AGPL-3.0-or-later
//! evorule-wasm-demo — stateful WASM API on top of the pure TCB.
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
//!
//! # Stateful API (Stage1 M1)
//!
//! `EvoRuleEngine` (wasm32-only) wraps the same pure path into a stateful class
//! holding: loaded ruleset, working payload/queue, in-memory `FactsLog`, a
//! monotonic fact-id counter and an `Auditor`. `io_request` from the TCB is
//! surfaced to JS as an `io_required` signal; the JS side mocks the service and
//! calls `resolve_io`, which injects `__io_result__` into the payload and
//! **replays** `execute_transition` from the original inputs (D11 contract).

use evorule_governance::Auditor;
use evorule_reactor::{serde_to_tcb, tcb_to_serde, Fact, FactId, FactsLog, TraceHit};
use evorule_tcb::{execute_transition, JsonValue, TransitionResult};

// `IoType` and `RuleHit` are referenced **only** from inside the
// `#[cfg(target_arch = "wasm32")]`-gated `EvoRuleEngine` impl blocks below
// (`Fact::IoRequest { io_type: IoType::new(..) }` and the `|h: &RuleHit|`
// closure annotation). On a native build that code is cfg'd out, so a
// top-level `use` of these two names is genuinely unused and fails the
// workspace lint stage under `-D warnings` (clippy: `unused_imports`).
// Keep them behind the same cfg predicate as their only users.
#[cfg(target_arch = "wasm32")]
use evorule_reactor::IoType;
#[cfg(target_arch = "wasm32")]
use evorule_tcb::RuleHit;

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
    let result =
        execute_transition(&core_eval, &instruction, &payload, &queue).expect("execute_transition");
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
        .append(Fact::Stable {
            id: FactId(4),
            version: 1,
        })
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
// Stage1 M1: stateful EvoRuleEngine (wasm32-only wasm-bindgen API).
// ============================================================================
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub struct EvoRuleEngine {
    /// Compiled transform ruleset (loaded via `load_rules`).
    rules: Vec<JsonValue>,
    /// Working payload view (rewind mutates this; FactsLog stays append-only).
    view_payload: JsonValue,
    /// Working pending-queue view.
    view_queue: Vec<JsonValue>,
    /// Working-view version (rewind moves this; FactsLog.version() stays monotonic).
    view_version: u64,
    /// Append-only in-memory audit log (BLAKE3 hash chain).
    facts_log: FactsLog,
    /// BLAKE3 audit-chain auditor, incrementally fed from `facts_log`.
    auditor: Auditor,
    /// Monotonic fact-id counter (mirrors FactIdGenerator).
    next_fact_id: u64,
    /// Instruction paused mid-flight by an IoRequired signal (D11 replay).
    pending_instruction: Option<JsonValue>,
    /// FactId of the pending IoRequest (cause-link for the replay commit).
    pending_io_id: Option<FactId>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl EvoRuleEngine {
    /// Create an empty engine (no rules loaded yet).
    #[wasm_bindgen(constructor)]
    pub fn new() -> EvoRuleEngine {
        let facts_log = FactsLog::new();
        EvoRuleEngine {
            rules: Vec::new(),
            view_payload: JsonValue::empty_object(),
            view_queue: Vec::new(),
            view_version: 0,
            auditor: Auditor::new(facts_log.clone()),
            facts_log,
            next_fact_id: 1,
            pending_instruction: None,
            pending_io_id: None,
        }
    }

    /// Load a ruleset. Accepts either a bare JSON array of transform rules, or
    /// a full rule-set object whose `transform` field holds the array.
    pub fn load_rules(&mut self, rules_json: &str) -> Result<(), wasm_bindgen::JsValue> {
        let parsed: serde_json::Value = serde_json::from_str(rules_json)
            .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("parse rules_json: {e}")))?;
        let arr: &Vec<serde_json::Value> = match &parsed {
            serde_json::Value::Array(a) => a,
            serde_json::Value::Object(_) => parsed["transform"].as_array().ok_or_else(|| {
                wasm_bindgen::JsValue::from_str("rules object has no \"transform\" array")
            })?,
            other => {
                return Err(wasm_bindgen::JsValue::from_str(&format!(
                    "rules_json must be an array or rule-set object, got {}",
                    crate_json_type(other)
                )))
            }
        };
        self.rules = arr.iter().map(serde_to_tcb).collect();
        Ok(())
    }

    /// Execute a single instruction. Pure synchronous path.
    ///
    /// - `TransitionResult::State` -> commit payload/queue, append the fact chain
    ///   (Command -> StateTransition -> TransitionTrace -> Stable), audit, and
    ///   return a `state` JSON.
    /// - `TransitionResult::IoRequired` -> do **not** commit; remember the
    ///   instruction and return an `io_required` JSON for JS to resolve via
    ///   `resolve_io` (D11 replay contract).
    pub fn execute_instruction(
        &mut self,
        instruction_json: &str,
    ) -> Result<String, wasm_bindgen::JsValue> {
        let cmd: serde_json::Value = serde_json::from_str(instruction_json)
            .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("parse instruction: {e}")))?;
        let instruction = serde_to_tcb(&cmd);
        self.run_step(&instruction, None)
    }

    /// Resolve a pending `io_request` with a JS-side mock result.
    ///
    /// Per the D11 contract, this injects `__io_result__` into the (unchanged)
    /// payload and **replays** `execute_transition` with the original instruction
    /// and inputs. The committed transition then lands on the audit chain.
    pub fn resolve_io(&mut self, io_result_json: &str) -> Result<String, wasm_bindgen::JsValue> {
        let instruction = self
            .pending_instruction
            .take()
            .ok_or_else(|| wasm_bindgen::JsValue::from_str("resolve_io: no pending io_request"))?;
        let io_id = self.pending_io_id.take();

        let io_result: serde_json::Value = serde_json::from_str(io_result_json)
            .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("parse io_result: {e}")))?;
        let io_result_tcb = serde_to_tcb(&io_result);

        let mut replay_payload = self.view_payload.clone();
        let map = replay_payload
            .as_object_mut()
            .ok_or_else(|| wasm_bindgen::JsValue::from_str("replay payload is not an object"))?;
        map.insert("__io_result__".to_string(), io_result_tcb);

        let result =
            execute_transition(&self.rules, &instruction, &replay_payload, &self.view_queue)
                .map_err(|e| {
                    wasm_bindgen::JsValue::from_str(&format!("replay execute_transition: {e}"))
                })?;

        self.handle_transition_result(result, &instruction, io_id)
    }

    /// Current working payload + version snapshot.
    pub fn get_state(&self) -> String {
        serde_json::json!({
            "payload": tcb_to_serde(&self.view_payload),
            "version": self.view_version,
        })
        .to_string()
    }

    /// Full audit chain: one entry per fact, with fact_id / fact_type /
    /// logical_time / content_hash / prev_hash / cause.
    pub fn get_audit_chain(&self) -> String {
        let entries: Vec<serde_json::Value> = self
            .auditor
            .entries()
            .iter()
            .map(|e| {
                serde_json::json!({
                    "fact_id": e.fact_id.0,
                    "fact_type": e.fact_type,
                    "logical_time": e.logical_time,
                    "content_hash": e.content_hash,
                    "prev_hash": e.prev_hash,
                    "cause": e.cause.map(|c| c.0),
                })
            })
            .collect();
        serde_json::Value::Array(entries).to_string()
    }

    /// Verify the BLAKE3 hash chain end-to-end.
    pub fn verify_audit_chain(&self) -> bool {
        self.auditor.verify()
    }

    /// Time-travel: restore the working payload/queue to the state at `version`
    /// (number of committed transitions). The append-only FactsLog is left
    /// intact — only the working view moves. Returns the restored state JSON.
    pub fn rewind(&mut self, version: f64) -> Result<String, wasm_bindgen::JsValue> {
        let target = version as u64;
        let facts = self.facts_log.read_from(0);
        let mut v: u64 = 0;
        let mut payload = JsonValue::empty_object();
        let mut queue: Vec<JsonValue> = Vec::new();
        for f in &facts {
            if let Fact::StateTransition {
                new_payload,
                new_queue,
                ..
            } = f
            {
                v += 1;
                payload = new_payload.clone();
                queue = new_queue.clone();
            }
            if v == target {
                break;
            }
        }
        if v != target {
            return Err(wasm_bindgen::JsValue::from_str(&format!(
                "rewind: version {target} not found (have {v} committed transitions)"
            )));
        }
        self.view_payload = payload;
        self.view_queue = queue;
        self.view_version = target;
        self.pending_instruction = None;
        self.pending_io_id = None;
        Ok(self.get_state())
    }

    /// Clear the working state (payload/queue/audit counter) but keep the loaded
    /// ruleset, so a fresh session can start.
    pub fn reset(&mut self) {
        let facts_log = FactsLog::new();
        self.auditor = Auditor::new(facts_log.clone());
        self.facts_log = facts_log;
        self.view_payload = JsonValue::empty_object();
        self.view_queue = Vec::new();
        self.view_version = 0;
        self.next_fact_id = 1;
        self.pending_instruction = None;
        self.pending_io_id = None;
    }
}

// --- private helpers (Rust-only, not exposed to JS) ---
//
// Split into a separate (non-`#[wasm_bindgen]`) impl block so wasm-bindgen
// does not attempt to export these internal methods to JS.
#[cfg(target_arch = "wasm32")]
impl EvoRuleEngine {
    /// Issue the next monotonic fact id.
    fn next_id(&mut self) -> FactId {
        let id = FactId(self.next_fact_id);
        self.next_fact_id += 1;
        id
    }

    /// Run one transition against the current working view and record facts.
    /// `cause_hint` is used by `resolve_io` to link the replay commit to the
    /// prior IoRequest; a fresh Command fact is appended otherwise.
    fn run_step(
        &mut self,
        instruction: &JsonValue,
        cause_hint: Option<FactId>,
    ) -> Result<String, wasm_bindgen::JsValue> {
        let cmd_id = if let Some(c) = cause_hint {
            // replay already has its cause (IoRequest); still record an IoResponse-ish
            // Command for traceability so the fact chain stays causal.
            let id = self.next_id();
            self.facts_log
                .append(Fact::Command {
                    id,
                    instruction: instruction.clone(),
                })
                .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("append Command: {e}")))?;
            c
        } else {
            let id = self.next_id();
            self.facts_log
                .append(Fact::Command {
                    id,
                    instruction: instruction.clone(),
                })
                .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("append Command: {e}")))?;
            id
        };

        let result = execute_transition(
            &self.rules,
            instruction,
            &self.view_payload,
            &self.view_queue,
        )
        .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("execute_transition: {e}")))?;

        self.handle_transition_result(result, instruction, Some(cmd_id))
    }

    /// Map a TCB `TransitionResult` to facts + a reply JSON.
    fn handle_transition_result(
        &mut self,
        result: TransitionResult,
        instruction: &JsonValue,
        cause: Option<FactId>,
    ) -> Result<String, wasm_bindgen::JsValue> {
        let cause = cause.unwrap_or(FactId(0));
        match result {
            TransitionResult::State {
                new_payload,
                new_queue,
                rule_hits,
            } => {
                let st_id = self.next_id();
                self.facts_log
                    .append(Fact::StateTransition {
                        id: st_id,
                        cause,
                        new_payload: new_payload.clone(),
                        new_queue: new_queue.clone(),
                    })
                    .map_err(|e| {
                        wasm_bindgen::JsValue::from_str(&format!("append StateTransition: {e}"))
                    })?;

                let trace_hits: Vec<TraceHit> = rule_hits
                    .iter()
                    .map(|h| TraceHit {
                        index: h.index as u64,
                        instr_type: h.instr_type.clone(),
                        hit: h.hit,
                    })
                    .collect();
                let trace_id = self.next_id();
                self.facts_log
                    .append(Fact::TransitionTrace {
                        id: trace_id,
                        cause: st_id,
                        rule_hits: trace_hits,
                    })
                    .map_err(|e| {
                        wasm_bindgen::JsValue::from_str(&format!("append TransitionTrace: {e}"))
                    })?;

                let version = self.facts_log.version();
                let stable_id = self.next_id();
                self.facts_log
                    .append(Fact::Stable {
                        id: stable_id,
                        version,
                    })
                    .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("append Stable: {e}")))?;

                self.view_payload = new_payload;
                self.view_queue = new_queue;
                self.view_version = version;

                self.auditor.audit_new();
                let verified = self.auditor.verify();

                let hits_json: Vec<serde_json::Value> = rule_hits
                    .iter()
                    .map(|h: &RuleHit| {
                        serde_json::json!({
                            "index": h.index,
                            "instr_type": h.instr_type,
                            "hit": h.hit,
                        })
                    })
                    .collect();

                Ok(serde_json::json!({
                    "type": "state",
                    "payload": tcb_to_serde(&self.view_payload),
                    "version": version,
                    "audit_verified": verified,
                    "rule_hits": hits_json,
                })
                .to_string())
            }
            TransitionResult::IoRequired { io_type, params } => {
                let io_id = self.next_id();
                self.facts_log
                    .append(Fact::IoRequest {
                        id: io_id,
                        cause,
                        io_type: IoType::new(&io_type),
                        params: params.clone(),
                    })
                    .map_err(|e| {
                        wasm_bindgen::JsValue::from_str(&format!("append IoRequest: {e}"))
                    })?;
                self.pending_instruction = Some(instruction.clone());
                self.pending_io_id = Some(io_id);
                self.auditor.audit_new();
                Ok(serde_json::json!({
                    "type": "io_required",
                    "io_type": io_type,
                    "params": tcb_to_serde(&params),
                })
                .to_string())
            }
            TransitionResult::Halted { rule_index, reason } => {
                let v_id = self.next_id();
                self.facts_log
                    .append(Fact::Violation {
                        id: v_id,
                        cause,
                        rule_index: rule_index as u64,
                        reason: reason.clone(),
                        instruction: instruction.clone(),
                    })
                    .map_err(|e| {
                        wasm_bindgen::JsValue::from_str(&format!("append Violation: {e}"))
                    })?;
                self.auditor.audit_new();
                Ok(serde_json::json!({
                    "type": "halted",
                    "rule_index": rule_index,
                    "reason": reason,
                })
                .to_string())
            }
            TransitionResult::Ignored {
                instruction_type,
                reason,
                rule_hits,
            } => {
                self.auditor.audit_new();
                let hits_json: Vec<serde_json::Value> = rule_hits
                    .iter()
                    .map(|h| {
                        serde_json::json!({
                            "index": h.index,
                            "instr_type": h.instr_type,
                            "hit": h.hit,
                        })
                    })
                    .collect();
                Ok(serde_json::json!({
                    "type": "ignored",
                    "instruction_type": instruction_type,
                    "reason": reason,
                    "rule_hits": hits_json,
                })
                .to_string())
            }
        }
    }
}

/// Tiny JSON-type-name helper for load_rules error messages (kept free of
/// serde_json import noise in signatures).
#[cfg(target_arch = "wasm32")]
fn crate_json_type(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
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
        let v: Vec<serde_json::Value> = serde_json::from_str(&json).expect("parse rules array");
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
