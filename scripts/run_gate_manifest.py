#!/usr/bin/env python3
"""按 scripts/gate_manifest.toml 跑收口判据（单一真值源的执行器）。

设计约束（都是本波次踩出来的）：
  * **退出码从子进程直接取**，不经管道 —— 上轮我用 `| Select-Object -First` 读到过失真的 0。
  * 每条判据必须同时有 `spec_ref` 与 `note`，否则视为清单失真 → 退 2（不可判定），**不判通过**。
  * heavy 项跑前查磁盘；不足则记 UNKNOWN 并退 2，绝不静默跳过（静默跳过 = 假绿，RK-P30 同族）。
  * 期望值可为 1（"按设计为红"，如 T2 前的耦合审计、T6 heredoc 未退役）；此时 rc==1 才算符合。

用法：
    py -3 scripts/run_gate_manifest.py              # 只跑 light（默认）
    py -3 scripts/run_gate_manifest.py --with-heavy # 连 heavy 一起（受磁盘门槛约束）
    py -3 scripts/run_gate_manifest.py --list       # 只列清单不执行
退出码：0 全部符合期望 / 1 有不符合 / 2 清单失真或资源不足（不可判定）
"""
import hashlib
import io
import os
import re
import shlex
import shutil
import subprocess
import sys
import tomllib

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MANIFEST = os.path.join(ROOT, "scripts", "gate_manifest.toml")
LOGDIR = os.path.join(ROOT, "tmp", "gate_logs")
REQUIRED = ("id", "spec_ref", "cmd", "expect", "kind", "note")
CR_BYTE = b"\x0d"


def load():
    with open(MANIFEST, "rb") as f:
        doc = tomllib.load(f)
    gates = doc.get("gate", [])
    problems = []
    if not gates:
        problems.append("清单里没有任何 [[gate]] 条目")
    for g in gates:
        missing = [k for k in REQUIRED if k not in g]
        if missing:
            problems.append(f"{g.get('id', '?')}: 缺字段 {missing}")
        if g.get("kind") not in ("light", "heavy"):
            problems.append(f"{g.get('id', '?')}: kind 必须是 light|heavy，实为 {g.get('kind')!r}")
    ids = [g.get("id") for g in gates]
    if len(ids) != len(set(ids)):
        problems.append("存在重复 id")
    return gates, problems


def disk_free_gb():
    return shutil.disk_usage(ROOT).free / 1024 ** 3


def augment_path():
    """把项目内工具链加入本进程 PATH（否则 bare shell 下 cargo/bash 根本找不到）。

    清单要能"从干净 shell 直接跑"才有意义；否则每次执行前都得手工设 env，
    就又回到了"跑法只存在于某人脑子里"的多源分裂。
    """
    extra = [os.path.join(ROOT, ".toolchain", "cargo", "bin"),
             os.path.join(ROOT, ".toolchain", "rustup")]
    mingw = r"D:\msys64\mingw64\bin"
    if os.path.isdir(mingw):
        extra.append(mingw)
    cur = os.environ.get("PATH", "")
    add = [p for p in extra if os.path.isdir(p) and p not in cur.split(os.pathsep)]
    if add:
        os.environ["PATH"] = os.pathsep.join(add + [cur])
    return add


# 只有 shell/加载器本身能吐出的消息：出现即证明判据没执行，无需再看形态。
UNCONDITIONAL_ENV_SIGS = (
    "Bad interpreter",            # shebang 失效（BOM / 空壳解释器路径）
    "command not found",          # 依赖的外部命令缺失
    "cannot execute binary file",
)
# 既可能是环境故障、也可能出现在门的真判定正文里的消息：必须再过形态校验。
AMBIGUOUS_ENV_SIGS = (
    "invalid option name",        # 非 GNU bash 的 sh 会拒绝 `set -o pipefail`
    "No such file or directory",  # 脚本自身缺失 or 门报"某证据文件不存在"
    "SyntaxError",                # 被调脚本没编译过
    "Traceback (most recent call last)",  # 子进程崩溃
)
ENV_FAILURE_SIGNATURES = UNCONDITIONAL_ENV_SIGS + AMBIGUOUS_ENV_SIGS
# shell 解释器报错的形态（`<path>: line <N>: ...`）；用于把 `.sh` 判据的误判风险压到零。
SHELL_ERROR_RE = re.compile(r"^.+: line \d+: ")


