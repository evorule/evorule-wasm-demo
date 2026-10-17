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
use evorule_reactor::{
    chain_step, content_hash, fact_to_stable_json, serde_to_tcb, tcb_to_serde, Fact, FactId,
    FactsLog, IoType, TraceHit,
};
use evorule_tcb::{execute_transition, JsonValue, TransitionResult};

// `RuleHit` is referenced only from inside the `#[cfg(target_arch =
// "wasm32")]`-gated `EvoRuleEngine` impl blocks; natively it would be an
// unused import under `-D warnings`, so it keeps the same cfg predicate as
// its only user. `IoType` moved to the unconditional import above (the
// native `LlmAuditSession` uses it too).
#[cfg(target_arch = "wasm32")]
use evorule_tcb::RuleHit;

/// Embedded constitution (vendored copy of `evorule-tcb/core_eval.json` at the
/// git tag this crate pins in Cargo.toml; hash-asserted by
/// `constitution::tests` so the copy cannot silently drift). Compiled in via
/// include_str! so it is byte-identical on native and wasm32 (no filesystem /
/// no network at runtime).
const CONSTITUTION: &str = include_str!("../constitution/core_eval.json");

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
    let trace_hits: Vec<TraceHit> = rule_hits.iter().map(TraceHit::from_rule_hit).collect();
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

/// Offline audit-chain verification over [`EvoRuleEngine::export_chain`]
/// output (pure function; also the wasm32 export `verify_chain_json`).
///
/// Recomputes, per entry, (a) the content hash over `stable_json` and (b) the
/// chain link from the previous entry — the same math the production
/// audit/export + CLI verify_chain path performs over hash-chain metadata.
/// Returns a report: per-index `content_ok` / `link_ok`, the first broken
/// index (if any) and the recomputed chain tip.
pub fn verify_exported_chain_impl(entries_json: &str) -> Result<serde_json::Value, String> {
    let entries: Vec<serde_json::Value> =
        serde_json::from_str(entries_json).map_err(|e| format!("parse entries: {e}"))?;

    const GENESIS: &str = "genesis";
    let mut prev: String = GENESIS.to_string();
    let mut first_broken: Option<usize> = None;
    let mut report: Vec<serde_json::Value> = Vec::with_capacity(entries.len());

    for (i, e) in entries.iter().enumerate() {
        let field = |name: &str| {
            e.get(name)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| format!("entry {i}: missing string field '{name}'"))
        };
        let stable_json = field("stable_json")?;
        let content_hash_recorded = field("content_hash")?;
        let entry_prev = field("prev_hash")?;

        // (a) content integrity: the production hash function over the exact
        // text that was hashed at append time.
        let stable_value: serde_json::Value = serde_json::from_str(&stable_json)
            .map_err(|e| format!("entry {i}: parse stable_json: {e}"))?;
        let recomputed_content = content_hash(&serde_to_tcb(&stable_value))
            .map_err(|e| format!("entry {i}: content_hash: {e}"))?;
        let content_ok = recomputed_content == content_hash_recorded;

        // (b) link integrity: this entry must hang off the tip recomputed
        // from the PREVIOUS entry's *recomputed-from-text* content hash —
        // not its recorded one. Data-derived linking is what makes tampering
        // cascade: editing entry i's text breaks i's content hash and
        // detaches every later link (an attacker must re-chain the entire
        // tail to restore consistency — the demo narrative's "cost").
        let link_ok = entry_prev == prev;
        let recomputed_tip = chain_step(&prev, &recomputed_content);

        if !(content_ok && link_ok) && first_broken.is_none() {
            first_broken = Some(i);
        }
        report.push(serde_json::json!({
            "index": i,
            "fact_id": e.get("fact_id").cloned().unwrap_or(serde_json::Value::Null),
            "fact_type": e.get("fact_type").cloned().unwrap_or(serde_json::Value::Null),
            "content_ok": content_ok,
            "link_ok": link_ok,
        }));
        prev = recomputed_tip;
    }

    Ok(serde_json::json!({
        "verified": first_broken.is_none() && !report.is_empty(),
        "entry_count": report.len(),
        "first_broken_index": first_broken,
        "chain_tip": prev,
        "entries": report,
    }))
}

