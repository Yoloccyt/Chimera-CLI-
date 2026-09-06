#!/usr/bin/env python3
"""按 Spec 字面验收条目对当前工作区做完成度审计（证据导向）。

Spec 文件：`Phase_R_收口与治理偿还_task-472.md`（93 行）。本脚本只做**可机械判定**的
条目，输出每一项的"证据 + 判定"，判定值限定为 PROVEN / NOT PROVEN / MISSING。
"""
import io
import os
import re
import sys

ENC = "utf-8"


def read(p):
    if not os.path.exists(p):
        return None
    return io.open(p, encoding=ENC, errors="replace").read()


def crlf_and_bom(p):
    """返回 (CRLF数, 是否BOM, 非ASCII字节数)；**文件不存在时返回 None**。

    不能直接 open：缺文件必须被判为 MISSING（并进退出码），而不是把整个审计抛崩。
    本函数初版就是这么崩的（隔离目录负对照才抓出来）。
    """
    if not os.path.exists(p):
        return None
    raw = open(p, "rb").read()
    return raw.count(b"\r\n"), raw[:3] == b"\xef\xbb\xbf", sum(1 for b in raw if b > 127)


RESULTS = []


def verdict(item, state, evidence):
    RESULTS.append((item, state, evidence))


# --- P0-A ---
sh_meas = crlf_and_bom("scripts/check_perf_redlines.sh")
if sh_meas is None:
    verdict("P0-A .sh LF/无BOM/纯ASCII", "MISSING", "scripts/check_perf_redlines.sh 不在盘上")
else:
    c, bom, nonascii = sh_meas
    ok = c == 0 and not bom and nonascii == 0
    verdict("P0-A .sh LF/无BOM/纯ASCII", "PROVEN" if ok else "NOT PROVEN",
            f"CRLF={c} BOM={bom} nonAscii={nonascii}")
ps1_meas = crlf_and_bom("scripts/check_perf_redlines.ps1")
if ps1_meas is None:
    verdict("P0-A .ps1 去BOM/纯ASCII", "MISSING", "scripts/check_perf_redlines.ps1 不在盘上")
else:
    c, bom, nonascii = ps1_meas
    verdict("P0-A .ps1 去BOM/纯ASCII", "PROVEN" if (not bom and nonascii == 0) else "NOT PROVEN",
            f"BOM={bom} nonAscii={nonascii}")

# --- T3 交付物 2：第四轮执行报告 ---
p = "docs/reports/audit-redundancy/第四轮执行报告.md"
verdict("T3-② 第四轮执行报告.md", "PROVEN" if os.path.exists(p) else "MISSING", p)

# --- T3 交付物 1：CHANGELOG 五项补登 ---
ch = read("CHANGELOG.md") or ""
keys = {
    "cosine 下沉": ["cosine", "下沉"],
    "CLV::basis": ["CLV::basis", "basis"],
    "hcw_summary 错误路径": ["hcw_summary"],
    "util.rs clippy 修复": ["manual_range_contains", "clippy"],
    "fmt 归一 WS-H": ["WS-H", "fmt"],
}
for label, pats in keys.items():
    hit = any(pt in ch for pt in pats)
    verdict(f"T3-① CHANGELOG 补登「{label}」", "PROVEN" if hit else "MISSING",
            "命中关键词" if hit else "未命中")

# --- T3 交付物 3/4：agents.md 订正 ---
ag = read("agents.md") or ""
verdict("T3-③ agents.md 含 --jobs 限流", "PROVEN" if "--jobs" in ag else "MISSING",
        f"--jobs x{ag.count('--jobs')}")
verdict("T3-③ agents.md 含 fmt 逐包跑法", "PROVEN" if ("逐包" in ag or "per-package" in ag) else "MISSING",
        f"逐包 x{ag.count('逐包')}")
verdict("T3-④ agents.md 含 v2.27.1 无 tag 订正", "PROVEN" if "未打 tag" in ag or "从未打" in ag else "MISSING",
        "措辞核对")
cl = read(".claude/CLAUDE.md") or ""
verdict("T3-④ CLAUDE.md 锚点改指", "PROVEN" if "AGENTS.md" in cl and "7.2" in cl else "MISSING",
        "§7.2 引用检查")
dk = read("Dockerfile") or ""
# 实际写法是 `ARG VERSION=2.28.0-omega`（无空格）；要求 \s+ 会把已改好的值误报为缺失。
m = re.search(r"ARG\s+VERSION\s*=?\s*(\S+)", dk)
verdict("T3-④ Dockerfile ARG VERSION=2.28.0", "PROVEN" if m and m.group(1).startswith("2.28.0") else "MISSING",
        m.group(0) if m else "未见 ARG VERSION")

