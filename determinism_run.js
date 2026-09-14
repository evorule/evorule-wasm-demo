// SPDX-License-Identifier: AGPL-3.0-or-later
// determinism_run.js — WASM(wasm32) vs native(x86_64) 逐字节一致性驱动。
//
// 流程:
//   1. 合并 5 个规则文件(与 console-cloud rules-loader 同序) -> rules_merged.json
//   2. 生成 32 条 fresh 用例(16 规则 x pass/fail) + 3 条 sequence 用例 -> plan.json
//   3. 子进程跑 native: cargo run --example determinism_driver -- rules plan
//   4. Node 侧加载 pkg/ 产物, 驱动 EvoRuleEngine 跑同一 plan
//   5. 逐字段 diff, 打印每条 PASS/FAIL
//
// 运行: node determinism_run.js
'use strict';

const { execFileSync } = require('node:child_process');
const { readFileSync, writeFileSync } = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');

const RULES_SRC_DIR = 'D:\\evorule-console-cloud\\static\\rules';
const PKG_DIR = path.join(__dirname, 'pkg');
const WORK = __dirname;

// ---------- 1. 合并规则(与 rules-loader.ts 同序) ----------
const RULE_FILES = [
  'core_eval.json',
  '20_finance_rules.json',
  '21_medical_rules.json',
  '22_djbh_rules.json',
  '10_role13_demo.json',
];
const merged = [];
for (const f of RULE_FILES) {
  const obj = JSON.parse(readFileSync(path.join(RULES_SRC_DIR, f), 'utf8'));
  const arr = Array.isArray(obj) ? obj : obj.transform;
  if (!Array.isArray(arr)) throw new Error('no transform: ' + f);
  merged.push(...arr);
}
const rulesMergedJson = JSON.stringify(merged);
const rulesPath = path.join(WORK, 'rules_merged.json');
writeFileSync(rulesPath, rulesMergedJson);

