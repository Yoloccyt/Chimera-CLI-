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
# 清单驱动的执行面（TC-C）：ci.yml 不再逐条枚举零 cargo 的静态门，改由
# `scripts/run_gate_manifest.py --ci` 一条命令驱动整张清单。若仍要求"脚本名逐字
# 出现在某个 run 行里"，这条执行面一落地，全部判据都会被报成 MISSING —— 那是
# 假红，不是发现。故方向 B（MISSING）接受第二种接线证据：某 workflow **执行**
# 驱动脚本（且不在 continue-on-error 语境下）即视为覆盖它在 pr profile 里选中的门。
DRIVER_TOKEN = "scripts/run_gate_manifest.py"
_RUNLIKE = re.compile(r"(run:|bash |pwsh |py |python3? |sh |\|\s*$)")
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
    """返回 (expect=0 判据引用的脚本签名集合, 声明不接 CI 的集合, 声明 CI 内非阻塞的集合)。

    除原有的 expect=0 集合外，新增对 `ci_exempt = "<reason>"` 字段的识别：
    某门若因设计原因有意不接入任何 workflow（如仅收口时人工跑的审计项），
    必须在清单里写明理由；否则"清单标 expect=0 但 CI 从不调用"会被判 MISSING。

    WHY 需要这个豁免口子：不提供显式豁免，反向断言会把"有意本地跑"的门误报成缺陷，
    逼人删门或硬塞进 CI；而提供豁免又强制写理由，避免它变成静默的逃避通道。

    ★ `ci_advisory = "<reason>"` 是同一形状的**另一半**（F39c，2026-09-21 由登记孤儿门逼出）：
      有些判据故意在 CI 里挂 `continue-on-error: true`（性能趋势探针、宿主相关的 bench 阈值），
      它**接了 CI 但不阻塞**。此前没有这种表达法，于是两难：不登记 ⇒ 台账看不见这道门（正是
      F39 的盲区）；登记 ⇒ gap 断言"要求必过的门却挂在非阻塞语境下"直接判红。
      加字段而不是放宽断言：advisory 必须写理由，且**只豁免 gap 断言，不豁免 MISSING**
      —— 声明 advisory 却根本没接 CI，仍然是缺陷。
    """
    if not os.path.isfile(manifest_path):
        return None, None, None
    with open(manifest_path, "rb") as f:
        raw = f.read().decode("utf-8-sig")
    scripts, exempt, advisory = set(), set(), set()
    cur_cmd, cur_expect, cur_exempt, cur_adv = None, None, None, None
    for line in raw.splitlines():
        s = line.strip()
        if s == "[[gate]]":
            if cur_expect == 0 and cur_cmd:
                sigs = set(script_signatures(cur_cmd))
                scripts |= sigs
                if cur_exempt:
                    exempt |= sigs
                if cur_adv:
                    advisory |= sigs
            cur_cmd, cur_expect, cur_exempt, cur_adv = None, None, None, None
        elif s.startswith("cmd"):
            cur_cmd = s.split("=", 1)[1].strip().strip('"') if "=" in s else ""
        elif s.startswith("expect"):
            try:
                cur_expect = int(s.split("=", 1)[1].strip())
            except ValueError:
                cur_expect = None
        elif re.match(r"ci_exempt\s*=", s):
            # ★ 键名必须精确匹配（F39d）：旧写法 `startswith("ci_exempt")` 让 `ci_exemptX = "..."`
            #   这类**拼错的键**也拿到豁免权 ⇒ 一次打字错误就能静默关掉一条断言。
            #   （这是我为 F39c 做负控时撞出来的：把 ci_advisory 改名成 ci_advisoryX，门照样绿。）
            cur_exempt = s.split("=", 1)[1].strip().strip('"') if "=" in s else ""
        elif re.match(r"ci_advisory\s*=", s):
            cur_adv = s.split("=", 1)[1].strip().strip('"') if "=" in s else ""
    if cur_expect == 0 and cur_cmd:  # 末尾无后继 [[gate]]
        sigs = set(script_signatures(cur_cmd))
        scripts |= sigs
        if cur_exempt:
            exempt |= sigs
        if cur_adv:
            advisory |= sigs
    return scripts, exempt, advisory


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


