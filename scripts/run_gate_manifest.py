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
import io
import os
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


def main(argv):
    os.chdir(ROOT)
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
    print("-" * 96)
    if list_only:
        for g in gates:
            print(f"  [{g['id']}] {g['kind']:5} expect={g['expect']} spec§{g['spec_ref']:26} {g['cmd']}")
        return 0

    if not os.path.isdir(LOGDIR):
        os.makedirs(LOGDIR)

    bad, unknown = [], []
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
        log = os.path.join(LOGDIR, f"{g['id']}.log")
        try:
            with open(log, "wb") as lf:
                rc = subprocess.run(args, stdout=lf, stderr=subprocess.STDOUT).returncode
        except FileNotFoundError:
            print(f"  [{g['id']}] FAIL  命令不存在: {args[0]}")
            bad.append((g["id"], "binary-missing"))
            continue
        ok = rc == g["expect"]
        print(f"  [{g['id']}] {'OK     ' if ok else 'MISMATCH'} rc={rc} expect={g['expect']}  {g['cmd']}")
        if not ok:
            bad.append((g["id"], f"rc={rc}!={g['expect']}"))
            tail = io.open(log, encoding="utf-8", errors="replace").read().splitlines()[-4:]
            for l in tail:
                print("        |", l.strip()[:118])

    print("-" * 96)
    if unknown:
        print(f"[UNKNOWN] 资源不足未能判定：{', '.join(unknown)} —— 不得当作通过")
    if bad:
        print(f"[FAIL] 与期望不符 {len(bad)} 条：" + "; ".join(f"{i}({w})" for i, w in bad))
        return 1
    if unknown:
        return 2
    print("[OK] 全部判据符合期望")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