// ---------- 2. 测试用例矩阵 ----------
// 每条规则: 1 条 pass(通过闸门) + 1 条 fail(触发阻断/拒绝)
const FRESH_CASES = [
  // --- 财务 6 ---
  { id: 'fin_expense_limit_pass', instruction: { type: 'finance_expense_limit_check', params: { expense: { type: 'travel', amount: 1000 } } } },
  { id: 'fin_expense_limit_fail', instruction: { type: 'finance_expense_limit_check', params: { expense: { type: 'travel', amount: 5000 } } } },
  { id: 'fin_approval_pass', instruction: { type: 'finance_approval_routing_check', params: { expense: { amount: 400 } } } },
  { id: 'fin_approval_fail', instruction: { type: 'finance_approval_routing_check', params: { expense: { amount: 60000 } } } },
  { id: 'fin_invoice_pass', instruction: { type: 'finance_invoice_compliance_check', params: { invoice: { invoice_no: 'INV-001', buyer_name: 'ACME', buyer_tax_id: '91110000MA001XY', amount: 1000, date: '2026-01-01' } } } },
  { id: 'fin_invoice_fail', instruction: { type: 'finance_invoice_compliance_check', params: { invoice: { invoice_no: '', buyer_name: '', buyer_tax_id: '', amount: 0, date: '' } } } },
  { id: 'fin_split_pass', instruction: { type: 'finance_expense_split_check', params: { split_suspected: false } } },
  { id: 'fin_split_fail', instruction: { type: 'finance_expense_split_check', params: { split_suspected: true } } },
  { id: 'fin_budget_pass', instruction: { type: 'finance_budget_check', params: { budget: { within_budget: true, remaining: 500 } } } },
  { id: 'fin_budget_fail', instruction: { type: 'finance_budget_check', params: { budget: { within_budget: false, remaining: 0 } } } },
  { id: 'fin_encryption_pass', instruction: { type: 'finance_storage_encryption_gate', params: { operation: { fields: {} }, encryption: 'aes256' } } },
  { id: 'fin_encryption_fail', instruction: { type: 'finance_storage_encryption_gate', params: { operation: { fields: { id_card: '110101199001011234' } }, encryption: 'none' } } },
  // --- 医疗 6 ---
  { id: 'med_antibiotic_tier_pass', instruction: { type: 'medical_antibiotic_tier_check', params: { prescription: { antibiotic_tier: 'non_restricted' }, visit: { type: 'outpatient' } } } },
  { id: 'med_antibiotic_tier_fail', instruction: { type: 'medical_antibiotic_tier_check', params: { prescription: { antibiotic_tier: 'special' }, visit: { type: 'outpatient' } } } },
  { id: 'med_indication_pass', instruction: { type: 'medical_drug_indication_check', params: { diagnosis: { type: 'bacterial' }, lab: { has_drug_sensitivity: true } } } },
  { id: 'med_indication_fail', instruction: { type: 'medical_drug_indication_check', params: { diagnosis: { type: 'viral' }, lab: {} } } },
  { id: 'med_allergy_pass', instruction: { type: 'medical_drug_allergy_check', params: { patient: { has_penicillin_allergy: false }, prescription: { drug_class: 'penicillin' } } } },
  { id: 'med_allergy_fail', instruction: { type: 'medical_drug_allergy_check', params: { patient: { has_penicillin_allergy: true }, prescription: { drug_class: 'penicillin' } } } },
  { id: 'med_override_pass', instruction: { type: 'medical_antibiotic_override_check', params: { prescription: { is_emergency_override: false, paperwork_filed: false, hours_since_prescription: 0 } } } },
  { id: 'med_override_fail', instruction: { type: 'medical_antibiotic_override_check', params: { prescription: { is_emergency_override: true, paperwork_filed: false, hours_since_prescription: 30 } } } },
  { id: 'med_triage_pass', instruction: { type: 'medical_triage_check', params: { patient: { age: 30, temperature_tenths: 370 }, lab: { wbc: 8000, crp: 10 } } } },
  { id: 'med_triage_fail', instruction: { type: 'medical_triage_check', params: { patient: { age: 70, temperature_tenths: 390 }, lab: { wbc: 20000, crp: 60 } } } },
  { id: 'med_mfa_pass', instruction: { type: 'medical_mfa_gate', params: { operation: { type: 'read_medical_record' }, auth: { mfa_verified: true } } } },
  { id: 'med_mfa_fail', instruction: { type: 'medical_mfa_gate', params: { operation: { type: 'read_medical_record' }, auth: { mfa_verified: false } } } },
  // --- 等保 4 ---
  { id: 'djbh_mfa_pass', instruction: { type: 'djbh_mfa_check', params: { operation_risk: 'high', mfa_factor_count: 2 } } },
  { id: 'djbh_mfa_fail', instruction: { type: 'djbh_mfa_check', params: { operation_risk: 'high', mfa_factor_count: 1 } } },
  { id: 'djbh_access_pass', instruction: { type: 'djbh_access_control_check', params: { requested_permission: 'read', user_role: 'auditor' } } },
  { id: 'djbh_access_fail', instruction: { type: 'djbh_access_control_check', params: { requested_permission: 'admin', user_role: 'auditor' } } },
  { id: 'djbh_audit_pass', instruction: { type: 'djbh_audit_check', params: { audit_enabled: true, audit_log_protected: true } } },
  { id: 'djbh_audit_fail', instruction: { type: 'djbh_audit_check', params: { audit_enabled: false, audit_log_protected: true } } },
  { id: 'djbh_conf_pass', instruction: { type: 'djbh_confidentiality_check', params: { data_classification: 'internal', encryption: 'none' } } },
  { id: 'djbh_conf_fail', instruction: { type: 'djbh_confidentiality_check', params: { data_classification: 'sensitive', encryption: 'none' } } },
];

// 3 条指令序列(多 StateTransition, 用于审计链哈希链跨指令一致)
const SEQUENCE_CASES = [
  { id: 'seq1_travel_allowed', instruction: { type: 'finance_expense_limit_check', params: { expense: { type: 'travel', amount: 1000 } } } },
  { id: 'seq2_invoice_compliant', instruction: { type: 'finance_invoice_compliance_check', params: { invoice: { invoice_no: 'INV-S1', buyer_name: 'X', buyer_tax_id: '91110000MA001XY', amount: 500, date: '2026-02-02' } } } },
  { id: 'seq3_mfa_blocked', instruction: { type: 'medical_mfa_gate', params: { operation: { type: 'read_medical_record' }, auth: { mfa_verified: false } } } },
];

const plan = { fresh: FRESH_CASES, sequence: SEQUENCE_CASES };
const planPath = path.join(WORK, 'plan.json');
writeFileSync(planPath, JSON.stringify(plan));