/// wasm-bindgen export of the offline verifier (same JSON-in/JSON-out
/// contract as the native [`verify_exported_chain_impl`]).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = verify_chain_json)]
pub fn verify_chain_json_js(entries_json: &str) -> Result<String, wasm_bindgen::JsValue> {
    verify_exported_chain_impl(entries_json)
        .map(|v| v.to_string())
        .map_err(|e| wasm_bindgen::JsValue::from_str(&e))
}

/// Build identity of this demo artifact: the wasm-demo crate version plus the
/// evorule-tcb version it was compiled against (extracted from Cargo.toml by
/// build.rs). The live demo footer shows this, so engine staleness relative to
/// the latest release is always self-evident.
pub fn engine_version() -> String {
    // The pin tag already carries its leading "v" (e.g. "v0.8.5").
    format!(
        "evorule-wasm-demo {} / TCB {}",
        env!("CARGO_PKG_VERSION"),
        env!("EVORULE_TCB_DEP_VERSION")
    )
}

// ============================================================================
// LLM audit session (paper's interactive appendix core): a call_external-
// shaped scripted conversation recorded on the REAL audit mechanism.
//
// What is real here: the Fact variants, the append-only FactsLog, the BLAKE3
// hash chain, the causal cause links, the transcript payload facts, and the
// offline verifier. What is scripted: the conversation content itself (so the
// demo is byte-reproducible; the page labels this explicitly).
// ============================================================================

/// Shared chain-export builder: per-fact chain metadata + the exact
/// stable-JSON text that was hashed (offline verifier input). Used by both
/// the rule engine and the LLM audit session.
fn chain_export(facts_log: &FactsLog, auditor: &Auditor) -> String {
    let facts = facts_log.read_from(0).unwrap_or_default();
    let entries: Vec<serde_json::Value> = auditor
        .entries()
        .iter()
        .zip(facts.iter())
        .map(|(e, f)| {
            let stable = fact_to_stable_json(f)
                .ok()
                .and_then(|v| serde_json::to_string(&v).ok());
            serde_json::json!({
                "fact_id": e.fact_id.0,
                "fact_type": e.fact_type,
                "logical_time": e.logical_time,
                "cause": e.cause.map(|c| c.0),
                "prev_hash": e.prev_hash,
                "content_hash": e.content_hash,
                "stable_json": stable,
            })
        })
        .collect();
    serde_json::Value::Array(entries).to_string()
}

/// One scripted LLM call session on the real audit mechanism.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct LlmAuditSession {
    /// Declared model name (recorded into every call_external command fact).
    model: String,
    facts_log: FactsLog,
    auditor: Auditor,
    next_fact_id: u64,
    /// Messages already persisted as transcript payload facts.
    persisted_messages: usize,
    /// (io_request fact id, command fact id) awaiting a response.
    pending: Option<FactId>,
    /// Aggregated provider token truth across calls.
    tokens_prompt: u64,
    tokens_completion: u64,
}

impl LlmAuditSession {
    /// Open a session for the declared model.
    pub fn new(model: &str) -> Self {
        let facts_log = FactsLog::new();
        Self {
            model: model.to_string(),
            auditor: Auditor::new(facts_log.clone()),
            facts_log,
            next_fact_id: 1,
            persisted_messages: 0,
            pending: None,
            tokens_prompt: 0,
            tokens_completion: 0,
        }
    }

    fn next_id(&mut self) -> FactId {
        let id = FactId(self.next_fact_id);
        self.next_fact_id += 1;
        id
    }

