#!/usr/bin/env python3
"""校验 phaseR-t2-commit-batches.md 点名的每个路径确实存在且被跟踪状态与文述一致。

动因（RK-P40）：文档里的路径若凭记忆写，就会出现"清单指向不存在的文件"，
放行后 `git add` 直接报错或静默漏项。本校验器把清单变成可证伪对象。
"""
import io
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

DOC = "docs/reports/phaseR-t2-commit-batches.md"
text = io.open(DOC, encoding="utf-8", errors="replace").read()

# 从清单里抽路径：只取看起来是仓库相对路径的 token（含 / 且带扩展名）
# 注意扩展名列表必须包含 md：上一版漏了 md，导致 [C] 恒为 0 —— 与已实测的
# `git check-ignore` 证据（CHANGELOG.md 等均被 ignore）直接矛盾。
# 一个**看不见某类对象的校验器**比没有校验器更危险，因为它会递给你一个假"干净"。
TOK = re.compile(r"[A-Za-z0-9_.\-/]+\.(?:py|sh|ps1|toml|txt|yml|rs|lock|md)")
cands = set()
for m in TOK.finditer(text):
    p = m.group(0)
    if p.startswith(("scripts/", ".github/", ".config/", "crates/", "docs/")) or p in (
            "Cargo.lock", "Cargo.toml", "Dockerfile", "CHANGELOG.md"):
        cands.add(p)

# 通配 / 前缀型条目（无法直接 stat）单独列出，人工确认
wild = sorted(p for p in cands if "*" in p or p.endswith("/"))
plain = sorted(p for p in cands if p not in wild)


def tracked(p):
    return subprocess.run(["git", "ls-files", "--error-unmatch", p], capture_output=True).returncode == 0


def ignored(p):
    return subprocess.run(["git", "check-ignore", "-q", "--stdin"],
                          input=p.encode(), capture_output=True).returncode == 0


print(f"清单点名路径 {len(cands)} 个（通配/目录型 {len(wild)} 个另计）")
missing, untracked_committable, md_ignored = [], [], []
for p in plain:
    exists = os.path.exists(p)
    if not exists:
        # 相对 docs/reports 写的裸文件名（如 compressor.txt）要按仓库根判定，先跳过歧义项
        missing.append(p)
        continue
    if p.endswith(".md"):
        if ignored(p):
            md_ignored.append(p)
    elif not tracked(p):
        untracked_committable.append(p)

print(f"\n[A] 清单点名但盘上不存在（{len(missing)}）：")
for p in missing:
    print(f"    {p}")
print(f"\n[B] 存在、可提交、当前未跟踪（= T2 应保护的对象，{len(untracked_committable)}）：")
for p in untracked_committable:
    print(f"    {p}")
print(f"\n[C] 存在但被 gitignore 挡住的 .md（commit 保护不了，{len(md_ignored)}）：")
for p in md_ignored:
    print(f"    {p}")
print(f"\n[D] 通配/目录型条目（需人工核对，{len(wild)}）：")
for p in wild:
    print(f"    {p}")

# 名单脚本是否仍在
for helper in ("tmp/classify_fmt_vs_semantic.py", "tmp/batch_fmt_only.txt",
               "tmp/batch_semantic_rs.txt", "tmp/t2_coupling_audit.py"):
    print(f"辅助产物 {'在' if os.path.exists(helper) else '缺失'}：{helper}")

print("\n判定：")
print(f"  [A] 有 {len(missing)} 项需核实（若为简写/相对名则属正常，需逐条确认）")
print(f"  [B] T2 可保护 {len(untracked_committable)} 项")
print(f"  [C] commit 无法保护 {len(md_ignored)} 项 .md")
# 自检：清单里点了 .md 的名却没进 [C]，说明分类本身漏了（防假干净）
md_named = [p for p in plain if p.endswith(".md")]
if md_named and not md_ignored:
    print("  !! 自检失败：清单点名了 .md 但无一被判定为 ignore，请核对正则/判定逻辑")
