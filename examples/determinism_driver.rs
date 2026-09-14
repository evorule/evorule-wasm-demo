// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 EvoRule Project
//
// determinism_driver —— 原生（x86_64）对照驱动。
//
// 逐行镜像 src/lib.rs 中 EvoRuleEngine 的有状态事实链逻辑：
//   Command -> StateTransition -> TransitionTrace -> Stable -> auditor.audit_new()
// 用与 wasm32 完全相同的 TCB 纯函数 + BLAKE3 审计链，对同一份合并规则集、
// 同一指令序列执行，把结果 JSON 打到 stdout，供 Node 侧与 wasm32 产物逐字节 diff。
//
// 用法:
//   cargo run --example determinism_driver -- <rules_json_path> <plan_json_path>
//
// plan 格式:
//   {
//     "fresh":    [ {"id":"...", "instruction":{...}}, ... ],  // 每条用全新引擎
//     "sequence": [ {"id":"...", "instruction":{...}}, ... ]   // 单引擎顺序执行
//   }
// 输出:
//   {
//     "fresh":    [ {"id","payload","version","audit_verified","audit_len"} ],
//     "sequence": [ {"id","payload","version","audit_verified"} ],
//     "chain":    [ {"fact_id","fact_type","logical_time","content_hash","prev_hash","cause"} ]
//   }
#![allow(clippy::unwrap_used, clippy::expect_used)]

use evorule_governance::Auditor;
use evorule_reactor::{serde_to_tcb, tcb_to_serde, Fact, FactId, FactsLog, TraceHit};
use evorule_tcb::{execute_transition, JsonValue, TransitionResult};
use std::fs;

/// 与 EvoRuleEngine 一一对应的有状态执行器。
struct Engine {
    rules: Vec<JsonValue>,
    view_payload: JsonValue,
    view_queue: Vec<JsonValue>,
    view_version: u64,
    facts_log: FactsLog,
    auditor: Auditor,
    next_fact_id: u64,
}

impl Engine {
    fn new(rules_json: &str) -> Self {
        let parsed: serde_json::Value = serde_json::from_str(rules_json).unwrap();
        let arr = parsed.as_array().expect("rules must be a JSON array");
        let rules = arr.iter().map(serde_to_tcb).collect();
        let facts_log = FactsLog::new();
        Engine {
            rules,
            view_payload: JsonValue::empty_object(),
            view_queue: Vec::new(),
            view_version: 0,
            auditor: Auditor::new(facts_log.clone()),
            facts_log,
            next_fact_id: 1,
        }
    }

    fn next_id(&mut self) -> FactId {
        let id = FactId(self.next_fact_id);
        self.next_fact_id += 1;
        id
    }

    /// 执行一条指令并提交事实链（镜像 EvoRuleEngine::run_step + handle_transition_result）。
    fn step(&mut self, instruction_json: &str) -> serde_json::Value {
        let cmd: serde_json::Value = serde_json::from_str(instruction_json).unwrap();
        let instruction = serde_to_tcb(&cmd);

        let cmd_id = self.next_id();
        self.facts_log
            .append(Fact::Command { id: cmd_id, instruction: instruction.clone() })
            .unwrap();

        let result =
            execute_transition(&self.rules, &instruction, &self.view_payload, &self.view_queue)
                .unwrap();

        match result {
            TransitionResult::State { new_payload, new_queue, rule_hits } => {
                let st_id = self.next_id();
                self.facts_log
                    .append(Fact::StateTransition {
                        id: st_id,
                        cause: cmd_id,
                        new_payload: new_payload.clone(),
                        new_queue: new_queue.clone(),
                    })
                    .unwrap();

                let trace_hits: Vec<TraceHit> = rule_hits
                    .iter()
                    .map(|h| TraceHit {
                        index: h.index as u64,
                        instr_type: h.instr_type.clone(),
                        hit: h.hit.clone(),
                    })
                    .collect();
                let trace_id = self.next_id();
                self.facts_log
                    .append(Fact::TransitionTrace { id: trace_id, cause: st_id, rule_hits: trace_hits })
                    .unwrap();

                let version = self.facts_log.version();
                let stable_id = self.next_id();
                self.facts_log
                    .append(Fact::Stable { id: stable_id, version })
                    .unwrap();

                self.view_payload = new_payload;
                self.view_queue = new_queue;
                self.view_version = version;

                self.auditor.audit_new();
                let verified = self.auditor.verify();

                serde_json::json!({
                    "type": "state",
                    "payload": tcb_to_serde(&self.view_payload),
                    "version": version,
                    "audit_verified": verified,
                })
            }
            other => {
                // 16 条纯 transform 规则不触发 io/halt/ignored；此处如实回显以便定位差异。
                serde_json::json!({
                    "type": format!("{:?}", other).chars().take_while(|c| c.is_alphabetic()).collect::<String>().to_lowercase(),
                    "raw": format!("{:?}", other),
                })
            }
        }
    }

    fn audit_len(&self) -> usize {
        self.auditor.entries().len()
    }

    fn verify(&self) -> bool {
        self.auditor.verify()
    }

    fn chain_json(&self) -> serde_json::Value {
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
        serde_json::Value::Array(entries)
    }
}

fn main() {
    let rules_path = std::env::args().nth(1).expect("usage: determinism_driver <rules> <plan>");
    let plan_path = std::env::args().nth(2).expect("usage: determinism_driver <rules> <plan>");

    let rules_json = fs::read_to_string(&rules_path).expect("read rules");
    let plan: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&plan_path).expect("read plan"))
            .expect("parse plan");

    // --- fresh cases: 每条用全新引擎 ---
    let mut fresh_out = Vec::new();
    for case in plan["fresh"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap().to_string();
        let instr = case["instruction"].to_string();
        let mut eng = Engine::new(&rules_json);
        let reply = eng.step(&instr);
        fresh_out.push(serde_json::json!({
            "id": id,
            "payload": reply.get("payload").cloned().unwrap_or(serde_json::Value::Null),
            "version": reply.get("version").cloned().unwrap_or(serde_json::Value::Null),
            "audit_verified": reply.get("audit_verified").cloned().unwrap_or(serde_json::Value::Null),
            "audit_len": eng.audit_len(),
        }));
    }

    // --- sequence: 单引擎顺序执行 ---
    let mut seq_out = Vec::new();
    let mut eng = Engine::new(&rules_json);
    for case in plan["sequence"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap().to_string();
        let instr = case["instruction"].to_string();
        let reply = eng.step(&instr);
        seq_out.push(serde_json::json!({
            "id": id,
            "payload": reply.get("payload").cloned().unwrap_or(serde_json::Value::Null),
            "version": reply.get("version").cloned().unwrap_or(serde_json::Value::Null),
            "audit_verified": reply.get("audit_verified").cloned().unwrap_or(serde_json::Value::Null),
        }));
    }
    let chain = eng.chain_json();
    let final_verified = eng.verify();

    let out = serde_json::json!({
        "fresh": fresh_out,
        "sequence": seq_out,
        "chain": chain,
        "sequence_final_verified": final_verified,
    });
    // 机器可读：仅打印 JSON 本体（stderr 留给日志）。
    println!("{}", out.to_string());
}
