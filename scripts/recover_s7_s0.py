#!/usr/bin/env python3
"""取回原计划 §7（数字来源台账）与 §0（冲突裁决）正文。

当前 Spec 文件 L5 声明这两节"继续有效"，但正文在 rewrite 时被覆盖、不在盘上。
§5 的复扫已证明这类"看起来只是引用"的节里藏着会被改动打破的事实断言，
所以 §7 也必须取回原文逐条核，而不是凭记忆引用（RK-P40）。
"""
import io
import json
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

# 会话记录路径必须显式给出（argv 或 CHIMERA_TRANSCRIPT）：本工具跟本会话绑定，
# 不该把某台机器的路径当作仓库事实提交入库。缺输入时退 2 判不可判定，不猜路径。
import os as _os
TR = _os.environ.get("CHIMERA_TRANSCRIPT") or (sys.argv[1] if len(sys.argv) > 1 else "")
if not TR or not _os.path.exists(TR):
    print("用法: recover_s7_s0.py <transcript.jsonl>（或设 CHIMERA_TRANSCRIPT）")
    print("未得到可读的 transcript 路径 -> 不可判定（不视为“没找到正文就算通过”）")
    sys.exit(2)


def walk(o):
    if isinstance(o, dict):
        for v in o.values():
            yield from walk(v)
    elif isinstance(o, list):
        for v in o:
            yield from walk(v)
    elif isinstance(o, str):
        yield o


found = set()
for line in io.open(TR, encoding="utf-8", errors="replace"):
    try:
        obj = json.loads(line)
    except Exception:
        continue
    for s in walk(obj):
        for head in ("## 7", "## 0"):
            if head in s and ("数字来源" in s if head == "## 7" else "冲突裁决" in s):
                i = s.find(head)
                nxt = s.find("\n## ", i + 5)
                seg = s[i:nxt if nxt > i else i + 3200]
                key = seg[:70]
                if key in found:
                    continue
                found.add(key)
                print(f"########## 取回 {head} ##########")
                print(seg.replace("\\n", "\n")[:3400])
                print(f"########## END {head} ##########\n")
if not found:
    print("未取回（可能已在压缩中丢失）；此时应改用 Spec 现存正文可判定项，不凭记忆补")
