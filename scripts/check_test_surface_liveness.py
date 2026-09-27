#!/usr/bin/env python3
"""「注册了但不跑」的测试面活性门 —— 与 #[ignore] 归属登记表同族的新态。

判据三态（每条发现都必须落 `scripts/test_surface_freeze.txt`，否则 [FAIL]）：
  MUTED          某个 [[test]] target 的源文件带 `#![cfg(any())]`（整 target 编译期屏蔽）
  NEVER_COMPILED tests/ 下的 .rs 既不是任何 target 的 path，也不被任何已编译文件的 `mod` 声明到达
  STALE          登记表指向的对象已不在盘（清单自身要防腐，与 ignore 登记表同规矩）

退出码：0 全部已登记 / 1 有未登记或陈旧条目 / 2 不可判定（解析根数为 0、清单缺失、崩溃）

WHY 需要它：批次 A+B 删 auto-dpo 时用 `#![cfg(any())]` 把 rhi_cg_validation 整个 target 屏蔽，
  `cargo test --workspace` 照绿、target 计数照算，25 个测试静默归零；它唯一依赖的夹具文件因此
  也失去 `mod` 入口而彻底不被编译（再多 7 个）。这类"负覆盖"没有任何门在检，且 `#[ignore]`
  登记表覆盖不到它（作用域是 crates/**，且机制不是 #[ignore]）。
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tomllib

import gate_rc  # 崩溃经 excepthook 退 2，不借默认 1
gate_rc.install()

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

FREEZE = "scripts/test_surface_freeze.txt"
# 整 target 屏蔽的惯用写法：恒假 cfg。`cfg(all())` 是恒真，不算屏蔽。
MUTED_RE = re.compile(r"^\s*#!\[cfg\(\s*any\(\)\s*\)\s*\]", re.M)
MOD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.M)
PATH_ATTR_RE = re.compile(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]\s*\n\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;')
TESTFN_RE = re.compile(r"^\s*#\[(?:tokio::)?test\]", re.M)


def read(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return None


def posix(p):
    return str(p).replace("\\", "/")


def parse_targets(root):
    """返回 {target_name: src_path}，含 Cargo 自动发现的 tests/*.rs 顶层文件。"""
    man_path = os.path.join(root, "Cargo.toml")
    text = read(man_path)
    if text is None:
        return None
    man = tomllib.loads(text)
    targets = {t.get("name", "?"): posix(t["path"]).removeprefix("./")
               for t in man.get("test", []) if "path" in t}
    tests_dir = os.path.join(root, "tests")
    if os.path.isdir(tests_dir):
        declared = set(targets.values())
        for name in sorted(os.listdir(tests_dir)):
            if not name.endswith(".rs"):
                continue
            # 两端一律 root 相对 POSIX：绝对/相对混用会让 files-compiled 差集把
            # 「已注册但写成绝对路径」的文件全判成从未编译（首版即中招，40 条假缺陷）
            rel = f"tests/{posix(name)}"
            if rel not in declared:
                targets[name[:-3]] = rel
    return targets


def module_closure(root, targets):
    """从各 target 入口出发，按 mod / #[path] 声明求"会被编译"的文件闭包（root 相对 POSIX）。"""
    compiled, queue = set(targets.values()), list(targets.values())
    while queue:
        cur = queue.pop()
        body = read(os.path.join(root, cur))
        if body is None:
            continue
        base = posix(os.path.dirname(cur))
        mods = [f"{base}/{m}.rs" for m in MOD_RE.findall(body)]
        mods += [f"{base}/{m}/mod.rs" for m in MOD_RE.findall(body)]
        for attr in PATH_ATTR_RE.findall(body):
            mods.append(posix(f"{base}/{attr}"))
        for rel in mods:
            norm = posix(os.path.normpath(rel))
            if norm not in compiled and os.path.isfile(os.path.join(root, norm)):
                compiled.add(norm)
                queue.append(norm)
    return compiled


def scan(root):
    targets = parse_targets(root)
    if targets is None:
        print("[UNKNOWN] 找不到根 Cargo.toml")
        return None
    if len(targets) < 30:
        # 规模闸门：解析面塌陷时"全部干净"是假绿（本仓实测量级 = 38 target）
        print(f"[UNKNOWN] 解析到 target 仅 {len(targets)} 个（<30），疑似扫描面塌陷")
        return None
    files = {posix(os.path.relpath(os.path.join(dp, f), root))
             for dp, _, fs in os.walk(os.path.join(root, "tests")) for f in fs if f.endswith(".rs")}
    compiled = module_closure(root, targets)

    findings = []
    for name, path in sorted(targets.items()):
        body = read(os.path.join(root, path))
        if body and MUTED_RE.search(body):
            findings.append(("MUTED", path, f"target={name} 屏蔽测试 {len(TESTFN_RE.findall(body))} 个"))
    for f in sorted(files - compiled):
        body = read(os.path.join(root, f)) or ""
        findings.append(("NEVER_COMPILED", f, f"内含测试 {len(TESTFN_RE.findall(body))} 个"))
    return findings, len(targets), len(files)


def load_freeze(root):
    text = read(os.path.join(root, FREEZE))
    if text is None:
        print(f"[UNKNOWN] 登记表缺失：{FREEZE}")
        return None
    reg = {}
    for line in text.splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        parts = s.split("|")
        if len(parts) >= 3:
            reg[(parts[0], parts[1])] = parts[2]
    return reg


def judge(root):
    scanned = scan(root)
    if scanned is None:
        return 2
    findings, ntargets, nfiles = scanned
    reg = load_freeze(root)
    if reg is None:
        return 2
    new = [(k, p, d) for (k, p, d) in findings if (k, p) not in reg]
    stale = [key for key in reg if key not in {(k, p) for (k, p, _) in findings}]
    print(f"targets={ntargets} tests/*.rs={nfiles} 发现={len(findings)} 已登记={len(reg)}")
    for k, p, d in new:
        print(f"  [UNREGISTERED] {k} {p} —— {d}")
    for k, p in stale:
        print(f"  [STALE] {k} {p} —— 登记表指向的对象已不在盘")
    if new or stale:
        print("RESULT: FAIL（「注册了但不跑」必须是被登记的决策，或恢复编译）")
        return 1
    print("RESULT: PASS（每条发现均已登记）")
    return 0


def selftest():
    """植入式正对照：三类判定必须各自「会触发」，且干净夹具必须放过。"""
    import tempfile
    tmp = tempfile.mkdtemp(prefix="tsl_")
    os.makedirs(os.path.join(tmp, "tests", "sub"))
    os.makedirs(os.path.join(tmp, "scripts"))
    def w(rel, body):
        with open(os.path.join(tmp, rel), "w", encoding="utf-8") as fh:
            fh.write(body)

    entries = []
    for i in range(40):  # 过规模闸门
        entries.append(f'[[test]]\nname = "t{i}"\npath = "tests/t{i}.rs"\n')
        w(f"tests/t{i}.rs", "fn main() {}\n")
    w("tests/mutated.rs", "#![cfg(any())]\n#[test]\nfn a() {}\n")
    entries.append('[[test]]\nname = "mutated"\npath = "tests/mutated.rs"\n')
    w("tests/sub/orphan_dead.rs", "#[test]\nfn never_compiled() {}\n")
    w("Cargo.toml", "\n".join(entries))
    w(FREEZE, "")
    rc = judge(tmp)
    out = []
    findings, _, _ = scan(tmp)
    kinds = {(k, p) for k, p, _ in findings}
    out.append(("plant-MUTED-caught", ("MUTED", "tests/mutated.rs") in kinds))
    out.append(("plant-NEVER_COMPILED-caught", ("NEVER_COMPILED", "tests/sub/orphan_dead.rs") in kinds))
    out.append(("unregistered-fires", rc == 1))

    # 登记后必须转绿
    w(FREEZE, "MUTED|tests/mutated.rs|fixture\nNEVER_COMPILED|tests/sub/orphan_dead.rs|fixture\n")
    out.append(("registered-suppresses", judge(tmp) == 0))
    # 陈旧登记必须红
    w(FREEZE, "MUTED|tests/gone.rs|stale entry\n")
    out.append(("stale-fires", judge(tmp) == 1))
    # 规模闸门：另起一个小 root（tmp 里 41 个自动发现 target 会把闸门撑过去，
    # 首版正是把这条断言写在 tmp 上才让它在仪器有 bug 时"意外通过"）
    small = tempfile.mkdtemp(prefix="tsl_small_")
    os.makedirs(os.path.join(small, "tests"))
    with open(os.path.join(small, "Cargo.toml"), "w", encoding="utf-8") as fh:
        fh.write('[[test]]\nname = "only"\npath = "tests/t0.rs"\n')
    with open(os.path.join(small, "tests", "t0.rs"), "w", encoding="utf-8") as fh:
        fh.write("fn main() {}\n")
    os.makedirs(os.path.join(small, "scripts"))
    with open(os.path.join(small, FREEZE), "w", encoding="utf-8") as fh:
        fh.write("")
    out.append(("scale-abort", judge(small) == 2))
    # 真实树：当前必须 PASS（否则本门的自测与存量不一致）
    out.append(("real-tree-holds", judge(".") == 0))

    bad = [n for n, ok in out if not ok]
    for n, ok in out:
        print(f"  selftest {n}: {'ok' if ok else 'FAILED'}")
    held = len(out) - len(bad)
    print(f"RESULT: {'PASS' if not bad else 'FAIL'} ({held}/{len(out)} assertions held"
          + (f", failed: {', '.join(bad)}" if bad else "") + ")")
    return 1 if bad else 0


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ns = ap.parse_args()
    sys.exit(selftest() if ns.selftest else judge("."))