def all_scripts_in_workflows(wf_glob):
    """返回所有 workflow 的 run 命令中出现过的脚本签名集合（不论阻塞与否）。

    用于**反向断言**：gate_manifest 里标 expect=0 的门必须在某个 workflow 里真的被
    调用过，否则就是"只登记清单、没接进 CI" —— 这样的门永远不会红，保护不了任何东西。

    WHY（2026-09-11 教训）：方向 2 交付的持锁跨 await 门只写进 gate_manifest、忘了接
    ci.yml，而本脚本原先只校验"CI 中出现的门是否被 continue-on-error 吞掉"，对"根本没
    出现在 CI 里"的情形判为 intentionally absent 而放行 —— 门等于没装、且无人察觉。
    """
    used = {}
    for wf in sorted(glob.glob(wf_glob)):
        with open(wf, encoding="utf-8", errors="replace") as f:
            for line in f:
                if re.search(r"(run:|bash |pwsh |py |python3? |sh |\|\s*$)", line) or line.strip().startswith("bash"):
                    for sig in script_signatures(line):
                        used.setdefault(sig, set()).add(os.path.basename(wf))
    return used


def gate_blocks(manifest_path):
    """Return [[line, ...], ...] — one block per [[gate]] row (raw lines)."""
    if not os.path.isfile(manifest_path):
        return []
    with open(manifest_path, "rb") as f:
        raw = f.read().decode("utf-8-sig")
    blocks, cur = [], None
    for line in raw.splitlines():
        if line.strip() == "[[gate]]":
            if cur is not None:
                blocks.append(cur)
            cur = []
            continue
        if cur is not None:
            cur.append(line)
    if cur is not None:
        blocks.append(cur)
    return blocks


def driver_covered(manifest_path):
    """(basename, first-arg) signatures the manifest driver runs on pr profile.

    口径与 check_ci_manifest_parity.driver_covered 一致，且同样是**保守**的：
    只认 kind=light、未声明 profiles、未声明 mutating 的行 —— 这正是 govd 的
    pr profile 无附加旗标时会选中的集合（权威实现 govd/model.py::Task.selected_by）。
    证不出来的行留在外面，仍然要求逐字接线：宁可漏报"已接线"，不可虚报。
    """
    covered = set()
    for block in gate_blocks(manifest_path):
        cmd, kind, has_profiles, mutating = None, None, False, False
        for line in block:
            s = line.strip()
            if s.startswith("cmd") and "=" in s and cmd is None:
                cmd = s.split("=", 1)[1].strip().strip('"')
            elif s.startswith("kind") and "=" in s:
                kind = s.split("=", 1)[1].strip().strip('"')
            elif re.match(r"profiles\s*=", s):
                has_profiles = True
            elif re.match(r"mutating\s*=\s*true", s):
                mutating = True
        if not cmd or kind != "light" or has_profiles or mutating:
            continue
        covered |= set(script_signatures(cmd))
    return covered