def env_failure_reason(log_text, cmd):
    """判据"根本没执行"与"执行了并给出红判定"的区分。

    WHY 只看前 5 行：解释器层报错必然先于门的任何业务输出出现；扫全文会让
    "门正常报出某文件不存在"这类**真判定**被误读成环境故障（本仓库 G-11/G-31 这类
    审计门的正文就可能出现类似字样）——那等于把真红藏起来，比原病更糟。
    WHY 分两级：`Bad interpreter` 之类只有 shell/加载器能产出，直接采信；
    而 `No such file or directory` 之类的歧义消息，对 `bash`/`sh` 判据还要求首行是
    `<path>: line N: ` 形状。宁可漏分类（仍显示 MISMATCH 供人复核），不可误分类（藏真红）。

    背景：2026-09-21 两次清单跑各以 `set: pipefail` + 换行 + `: invalid option name`
    令 5 条 `.sh` 判据退 rc=2，而同命令同解释器（自报为 Git bash 5.3.15）前台直跑正常；
    CRLF 与 POSIXLY_CORRECT 两个假设均被实测否证，机制未定位（见报告 §45）。
    """
    lines = [l for l in log_text.splitlines() if l.strip()]
    if not lines:
        return None
    head = "\n".join(lines[:5])
    first = lines[0]
    for sig in UNCONDITIONAL_ENV_SIGS:
        if sig in head:
            return sig
    sig = next((s for s in AMBIGUOUS_ENV_SIGS if s in head), None)
    if sig is None:
        return None
    argv0 = cmd.strip().split()[:1]
    if argv0 and argv0[0] in ("bash", "sh") and not SHELL_ERROR_RE.match(first):
        return None
    return sig


def classify(rc, expect, log_text, cmd):
    """一条门的三分判决: env / ok / mismatch。

    ★ 顺序本身就是判据（F33）：环境故障必须先于 `rc == expect` 判。
    `expect = 1` 的门若在解释器层崩掉（SyntaxError、导入失败），rc 恰好等于期望值,
    于是"根本没给出判定"会被记成"符合期望" —— 台账上是一次**静默假绿**。
    进程内守卫（gate_rc）救不到这一类：语法错误发生在解析期, 守卫代码还没被执行到。
    """
    sig = env_failure_reason(log_text, cmd)
    if sig:
        return "env", sig
    return ("ok", None) if rc == expect else ("mismatch", None)


KNOWN_FLAGS = ("--selftest", "--list", "--with-heavy")
USAGE = ("用法: py -3 scripts/run_gate_manifest.py [--list] [--selftest] [--with-heavy]\n"
         "  无参数 = 跑全部 light 判据（heavy 需显式 --with-heavy）\n"
         "  ⚠ 未知参数不再静默降级为「跑全部」，而是退 2（不可判定）")


def unknown_flags(argv):
    """返回本工具不认识的参数。空列表才算"调用方式合法"。"""
    return [a for a in argv if a not in KNOWN_FLAGS]


def is_launcher_absent(err: OSError) -> bool:
    """区分"执行器本身不存在"(判据从未运行) 与"启动时出了别的 OS 错"(仍按不符记账)。

    WHY: `pwsh` 未安装的宿主上, `.ps1` 判据根本没跑; 把它记进"与期望不符"红名单,
    读清单的人会以为 Windows 侧判出了红 —— 与 F15 同一类病, 只是这次发生在启动前。
    保守取向沿用 `env_failure_reason` 的纪律: 只认 FileNotFoundError,
    其它 OSError(权限/资源) 宁可留在红名单里供人复核, 不可误分类藏真红。
    """
    return isinstance(err, FileNotFoundError)


