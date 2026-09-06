#!/usr/bin/env python3
"""门禁 expect 与 workflow continue-on-error 一致性交叉校验（反馈链路，审计 finding #4）。

背景：本项目多次出现"判据写在 A、跑法写在 B、实际跑的是 C"的三源分裂，最阴的一种是
`gate_manifest.toml` 把某门标为 expect=0（收口必须绿），但 CI 里同一脚本却在
`continue-on-error: true` 的 job/step 下运行 —— 于是"清单说阻塞、实际失败被吞"，
本地收口看着要红、PR 却静默放行。coverage.yml 旧版正是此病（现已去 continue-on-error）。

本脚本静态对拍两者：
  1. 从 `scripts/gate_manifest.toml` 取所有 expect=0 的判据所引用的脚本 basename 集合 S。
  2. 扫描 `.github/workflows/*.yml`，找出处于 `continue-on-error: true` 语境（job 级或
     step 级）的 `run:` 命令，取其引用的脚本 basename 集合 N。
  3. GAP = S ∩ N：某"收口必须绿"的门在 CI 被非阻塞运行。列名并退 1。

刻意不做的事：不猜命令语义、不解析 YAML 全文（用行级缩进扫描，避免引第三方 yaml 依赖 +
避免把 slo/stress 这类"设计上观测态、且本就不在 expect=0 清单"的 workflow 误报）。

已知限制：扫描器识别"COE 行在 run 行之前"的语境（job 级 continue-on-error，以及
step 内 `- name` → `continue-on-error: true` → `run:` 这一常见书写序），
本仓真实风险面（slo.yml / stress.yml / 旧 coverage.yml）均为 job 级 COE 前置，已覆盖；
若某 step 把 `continue-on-error` 写在 `run:` 之后则漏抓（假阴），属已知边界，非静默假装通过。

退出码：0 无不一致 / 1 存在 GAP（清单必绿门被 CI 吞）/ 2 环境/清单不可读（不可判定）。
输出 ASCII（Windows GBK 控制台约定）。
"""
import glob
import os
import re
import subprocess
import sys

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:  # noqa: BLE001
    pass

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))
MANIFEST = os.path.join(HERE, "gate_manifest.toml")
WORKFLOWS_GLOB = os.path.join(ROOT, ".github", "workflows", "*.yml")
SCRIPT_RE = re.compile(r"([\w./-]+\.(?:py|sh|ps1))\b")
COE_RE = re.compile(r"^\s*continue-on-error:\s*true\s*$")
# 解释器/包装词：跳过它们以取脚本后的第一个参数（子命令/flag）作为签名区分。
_INTERP = {"bash", "sh", "pwsh", "powershell", "py", "python", "python3", "{python}"}


def script_signatures(cmd):
    """从命令串抽取 (script_basename, 首个参数签名) 列表。

    参数签名 = 脚本 token 后第一个既非解释器、也非另一脚本的 token（如 --selftest /
    emit-slo-filter / --static-only）；无参数则为空串。区分同一脚本的不同用途，
    避免把 slo.yml 的 `emit-slo-filter` 误当成 G-01/02 的门调用。
    """
    toks = cmd.split()
    out = []
    for i, t in enumerate(toks):
        base = os.path.basename(t.rstrip('"\''))
        if base.endswith((".py", ".sh", ".ps1")):
            arg = ""
            for j in range(i + 1, len(toks)):
                nxt = toks[j]
                nb = os.path.basename(nxt.rstrip('"\''))
                if nb.endswith((".py", ".sh", ".ps1")):
                    break  # 下一个脚本，本条无参
                if nxt.lower() in _INTERP or nxt in ("|", "|&", "&&", ";", "2>&1", ")", '"'):
                    break
                if nxt.startswith("$") or nxt.startswith("|"):
                    break
                arg = nxt
                break
            out.append((base, arg))
    return out


def blocking_gate_scripts(manifest_path):
    """返回 gate_manifest 中 expect=0 判据引用的脚本 basename 集合。"""
    if not os.path.isfile(manifest_path):
        return None
    with open(manifest_path, "rb") as f:
        raw = f.read().decode("utf-8-sig")
    scripts = set()
    cur_cmd, cur_expect = None, None
    for line in raw.splitlines():
        s = line.strip()
        if s == "[[gate]]":
            if cur_expect == 0 and cur_cmd:
                scripts |= set(script_signatures(cur_cmd))
            cur_cmd, cur_expect = None, None
        elif s.startswith("cmd"):
            cur_cmd = s.split("=", 1)[1].strip().strip('"') if "=" in s else ""
        elif s.startswith("expect"):
            try:
                cur_expect = int(s.split("=", 1)[1].strip())
            except ValueError:
                cur_expect = None
    if cur_expect == 0 and cur_cmd:  # 末尾无后继 [[gate]]
        scripts |= set(script_signatures(cur_cmd))
    return scripts


