// SPDX-License-Identifier: AGPL-3.0-or-later
// bench_node_sync.js 鈥?WASM 渚ф€ц兘鍩哄噯(Node, initSync 鍚屾鍒濆鍖?銆?// 鍙傛暟涓?examples/bench_native.rs 瀹屽叏涓€鑷?渚夸簬妯悜瀵规瘮銆?'use strict';
const { readFileSync } = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');

const PKG = path.join(__dirname, 'pkg');

function median(xs) {
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)];
}
function measureUs(fn) {
  const t0 = process.hrtime.bigint();
  const checksum = fn();
  const dt = Number(process.hrtime.bigint() - t0) / 1000; // microseconds
  return { us: dt, checksum };
}
function runThree(fn) {
  return [0, 1, 2].map(() => fn());
}

async function main() {
  const mod = await import(pathToFileURL(path.join(PKG, 'evorule_wasm_demo.js')).href);
  mod.initSync({ module: readFileSync(path.join(PKG, 'evorule_wasm_demo_bg.wasm')) });
  const { bench_load_rules, bench_execute, bench_audit } = mod;

  // warmup
  bench_load_rules(10, 50);
  bench_execute(200);
  bench_audit(50);

  // Dimension 1: load rules
  const loadReps = 500;
  const loadNs = [10, 100, 1000];
  const byN = {};
  for (const n of loadNs) {
    const runs = runThree(() => {
      const r = measureUs(() => bench_load_rules(n, loadReps));
      return { total_us: r.us, per_op_us: r.us / loadReps, checksum: r.checksum };
    });
    byN[n] = {
      runs_total_us: runs.map((r) => r.total_us),
      per_op_us: runs.map((r) => r.per_op_us),
      per_op_us_median: median(runs.map((r) => r.per_op_us)),
    };
  }

  // Dimension 2: execute
  const iters = 2000;
  const execRuns = runThree(() => {
    const r = measureUs(() => bench_execute(iters));
    return { total_us: r.us, per_op_us: r.us / iters, checksum: r.checksum };
  });

  // Dimension 3: audit
  const auditN = 200;
  const auditRuns = runThree(() => measureUs(() => bench_audit(auditN)).us);

  const report = {
    binary: 'wasm32 (node, initSync, process.hrtime)',
    load_rules: { n_values: loadNs, reps_per_measurement: loadReps, by_n: byN },
    execute: {
      iterations_per_run: iters,
      runs_total_us: execRuns.map((r) => r.total_us),
      per_op_us: execRuns.map((r) => r.per_op_us),
      per_op_us_median: median(execRuns.map((r) => r.per_op_us)),
    },
    audit: {
      facts_per_run: auditN,
      runs_us: auditRuns,
      median_us: median(auditRuns),
    },
  };
  console.log('WASM_BENCH: ' + JSON.stringify(report));
}
main().catch((e) => { console.error(e); process.exit(1); });