def mode_selftest():
    """分类器自身的取证：漏分类只是少个标签（仍显示 MISMATCH），误分类会**藏真红**。

    后者的代价高得多，故负例与正例同等重要——本项目整批治理的就是"看起来绿"。
    """
    cases = [
        # (名称, 日志文本, 命令, 期望是否判为环境故障)
        ("ghost-pipefail-two-line",
         "scripts/check_doc_drift.sh: line 40: set: pipefail\n: invalid option name\n", "bash s.sh", True),
        ("ghost-bad-interpreter", "/bin/sh: Bad interpreter.\n", "sh s.sh", True),
        ("py-crash-traceback", "Traceback (most recent call last):\n  File \"x.py\", line 1\n",
         "{python} x.py", True),
        ("real-finding-nosuchfile-midbody",
         "[GAP-A] a -> b violates rule\nreferenced path: No such file or directory (context only)\n",
         "bash s.sh", False),
        ("real-verdict-chinese-undecidable",
         "!! 不可判定：找不到可执行 pwsh\n", "{python} x.py", False),
        ("real-verdict-normal-fail",
         "[FAIL] three-way reconciliation found gaps\n", "bash s.sh", False),
        ("shell-sig-without-error-shape",
         "note: invalid option name is not accepted by this tool\n[FAIL] 2 gaps\n", "bash s.sh", False),
        ("empty-log", "\n\n", "bash s.sh", False),
    ]
    fails = 0
    for name, text, cmd, want in cases:
        got = env_failure_reason(text, cmd) is not None
        ok = got == want
        fails += 0 if ok else 1
        print(f"  [{'PASS' if ok else 'FAIL'}] selftest-{name} (want_env={want} got_env={got})")
    launcher_cases = [
        # (名称, 启动异常, 期望是否判为环境故障)
        ("launcher-absent-pwsh", FileNotFoundError(2, "No such file or directory: 'pwsh'"), True),
        ("launcher-permission-denied", PermissionError(13, "Permission denied: 'bash'"), False),
        ("launcher-oserror-other", OSError(22, "Invalid argument"), False),
    ]
    for name, err, want in launcher_cases:
        got = is_launcher_absent(err)
        ok = got == want
        fails += 0 if ok else 1
        print(f"  [{'PASS' if ok else 'FAIL'}] selftest-{name} (want_env={want} got_env={got})")
    # F24: 参数校验——未知旗标必须被列出来（否则会被静默当成"跑全部"）
    flag_cases = [
        ("flags-empty-ok", [], 0),
        ("flags-known-combo", ["--list", "--with-heavy", "--selftest"], 0),
        ("flags-help-unknown", ["--help"], 1),
        ("flags-typo", ["--lit"], 1),
        ("flags-positional", ["gate"], 1),
    ]
    for name, argv_in, want_unknown in flag_cases:
        got = len(unknown_flags(argv_in))
        ok = got == want_unknown
        fails += 0 if ok else 1
        print(f"  [{'PASS' if ok else 'FAIL'}] selftest-{name} (want_unknown={want_unknown} got={got})")
    # F33: 判决的**顺序**也要有夹具 —— 只看分类器有牙不够, expect=1 的门崩掉时必须
    # 落进 env 而不是 ok, 否则台账上是一次静默假绿（这条正是本轮实测到的形状）。
    verdict_cases = [
        # (名称, rc, expect, 日志, 期望判决)
        ("expect1-crash-is-not-ok", 1, 1,
         "Traceback (most recent call last):\nSyntaxError: 'ascii' codec can't decode\n", "env"),
        ("expect1-real-red-is-ok", 1, 1,
         "[GAP] two sources disagree\n[FAIL] 2 gaps\n", "ok"),
        ("expect0-green-is-ok", 0, 0, "[OK] all gates as expected\n", "ok"),
        ("expect0-undecidable-is-mismatch", 2, 0,
         "[UNDECIDABLE] missing data file: scripts/x.toml\n", "mismatch"),
    ]
    for name, rc, exp, text, want in verdict_cases:
        got = classify(rc, exp, text, "{python} x.py")[0]
        ok = got == want
        fails += 0 if ok else 1
        print(f"  [{'PASS' if ok else 'FAIL'}] selftest-{name} (want={want} got={got})")
    total = len(cases) + len(launcher_cases) + len(flag_cases) + len(verdict_cases)
    print(f"  RESULT: {'FAIL' if fails else 'PASS'} ({fails}/{total} assertion(s) violated)")
    return 1 if fails else 0


