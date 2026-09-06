#!/usr/bin/env python3
"""审计"审计脚本"的覆盖面：把 Spec 点名的要求/交付物枚举出来，反查两处登记。

WHY：`audit_phaseR_spec.py` 是我写的，它自身没有对抗性检验 —— 如果我从 Spec 里漏抄了一条要求，
它照样会输出"28/29 PROVEN"，而那个数字看起来完全可信。这类"审计器自身的盲区"正是本波次反复
出现的根问题（写了不跑 / 登记不偿还 / 声明无真值源）。

两处反查：
  A. 该要求是否出现在 audit_phaseR_spec.py 的判定项里（源码文本比对）
  B. 该要求是否被 scripts/gate_manifest.toml 的 spec_ref 登记

退出码：0 = 全部有 A 或 B；1 = 存在无登记的 Spec 条目（审计可能有盲区）；2 = 不可判定（Spec/审计脚本缺失）。
"""
import io
import os
import re
import sys
import tomllib

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

def _find_spec():
    """Spec 路径：环变量 > argv[1] > 常见客户端缓存目录的 glob。

    不再把某台机器的绝对路径写死当默认：那样“别人跑不起”会被掩盖，
    而本工具是常驻判据 G-16——必须能在一台干净机器上给出**可信结论**，
    包括“拿不到 Spec → 不可判定（退 2）”，而不是默认的“没找到就算通过”。
    """
    import glob as _glob
    explicit = os.environ.get("CHIMERA_SPEC_PATH", "") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if explicit:
        # 显式给定就必须只用它：给错/不存在时退 2。上一版写的是“显式路径无效则退回 glob”，
        # 结果是拿着**另一份 Spec** 去做反查还能判出 OK——典型的 fail-open。
        return explicit if os.path.exists(explicit) else ""
    for base in (os.path.expandvars(r"%APPDATA%\QoderCN\SharedClientCache\cache\plans"),
                 os.path.expandvars(r"%LOCALAPPDATA%\QoderCN\SharedClientCache\cache\plans"),
                 os.path.expanduser("~/AppData/Roaming/QoderCN/SharedClientCache/cache/plans")):
        for c in sorted(_glob.glob(os.path.join(base, "Phase_R_*.md"))):
            if os.path.exists(c):
                return c
    return ""


SPEC = _find_spec()
# 允许外部指定 Spec：既为负对照（喂一份含"不存在的任务"的 Spec，验证本工具会报盲区），
# 也让这套反查在换计划文件时不必改代码。
AUDIT = "scripts/audit_phaseR_spec.py"
MANIFEST = "scripts/gate_manifest.toml"

for p in (SPEC, AUDIT, MANIFEST):
    if not os.path.exists(p):
        print(f"[UNKNOWN] 缺输入：{p} —— 不视为通过")
        sys.exit(2)

spec = io.open(SPEC, encoding="utf-8", errors="replace").read()
audit = io.open(AUDIT, encoding="utf-8", errors="replace").read()
with open(MANIFEST, "rb") as f:
    mf = tomllib.load(f)
spec_refs = " ".join(str(g.get("spec_ref", "")) for g in mf.get("gate", []))

# --- 从 Spec 抽取"可追踪单元" ---
tasks = sorted(set(re.findall(r"\bR1-T\d+\b", spec)) | set(re.findall(r"\bP0-[AB]\b", spec)))
risks = sorted(set(re.findall(r"\bRK-P\d+\b", spec)))
facts = sorted(set(re.findall(r"\bN-0\d\b", spec)))
# 命名交付物：Spec 里以反引号包住的仓库路径
deliv = sorted({m for m in re.findall(r"`([A-Za-z0-9_.\-/]+\.(?:py|sh|ps1|toml|txt|yml|md|rs))`", spec)
                if "/" in m or m.endswith(".toml")})

print(f"Spec 抽出：任务 {len(tasks)} · 风险 {len(risks)} · 新事实 {len(facts)} · 命名交付物 {len(deliv)}")


