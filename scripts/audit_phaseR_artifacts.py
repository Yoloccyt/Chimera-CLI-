#!/usr/bin/env python3
"""Phase R 交付物与引用证据的存亡清点。

动因（本会话实测）：会话过程中多个**未跟踪**文件凭空消失 ——
`docs/reports/audit_deep_dive_R4_2026-08-31.md`（曾完整读取并多处引用）、
4 个 `crates/chimera-cli/tests/*rate_limit*/search_*` 目标（从未进 git）、
`tmp/phaseR_t10_ci_fast.log`、以及初稿编号为 ADR-161/166 的两份草稿。
本清单逐条核对：交付物是否在盘上 + git 是否跟踪 + 报告引用的证据日志是否存在。
"""
import io
import os
import re
import subprocess

# 我在 Phase R 声称产出的文件（Spec 的交付物 + 报告里引用的证据）
# 名单必须从**实际写入记录**构造，不能凭记忆列举：初版凭记忆写了 3 个本仓库从未存在的
# 路径（docs/audit/phaseR-hi-lat-debt-inventory-*、docs/reports/audit_deep_dive_R4_*、
# crates/chimera-cli/tests/agent_web_search_rate_limit_e2e.rs），导致清点把"自己编的名单"
# 当成"消失的交付物"报红（已记 RK-P40）。这三个名字不得再回到本清单。
DELIVERABLES = [
    "scripts/check_perf_redlines.py",
    "scripts/check_perf_redlines.ps1",
    "scripts/check_perf_redlines.sh",
    "scripts/perf_redlines.toml",
    "scripts/perf_thresholds.toml",
    "scripts/bench_inventory_freeze.txt",
    "scripts/ignored_test_inventory_freeze.txt",
    "scripts/audit_cmd_sync.py",
    ".github/workflows/slo.yml",
    ".config/nextest.toml",
    "docs/governance/RK-P_risk_register.md",
    "docs/reports/phaseR-wave1-closure.md",
    "docs/reports/phaseR-release-checklist-v2.28.0.md",
    "docs/reports/phaseR-t2-commit-batches.md",
    "docs/architecture/ADR-161-island-repayment-roadmap-and-batch-ratchet.md",
    "docs/architecture/ADR-166-doc-consistency-gate-execution-plane.md",
    "docs/architecture/ADR-167-audit-gate-new-advisories.md",
    "docs/architecture/ADR-168-closure-writeback-and-doc-numeric-ssot.md",
    "docs/architecture/ADR-169-slo-observation-period-transition.md",
    "Dockerfile",
    "CHANGELOG.md",
    "Cargo.lock",
]

# 探针灵敏度对照（**不是**交付物）：这些路径已知不存在，列出只为证明"缺失会被报出来"。
# 必须与 DELIVERABLES 分开：初版把二者混在一个表里，输出"缺失 3 项"被我当成
# "3 项交付物消失"写进报告结论，还引用了一个不存在文件的行号（RK-P40 根因）。
PROBE_SENSITIVITY = [
    "docs/audit/phaseR-hi-lat-debt-inventory-2026-08-31.md",
    "docs/reports/audit_deep_dive_R4_2026-08-31.md",
    "crates/chimera-cli/tests/agent_web_search_rate_limit_e2e.rs",
]

REPORTS = [
    "docs/reports/phaseR-wave1-closure.md",
    "docs/reports/phaseR-release-checklist-v2.28.0.md",
    "docs/governance/RK-P_risk_register.md",
]


def git_tracked(path: str) -> bool:
    r = subprocess.run(["git", "ls-files", "--error-unmatch", path],
                       capture_output=True, text=True)
    return r.returncode == 0


def all_basenames() -> set:
    """全库（排除构建产物）现存文件名集合，用于判定一个引用是否真的消失了。

    为何不能猜基目录：报告里证据有三种写法 —— `tmp/x.log`、`scripts/x.sh`、
    以及 **后缀简写**（`tmp/a.log` + `_exit.txt`，后者不是一个独立文件）。
    上一版只试 `tmp/` 与当前目录两个 base，把 21 条引用误报成丢失，
    真阴性信号反而被假阳淹没。
    """
    names = set()
    for root, dirs, files in os.walk("."):
        dirs[:] = [d for d in dirs if d not in ("target", "tmp_podman", ".git", "node_modules")]
        names.update(files)
    return names


def main() -> int:
    print("=== 1) 交付物存亡 + git 跟踪状态 ===")
    lost, untracked = [], []
    for p in DELIVERABLES:
        exists = os.path.exists(p)
        tr = git_tracked(p) if exists else False
        flag = "OK " if exists else "缺失"
        print(f"  [{flag}] {p:70} git={'跟踪' if tr else '未跟踪'}")
        if not exists:
            lost.append(p)
        elif not tr:
            untracked.append(p)

    print(f"\n  缺失 {len(lost)} 项；存在但未跟踪（丢失风险最高）{len(untracked)} 项")
    for p in untracked:
        print(f"    未跟踪: {p}")

    # 灵敏度对照：这些项**应当**全部缺失；若出现存在的，说明清单需人工定性
    # （误建？还是它其实是真交付物、被错分到对照表？）。
    sens_present = [p for p in PROBE_SENSITIVITY if os.path.exists(p)]
    sens_absent = [p for p in PROBE_SENSITIVITY if not os.path.exists(p)]
    print(f"\n  [sensitivity] 已知不存在路径 {len(PROBE_SENSITIVITY)} 个，按缺失报出 {len(sens_absent)} 个"
          f"（期望=全部，否则探针失去牙齿）")
    for p in sens_present:
        print(f"    [SENSITIVITY-DRIFT] 对照路径现在存在，需人工定性: {p}")

    print("\n=== 2) 报告引用的证据文件是否仍在 ===")
    live = all_basenames()
    missing_logs = []
    for rep in REPORTS:
        if not os.path.exists(rep):
            continue
        text = io.open(rep, encoding="utf-8", errors="replace").read()
        # 首字符必须允许 `_`：否则 `_exit.txt` 被截成 `exit.txt`，下面的简写跳过规则会失效。
        cands = set(re.findall(r"[A-Za-z0-9_][A-Za-z0-9_./-]*\.(?:log|txt|json|py|sh|ps1|toml)\b", text))
        gone = []
        for c in sorted(cands):
            base = c.rsplit("/", 1)[-1]
            if base.startswith("_"):
                continue  # 后缀简写（`a.log` + `_exit.txt`），不是独立文件
            if c.startswith(("tmp/", "docs/", "scripts/", ".github/")) and os.path.exists(c):
                continue
            if base in live:
                continue
            gone.append(c)
        print(f"  {rep}: 引用 {len(cands)} 个文件式引用，其中 {len(gone)} 个全库找不到（真丢失）")
        for g in gone:
            print(f"      丢失: {g}")
        missing_logs += gone

    print("\n=== 结论 ===")
    print(f"  交付物缺失: {len(lost)}")
    print(f"  未跟踪交付物（随时可能丢）: {len(untracked)}")
    print(f"  报告引用但已消失的证据文件: {len(missing_logs)}")
    print(f"  灵敏度对照按缺失报出: {len(sens_absent)}/{len(PROBE_SENSITIVITY)}")
    # 退出码只受**真交付物**影响；灵敏度项永远缺失，不能把它算成失败
    return 1 if (lost or missing_logs or sens_present) else 0


if __name__ == "__main__":
    raise SystemExit(main())
