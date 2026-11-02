#!/usr/bin/env python3
# =============================================================================
# check_certify_whitelist.py - "Certify 白名单治理门" (v2.3 §(b.1)③)
# =============================================================================
# Purpose: 裁决权外置要求 agent 只能 Claim,"验证成功"(Certify)必须由可信验证器
#          签发。v2.3 §(b.1) 修正② 记录:编译期写隔离不可直接落地(publish_critical
#          / publish_certify 均为公共 API 无 ACL,见 bus.rs),故一期改为可落地形态:
#          **API 语义分离 + 白名单治理门 + 测试**。本门即其中的静态门:
#          扫描 crates/**/src 生产作用域内对 Certify 签发面(`CertifyIssued` 构造 /
#          `publish_certify` 调用)的引用,只允许白名单验证器 crate 出现;越权即红。
#
# 白名单默认仅 `event-bus`(定义/注册/穷举夹具所在)。新增验证器 crate(如未来
# formal/异构审查签发方)须在此登记并附证据。测试作用域(crates/*/tests、#[cfg(test)])
# 与穷举夹具不算签发方。
#
# ★ 扫描域判据的教训(2026-10-08 / v3.5 W6-1):旧实现用 `"/tests/" in "/" + rel_dir`
#   排除测试目录,对**顶层** tests 恒为 False(末尾无斜杠),于是把 crates/*/tests/*.rs
#   当生产码扫 —— 与本文件第 13-14 行自述口径相反的门,就是假阳性门(G-83 因
#   chimera-tui/tests/review_queue_panel_test.rs 报红即此因)。现按路径分段判定,
#   并由 --selftest 的双向夹具钉住"既抽得到真越权、也不误伤夹具"。
#
# 输出保持 ASCII(Windows GBK 控制台);退出码 0=绿 / 1=有越权签发方 / 2=配置错。
# Usage: py -3 scripts/check_certify_whitelist.py [--json | --selftest]
# =============================================================================
import argparse
import json
import os
import re
import sys

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPTS)
CRATES = os.path.join(ROOT, "crates")

# 允许签发 Certify 的 crate(定义方 + 可信验证器)。
# - event-bus: 定义/注册/穷举夹具所在;不视为签发行为。
# - parliament: v2.3 §(b.4)(1) 异构对抗审查者/治理裁决宿主(certify_if_reviewed
#   审查通过才签发),属 §(b.1) 验证器白名单成员。
ALLOWLIST = {"event-bus", "parliament"}

# 生产签发面信号:构造 `NexusEvent::CertifyIssued {` 或调用 `.publish_certify(`。
CONSTRUCT = re.compile(r"NexusEvent\s*::\s*CertifyIssued\s*\{")
PUBLISH = re.compile(r"\.\s*publish_certify\s*\(")
# 部分模式(`{ .. }`)/注释/文档不算法:
PATTERN_HINT = re.compile(r"CERTIFYISSUED\s*\{\s*\.\.", re.I)
SKIP_DIR_PARTS = ("target", ".git", ".toolchain", "node_modules")


def _is_comment(line):
    s = line.strip()
    return s.startswith("//") or s.startswith("///") or s.startswith("//!")


def is_production_src(rel_dir):
    """rel_dir(形如 crates/<crate>/src[/...])是否属于生产 src 作用域。

    WHY 按**路径分段**判定而不是找 "/tests/" 子串:tests/benches/examples 既可能
    是顶层目录(末尾无斜杠,子串法看不见),也可能嵌套;分段法对任意深度同一条规则
    成立,且与"只扫 crates/<crate>/src"的文档口径逐字对齐(顺带把 crate 根的
    build.rs、examples/ 一并排除在外)。
    """
    parts = rel_dir.split("/")
    return len(parts) >= 3 and parts[0] == "crates" and parts[2] == "src"


def scan_lines(crate, rel_file, lines):
    """对单个文件的行序列求越权命中,返回 [(crate, rel:line, kind)]。

    行内 `#[cfg(test)]` 模块不算签发方(测试夹具):按行扫描,见到 `#[cfg(test)]`
    即跳过该文件其余部分(测试 mod 惯位于文件末尾)。
    拆成纯函数是为了让 --selftest 能对真实文本形状做双向夹具(生产越权必被抽到 /
    夹具与注释必不被误抽),而不必先往仓里植一个真违规文件。
    """
    hits = []
    in_test_mod = False
    for i, line in enumerate(lines, 1):
        if "#[cfg(test)]" in line:
            in_test_mod = True
        if in_test_mod:
            continue
        if _is_comment(line):
            continue
        rel = "%s:%d" % (rel_file, i)
        if CONSTRUCT.search(line):
            if not PATTERN_HINT.search(line):
                hits.append((crate, rel, "CertifyIssued-ctor"))
        elif PUBLISH.search(line):
            hits.append((crate, rel, "publish_certify-call"))
    return hits


