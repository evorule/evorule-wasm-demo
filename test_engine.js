// test_engine.js — Node.js driver for the stateful EvoRuleEngine WASM API.
//
// Loads the --target-web ES-module glue via dynamic import, initializes the
// wasm module synchronously from the .wasm bytes (no browser fetch needed),
// then exercises the stateful API against the three demo rulesets:
//   20_finance_rules.json, 21_medical_rules.json, 22_djbh_rules.json.
//
// Run:  node test_engine.js
'use strict';

const { readFileSync } = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');

const RULES_DIR = 'D:\\evorule-server\\rules';
const PKG_DIR = path.join(__dirname, 'pkg');

let failures = 0;
function check(name, cond, detail) {
  const ok = !!cond;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== undefined ? '  |  ' + detail : ''}`);
  if (!ok) failures++;
}

async function main() {
  // 1. Load the wasm glue (ESM) and initialize synchronously from bytes.
  const mod = await import(pathToFileURL(path.join(PKG_DIR, 'evorule_wasm_demo.js')).href);
  const wasmBytes = readFileSync(path.join(PKG_DIR, 'evorule_wasm_demo_bg.wasm'));
  mod.initSync({ module: wasmBytes });
  const { EvoRuleEngine } = mod;

  console.log('=== EvoRuleEngine stateful API smoke test ===\n');

  // ---------- Finance rules ----------
  console.log('--- [1] FINANCE: 差旅报销 5000 元 ---');
  const finRules = JSON.parse(
    readFileSync(path.join(RULES_DIR, '20_finance_rules.json'), 'utf8')
  );
  const fin = new EvoRuleEngine();
  fin.load_rules(JSON.stringify(finRules.transform));

  // Command A: travel expense 5000 -> blocked (>= 3000)
  const a = JSON.parse(fin.execute_instruction(JSON.stringify({
    type: 'finance_expense_limit_check',
    params: { expense: { type: 'travel', amount: 5000 } },
  })));
  console.log('  cmd A result.type   =', a.type);
  console.log('  cmd A version       =', a.version);
  console.log('  cmd A audit_verified=', a.audit_verified);
  console.log('  cmd A payload       =', JSON.stringify(a.payload));

  const decisionA = a.payload && a.payload.data && a.payload.data.result
    ? a.payload.data.result.decision
    : undefined;
  check('finance cmd A decision === "blocked"', decisionA === 'blocked', 'decision=' + decisionA);
  check('finance cmd A audit_verified === true', a.audit_verified === true);

  // Command B: travel expense 1000 -> allowed (version bumps to 2, so rewind is meaningful)
  const b = JSON.parse(fin.execute_instruction(JSON.stringify({
    type: 'finance_expense_limit_check',
    params: { expense: { type: 'travel', amount: 1000 } },
  })));
  const decisionB = b.payload && b.payload.data && b.payload.data.result
    ? b.payload.data.result.decision
    : undefined;
  check('finance cmd B decision === "allowed"', decisionB === 'allowed', 'decision=' + decisionB + ' version=' + b.version);

  // Audit chain checks (run after both commands)
  const chain = JSON.parse(fin.get_audit_chain());
  check('finance get_audit_chain() non-empty array', Array.isArray(chain) && chain.length > 0,
    'entries=' + chain.length);
  check('finance verify_audit_chain() === true', fin.verify_audit_chain() === true);
  if (chain.length > 0) {
    const e = chain[0];
    check('audit entry has fact_id/fact_type/content_hash/prev_hash',
      'fact_id' in e && 'fact_type' in e && 'content_hash' in e && 'prev_hash' in e,
      'first=' + JSON.stringify({ fact_id: e.fact_id, fact_type: e.fact_type }));
  }

  // Time travel: rewind to version 1 -> should show the blocked state again.
  const back = JSON.parse(fin.rewind(1));
  console.log('  rewind(1) state     =', JSON.stringify(back));
  const decisionBack = back.payload && back.payload.data && back.payload.data.result
    ? back.payload.data.result.decision
    : undefined;
  check('finance rewind(1) version === 1', back.version === 1, 'version=' + back.version);
  check('finance rewind(1) restores blocked decision', decisionBack === 'blocked',
    'decision=' + decisionBack);

  // get_state() snapshot consistency
  const st = JSON.parse(fin.get_state());
  check('finance get_state().version === 1 after rewind', st.version === 1);
  fin.free();

  // ---------- Medical rules ----------
  console.log('\n--- [2] MEDICAL: special-tier antibiotic outpatient prescription ---');
  const medRules = JSON.parse(
    readFileSync(path.join(RULES_DIR, '21_medical_rules.json'), 'utf8')
  );
  const med = new EvoRuleEngine();
  med.load_rules(JSON.stringify(medRules.transform));
  const m = JSON.parse(med.execute_instruction(JSON.stringify({
    type: 'medical_antibiotic_tier_check',
    params: {
      prescription: { antibiotic_tier: 'special' },
      visit: { type: 'outpatient' },
    },
  })));
  console.log('  med result.type     =', m.type);
  console.log('  med version         =', m.version);
  console.log('  med payload         =', JSON.stringify(m.payload));
  const medDecision = m.payload && m.payload.data && m.payload.data.result
    ? m.payload.data.result.decision : undefined;
  check('medical decision === "blocked"', medDecision === 'blocked', 'decision=' + medDecision);
  check('medical verify_audit_chain() === true', med.verify_audit_chain() === true);
  const medChain = JSON.parse(med.get_audit_chain());
  check('medical audit chain non-empty', Array.isArray(medChain) && medChain.length > 0,
    'entries=' + medChain.length);
  med.free();

  // ---------- 等保 (DJBH) rules ----------
  console.log('\n--- [3] MLPS (DJBH): non-admin requesting admin permission ---');
  const djRules = JSON.parse(
    readFileSync(path.join(RULES_DIR, '22_djbh_rules.json'), 'utf8')
  );
  const dj = new EvoRuleEngine();
  dj.load_rules(JSON.stringify(djRules.transform));
  const d = JSON.parse(dj.execute_instruction(JSON.stringify({
    type: 'djbh_access_control_check',
    params: { requested_permission: 'admin', user_role: 'auditor' },
  })));
  console.log('  djbh result.type    =', d.type);
  console.log('  djbh version        =', d.version);
  console.log('  djbh payload        =', JSON.stringify(d.payload));
  const djDecision = d.payload && d.payload.data && d.payload.data.result
    ? d.payload.data.result.decision : undefined;
  check('djbh decision === "blocked"', djDecision === 'blocked', 'decision=' + djDecision);
  check('djbh verify_audit_chain() === true', dj.verify_audit_chain() === true);
  const djChain = JSON.parse(dj.get_audit_chain());
  check('djbh audit chain non-empty', Array.isArray(djChain) && djChain.length > 0,
    'entries=' + djChain.length);
  dj.free();

  console.log('\n=== ' + (failures === 0 ? 'ALL TESTS PASSED' : failures + ' TEST(S) FAILED') + ' ===');
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error('FATAL:', e);
  process.exit(2);
});
