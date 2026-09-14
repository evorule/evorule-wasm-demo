// Day5 WASM performance benchmark — timed from JS side (wasm32 has no
// std::time::Instant). Same parameters as examples/bench_native.rs.
const { bench_load_rules, bench_execute, bench_audit } = require('./pkg/evorule_wasm_demo.js');

function median(xs) {
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)];
}

function timeMs(fn) {
  const t0 = performance.now();
  const checksum = fn();
  return { ms: performance.now() - t0, checksum };
}

// Warmup (not measured).
bench_load_rules(10, 50);
bench_execute(200);
bench_audit(50);

// --- Dimension 1: rule loading ---
const loadReps = 500;
const loadNs = [10, 100, 1000];
const byN = {};
for (const n of loadNs) {
  const runsTotal = [];
  for (let r = 0; r < 3; r++) {
    const { ms, checksum } = timeMs(() => bench_load_rules(n, loadReps));
    runsTotal.push({ ms, checksum });
  }
  const perOp = runsTotal.map((x) => (x.ms * 1000) / loadReps); // us per load
  byN[n] = {
    runs_total_us: runsTotal.map((x) => x.ms * 1000),
    checksums: runsTotal.map((x) => x.checksum),
    per_op_us: perOp,
    per_op_us_median: median(perOp),
  };
}

// --- Dimension 2: instruction execution ---
const iters = 2000;
const execRuns = [];
for (let r = 0; r < 3; r++) {
  const { ms, checksum } = timeMs(() => bench_execute(iters));
  execRuns.push({ ms, checksum });
}
const execPerOp = execRuns.map((x) => (x.ms * 1000) / iters);

// --- Dimension 3: audit chain ---
const auditN = 200;
const auditRuns = [];
for (let r = 0; r < 3; r++) {
  const { ms, checksum } = timeMs(() => bench_audit(auditN));
  auditRuns.push({ ms, checksum });
}

const report = {
  binary: 'wasm32-unknown-unknown (node, performance.now)',
  load_rules: { n_values: loadNs, reps_per_measurement: loadReps, by_n: byN },
  execute: {
    iterations_per_run: iters,
    runs_total_us: execRuns.map((x) => x.ms * 1000),
    checksums: execRuns.map((x) => x.checksum),
    per_op_us: execPerOp,
    per_op_us_median: median(execPerOp),
  },
  audit: {
    facts_per_run: auditN,
    runs_us: auditRuns.map((x) => x.ms * 1000),
    checksums: auditRuns.map((x) => x.checksum),
    median_us: median(auditRuns.map((x) => x.ms * 1000)),
  },
};
console.log('WASM_BENCH: ' + JSON.stringify(report));
