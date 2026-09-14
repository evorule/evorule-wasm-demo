// Node.js driver for the evorule WASM prototype.
// Loads the wasm bindings, runs one rule-evaluation round, prints the JSON.
//
// Run:  node test.js
//
// NOTE (2026-09-14 复核修正): pkg/ 由 `wasm-bindgen --target web` 产出，是 ES module，
// 必须先初始化 wasm 实例才能调用导出函数。此前直接 require() 后调 run_demo()，
// 内部 `wasm` 绑定仍为 undefined，抛
//   TypeError: Cannot read properties of undefined (reading '__wbindgen_free')
// 现改为从 .wasm 字节同步初始化（无需浏览器 fetch），与 test_engine.js 同一做法。
'use strict';

const { readFileSync } = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');

const PKG_DIR = path.join(__dirname, 'pkg');

async function main() {
  const mod = await import(pathToFileURL(path.join(PKG_DIR, 'evorule_wasm_demo.js')).href);
  const wasmBytes = readFileSync(path.join(PKG_DIR, 'evorule_wasm_demo_bg.wasm'));
  mod.initSync({ module: wasmBytes });
  const { run_demo } = mod;

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
}

main();