    /// Begin one call_external call: persist any not-yet-on-chain messages as
    /// transcript payload facts, then append the command fact (the exact
    /// params sent to the model) and the IoRequest fact.
    pub fn begin_call(&mut self, messages_json: &str) -> Result<serde_json::Value, String> {
        if self.pending.is_some() {
            return Err("begin_call: previous call still awaiting respond()".into());
        }
        let messages: Vec<serde_json::Value> =
            serde_json::from_str(messages_json).map_err(|e| format!("parse messages: {e}"))?;
        if messages.len() < self.persisted_messages {
            return Err("begin_call: messages shorter than already-persisted transcript".into());
        }

        // Transcript payload facts (real-path shape `__memory__.{ns}.session_{sid}.messages.{idx}`).
        for msg in &messages[self.persisted_messages..] {
            let id = self.next_id();
            let path = format!(
                "__memory__.demo.session_1.messages.{}",
                self.persisted_messages
            );
            self.facts_log
                .append(Fact::PayloadUpdate {
                    id,
                    path,
                    value: serde_to_tcb(msg),
                })
                .map_err(|e| format!("append transcript fact: {e}"))?;
            self.persisted_messages += 1;
        }
        self.auditor.audit_new();

        // call_external command fact: the exact request params.
        let params = serde_json::json!({
            "model": self.model,
            "temperature": 0.3,
            "max_tokens": 4096,
            "stream": true,
            "audit_purpose": "react",
            "messages": messages,
        });
        let instruction = serde_json::json!({ "type": "call_external", "params": params });
        let cmd_id = self.next_id();
        self.facts_log
            .append(Fact::Command {
                id: cmd_id,
                instruction: serde_to_tcb(&instruction),
            })
            .map_err(|e| format!("append command: {e}"))?;

        let io_id = self.next_id();
        self.facts_log
            .append(Fact::IoRequest {
                id: io_id,
                cause: cmd_id,
                io_type: IoType::new("call_external"),
                params: serde_to_tcb(&params),
            })
            .map_err(|e| format!("append io_request: {e}"))?;
        self.auditor.audit_new();

        self.pending = Some(io_id);
        Ok(serde_json::json!({
            "command_fact_id": cmd_id.0,
            "io_request_fact_id": io_id.0,
            "messages_sent": messages.len(),
        }))
    }