def nonblocking_scripts_in_workflows(wf_glob):
    """返回在 continue-on-error: true 语境下被 run 的脚本 basename 集合 + 出处。"""
    found = {}
    for wf in sorted(glob.glob(wf_glob)):
        with open(wf, encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
        # 缩进追踪：任一 continue-on-error:true 出现的块（job 或 step），其后续更/等深缩进的
        # run 命令视为该非阻塞语境。用简易缩进窗口（COE 行之后、缩进回落到 COE 缩进以下的 run）。
        coe_indent = None
        for i, line in enumerate(lines):
            if COE_RE.match(line):
                coe_indent = len(line) - len(line.lstrip())
                continue
            if coe_indent is not None:
                ind = len(line) - len(line.lstrip())
                if line.strip() and ind < coe_indent:
                    coe_indent = None  # 离开该 continue-on-error 块
                    continue
                # 仅看真正执行命令的行（run:/续行/带解释器的脚本调用）
                if re.search(r"(run:|bash |pwsh |py |python3? |sh |\|\s*$)", line) or line.strip().startswith("bash"):
                    for sig in script_signatures(line):
                        found.setdefault(sig, set()).add(os.path.basename(wf))
    return found


def check(manifest_path, wf_glob):
    blocking = blocking_gate_scripts(manifest_path)
    if blocking is None:
        return None
    nonblock = nonblocking_scripts_in_workflows(wf_glob)
    gaps = sorted(blocking & set(nonblock.keys()))
    return gaps, nonblock, blocking


def mode_selftest():
    print("=== selftest: gate-parity checker must have teeth ===")
    fails = []

    def expect(name, cond):
        print(f"  [{'ok' if cond else 'FAIL'}] {name}")
        if not cond:
            fails.append(name)

    import tempfile
    tmp = tempfile.mkdtemp(prefix="parity_")
    # 造一份 mini workflow：A.py 在 continue-on-error 下跑（应被抓），B.py 在阻塞下跑（不抓）
    wfdir = os.path.join(tmp, "wf")
    os.makedirs(wfdir)
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write(
            "jobs:\n"
            "  swallow:\n"
            "    steps:\n"
            "      - name: s\n"
            "        continue-on-error: true\n"
            "        run: |\n"
            "          bash scripts/A.py\n"
            "  block:\n"
            "    steps:\n"
            "      - name: s2\n"
            "        run: |\n"
            "          bash scripts/B.py\n"
        )
    nb = nonblocking_scripts_in_workflows(os.path.join(wfdir, "*.yml"))
    expect("selftest-1 flags script under continue-on-error", ("A.py", "") in nb)
    expect("selftest-2 does NOT flag blocking-context script", ("B.py", "") not in nb)
    # mini gate_manifest：A.py expect=0（必绿），B.py expect=0
    man = os.path.join(tmp, "gm.toml")
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "X"\ncmd = "bash scripts/A.py"\nexpect = 0\nkind = "light"\n'
                '[[gate]]\nid = "Y"\ncmd = "bash scripts/B.py"\nexpect = 0\nkind = "light"\n')
    gaps, _nb, _bl = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-3 gap = {(A.py,'')} only (B.py blocking ok)", gaps == [("A.py", "")])
    # 参数敏感：manifest 门用 A.py --gate，COE(job级,run 前) 里用 A.py --other → 签名不同 → 不误报
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write("jobs:\n  s:\n    continue-on-error: true\n    steps:\n      - run: |\n          bash scripts/A.py --other\n")
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "X"\ncmd = "bash scripts/A.py --gate"\nexpect = 0\nkind = "light"\n')
    gaps_arg, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-6 arg-signature prevents false positive (--gate vs --other)", gaps_arg == [])
    # 同签名仍应抓：COE(job级) 用 A.py --gate，manifest 也 --gate → gap
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write("jobs:\n  s:\n    continue-on-error: true\n    steps:\n      - run: |\n          bash scripts/A.py --gate\n")
    gaps_same, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-7 same arg-signature under job-level COE is flagged", gaps_same == [("A.py", "--gate")])
    # 无 expect=0 命中非阻塞 → 空 gap
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write("jobs:\n  b:\n    steps:\n      - run: |\n          bash scripts/B.py\n")
    gaps2, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-4 no gap when all blocking-context", gaps2 == [])
    # 清单缺失 → check 返回 None（不可判定）
    expect("selftest-5 missing manifest -> None", check(os.path.join(tmp, "nope.toml"), wfdir) is None)
    import shutil
    shutil.rmtree(tmp, ignore_errors=True)
    print("=== selftest result:", "ALL PASS" if not fails else f"{len(fails)} FAIL", "===")
    return 0 if not fails else 1


def mode_check():
    res = check(MANIFEST, WORKFLOWS_GLOB)
    if res is None:
        print("[FAIL] gate_manifest not readable (undeterminable)")
        return 2
    gaps, nonblock, blocking = res
    print(f"[info] expect=0 gate scripts: {len(blocking)}; non-blocking-context scripts in CI: {len(nonblock)}")
    if gaps:
        print(f"[FAIL] {len(gaps)} gate(s) the closure list requires-passing run under continue-on-error in CI:")
        for g in gaps:
            print(f"   - {g}  (in {sorted(nonblock[g])})")
        return 1
    print("[OK] no parity gap: every closure-required gate runs blocking in CI (or is intentionally absent)")
    return 0


def main(argv):
    if "--selftest" in argv:
        return mode_selftest()
    return mode_check()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