// ---------- 3. native 侧 ----------
console.log('=== Running native driver (cargo build may take a moment) ===');
const nativeStdout = execFileSync(
  'cargo',
  ['run', '--quiet', '--example', 'determinism_driver', '--', rulesPath, planPath],
  { cwd: __dirname, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
);
const nativeResult = JSON.parse(nativeStdout.trim());

// ---------- 4. WASM 侧 ----------
async function runWasm() {
  const mod = await import(pathToFileURL(path.join(PKG_DIR, 'evorule_wasm_demo.js')).href);
  const wasmBytes = readFileSync(path.join(PKG_DIR, 'evorule_wasm_demo_bg.wasm'));
  mod.initSync({ module: wasmBytes });
  const { EvoRuleEngine } = mod;

  const out = { fresh: [], sequence: [], chain: [], sequence_final_verified: null };

  for (const c of FRESH_CASES) {
    const eng = new EvoRuleEngine();
    eng.load_rules(rulesMergedJson);
    const reply = JSON.parse(eng.execute_instruction(JSON.stringify(c.instruction)));
    out.fresh.push({
      id: c.id,
      payload: reply.payload ?? null,
      version: reply.version ?? null,
      audit_verified: reply.audit_verified ?? null,
      audit_len: JSON.parse(eng.get_audit_chain()).length,
    });
    eng.free();
  }

  const eng = new EvoRuleEngine();
  eng.load_rules(rulesMergedJson);
  for (const c of SEQUENCE_CASES) {
    const reply = JSON.parse(eng.execute_instruction(JSON.stringify(c.instruction)));
    out.sequence.push({
      id: c.id,
      payload: reply.payload ?? null,
      version: reply.version ?? null,
      audit_verified: reply.audit_verified ?? null,
    });
  }
  out.chain = JSON.parse(eng.get_audit_chain());
  out.sequence_final_verified = eng.verify_audit_chain();
  eng.free();
  return out;
}

// ---------- 5. diff ----------
function eq(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

async function main() {
  const wasmResult = await runWasm();

  let pass = 0, fail = 0;
  const failures = [];
  function check(name, cond, detail) {
    if (cond) { pass++; console.log('PASS  ' + name); }
    else { fail++; console.log('FAIL  ' + name + (detail ? '  |  ' + detail : '')); failures.push(name); }
  }

  console.log('\n=== [1] FRESH single-instruction determinism (' + FRESH_CASES.length + ' cases) ===');
  for (let i = 0; i < FRESH_CASES.length; i++) {
    const n = nativeResult.fresh[i];
    const w = wasmResult.fresh[i];
    const tag = n.id;
    check(tag + ' payload identical', eq(n.payload, w.payload),
      'native=' + JSON.stringify(n.payload).slice(0, 120) + ' wasm=' + JSON.stringify(w.payload).slice(0, 120));
    check(tag + ' version identical', n.version === w.version, 'native=' + n.version + ' wasm=' + w.version);
    check(tag + ' audit_verified identical', n.audit_verified === w.audit_verified, 'native=' + n.audit_verified + ' wasm=' + w.audit_verified);
    check(tag + ' audit_len identical', n.audit_len === w.audit_len, 'native=' + n.audit_len + ' wasm=' + w.audit_len);
  }

  console.log('\n=== [2] SEQUENCE multi-instruction determinism ===');
  for (let i = 0; i < SEQUENCE_CASES.length; i++) {
    const n = nativeResult.sequence[i];
    const w = wasmResult.sequence[i];
    check(n.id + ' payload identical', eq(n.payload, w.payload));
    check(n.id + ' version identical', n.version === w.version, 'native=' + n.version + ' wasm=' + w.version);
  }

  console.log('\n=== [3] AUDIT CHAIN hash byte-identical (content_hash / prev_hash) ===');
  check('chain length equal', nativeResult.chain.length === wasmResult.chain.length,
    'native=' + nativeResult.chain.length + ' wasm=' + wasmResult.chain.length);
  const nChain = nativeResult.chain;
  const wChain = wasmResult.chain;
  const len = Math.min(nChain.length, wChain.length);
  for (let i = 0; i < len; i++) {
    const n = nChain[i], w = wChain[i];
    const label = 'chain[' + i + '] fact_id=' + n.fact_id + ' type=' + n.fact_type;
    check(label + ' fact_type identical', n.fact_type === w.fact_type);
    check(label + ' content_hash byte-identical', n.content_hash === w.content_hash,
      'native=' + n.content_hash + ' wasm=' + w.content_hash);
    check(label + ' prev_hash byte-identical', n.prev_hash === w.prev_hash,
      'native=' + n.prev_hash + ' wasm=' + w.prev_hash);
    check(label + ' logical_time identical', n.logical_time === w.logical_time);
  }
  check('sequence_final_verified both true',
    nativeResult.sequence_final_verified === true && wasmResult.sequence_final_verified === true,
    'native=' + nativeResult.sequence_final_verified + ' wasm=' + wasmResult.sequence_final_verified);

  console.log('\n=== SUMMARY ===');
  console.log('PASS=' + pass + '  FAIL=' + fail);
  if (fail > 0) {
    console.log('FAILED ITEMS: ' + failures.join(', '));
    process.exit(1);
  }
}

main().catch((e) => { console.error('FATAL:', e); process.exit(2); });
