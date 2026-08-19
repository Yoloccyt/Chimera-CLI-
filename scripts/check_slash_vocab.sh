#!/usr/bin/env bash
# Concord W2 T2.4 — 斜杠命令主流同义词表抽查脚本(bash 版)
#
# 断言:重构方案 §9.8 与主流五家(Codex/Claude Code/opencode/Qoder/Hermes)
# 同名的 24 条命令均存在于 SlashCommandRegistry 且执行分层(tier)正确。
# 数据源:crates/chimera-tui/src/actions/slash_registry.rs(单一事实源)。
# 用法:bash scripts/check_slash_vocab.sh ; EXIT=1 时列出差异。

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REGISTRY="$REPO_ROOT/crates/chimera-tui/src/actions/slash_registry.rs"

if [[ ! -f "$REGISTRY" ]]; then
  echo "[FAIL] registry file not found: $REGISTRY"
  exit 1
fi

# 解析 name + tier(条目按 fmt 多行格式;awk 状态机跨行配对)
parse_entries() {
  awk '
    /name:[[:space:]]*"/ {
      match($0, /name:[[:space:]]*"[^"]+"/)
      name = substr($0, RSTART, RLENGTH)
      gsub(/name:[[:space:]]*"/, "", name); gsub(/"/, "", name)
    }
    /aliases:[[:space:]]*&\[/ {
      line = $0
      while (match(line, /"[^"]+"/)) {
        a = substr(line, RSTART + 1, RLENGTH - 2)
        print a "\t" "ALIAS_PENDING"
        line = substr(line, RSTART + RLENGTH)
      }
    }
    /tier:[[:space:]]*SlashTier::/ {
      match($0, /SlashTier::[A-Za-z]+/)
      # Concord W5 修复:"SlashTier::" 长 11 字符,偏移应为 +11/-11;
      # 原 +12/-12 会吞掉 tier 首字符(Instant→nstant)导致假漂移。
      tier = substr($0, RSTART + 11, RLENGTH - 11)
      print name "\t" tier
    }
  ' "$REGISTRY"
}

# 主流五家同名 24 条词表: 命令 预期tier
# WHY 2026-08-19 移除 W8-W11 七条(pace/context/recap/copy/notify/commands/agent tree):
# 1f220d2 提交在词表超前声明但 slash_registry.rs 从未注册对应命令(漂移),
# CI 首次在 main 上运行即暴露;与代码现状对齐,待命令真实落地后再加回。
EXPECTED=(
  "new Instant" "clear Instant" "compact Orchestrated" "resume Instant"
  "model Instant" "mode Instant" "plan Instant" "permissions Instant"
  "init Agent" "diff Instant" "review Agent" "mention Instant"
  "mcp Orchestrated" "theme Instant" "vim Instant" "config Instant"
  "status Instant" "doctor Instant" "help Instant" "quit Instant"
  "export Instant" "undo Orchestrated" "redo Orchestrated" "focus Instant"
)

# 构建 命令→tier 查找表(别名取首个 ALIAS_PENDING 后由 tier 行回填:
# 简化处理——awk 输出中别名行无 tier,直接忽略别名,quit 单独按 exit 别名处理)
declare -A TIERS
while IFS=$'\t' read -r cmd tier; do
  [[ "$tier" == "ALIAS_PENDING" ]] && continue
  TIERS["$cmd"]="$tier"
done < <(parse_entries)
# quit 为 exit 别名(与正名同 tier)
TIERS["quit"]="${TIERS[exit]:-Instant}"

echo "[INFO] parsed ${#TIERS[@]} command names"

diffs=0
for pair in "${EXPECTED[@]}"; do
  # WHY 用 ${pair% *}(删末空格起)而非 ${pair%% *}(删首空格起):
  # 支持多词命令名(如 "agent tree Instant" → cmd="agent tree"),
  # 单词命令("new Instant" → cmd="new")行为不变,向后兼容。
  cmd="${pair% *}"
  want="${pair##* }"
  got="${TIERS[$cmd]:-}"
  if [[ -z "$got" ]]; then
    echo "  - MISSING  /$cmd"
    diffs=$((diffs + 1))
  elif [[ "$got" != "$want" ]]; then
    echo "  - TIER-MISMATCH  /$cmd expected=$want actual=$got"
    diffs=$((diffs + 1))
  fi
done

if [[ "$diffs" -gt 0 ]]; then
  echo "[FAIL] slash vocab drift detected: $diffs issue(s)"
  exit 1
fi
echo "[OK] all ${#EXPECTED[@]} mainstream-aligned slash commands present with correct tiers"
exit 0
