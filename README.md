# evorule-wasm-demo

The real evorule kernel (TCB pure functions + FactsLog + BLAKE3 auditor) compiled to WebAssembly: deterministic rule execution running entirely in the browser — no server, no tokio, no clock. Split out of the kernel repo's `evorule-wasm-demo/` subdirectory as a standalone repo on 2026-10-17 (full subdirectory history via `git subtree split`).

[![CI][badge-ci]][ci-link]
[![License: AGPL-3.0-or-later][badge-license]][license-link]

**License**: [AGPL-3.0-or-later](LICENSE) with [dual licensing](DUAL_LICENSE.md)

## Prerequisites

Rust with the wasm32 target (wasm-pack), Node 22 for the determinism and engine test scripts.

## Build

```bash
# wasm artifacts (web target, imported by the page)
wasm-pack build --target web --out-dir pkg

# native side: the same run_core logic as a cross-check + determinism verification
cargo build --examples
cargo test

# deterministic regeneration of the merged rule set + the engine test surface
node determinism_run.js
node test_engine.js
```

## Run

Online demo: **https://evorule.github.io/wasm-demo/**

```bash
wasm-pack build --target web --out-dir pkg    # 1. rebuild artifacts in this repo
node build-rules.js && node deploy.mjs        # 2. in the Pages repo: regenerate rules, sync pkg/
git commit && git push                        # 3. GitHub Pages redeploys
```

The wasm surface is three things: stateless `run_core` / `run_demo` exports (one rule evaluation + fact-chain append + audit-chain verification per call), a stateful `EvoRuleEngine` class (`load_rules` → `execute_instruction` → pause on `io_required` → `resolve_io` → `get_audit_chain` / `verify_audit_chain` / `rewind` / `reset`), and `engine_version` (crate version + pinned kernel tag) shown in the demo footer.

Kernel crates — evorule-tcb / evorule-reactor / evorule-governance — are git dependencies pinned by tag. `constitution/core_eval.json` is an embedded copy of that tag's constitution, kept in lockstep with the pin: drift fails visibly via the `constitution_matches_synced_hash` test (upgrade flow: `constitution/README.md`).

## License

**AGPL-3.0-or-later** (unified across the ecosystem): [LICENSE](LICENSE), with closed-source use under [DUAL_LICENSE.md](DUAL_LICENSE.md), [FREE_COMMERCIAL_LICENSE.md](FREE_COMMERCIAL_LICENSE.md) (free waiver for eligible entities), or [COMMERCIAL_LICENSE.md](COMMERCIAL_LICENSE.md). Third-party notices: [NOTICE.md](NOTICE.md).

[badge-ci]: https://github.com/evorule/evorule-wasm-demo/actions/workflows/ci.yml/badge.svg
[ci-link]: https://github.com/evorule/evorule-wasm-demo/actions/workflows/ci.yml
[badge-license]: https://img.shields.io/badge/license-AGPL--3.0--or--later-blue?style=flat-square
[license-link]: LICENSE
