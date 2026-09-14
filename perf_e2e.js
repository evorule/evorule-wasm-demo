// perf_e2e.js — 真实 16 规则合并规则集下,WasmBackend 关键操作端到端耗时(Node)。
'use strict';
const { readFileSync } = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const PKG = path.join(__dirname, 'pkg');
const RULES_SRC = 'D:\\evorule-console-cloud\\static\\rules';

function median(xs){const s=[...xs].sort((a,b)=>a-b);return s[Math.floor(s.length/2)];}
function measureMs(fn){const t0=process.hrtime.bigint();fn();const dt=Number(process.hrtime.bigint()-t0)/1e6;return dt;}

async function main(){
  const mod = await import(pathToFileURL(path.join(PKG,'evorule_wasm_demo.js')).href);
  mod.initSync({ module: readFileSync(path.join(PKG,'evorule_wasm_demo_bg.wasm')) });
  const { EvoRuleEngine } = mod;

  const files=['core_eval.json','20_finance_rules.json','21_medical_rules.json','22_djbh_rules.json','10_role13_demo.json'];
  const merged=[];
  for(const f of files){const o=JSON.parse(readFileSync(path.join(RULES_SRC,f),'utf8'));merged.push(...(Array.isArray(o)?o:o.transform));}
  const rulesJson=JSON.stringify(merged);
  console.log('merged rules count =', merged.length);

  const loadSamples=[];
  for(let i=0;i<20;i++){const e=new EvoRuleEngine();const t=measureMs(()=>e.load_rules(rulesJson));loadSamples.push(t);e.free();}

  const sessionSamples=[];
  for(let i=0;i<10;i++){const t=measureMs(()=>{const e=new EvoRuleEngine();e.load_rules(rulesJson);return e;});sessionSamples.push(t);}

  const travel={type:'finance_expense_limit_check',params:{expense:{type:'travel',amount:2000}}};
  const e=new EvoRuleEngine(); e.load_rules(rulesJson);
  const execSamples=[];
  for(let i=0;i<50;i++){execSamples.push(measureMs(()=>e.execute_instruction(JSON.stringify(travel))));}

  for(let i=0;i<9;i++) e.execute_instruction(JSON.stringify(travel));
  const auditSamples=[], verifySamples=[];
  for(let i=0;i<20;i++){auditSamples.push(measureMs(()=>e.get_audit_chain()));verifySamples.push(measureMs(()=>e.verify_audit_chain()));}
  const chainLen=JSON.parse(e.get_audit_chain()).length;
  e.free();

  const report={
    load_rules_ms_median: median(loadSamples),
    createSession_load_rules_ms_median: median(sessionSamples),
    submitCommand_ms_median: median(execSamples),
    getAudit_ms_median: median(auditSamples),
    verifyAudit_ms_median: median(verifySamples),
    chain_len_after_10_cmds: chainLen,
  };
  console.log('E2E_PERF: '+JSON.stringify(report));
}
main().catch(e=>{console.error(e);process.exit(1);});