def subject_state(cmd):
    """命中环境故障时,把被调脚本的字节实况钉进日志——让下一次复现自带诊断。

    WHY：同一命令在清单内 3/3 失败、清单外 0/4 正常, 而两侧解释器同一路径
    （Git bash 5.3.15）; CRLF 与 POSIXLY_CORRECT 两个假设都已被实测否证。
    与其继续猜, 不如在命中瞬间抓取现场（size/mtime/sha1/CR 数/`set` 行原始字节），
    把"不可复现"变成"每次都留一份可比对的物证"。
    """
    try:
        parts = shlex.split(cmd, posix=True)
    except ValueError:
        parts = cmd.split()
    path = next((p for p in parts[1:] if "/" in p and not p.startswith("-")), None)
    if not path or not os.path.isfile(path):
        return f"被调对象不可定位：{cmd}"
    raw = open(path, "rb").read()
    st = os.stat(path)
    setraw = next((l for l in raw.split(b"\n") if b"set " in l and b"pipefail" in l), None)
    return (f"被调={path} size={st.st_size} mtime={int(st.st_mtime)} "
            f"sha1={hashlib.sha1(raw).hexdigest()[:8]} CR={raw.count(CR_BYTE)} "
            f"set行原始字节={setraw!r}")


def usable_bash():
    """挑一个"看得懂 Windows 路径、会剥 CRLF 行尾"的 GNU bash，返回 (路径, 说明)。

    返回 (None, 原因) 表示本机没有可用 bash —— 调用方必须据此把 `.sh` 判据记为
    环境故障，而不是让它们各自报解释器错、再被误读成"门红了"。

    WHY 显式候选表：Windows 上 PATH 顺序不受本工具控制（后台任务环境里 `bash` 可能解析成
    WSL 的 `/bin/bash`：Linux 构建看不懂 `D:/...`，也**不会剥行尾 \\r** ⇒ CRLF 的 .sh 判据
    全部报 `invalid option name`、rc=2）。2026-09-21 实测中招 5 条判据，且中招的恰好只有
    `w/crlf` 的三个文件 —— 即"错误解释器 × CRLF 工作树"两个因素同时成立才会坏。
    """
    candidates = []
    found = shutil.which("bash")
    if found:
        candidates.append(found)
    if os.name == "nt":
        candidates += [r"C:\Program Files\Git\usr\bin\bash.exe", r"D:\msys64\usr\bin\bash.exe"]
    rejected = []
    for path in candidates:
        if not path or not os.path.isfile(path):
            continue
        try:
            out = subprocess.run([path, "--version"], capture_output=True, text=True, timeout=20).stdout
        except Exception as e:
            rejected.append(f"{path}: {type(e).__name__}")
            continue
        first = (out.splitlines() or [""])[0].strip()
        if not first.startswith("GNU bash"):
            rejected.append(f"{path}: 非 GNU bash（{first or '无输出'}）")
            continue
        if os.name == "nt" and "linux" in first.lower():
            # Linux 构建（WSL）在 Windows 宿主上不可用；继续找下一个候选
            rejected.append(f"{path}: {first}")
            continue
        return path, first
    return None, "; ".join(rejected) or "无可用 bash"


