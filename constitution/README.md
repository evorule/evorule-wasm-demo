# constitution/

内核宪法的内嵌副本（`evorule-tcb/core_eval.json` @ 本仓 Cargo.toml 锁定的
git tag）。`src/lib.rs` 经 `include_str!` 编译进 native 与 wasm32 两个产物，
运行期零文件系统/零网络——这是「双目标逐字节一致」确定性的载体。

纪律：本副本必须与依赖 pin 的 tag 严格一致。升 tag 时执行：

```bash
# 在同级内核检出（或任一内核仓克隆）中：
git show <新tag>:evorule-tcb/core_eval.json > constitution/core_eval.json
python -c "import blake3;print('blake3:'+blake3.blake3(open('constitution/core_eval.json','rb').read()).hexdigest())"
# 用输出更新 src/lib.rs 测试中的 EXPECTED 哈希，随依赖 pin 同一提交落库
```

`scripts/sync-constitution.sh` 一键完成上述两步（读 Cargo.toml 的 pin
tag，取同级 `../evorule` 检出）。漂移会在
`constitution_matches_synced_hash` 测试处 fail-visible，不允许静默。
