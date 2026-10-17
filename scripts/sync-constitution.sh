#!/usr/bin/env bash
# 同步宪法内嵌副本到 Cargo.toml 锁定的内核 tag 版本，并输出新哈希。
# 用法：bash scripts/sync-constitution.sh [内核检出路径，默认 ../evorule]
set -euo pipefail
cd "$(dirname "$0")/.."

KERNEL_DIR="${1:-../evorule}"
TAG=$(grep -m1 '^evorule-tcb = ' Cargo.toml | sed 's/.*tag = "\([^"]*\)".*/\1/')
[ -n "$TAG" ] || { echo "ERROR: 未在 Cargo.toml 找到 evorule-tcb 的 tag pin"; exit 1; }

echo "pin tag: $TAG, kernel checkout: $KERNEL_DIR"
(cd "$KERNEL_DIR" && git show "$TAG:evorule-tcb/core_eval.json") > constitution/core_eval.json

HASH=$(python -c "import blake3;print('blake3:'+blake3.blake3(open('constitution/core_eval.json','rb').read()).hexdigest())")
echo "new hash: $HASH"
echo "下一步：更新 src/lib.rs constitution_matches_synced_hash 的 EXPECTED，与依赖 pin 同一提交落库。"