def main(argv):
    os.chdir(ROOT)
    bad_flags = unknown_flags(argv)
    if bad_flags:
        # F24: 旧行为是"未知旗标被忽略 ⇒ 静默跑完整清单"（`--help` 实测触发过一次全量扫描）。
        # 误用调用方式既不是通过也不是判据红，按本仓方言记 **2 = 不可判定**，并当场给用法。
        print(USAGE)
        print(f"[UNDECIDABLE] 未知参数: {' '.join(bad_flags)} —— 未执行任何判据，"
              f"不把拼错的旗标降级成「跑全部」")
        return 2
    if "--selftest" in argv:
        return mode_selftest()
    list_only = "--list" in argv
    with_heavy = "--with-heavy" in argv
    try:
        gates, problems = load()
    except Exception as e:
        # 工具自身报错不能伪装成“某条判据不符”：本函数初版就是 load() 抛异常后以 rc=1 退出，
        # 与“判据失败”同码，会把人引向去改仓库而不是去修工具。
        print(f"[TOOL-ERROR] 清单加载失败（不可判定，不等于判据通过）：{type(e).__name__}: {e}")
        return 2
    # 先校验每条判据引用的脚本存在。否则会出现“死判据”：如 kind=heavy 的条目在 light 模式下
    # 永远不跑，即使目标脚本根本不存在也不会暴露（本工具实测踩过）。
    dead = []
    for g in gates:
        for tok in str(g.get("cmd", "")).split():
            if "/" in tok and tok.endswith((".py", ".sh", ".ps1")) and not os.path.isfile(tok):
                dead.append(f"{g.get('id')}: 被引用脚本不存在 -> {tok}")
    if dead:
        print("[MANIFEST-INVALID] 判据指向不存在的脚本（这些判据不可执行，不得当作已覆盖）：")
        for d in dead:
            print("   ", d)
        return 2
    if problems:
        print("[MANIFEST-INVALID] 清单失真，判不可判定（不视为通过）：")
        for p in problems:
            print("   ", p)
        return 2

    free = disk_free_gb()
    added = augment_path()
    print(f"清单 {len(gates)} 条判据；D 盘空闲 {free:.1f} GiB；mode="
          + ("list" if list_only else ("light+heavy" if with_heavy else "light")))
    if added:
        print(f"（已补 PATH：{', '.join(os.path.basename(p) or p for p in added)}）")
    bash_path, bash_note = usable_bash()
    print(f"（bash={bash_path or '无可用'} :: {bash_note}）")
    print("-" * 96)
    if list_only:
        for g in gates:
            print(f"  [{g['id']}] {g['kind']:5} expect={g['expect']} spec§{g['spec_ref']:26} {g['cmd']}")
        return 0

    if not os.path.isdir(LOGDIR):
        os.makedirs(LOGDIR)

    bad, unknown, env_fail = [], [], []
    for g in gates:
        heavy = g["kind"] == "heavy"
        if heavy and not with_heavy:
            print(f"  [{g['id']}] SKIP  heavy 未选择执行（--with-heavy 才跑）")
            continue
        need = g.get("disk_gb", 0)
        if heavy and free < need:
            print(f"  [{g['id']}] UNKNOWN 磁盘 {free:.1f} GB < 门槛 {need} GB —— 不静默跳过，判不可判定")
            unknown.append(g["id"])
            continue
        cmd = g["cmd"]
        # 必须**先分词再换解释器**：把 sys.executable（Windows 路径带反斜杠）先填进去再
        # shlex.split(posix=True) 会把 `\U`、`\A` 当转义剥掉 → 得到 `C:Users...python.exe`，
        # 表现为 6 条判据“命令不存在”（本工具首跑实测）。这是脚本化才抓得到的坑。
        args = shlex.split(cmd, posix=True)
        if args and args[0] == "{python}":
            args = [sys.executable] + args[1:]
        if args and args[0] in ("bash", "sh"):
            if not bash_path:
                # 没有可用 bash ⇒ 根本不启动子进程：与其让 .sh 判据集体吐解释器错、
                # 再被读成"这些门都红了"，不如一次性说明执行器不可用。
                print(f"  [{g['id']}] ENV-FAIL (预检未执行)  {g['cmd']}")
                print(f"        | 无可用 GNU bash：{bash_note}")
                env_fail.append((g["id"], "preflight:no-usable-bash"))
                continue
            args[0] = bash_path  # 钉死绝对路径：不让 PATH 顺序把 WSL bash 塞进来
        log = os.path.join(LOGDIR, f"{g['id']}.log")
        try:
            with open(log, "wb") as lf:
                rc = subprocess.run(args, stdout=lf, stderr=subprocess.STDOUT).returncode
        except OSError as e:
            if is_launcher_absent(e):
                # 与 G-17(xdoc_precise)/G-18(audit_gate_selfreport) 同口径: 执行器缺失是
                # "判据未运行"⇒ 不可判定, 不得进红名单(否则宿主缺工具会顶掉一条真红的可见度)
                print(f"  [{g['id']}] ENV-FAIL (执行器缺失)  {g['cmd']}")
                print(f"        | 找不到可执行 {args[0]}；不视为通过，也不视为红。"
                      f"装该执行器或改用同判据的另一侧口径后重跑。")
                env_fail.append((g["id"], "preflight:launcher-absent"))
            else:
                print(f"  [{g['id']}] FAIL  启动失败: {type(e).__name__}: {e}")
                bad.append((g["id"], f"launch-error:{type(e).__name__}"))
            continue
        log_text = io.open(log, encoding="utf-8", errors="replace").read()
        # F33: 先分环境故障再比 expect —— 旧顺序把 `ok` 判在前面, 于是 expect=1 的门
        # 一旦崩在解释器层（rc 恰好 1）就被记成"符合期望", 台账上是一次静默假绿。
        verdict, sig = classify(rc, g["expect"], log_text, cmd)
        if verdict == "env":
            # 判据根本没执行 ⇒ 既不得计入"符合期望"，也不得计入"判据失败"；单列后整体 rc=2。
            print(f"  [{g['id']}] ENV-FAIL rc={rc} expect={g['expect']}  {g['cmd']}")
            print(f"        | 运行器/环境故障（签名 {sig!r}）—— 不视为该门给出判定")
            print("        | " + subject_state(cmd))
            env_fail.append((g["id"], f"rc={rc} env:{sig}"))
            continue
        if verdict == "ok":
            print(f"  [{g['id']}] OK      rc={rc} expect={g['expect']}  {g['cmd']}")
            continue
        print(f"  [{g['id']}] MISMATCH rc={rc} expect={g['expect']}  {g['cmd']}")
        bad.append((g["id"], f"rc={rc}!={g['expect']}"))
        for line in log_text.splitlines()[-4:]:
            print("        |", line.strip()[:118])

    print("-" * 96)
    if unknown:
        print(f"[UNKNOWN] 资源不足未能判定：{', '.join(unknown)} —— 不得当作通过")
    if env_fail:
        print(f"[ENV-FAIL] 判据未真正执行 {len(env_fail)} 条："
              + "; ".join(f"{i}({w})" for i, w in env_fail))
        print("           ⇒ 这些门的**状态未知**：修运行器/环境后重跑，不得写进红名单也不得写进绿名单")
    if bad:
        print(f"[FAIL] 与期望不符 {len(bad)} 条：" + "; ".join(f"{i}({w})" for i, w in bad))
        return 1
    if unknown or env_fail:
        return 2
    print("[OK] 全部判据符合期望")
    return 0


if __name__ == "__main__":
    import gate_rc  # 只在入口需要：崩溃必须退 2, 不得借 1 冒充"判过且红"（F32/F33）
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