    /// Complete the pending call: append the IoResponse fact (the model's
    /// full reply + provider token truth), persist the assistant message,
    /// then seal with Stable. Returns the verify verdict and token usage.
    pub fn respond(&mut self, response_json: &str) -> Result<serde_json::Value, String> {
        let io_id = self
            .pending
            .take()
            .ok_or_else(|| "respond: no pending call (begin_call first)".to_string())?;
        let response: serde_json::Value =
            serde_json::from_str(response_json).map_err(|e| format!("parse response: {e}"))?;

        let result = serde_json::json!({
            "content": response.get("content").cloned().unwrap_or(serde_json::Value::Null),
            "tool_calls": response.get("tool_calls").cloned().unwrap_or(serde_json::Value::Null),
            "finish_reason": response.get("finish_reason").cloned().unwrap_or(serde_json::Value::Null),
            "token_usage": response.get("token_usage").cloned().unwrap_or(serde_json::Value::Null),
        });

        let id = self.next_id();
        self.facts_log
            .append(Fact::IoResponse {
                id,
                request_id: io_id,
                result: serde_to_tcb(&result),
                error: None,
            })
            .map_err(|e| format!("append io_response: {e}"))?;

        // Assistant message lands on the transcript too (with tool_calls if present).
        let assistant = serde_json::json!({
            "role": "assistant",
            "content": response.get("content").cloned().unwrap_or(serde_json::Value::Null),
            "tool_calls": response.get("tool_calls").cloned().unwrap_or(serde_json::Value::Null),
        });
        let tid = self.next_id();
        let path = format!(
            "__memory__.demo.session_1.messages.{}",
            self.persisted_messages
        );
        self.facts_log
            .append(Fact::PayloadUpdate {
                id: tid,
                path,
                value: serde_to_tcb(&assistant),
            })
            .map_err(|e| format!("append assistant fact: {e}"))?;
        self.persisted_messages += 1;

        let version = self.facts_log.version();
        let sid = self.next_id();
        self.facts_log
            .append(Fact::Stable { id: sid, version })
            .map_err(|e| format!("append stable: {e}"))?;
        self.auditor.audit_new();

        let verified = self.auditor.verify();
        if let Some(usage) = response.get("token_usage") {
            self.tokens_prompt += usage
                .get("prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            self.tokens_completion += usage
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
        }
        Ok(serde_json::json!({
            "io_response_fact_id": id.0,
            "stable_fact_id": sid.0,
            "stable_version": version,
            "verified": verified,
            "session_tokens": {
                "prompt": self.tokens_prompt,
                "completion": self.tokens_completion,
                "total": self.tokens_prompt + self.tokens_completion,
            },
        }))
    }

    /// Chain view (one metadata entry per fact).
    pub fn audit_chain(&self) -> String {
        let facts = self.facts_log.read_from(0).unwrap_or_default();
        let entries: Vec<serde_json::Value> = self
            .auditor
            .entries()
            .iter()
            .zip(facts.iter())
            .map(|(e, f)| {
                serde_json::json!({
                    "fact_id": e.fact_id.0,
                    "fact_type": e.fact_type,
                    "logical_time": e.logical_time,
                    "cause": e.cause.map(|c| c.0),
                    "prev_hash": e.prev_hash,
                    "content_hash": e.content_hash,
                    "fact_json": tcb_to_serde(&f.to_json()),
                })
            })
            .collect();
        serde_json::Value::Array(entries).to_string()
    }

    /// Offline verification export (verifier input; tamper-friendly).
    pub fn export_chain(&self) -> String {
        chain_export(&self.facts_log, &self.auditor)
    }

    /// Engine-side end-to-end chain verification.
    pub fn verify(&self) -> bool {
        self.auditor.verify()
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl LlmAuditSession {
    #[wasm_bindgen(constructor)]
    pub fn with_model(model: &str) -> LlmAuditSession {
        LlmAuditSession::new(model)
    }

    #[wasm_bindgen(js_name = begin_call)]
    pub fn begin_call_js(&mut self, messages_json: &str) -> Result<String, wasm_bindgen::JsValue> {
        LlmAuditSession::begin_call(self, messages_json)
            .map(|v| v.to_string())
            .map_err(|e| wasm_bindgen::JsValue::from_str(&e))
    }

    #[wasm_bindgen(js_name = respond)]
    pub fn respond_js(&mut self, response_json: &str) -> Result<String, wasm_bindgen::JsValue> {
        LlmAuditSession::respond(self, response_json)
            .map(|v| v.to_string())
            .map_err(|e| wasm_bindgen::JsValue::from_str(&e))
    }

    #[wasm_bindgen(js_name = audit_chain)]
    pub fn audit_chain_js(&self) -> String {
        LlmAuditSession::audit_chain(self)
    }

    #[wasm_bindgen(js_name = export_chain)]
    pub fn export_chain_js(&self) -> String {
        LlmAuditSession::export_chain(self)
    }

    #[wasm_bindgen(js_name = verify)]
    pub fn verify_js(&self) -> bool {
        LlmAuditSession::verify(self)
    }
}

/// wasm-bindgen export of [`engine_version`] (wasm32 only, same JS name).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = engine_version)]
pub fn engine_version_js() -> String {
    engine_version()
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

        // IoResponse fact lands on the chain BEFORE the replay commit: the
        // response text (the external executor's full output — for LLM calls,
        // the model reply itself) is a first-class audit fact, cause-linked
        // to its IoRequest. Mirrors what the async reactor emits for
        // io_response submission in the real engine.
        let resp_id = self.next_id();
        self.facts_log
            .append(Fact::IoResponse {
                id: resp_id,
                request_id: io_id.unwrap_or(FactId(0)),
                result: io_result_tcb.clone(),
                error: None,
            })
            .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("append IoResponse: {e}")))?;
        self.auditor.audit_new();

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

    /// Offline verification export: per-fact chain metadata PLUS the exact
    /// stable-JSON text that was hashed (the verifier's input). Consumers may
    /// edit `stable_json` freely and re-run [`verify_exported_chain`] — a
    /// tampered byte breaks the content hash at that entry and cascades to
    /// every later link. This is what the live demo's tamper view is built on.
    pub fn export_chain(&self) -> String {
        let facts = self.facts_log.read_from(0).unwrap_or_default();
        let entries: Vec<serde_json::Value> = self
            .auditor
            .entries()
            .iter()
            .zip(facts.iter())
            .map(|(e, f)| {
                let stable = fact_to_stable_json(f)
                    .ok()
                    .and_then(|v| serde_json::to_string(&v).ok());
                serde_json::json!({
                    "fact_id": e.fact_id.0,
                    "fact_type": e.fact_type,
                    "logical_time": e.logical_time,
                    "cause": e.cause.map(|c| c.0),
                    "prev_hash": e.prev_hash,
                    "content_hash": e.content_hash,
                    "stable_json": stable,
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
        let facts = self.facts_log.read_from(0).unwrap();
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

                let trace_hits: Vec<TraceHit> =
                    rule_hits.iter().map(TraceHit::from_rule_hit).collect();
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

#[cfg(test)]
mod tests {
    use super::{engine_version, verify_exported_chain_impl, CONSTITUTION};

    /// engine_version() must report the TCB dependency tag extracted by
    /// build.rs — this is the string shown in the live demo footer.
    #[test]
    fn engine_version_reports_tcb() {
        let v = engine_version();
        assert!(
            v.starts_with("evorule-wasm-demo 0.1.0 / TCB v"),
            "unexpected engine_version format: {v}"
        );
        // Must not leak the raw env placeholder if build.rs failed to run.
        assert!(
            !v.contains("EVORULE_TCB_DEP_VERSION"),
            "build.rs did not run"
        );
    }

    /// Vendored constitution integrity: the embedded copy must match the
    /// hash recorded at sync time. If this test fails, the copy drifted from
    /// the pinned kernel tag — re-run scripts/sync-constitution.sh at the
    /// tag bump and update the expected hash below.
    #[test]
    fn constitution_matches_synced_hash() {
        const EXPECTED: &str =
            "blake3:4b7d1e7850ec8ab1a34861d9493def752e27bdef798a72b9a4ea0f2537ad4960";
        let actual = format!("blake3:{}", blake3::hash(CONSTITUTION.as_bytes()).to_hex());
        assert_eq!(
            actual, EXPECTED,
            "constitution/core_eval.json drifted from the pinned tag's copy"
        );
    }

    /// Offline verifier, honest chain: recomputed content hashes and links
    /// all check out, chain tip reported.
    #[test]
    fn verify_exported_chain_accepts_honest_chain() {
        use evorule_reactor::{chain_step, content_hash, serde_to_tcb};

        let mk_entry = |prev: &str, payload: serde_json::Value| {
            let stable = serde_json::json!({ "type": "Command", "instruction": payload });
            let content = content_hash(&serde_to_tcb(&stable)).unwrap();
            let prev_hash = chain_step(prev, &content);
            serde_json::json!({
                "fact_id": 1, "fact_type": "Command",
                "prev_hash": prev, "content_hash": content,
                "stable_json": serde_json::to_string(&stable).unwrap(),
                "_prev_hash": prev_hash,
            })
        };

        let mut e0 = mk_entry("genesis", serde_json::json!({"n": 0}));
        let mut e1 = mk_entry(
            e0["_prev_hash"].as_str().unwrap(),
            serde_json::json!({"n": 1}),
        );
        let mut e2 = mk_entry(
            e1["_prev_hash"].as_str().unwrap(),
            serde_json::json!({"n": 2}),
        );
        for e in [&mut e0, &mut e1, &mut e2] {
            e.as_object_mut().unwrap().remove("_prev_hash");
        }
        let chain = serde_json::Value::Array(vec![e0, e1, e2]).to_string();

        let report: serde_json::Value =
            verify_exported_chain_impl(&chain).expect("verify honest chain");
        assert_eq!(report["verified"], serde_json::Value::Bool(true));
        assert_eq!(report["first_broken_index"], serde_json::Value::Null);
        assert_eq!(report["entry_count"], 3);
    }

    /// Offline verifier, tampered chain: editing one entry's hashed text must
    /// break that entry's content hash AND cascade to every later link.
    #[test]
    fn verify_exported_chain_detects_tamper_with_cascade() {
        use evorule_reactor::{chain_step, content_hash, serde_to_tcb};

        let mk_entry = |prev: &str, payload: serde_json::Value| {
            let stable = serde_json::json!({ "type": "Command", "instruction": payload });
            let content = content_hash(&serde_to_tcb(&stable)).unwrap();
            let prev_hash = chain_step(prev, &content);
            serde_json::json!({
                "fact_id": 1, "fact_type": "Command",
                "prev_hash": prev, "content_hash": content,
                "stable_json": serde_json::to_string(&stable).unwrap(),
                "_prev_hash": prev_hash,
            })
        };

        let mut e0 = mk_entry("genesis", serde_json::json!({"n": 0}));
        let mut e1 = mk_entry(
            e0["_prev_hash"].as_str().unwrap(),
            serde_json::json!({"n": 1}),
        );
        let mut e2 = mk_entry(
            e1["_prev_hash"].as_str().unwrap(),
            serde_json::json!({"n": 2}),
        );
        for e in [&mut e0, &mut e1, &mut e2] {
            e.as_object_mut().unwrap().remove("_prev_hash");
        }

        // Tamper: one byte of entry 1's hashed text.
        let tampered = e1["stable_json"]
            .as_str()
            .unwrap()
            .replacen("\"n\":1", "\"n\":2", 1);
        assert_ne!(
            tampered,
            e1["stable_json"].as_str().unwrap(),
            "tamper must change bytes"
        );
        e1["stable_json"] = serde_json::Value::String(tampered);

        let chain = serde_json::Value::Array(vec![e0, e1, e2]).to_string();
        let report: serde_json::Value =
            verify_exported_chain_impl(&chain).expect("verify tampered chain");

        assert_eq!(report["verified"], serde_json::Value::Bool(false));
        assert_eq!(
            report["first_broken_index"], 1,
            "break lands on the tampered entry"
        );
        assert_eq!(
            report["entries"][1]["content_ok"],
            serde_json::Value::Bool(false)
        );
        assert_eq!(
            report["entries"][2]["link_ok"],
            serde_json::Value::Bool(false)
        );
    }

    // ----- LlmAuditSession: scripted call_external conversation on the real
    // audit mechanism (paper appendix core) -----

    const CALL_MESSAGES: &str = r#"[{"role":"system","content":"You are a demo assistant."},
            {"role":"user","content":"读取 foo.txt 并总结。"}]"#;

    /// One scripted call: fact stream shape, causal linkage, token truth in
    /// the IoResponse fact, transcript persistence, engine verify verdict.
    #[test]
    fn llm_session_fact_stream_shape_and_tokens() {
        use super::LlmAuditSession;

        let mut s = LlmAuditSession::new("audit-probe-mock");
        s.begin_call(CALL_MESSAGES).expect("begin_call");
        let respond = s
            .respond(
                r#"{"content":"MOCK-REPLY","finish_reason":"stop",
                    "token_usage":{"prompt_tokens":12,"completion_tokens":5,"total_tokens":17}}"#,
            )
            .expect("respond");

        assert_eq!(respond["verified"], serde_json::Value::Bool(true));
        assert_eq!(respond["session_tokens"]["total"], 17);

        let chain: Vec<serde_json::Value> = serde_json::from_str(&s.audit_chain()).unwrap();
        let types: Vec<&str> = chain
            .iter()
            .map(|e| e["fact_type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            vec![
                "PayloadUpdate",
                "PayloadUpdate",
                "Command",
                "IoRequest",
                "IoResponse",
                "PayloadUpdate",
                "Stable"
            ],
            "unexpected fact stream: {types:?}"
        );

        // Causal link: IoResponse hangs off its IoRequest.
        let io_req_idx = types.iter().position(|t| *t == "IoRequest").unwrap();
        let io_resp_idx = types.iter().position(|t| *t == "IoResponse").unwrap();
        assert_eq!(
            chain[io_resp_idx]["cause"], chain[io_req_idx]["fact_id"],
            "IoResponse must be cause-linked to its IoRequest"
        );

        // Token truth recorded inside the IoResponse fact content.
        let export: Vec<serde_json::Value> = serde_json::from_str(&s.export_chain()).unwrap();
        assert!(
            export[io_resp_idx]["stable_json"]
                .as_str()
                .unwrap()
                .contains("\"total_tokens\":17"),
            "token usage must be inside the hashed IoResponse content"
        );

        // Transcript paths follow the real payload-fact shape.
        let transcript_paths: Vec<&str> = chain
            .iter()
            .filter(|e| e["fact_type"] == "PayloadUpdate")
            .filter_map(|e| e["fact_json"]["path"].as_str())
            .collect();
        assert!(
            transcript_paths
                .iter()
                .all(|p| p.starts_with("__memory__.demo.session_1.messages.")),
            "transcript paths must follow the real payload-fact shape: {transcript_paths:?}"
        );
        assert!(s.verify());
    }

    /// Two-call scripted run (tool round + final answer): token aggregation
    /// across calls, and byte-identical determinism for identical scripts.
    #[test]
    fn llm_session_two_calls_deterministic() {
        use super::LlmAuditSession;

        let run_script = |s: &mut LlmAuditSession| {
            s.begin_call(CALL_MESSAGES).unwrap();
            s.respond(
                r#"{"content":"","tool_calls":[{"tool_name":"file_read","args":{"path":"foo.txt"}}],
                    "finish_reason":"tool_call",
                    "token_usage":{"prompt_tokens":12,"completion_tokens":5,"total_tokens":17}}"#,
            )
            .unwrap();
            let history = format!(
                "{}, {{\"role\":\"tool\",\"tool_name\":\"file_read\",\"content\":\"hello audit chain\"}}",
                CALL_MESSAGES.trim_end_matches(']')
            );
            s.begin_call(&format!("{history}]")).unwrap();
            s.respond(
                r#"{"content":"FILE-READ-DONE","finish_reason":"stop",
                    "token_usage":{"prompt_tokens":40,"completion_tokens":7,"total_tokens":47}}"#,
            )
            .unwrap()
        };

        let mut s = LlmAuditSession::new("audit-probe-mock");
        let respond_report = run_script(&mut s);
        assert_eq!(
            respond_report["session_tokens"]["prompt"], 52,
            "aggregated prompt tokens"
        );
        assert_eq!(
            respond_report["session_tokens"]["completion"], 12,
            "aggregated completion tokens"
        );
        assert_eq!(respond_report["verified"], serde_json::Value::Bool(true));

        let a = {
            let mut s = LlmAuditSession::new("audit-probe-mock");
            run_script(&mut s);
            s.export_chain()
        };
        let b = {
            let mut s = LlmAuditSession::new("audit-probe-mock");
            run_script(&mut s);
            s.export_chain()
        };
        assert_eq!(a, b, "identical scripts must produce byte-identical chains");
    }

    /// Tampering the session export (edit the model reply inside the hashed
    /// IoResponse text) breaks that entry and cascades to the tail.
    #[test]
    fn llm_session_tamper_cascades() {
        use super::{verify_exported_chain_impl, LlmAuditSession};

        let mut s = LlmAuditSession::new("audit-probe-mock");
        s.begin_call(CALL_MESSAGES).unwrap();
        s.respond(
            r#"{"content":"MOCK-REPLY","finish_reason":"stop",
                "token_usage":{"prompt_tokens":12,"completion_tokens":5,"total_tokens":17}}"#,
        )
        .unwrap();

        let mut exported: Vec<serde_json::Value> = serde_json::from_str(&s.export_chain()).unwrap();
        let resp_idx = exported
            .iter()
            .position(|e| e["fact_type"] == "IoResponse")
            .unwrap();
        let tampered = exported[resp_idx]["stable_json"]
            .as_str()
            .unwrap()
            .replace("MOCK-REPLY", "TAMPERED-REPLY");
        exported[resp_idx]["stable_json"] = serde_json::Value::String(tampered);

        let report: serde_json::Value =
            verify_exported_chain_impl(&serde_json::Value::Array(exported).to_string()).unwrap();
        assert_eq!(report["verified"], serde_json::Value::Bool(false));
        assert_eq!(report["first_broken_index"], resp_idx);
        let last = report["entries"].as_array().unwrap().len() - 1;
        assert_eq!(
            report["entries"][last]["link_ok"],
            serde_json::Value::Bool(false),
            "cascade must reach the tail entry"
        );
    }
}
