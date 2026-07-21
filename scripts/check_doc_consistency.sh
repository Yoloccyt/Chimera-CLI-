#!/usr/bin/env bash
# 架构文档一致性巡检 —— 以 Cargo.toml 的 workspace.members 为唯一权威(canonical truth)。
# WHY(对应 Better Loop 复盘 F4): README/CODE_WIKI/.cargo 曾长期把 crate 数写成 34,
#   而实际已是 35(v2.0.0-omega 新增 chimera-mas)。发布清单只校验版本号/CHANGELOG,
#   缺少"文档 crate 数 vs 代码"的巡检,导致漂移反复复发。本脚本提供可返回 clean/gap 的巡检。
#
# 检查项:
#   (1) 结构不变量: Cargo.toml [workspace].members 中的 crates/* 数 == 磁盘 crates/*/Cargo.toml 数
#   (2) 索引文档新鲜度: README.md / CODE_WIKI.md / CLAUDE.md 均含当前 crate 计数
#       (定值匹配,规避 CJK 正则字节问题;缺失文件即 gap,故本项含 CODE_WIKI.md 存在性校验)
# 退出码: 0=clean, 1=gap
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

status=0

# (1) canonical truth: 提取 members 数组内以 "crates/ 开头的成员并计数
n_members=$(awk '/members = \[/{f=1} f; /\]/{if(f)exit}' Cargo.toml | grep -oE '"crates/[^"]+"' | wc -l | tr -d ' ')
n_dirs=$(find crates -mindepth 2 -maxdepth 2 -name Cargo.toml 2>/dev/null | wc -l | tr -d ' ')
echo "[doc-consistency] canonical crate 数 (Cargo.toml members) = ${n_members}"

if [ "$n_members" != "$n_dirs" ]; then
  echo "[GAP] Cargo.toml members(${n_members}) 与磁盘 crates/*/Cargo.toml(${n_dirs}) 不一致"
  status=1
fi

# (2) 索引文档必须包含当前 crate 计数(任一定值形式命中即可,grep -F 定值匹配避免 CJK 正则字节坑)
#     覆盖 docs/architecture 索引(README + 权威源 CODE_WIKI)与 CLAUDE.md 三份文档;
#     缺失文件走 "[GAP] 缺少文档" 分支,故本项同时承担 CODE_WIKI.md 的存在性校验。
# WHY 用"正向包含"而非"检测旧值":CODE_WIKI.md 的变更历史表含合法历史 crate 数
#     (如 v1.0.0-omega 的 "34 crate"),只需断言当前值(${n_members})存在,天然规避历史值误报。
for f in docs/architecture/README.md docs/architecture/CODE_WIKI.md .claude/CLAUDE.md; do
  if [ ! -f "$f" ]; then echo "[GAP] 缺少文档: $f"; status=1; continue; fi
  if   grep -qF "${n_members} crate"           "$f"; then :;
  elif grep -qF "${n_members}个crate"          "$f"; then :;
  elif grep -qF "${n_members} 个 crate"        "$f"; then :;
  elif grep -qF "${n_members} Crate"           "$f"; then :;
  elif grep -qF "${n_members}/${n_members} crate" "$f"; then :;
  else
    echo "[GAP] ${f} 未包含当前 crate 计数(${n_members}) —— 可能仍是旧值,请对齐 Cargo.toml"
    status=1
  fi
done

if [ "$status" = 0 ]; then echo "[OK] 文档 crate 计数与 Cargo.toml(${n_members}) 一致"; fi
exit $status