def driver_contexts(wf_glob):
    """(blocking, nonblocking) — is the manifest driver executed, and in which 语境?

    `--selftest` 行不算：它证明的是驱动器的分类器有牙，而不是跑了任何一道门。
    把它算成覆盖，等于让"从未执行清单"的 workflow 看起来接线完整。
    """
    blocking = nonblocking = False
    for wf in sorted(glob.glob(wf_glob)):
        with open(wf, encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
        coe_indent = None
        for line in lines:
            if COE_RE.match(line):
                coe_indent = len(line) - len(line.lstrip())
                continue
            if coe_indent is not None:
                ind = len(line) - len(line.lstrip())
                if line.strip() and ind < coe_indent:
                    coe_indent = None
            if DRIVER_TOKEN in line and "--selftest" not in line and _RUNLIKE.search(line):
                if coe_indent is None:
                    blocking = True
                else:
                    nonblocking = True
    return blocking, nonblocking


def check(manifest_path, wf_glob, covered=None, driver_blocking=False,
          driver_nonblocking=False):
    blocking, exempt, advisory = blocking_gate_scripts(manifest_path)
    if blocking is None:
        return None
    nonblock = nonblocking_scripts_in_workflows(wf_glob)
    used = all_scripts_in_workflows(wf_glob)
    if covered and driver_blocking:
        for sig in covered:
            used.setdefault(sig, set()).add("run_gate_manifest.py")
    elif covered and driver_nonblocking:
        # 驱动脚本挂在 continue-on-error 下 = 整张清单被非阻塞执行 ⇒ 与"某门被吞"
        # 同一病灶，必须照样记成 gap，否则这条扩展反而成了新的静默通道。
        for sig in covered:
            nonblock.setdefault(sig, set()).add("run_gate_manifest.py(continue-on-error)")
    # advisory 门从 gap 断言里扣除，但**不**从 MISSING 里扣除（见 blocking_gate_scripts docstring）
    gaps = sorted((blocking - advisory) & set(nonblock.keys()))
    # 反向断言：expect=0 且未声明 ci_exempt 的门，必须被某个 workflow 真正调用过
    missing = sorted((blocking - exempt) - set(used.keys()))
    return gaps, nonblock, blocking, missing


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
    gaps, _nb, _bl, _miss = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-3 gap = {(A.py,'')} only (B.py blocking ok)", gaps == [("A.py", "")])
    # 参数敏感：manifest 门用 A.py --gate，COE(job级,run 前) 里用 A.py --other → 签名不同 → 不误报
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write("jobs:\n  s:\n    continue-on-error: true\n    steps:\n      - run: |\n          bash scripts/A.py --other\n")
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "X"\ncmd = "bash scripts/A.py --gate"\nexpect = 0\nkind = "light"\n')
    gaps_arg, _, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-6 arg-signature prevents false positive (--gate vs --other)", gaps_arg == [])
    # 同签名仍应抓：COE(job级) 用 A.py --gate，manifest 也 --gate → gap
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write("jobs:\n  s:\n    continue-on-error: true\n    steps:\n      - run: |\n          bash scripts/A.py --gate\n")
    gaps_same, _, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-7 same arg-signature under job-level COE is flagged", gaps_same == [("A.py", "--gate")])
    # 无 expect=0 命中非阻塞 → 空 gap
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write("jobs:\n  b:\n    steps:\n      - run: |\n          bash scripts/B.py\n")
    gaps2, _, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-4 no gap when all blocking-context", gaps2 == [])
    # 反向断言：清单里 expect=0 的门没有被任何 workflow 调用 → MISSING
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "Z"\ncmd = "bash scripts/C.py --gate"\nexpect = 0\nkind = "light"\n')
    _g8, _nb8, _bl8, miss8 = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-8 gate wired into no workflow is flagged MISSING", miss8 == [("C.py", "--gate")])
    # 显式豁免（须附理由）后不再报 MISSING —— 避免把"有意本地跑"的门误判成缺陷
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "Z"\ncmd = "bash scripts/C.py --gate"\nexpect = 0\nkind = "light"\n'
                'ci_exempt = "local-only audit, ADR-000"\n')
    _g9, _nb9, _bl9, miss9 = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-9 ci_exempt suppresses MISSING (reason required)", miss9 == [])
    # F39c：advisory（接了 CI 但**故意**非阻塞）—— 有理由 ⇒ 不报 gap；空理由 ⇒ 照报。
    # 这三条夹具是"加字段而不是放宽断言"的证据：删掉 ci_advisory 识别，11 会红；
    # 把 advisory 也用来豁免 MISSING，13 会红。
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write("jobs:\n  adv:\n    steps:\n      - name: s\n"
                "        continue-on-error: true\n        run: |\n          bash scripts/A.py\n")
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "X"\ncmd = "bash scripts/A.py"\nexpect = 0\nkind = "light"\n'
                'ci_advisory = "perf trend probe: advisory by design"\n')
    gaps_adv, _, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-11 ci_advisory suppresses gap (reason required)", gaps_adv == [])
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "X"\ncmd = "bash scripts/A.py"\nexpect = 0\nkind = "light"\n'
                'ci_advisory = ""\n')
    gaps_adv2, _, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-12 ci_advisory WITHOUT reason still gaps", gaps_adv2 == [("A.py", "")])
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "Z"\ncmd = "bash scripts/C.py --gate"\nexpect = 0\nkind = "light"\n'
                'ci_advisory = "declared advisory but wired nowhere"\n')
    _g, _nb, _bl, miss_adv3 = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-13 advisory does NOT exempt MISSING", miss_adv3 == [("C.py", "--gate")])
    # F39d：拼错的豁免键**不得**生效（旧解析用 startswith, `ci_advisoryX` 也会被认成 advisory
    # 而静默关掉 gap 断言 —— 一次打字错误 = 一条断言消失, 正是本工具存在的理由的反面）。
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "X"\ncmd = "bash scripts/A.py"\nexpect = 0\nkind = "light"\n'
                'ci_advisoryX = "typo key must not silence the gap assertion"\n')
    gaps_typo, _, _, _ = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-14 misspelled advisory key does NOT suppress gap", gaps_typo == [("A.py", "")])
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "Z"\ncmd = "bash scripts/C.py --gate"\nexpect = 0\nkind = "light"\n'
                'ci_exemptX = "typo key must not silence MISSING"\n')
    _g, _nb, _bl, miss_typo = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-15 misspelled exempt key does NOT suppress MISSING",
           miss_typo == [("C.py", "--gate")])
    # 清单缺失 → check 返回 None（不可判定）
    expect("selftest-5 missing manifest -> None", check(os.path.join(tmp, "nope.toml"), wfdir) is None)
    # 清单驱动执行面（TC-C）：驱动器阻塞执行 ⇒ 视作覆盖；COE 下执行 ⇒ 仍记 gap；
    # --selftest-only ⇒ 什么也不算。这三条是"扩展不放松判据"的证据。
    with open(man, "w", encoding="utf-8") as f:
        f.write('[[gate]]\nid = "Y"\ncmd = "bash scripts/B.py"\nexpect = 0\nkind = "light"\n')
    with open(os.path.join(wfdir, "t.yml"), "w") as f:
        f.write("jobs:\n  g:\n    steps:\n"
                "      - run: |\n          python3 scripts/run_gate_manifest.py --ci --jobs 4\n")
    _g, _nb, _bl, miss_drv = check(man, os.path.join(wfdir, "*.yml"),
                                   {("B.py", "")}, True, False)
    expect("selftest-16 driver-blocking covers a light gate (no MISSING)", miss_drv == [])
    _g2, _nb2, _bl2, miss_drv_off = check(man, os.path.join(wfdir, "*.yml"))
    expect("selftest-17 driver coverage is opt-in, not silent",
           miss_drv_off == [("B.py", "")])
    gaps_drv, _nb3, _bl3, _miss3 = check(man, os.path.join(wfdir, "*.yml"),
                                         {("B.py", "")}, False, True)
    expect("selftest-18 driver under continue-on-error is still a GAP",
           gaps_drv == [("B.py", "")])
    # 入口契约（F35）：未知旗标必须"没跑任何判据"并退 2，不能当作"没给旗标"而跑真对拍
    expect("selftest-10 unknown flag is undecidable, no check ran", main(["--nope"]) == 2)
    import shutil
    shutil.rmtree(tmp, ignore_errors=True)
    print("=== selftest result:", "ALL PASS" if not fails else f"{len(fails)} FAIL", "===")
    return 0 if not fails else 1