# --- RK-P31..34 是否入册 ---
rk = read("docs/governance/RK-P_risk_register.md") or ""
for n in ("RK-P31", "RK-P32", "RK-P33", "RK-P34"):
    verdict(f"§4 风险 {n} 入册", "PROVEN" if n in rk else "MISSING", f"x{rk.count(n)}")

# --- T5/T6 CI 接线 ---
# 注意：ci.yml 的注释里就写着“continue-on-error”这个词（解释为何不写），
# 所以**不能**用“首次出现位置 + N 字符窗口”扫，必须限定到该 step 自己的块。
ci = read(".github/workflows/ci.yml") or ""
step = re.search(r"- name: Perf redline inventory & lint gate\n(.*?)(?=\n\n|\n      - name:)", ci, re.S)
if step:
    body = step.group(1)
    ok = ("check_perf_redlines.sh --selftest" in body
          and "check_perf_redlines.sh --static-only" in body
          and "continue-on-error" not in body)
    verdict("T5 ci.yml 阻塞步（selftest+static-only，无 continue-on-error）",
            "PROVEN" if ok else "NOT PROVEN", f"step 体 {len(body)} 字符")
else:
    verdict("T5 ci.yml 阻塞步", "MISSING", "未找到该 name 的 step 块")
bc = read(".github/workflows/bench_check.yml") or ""
verdict("T6 bench_check 双跑 compare 步",
        "PROVEN" if ("check_perf_redlines.py compare" in bc and "bench_threshold_legacy.log" in bc) else "MISSING",
        "compare + tee legacy 日志")
# Spec 终态是“heredoc 换调 py 后删除”，但验收条件是**同一次 CI 内**等价性取证；
# 本地已证（36 键逐项相等），CI 未跑 → heredoc 故意保留。这里报 DEFERRED 而非 MISSING。
heredoc_alive = "python3 <<'PYEOF'" in bc
py_thresholds_called = "check_perf_redlines.py thresholds" in bc
verdict("T6 heredoc 退役（换调 py thresholds）",
        "PROVEN" if py_thresholds_called and not heredoc_alive else "NOT PROVEN",
        f"heredoc存在={heredoc_alive} py_thresholds已接={py_thresholds_called}"
        "— 待 CI 内双跑取证后删")

# --- T10 SLO 通道 ---
nt = read(".config/nextest.toml") or ""
verdict("T10 profile.slo 存在", "PROVEN" if "[profile.slo]" in nt else "MISSING", "")
slo = read(".github/workflows/slo.yml")
verdict("T10 slo.yml 存在", "PROVEN" if slo else "MISSING", ".github/workflows/slo.yml")
ig = read("scripts/ignored_test_inventory_freeze.txt") or ""
mm = re.search(r"max_unignore_pending\s*=\s*(\d+)", ig)
verdict("T10 unignore 棘轮已归零（真解除非登记）",
        "PROVEN" if mm and mm.group(1) == "0" else "NOT PROVEN", mm.group(0) if mm else "未见棘轮行")

# --- T4 单一真值源 ---
for f in ("scripts/perf_redlines.toml", "scripts/perf_thresholds.toml",
          "scripts/bench_inventory_freeze.txt", "scripts/ignored_test_inventory_freeze.txt",
          "scripts/check_perf_redlines.py"):
    verdict(f"T4 {os.path.basename(f)}", "PROVEN" if os.path.exists(f) else "MISSING", f)

# --- T7 检查单 14 项 ---
ck = read("docs/reports/phaseR-release-checklist-v2.28.0.md") or ""
rows = len(re.findall(r"^\| \d+b?(?:\.5b?)? \|", ck, re.M))
verdict("T7 检查单条目数", "PROVEN" if rows >= 14 else "NOT PROVEN", f"{rows} 行")

# --- 输出 ---
bad = [r for r in RESULTS if r[1] != "PROVEN"]
# 标记用 ASCII：Windows 默认控制台编码是 GBK，非 ASCII 符号会直接 UnicodeEncodeError
# 把审计结果吹掉（本脚本写错一次已实证）。治理脚本不得依赖宿主编码。
for item, st, ev in RESULTS:
    mark = {"PROVEN": "[OK]  ", "NOT PROVEN": "[WARN]", "MISSING": "[MISS]"}[st]
    print(f"{mark} {item:44} {st:11} | {ev[:70]}")
print(f"\nTOTAL {len(RESULTS)}: PROVEN {len(RESULTS) - len(bad)} / NOT-PROVEN {len(bad)}")
sys.exit(1 if bad else 0)