def scan():
    """返回 [(crate, rel:line, kind)] 越权签发命中列表。"""
    offenders = []
    for dirpath, dirs, files in os.walk(CRATES):
        dirs[:] = [d for d in dirs if d not in SKIP_DIR_PARTS]
        rel_dir = os.path.relpath(dirpath, ROOT).replace(os.sep, "/")
        crate = rel_dir.split("/")[1] if rel_dir.startswith("crates/") else "?"
        if crate in ALLOWLIST:
            continue
        # 只扫生产 src;tests/ 与 benches/ 等一律排除(夹具非签发方)。
        if not is_production_src(rel_dir):
            continue
        for f in files:
            if not f.endswith(".rs"):
                continue
            path = os.path.join(dirpath, f)
            with open(path, encoding="utf-8", errors="replace") as fh:
                rel_file = os.path.relpath(path, ROOT).replace(os.sep, "/")
                offenders.extend(scan_lines(crate, rel_file, fh))
    return offenders


def selftest():
    """判据本体的双向夹具:先证"能抽到",再证"不误伤",最后反向证绿不是恒绿。

    WHY 门自带夹具:本门曾因扫描域写法与自述口径相反而产生假阳性,而"绿"与"红"
    都可能来自判据缺陷而非代码事实。没有夹具的门,修判据时无法证明牙齿还在。
    """
    checks = []

    def expect(name, ok):
        checks.append((name, ok))

    ctor = "        NexusEvent::CertifyIssued { id, verdict };\n"
    publish = "        bus.publish_certify(&meta, verdict).await?;\n"
    partial = "    NexusEvent::CertifyIssued { .. } = ev;\n"

    # --- 扫描域:生产 src 内 / 外 ---
    expect("T1 crates/x/src 属生产域", is_production_src("crates/x/src") is True)
    expect("T2 crates/x/src/sub 属生产域", is_production_src("crates/x/src/sub") is True)
    expect("T3 顶层 tests 不属生产域(旧写法在此假阳性)",
           is_production_src("crates/x/tests") is False)
    expect("T4 嵌套 tests 不属生产域", is_production_src("crates/x/tests/extra") is False)
    expect("T5 benches 不属生产域", is_production_src("crates/x/benches") is False)
    expect("T6 examples 不属生产域", is_production_src("crates/x/examples") is False)
    expect("T7 crate 根(build.rs 所在)不属生产域", is_production_src("crates/x") is False)
    expect("T8 非 crates 路径不属生产域", is_production_src("scripts") is False)

    # --- 牙齿:真越权必须被抽到 ---
    hits = scan_lines("chimera-tui", "crates/chimera-tui/src/x.rs",
                      ["fn f() {", ctor, "}"])
    expect("T9 生产行构造 CertifyIssued 必被抽到",
           len(hits) == 1 and hits[0][2] == "CertifyIssued-ctor")
    hits = scan_lines("chimera-tui", "crates/chimera-tui/src/x.rs", [publish])
    expect("T10 publish_certify 调用必被抽到",
           len(hits) == 1 and hits[0][2] == "publish_certify-call")

    # --- 不误伤:注释 / doc / cfg(test) 夹具 / 部分模式 ---
    expect("T11 行注释不抽",
           scan_lines("c", "crates/c/src/x.rs", ["// " + ctor.strip()]) == [])
    expect("T12 doc 注释不抽",
           scan_lines("c", "crates/c/src/x.rs", ["/// " + ctor.strip()]) == [])
    expect("T13 cfg(test) 之后的夹具不抽",
           scan_lines("c", "crates/c/src/x.rs",
                      ["#[cfg(test)]", "mod tests {", ctor, "}"]) == [])
    expect("T14 部分模式 `{ .. }` 不算法",
           scan_lines("c", "crates/c/src/x.rs", [partial]) == [])

    # --- 反向实证:T13 的绿来自 cfg(test) 行,不是恒绿 ---
    expect("T15 去掉 #[cfg(test)] 行后同内容必被抽到",
           len(scan_lines("c", "crates/c/src/x.rs",
                          ["mod tests {", ctor, "}"])) == 1)

    failed = [name for name, ok in checks if not ok]
    for name, ok in checks:
        print("  [%s] %s" % ("OK" if ok else "FAIL", name))
    if failed:
        print("[SELFTEST] RED: %d/%d failed" % (len(failed), len(checks)))
        return 1
    print("[SELFTEST] GREEN: %d/%d" % (len(checks), len(checks)))
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--selftest", action="store_true",
                    help="跑判据本体的双向夹具,不扫真实仓库")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if not os.path.isdir(CRATES):
        print("[CONFIG] crates dir not found", file=sys.stderr)
        return 2
    offenders = scan()
    if args.json:
        print(json.dumps({"allowlist": sorted(ALLOWLIST), "offenders": offenders},
                         ensure_ascii=False))
    else:
        print("=== Certify authorization whitelist (v2.3 §(b.1)) ===")
        print("  allowlist=%s offenders=%d" % (sorted(ALLOWLIST), len(offenders)))
        for crate, loc, kind in offenders:
            print("  [UNAUTHORIZED] %s %s (%s)" % (crate, loc, kind))
        if not offenders:
            print("  [OK] no unauthorized Certify issuer outside allowlist")
    return 1 if offenders else 0


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(main))