def variants(tok: str):
    """R1-T5 / P0-A 这类编号在不同文档里有两种写法，必须都认。"""
    out = {tok}
    if tok.startswith("R1-"):
        out.add(tok[3:])
    else:
        out.add("R1-" + tok)
    return out


def covered_by_audit(tok: str) -> bool:
    return any(v in audit for v in variants(tok))


def covered_by_manifest(tok: str) -> bool:
    return any(v in spec_refs for v in variants(tok))


gaps = []
print("\n=== 任务 / P0 项 ===")
for t in tasks:
    a, b = covered_by_audit(t), covered_by_manifest(t)
    print(f"  {t:8} 审计项={'Y' if a else 'n'} 判据登记={'Y' if b else 'n'}")
    if not a and not b:
        gaps.append(f"任务 {t} 既无审计项也无判据登记")

print("\n=== Spec §4 新增风险 ===")
for r in risks:
    a = f"RK-P" in audit and r in audit
    print(f"  {r:8} 出现在审计脚本={'Y' if a else 'n'}（风险类条目只需入册，不强制判据）")
    if not os.path.exists("docs/governance/RK-P_risk_register.md"):
        gaps.append("RK-P 主册缺失")
    else:
        reg = io.open("docs/governance/RK-P_risk_register.md", encoding="utf-8", errors="replace").read()
        if r not in reg:
            gaps.append(f"{r} 未入册")

print("\n=== 命名交付物是否在盘上 ===")
# Spec 里"刻意不存在"的东西：缺失才是正确状态，不能报成交付物缺失
EXPECTED_ABSENT = {
    "rust-toolchain.toml": "N-01/红线：本仓刻意不提供该文件（GNU channel 由 install.ps1 -SetupEnv 保证）",
}
for d in deliv:
    base = os.path.basename(d)
    cand = [d, "crates/" + d, base, "scripts/" + base, "docs/" + base, "tests/" + base]
    found = next((c for c in cand if os.path.exists(c)), None)
    print(f"  {'OK  ' if found else 'MISS'} {d}" + (f"  -> {found}" if found and found != d else ""))
    if not found:
        ea = next((why for k, why in EXPECTED_ABSENT.items() if k in d), None)
        if ea:
            print(f"       ^ 预期不存在（正确）：{ea}")
        else:
            gaps.append(f"Spec 点名的交付物不存在：{d}")

print("\n=== 新事实 N-0x（是否已在文档留痕）===")
probe_files = ["docs/reports/phaseR-wave1-closure.md", "CHANGELOG.md", "agents.md",
               "docs/governance/RK-P_risk_register.md"]
blob = "".join(io.open(p, encoding="utf-8", errors="replace").read()
               for p in probe_files if os.path.exists(p))
# 每条"新事实"配实质关键词（文档不会写 N-03，只会写它的内容）；命中即视为留痕。
FACT_TERMS = {
    "N-01": ["os error 206", "逐包聚合"],
    "N-02": ["14/44", "fmt 归一", "53 个"],
    "N-03": ["--jobs 4", "编译面 OOM", "memory allocation"],
    "N-04": ["manual_range_contains", "util.rs:741"],
    "N-05": ["unignore-target", "max_unignore_pending"],
    "N-06": ["keyword_overlaps", "互为子串"],
    "N-07": ["calibration_pending", "100_000_000"],
    "N-08": ["total=120", "gated=12", "逐键等价", "零行为变更"],
}
for n in facts:
    terms = FACT_TERMS.get(n, [])
    hit = [x for x in terms if x in blob]
    print(f"  {n}: 实质命中 {len(hit)}/{len(terms)} {hit}")
    if not hit:
        gaps.append(f"{n} 的实质内容未在任何收口文档出现（terms={terms}）")

print("\n结论：")
if gaps:
    print(f"[GAP] {len(gaps)} 处审计盲区：")
    for g in gaps:
        print("   -", g)
    sys.exit(1)
print("[OK] Spec 抽出的全部单元均有登记/交付物在盘/新事实留痕")
sys.exit(0)
