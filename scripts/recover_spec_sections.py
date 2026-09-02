#!/usr/bin/env python3
"""从本任务 transcript 取回"原计划 §5 红线合规自检 / §7 数字来源台账 / §0 冲突裁决"的原文。

动因：当前 Spec 文件（93 行）第 5 行声明这些节"继续有效"，但正文已在 rewrite 时被覆盖，
不在盘上。继续凭记忆引用就会重犯 RK-P40（把记忆当事实）。transcript 是权威记录，直接取回。
"""
import io
import json
import re
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

# 会话记录路径必须显式给出（argv 或 CHIMERA_TRANSCRIPT）：不该把某台机器的路径当仓库事实入库。
import os as _os
TR = _os.environ.get("CHIMERA_TRANSCRIPT") or (sys.argv[1] if len(sys.argv) > 1 else "")
if not TR or not _os.path.exists(TR):
    print("用法: recover_spec_sections.py <transcript.jsonl>（或设 CHIMERA_TRANSCRIPT）")
    print("未得到可读 transcript 路径 -> 不可判定（不拿记忆补正文）")
    sys.exit(2)

KEYS = ["红线合规自检", "数字来源台账", "冲突裁决"]


def walk(o):
    if isinstance(o, dict):
        for v in o.values():
            yield from walk(v)
    elif isinstance(o, list):
        for v in o:
            yield from walk(v)
    elif isinstance(o, str):
        yield o


seen = set()
for line in io.open(TR, encoding="utf-8", errors="replace"):
    try:
        obj = json.loads(line)
    except Exception:
        continue
    for s in walk(obj):
        if "## 5" in s and "红线" in s:
            # 抓到含 §5 的整段（计划正文），截取 §5 到 §6/§7 之前
            i = s.find("## 5")
            j = s.find("## 6", i)
            seg = s[i:j if j > i else i + 2600]
            key = seg[:80]
            if key in seen:
                continue
            seen.add(key)
            print("========== 原计划 §5 原文（取回） ==========")
            print(seg.replace("\\n", "\n")[:3000])
            print("========== END §5 ==========\n")
if not seen:
    print("transcript 中未找到 §5 正文（可能被压缩掉）")
    for k in KEYS:
        n = 0
        for line in io.open(TR, encoding="utf-8", errors="replace"):
            if k in line:
                n += 1
        print(f"  关键词 {k!r} 在 transcript 中出现于 {n} 行")
