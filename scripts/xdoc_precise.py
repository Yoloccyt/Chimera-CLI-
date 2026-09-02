#!/usr/bin/env python3
"""精准跨文档一致性检查：只查本波次真正改动过、且可能留下"现状陈述过期"的四类断言。

上一版用宽正则扫全部数字，产出 136 行命中（多是"8 项决策""7 类密钥扫描"这类无关文本）——
**噪声大的检查等于没有检查**，没人会读它的红。这里改成"一条断言 = 一个权威源 + 一个期望值"。

判定口径：命中且**不在历史语境**（含"时点/演进/上一登记/v2.x.y-omega("等标注）即报 GAP。
"""
import io
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

DOCS = ["agents.md", ".claude/CLAUDE.md", "CHANGELOG.md",
        "docs/reports/phaseR-wave1-closure.md",
        "docs/reports/phaseR-release-checklist-v2.28.0.md",
        "docs/governance/RK-P_risk_register.md",
        "docs/architecture/adr_index.md",
        "docs/architecture/ADR-166-doc-consistency-gate-execution-plane.md",
        "docs/architecture/ADR-167-audit-gate-new-advisories.md",
        "docs/architecture/ADR-168-closure-writeback-and-doc-numeric-ssot.md"]

# 权威值当场读，不写死
ps1 = io.open("scripts/check_doc_consistency.ps1", encoding="utf-8", errors="replace").read()
r = subprocess.run(["pwsh", "-NoProfile", "-File", "scripts/check_doc_consistency.ps1"],
                   capture_output=True, text=True, encoding="utf-8", errors="replace")
out = (r.stdout or "") + (r.stderr or "")
m = re.search(r"\((\d+) categories / (\d+) check ids, self-reported\)", out)
if not m:
    print("!! 门禁未自报检查数，无法建立权威值"); print(out[-400:]); sys.exit(2)
PS_CAT, PS_ID = int(m.group(1)), int(m.group(2))

rs = subprocess.run(["bash", "scripts/check_doc_consistency.sh"],
                    capture_output=True, text=True, encoding="utf-8", errors="replace")
m2 = re.search(r"\((\d+) categories / (\d+) check ids, self-reported\)", (rs.stdout or "") + (rs.stderr or ""))
SH_ID = int(m2.group(2)) if m2 else -1

idx = io.open("docs/architecture/adr_index.md", encoding="utf-8", errors="replace").read()
m3 = re.search(r"ADR 声明总数 = (\d+)", idx)
ADR_DECL = int(m3.group(1)) if m3 else -1

print(f"权威值: .ps1 自报 {PS_CAT} 类/{PS_ID} id | .sh 自报 {SH_ID} id | adr_index 声明总数 {ADR_DECL}")

# 历史/测量记录的豁免标注：带这些词的数字化陈述属“当时实测”，按 R7 保留（只不得当现状用）。
# 不纳入豁免会让本工具产出大量“其实没问题”的红 —— 噪声大的检查等于没有检查。
HIST = re.compile(r"时点|演进|上一登记|原为|历史|改前|改后|曾|当前实测|本批实测|实测：|实测 "
                  r"|硬编码|无从核对|不输出|旧声明|→|v2\.\d+\.\d+-omega\(")
# 引用 Spec/门禁"要求值"的行不是对现状的断言（如 `passed ≥ 11,522` 是 Spec 原文阈值），
# 把它们当错去改就会把 Spec 抄错，故一并豁免。
QUOTED_REQ = re.compile(r"Spec|原文|≥|\bpassed ≥|验收|台账|来源标注|已过期|取代|现值|旧声明")

CHECKS = [
    ("ps1 检查数被写死", re.compile(r"(?:6|六)\s*[类组]\s*1[0-9]\s*项|6 categories / \d+ checks"),
     lambda s: f"该处写死了 .ps1 检查数（现为自报 {PS_ID}），应改为引用自报值或注明时点"),
    ("sh 检查数被写死", re.compile(r"5\s*类\s*1[0-9]\s*项|5 categories / \d+ checks"),
     lambda s: f"该处写死了 .sh 检查数（现自报 {SH_ID} id）"),
    ("ADR 声明数过期", re.compile(r"(?:声明总数|总数(?:截至)?|编号空间至)\D{0,10}(1[0-9]{2})"),
     lambda g: None if int(g) == ADR_DECL else f"声明 {g} 与 adr_index 现值 {ADR_DECL} 不符"),
    ("已被取代的测试数当作现状", re.compile(r"11[,_]5(?:56|22)"),
     None),
]

bad = []
for doc in DOCS:
    if not os.path.exists(doc):
        continue
    for n, l in enumerate(io.open(doc, encoding="utf-8", errors="replace").read().splitlines(), 1):
        for label, pat, verdict in CHECKS:
            for mm in pat.finditer(l):
                ctx = l.strip()
                if HIST.search(ctx) or (label == "已被取代的测试数当作现状" and QUOTED_REQ.search(ctx)):
                    continue  # 明示时点/演进 或 引用要求原文 的，不算现状断言
                if label == "ADR 声明数过期":
                    msg = verdict(mm.group(1))
                    if not msg:
                        continue
                elif callable(verdict):
                    msg = verdict(ctx)
                else:
                    msg = "疑似把已取代的测试数当现状陈述（无时点标注）"
                bad.append(f"{doc}:L{n} [{label}] {msg}\n      {ctx[:130]}")

print()
if bad:
    print(f"[GAP] 需修 {len(bad)} 处：")
    for b in bad:
        print("  ", b)
    sys.exit(1)
print("[OK] 四类断言无“现状陈述与权威值不符”（历史标注行按 R7 豁免）")
