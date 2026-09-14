// Node.js driver for the evorule WASM prototype.
// Loads the wasm bindings, runs one rule-evaluation round, prints the JSON.
const { run_demo } = require('./pkg/evorule_wasm_demo.js');

// Empty rules_json => use the embedded constitution (identical bytes as native).
const rules_json = "";
// Same command as the native program: increment payload.x by 1.
const command_json = JSON.stringify({
  type: "increment",
  params: { attr: "x", delta: 1 },
});

const t0 = Date.now();
const result = run_demo(rules_json, command_json);
const ms = Date.now() - t0;

console.log("WASM RESULT:", result);
console.log("WASM ELAPSED_MS:", ms);
