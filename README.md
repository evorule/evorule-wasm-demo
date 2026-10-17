# evorule-wasm-demo

真实 EvoRule 内核（TCB 纯函数 + FactsLog + BLAKE3 Auditor）编译到
WebAssembly 的演示 crate——浏览器内全程运行，无服务器、无 tokio、无时钟。
在线版：`https://evorule.github.io/wasm-demo/`（部署仓
`evorule.github.io/wasm-demo/`，本仓构建产物经 `deploy.mjs` 同步过去）。

> 2026-10-17 自内核仓 `evorule/evorule` 的 `evorule-wasm-demo/` 子目录
> 移出为独立仓（`git subtree split` 抽取完整子目录历史，291 提交）。

## API 面

- `run_core` / `run_demo`（wasm32 导出）：单次规则求值 + 事实落链 + 审计
  链构建校验（无状态一次性）；
- `EvoRuleEngine`（wasm32 有状态类）：`load_rules` → `execute_instruction`
  → `io_required` 暂停 → `resolve_io`（注入 `__io_result__` 后从原始输入
  重放提交，D11 契约）→ `get_audit_chain` / `verify_audit_chain` /
  `rewind`（时间机器：工作视图回退，append-only 链不动）→ `reset`；
- `engine_version`：构建身份自证（crate 版本 + 依赖 pin 的内核 tag），
  在线 demo footer 展示。

## 构建

```bash
# wasm 产物（web target，供页面 import）
wasm-pack build --target web --out-dir pkg

# native 侧（同 run_core 逻辑对照 + 确定性验证）
cargo build --examples
cargo test

# 确定性再生合并规则集 + 跑引擎测试面（node 22）
node determinism_run.js
node test_engine.js
```

## 依赖与宪法副本纪律

内核三件（evorule-tcb / evorule-reactor / evorule-governance）经 **git
依赖锁定 tag**（见 Cargo.toml；gitee 主源，github 镜像待网络条件）。
`constitution/core_eval.json` 是该 tag 版宪法的内嵌副本，与 pin 同版
同步，漂移由 `constitution_matches_synced_hash` 测试 fail-visible。
升版流程见 `constitution/README.md`。

## 本地开发

依赖本机同级内核检出时，向 `Cargo.toml` 临时追加（**不提交**）：

```toml
[patch."https://gitee.com/evorule/evorule.git"]
evorule-tcb = { path = "../evorule/evorule-tcb" }
evorule-reactor = { path = "../evorule/evorule-reactor" }
evorule-governance = { path = "../evorule/evorule-governance" }
```

## 部署（在线 demo）

```bash
# 1. 本仓重建产物
wasm-pack build --target web --out-dir pkg
# 2. 部署仓（evorule.github.io/wasm-demo/）内：
node build-rules.js   # 再生规则数据
node deploy.mjs       # 从本仓同步 pkg/
# 3. commit + push，GitHub Pages 自动重部署
```
