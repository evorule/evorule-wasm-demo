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
  const { EvoRuleEngine, LlmAuditSession, verify_chain_json } = mod;

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

  // ---- LLM audit appendix: io-shaped flow + LlmAuditSession on the real
  // audit mechanism (IoResponse on chain, offline verifier, tamper cascade).
  const ioRules = JSON.stringify([
    { type: 'branch', params: {
        domain: { type: 'all', inner: [
          { type: 'instruction', instruction_type: 'call_llm' },
          { type: 'not', inner: { type: 'exists', path: '__exec__.payload.__io_result__' } } ] },
        on_true: [ { type: 'io_request', params: {
          io_type: 'call_external', service_name: 'echo_svc',
          args: '__exec__.instruction.params.args' } } ],
        on_false: [] } },
    { type: 'branch', params: {
        domain: { type: 'all', inner: [
          { type: 'instruction', instruction_type: 'call_llm' },
          { type: 'exists', path: '__exec__.payload.__io_result__' } ] },
        on_true: [ { type: 'set', params: {
          attr: 'data.last_reply', operation: 'set',
          value: '__exec__.payload.__io_result__.content' } } ],
        on_false: [] } },
  ]);
  const ie = new EvoRuleEngine();
  ie.load_rules(ioRules);
  const ir1 = JSON.parse(ie.execute_instruction(JSON.stringify({
    type: 'call_llm', params: { args: { messages: [{ role: 'user', content: 'hello' }] } } })));
  check('io flow pauses with io_required', ir1.type === 'io_required', 'type=' + ir1.type);
  ie.resolve_io(JSON.stringify({ content: 'MOCK-LLM-REPLY', finish_reason: 'stop',
    token_usage: { prompt_tokens: 12, completion_tokens: 5, total_tokens: 17 } }));
  const ieChain = JSON.parse(ie.get_audit_chain());
  const ieTypes = ieChain.map((c) => c.fact_type);
  check('io flow IoResponse fact on chain', ieTypes.includes('IoResponse'),
    'types=' + ieTypes.join(','));
  const ieVerifyHonest = JSON.parse(verify_chain_json(ie.export_chain()));
  check('io flow exported chain verifies honest', ieVerifyHonest.verified === true,
    'first_broken=' + ieVerifyHonest.first_broken_index);
  const ieExport = JSON.parse(ie.export_chain());
  ieExport[0].stable_json = ieExport[0].stable_json.replace('hello', 'HELLO-TAMPERED');
  const ieVerifyTampered = JSON.parse(verify_chain_json(JSON.stringify(ieExport)));
  check('io flow tamper detected', ieVerifyTampered.verified === false
    && ieVerifyTampered.first_broken_index === 0,
    'first_broken=' + ieVerifyTampered.first_broken_index);
  const ieTail = ieVerifyTampered.entries[ieVerifyTampered.entries.length - 1];
  check('io flow tamper cascades to tail', ieTail.link_ok === false,
    'tail.link_ok=' + ieTail.link_ok);
  ie.free();

  const MSGS = JSON.stringify([
    { role: 'system', content: 'You are a demo assistant.' },
    { role: 'user', content: '读取 foo.txt 并总结。' },
  ]);
  const sess = new LlmAuditSession('audit-probe-mock');
  sess.begin_call(MSGS);
  sess.respond(JSON.stringify({ content: '', finish_reason: 'tool_call',
    tool_calls: [{ tool_name: 'file_read', args: { path: 'foo.txt' } }],
    token_usage: { prompt_tokens: 12, completion_tokens: 5, total_tokens: 17 } }));
  const MSGS2 = MSGS.slice(0, -1)
    + ',{"role":"tool","tool_name":"file_read","content":"hello audit chain"}]';
  sess.begin_call(MSGS2);
  const sessFinal = JSON.parse(sess.respond(JSON.stringify({
    content: 'FILE-READ-DONE', finish_reason: 'stop',
    token_usage: { prompt_tokens: 40, completion_tokens: 7, total_tokens: 47 } })));
  check('llm session tokens aggregate across calls',
    sessFinal.session_tokens.prompt === 52 && sessFinal.session_tokens.completion === 12,
    JSON.stringify(sessFinal.session_tokens));
  check('llm session verified', sessFinal.verified === true);
  const sessChain = JSON.parse(sess.audit_chain());
  const sessTypes = sessChain.map((c) => c.fact_type);
  check('llm session IoResponse on chain', sessTypes.includes('IoResponse'),
    'types=' + sessTypes.join(','));
  const sessRespIdx = sessTypes.lastIndexOf('IoResponse');
  const sessExport = JSON.parse(sess.export_chain());
  sessExport[sessRespIdx].stable_json =
    sessExport[sessRespIdx].stable_json.replace('FILE-READ-DONE', 'TAMPERED');
  const sessVerify = JSON.parse(verify_chain_json(JSON.stringify(sessExport)));
  check('llm session tamper detected at the edited fact',
    sessVerify.verified === false && sessVerify.first_broken_index === sessRespIdx,
    'first_broken=' + sessVerify.first_broken_index + ' expected=' + sessRespIdx);
  const sessTail = sessVerify.entries[sessVerify.entries.length - 1];
  check('llm session tamper cascades to tail', sessTail.link_ok === false,
    'tail.link_ok=' + sessTail.link_ok);
  sess.free();

  console.log('\n=== ' + (failures === 0 ? 'ALL TESTS PASSED' : failures + ' TEST(S) FAILED') + ' ===');
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error('FATAL:', e);
  process.exit(2);
});