def mode_check():
    covered = driver_covered(MANIFEST)
    drv_block, drv_nonblock = driver_contexts(WORKFLOWS_GLOB)
    res = check(MANIFEST, WORKFLOWS_GLOB, covered, drv_block, drv_nonblock)
    if res is None:
        print("[UNDECIDABLE] gate_manifest not readable —— 未执行任何对拍")
        return 2
    gaps, nonblock, blocking, missing = res
    print(f"[info] expect=0 gate scripts: {len(blocking)}; "
          f"non-blocking-context scripts in CI: {len(nonblock)}")
    print(f"[info] manifest driver (run_gate_manifest.py): "
          f"blocking={drv_block} nonblocking={drv_nonblock} covered={len(covered)}")
    if gaps:
        print(f"[FAIL] {len(gaps)} gate(s) the closure list requires-passing run under continue-on-error in CI:")
        for g in gaps:
            print(f"   - {g}  (in {sorted(nonblock[g])})")
        return 1
    if missing:
        print(f"[FAIL] {len(missing)} expect=0 gate(s) are never invoked by any workflow:")
        for m in missing:
            print(f"   - {m}")
        print("        a gate registered with expect=0 but wired into no workflow can never")
        print("        fail CI, so it protects nothing. Fix: add a blocking step in some")
        print('        workflow, or, if it is deliberately local-only, declare')
        print('        ci_exempt = "<reason>" in gate_manifest.toml.')
        print('        If it IS wired but intentionally non-blocking (advisory perf probe),')
        print('        declare ci_advisory = "<reason>" instead: the gate stays visible in')
        print('        the ledger without pretending it can block a release.')
        return 1
    print("[OK] no parity gap: every closure-required gate runs blocking in CI")
    print("     and is actually wired into at least one workflow")
    return 0


def main(argv):
    # F35（F24/F29 同族）：除 --selftest 外的一切参数都必须被拒，而不是"当作没给"
    # 顺手跑一次真对拍 —— 拼错旗标的人要的从来不是那份判定。
    unknown = sorted({a for a in argv if a != "--selftest"})
    if unknown:
        print("usage: py -3 scripts/audit_gate_parity.py [--selftest]", file=sys.stderr)
        print("[UNDECIDABLE] unknown argument(s): %s -- no check ran"
              % " ".join(unknown), file=sys.stderr)
        return 2
    if "--selftest" in argv:
        return mode_selftest()
    return mode_check()


if __name__ == "__main__":
    import gate_rc  # 只在入口需要：崩溃必须退 2, 不得借 1 冒充"判过且红"（F32/F33）
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
