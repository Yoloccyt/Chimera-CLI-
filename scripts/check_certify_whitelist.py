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
# 输出保持 ASCII(Windows GBK 控制台);退出码 0=绿 / 1=有越权签发方 / 2=配置错。
# Usage: py -3 scripts/check_certify_whitelist.py [--json]
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


def scan():
    """返回 [(crate, rel:line, kind)] 越权签发命中列表。"""
    offenders = []
    for dirpath, dirs, files in os.walk(CRATES):
        dirs[:] = [d for d in dirs if d not in SKIP_DIR_PARTS]
        rel_dir = os.path.relpath(dirpath, ROOT).replace(os.sep, "/")
        crate = rel_dir.split("/")[1] if rel_dir.startswith("crates/") else "?"
        if crate in ALLOWLIST:
            continue
        # 只扫生产 src;tests/ 与 benches/ 排除(夹具非签发方)
        if "/tests/" in "/" + rel_dir or "/benches/" in "/" + rel_dir:
            continue
        for f in files:
            if not f.endswith(".rs"):
                continue
            path = os.path.join(dirpath, f)
            with open(path, encoding="utf-8", errors="replace") as fh:
                # 行内 #[cfg(test)] 模块不算签发方(测试夹具);按行扫描,见到
                # #[cfg(test)] 即跳过该文件其余部分(测试 mod 惯位于文件末尾)。
                # 这落实本门文档已声明的「#[cfg(test)] 不算签发方」意图。
                in_test_mod = False
                for i, line in enumerate(fh, 1):
                    if "#[cfg(test)]" in line:
                        in_test_mod = True
                    if in_test_mod:
                        continue
                    if _is_comment(line):
                        continue
                    rel = "%s:%d" % (os.path.relpath(path, ROOT).replace(os.sep, "/"), i)
                    if CONSTRUCT.search(line):
                        if not PATTERN_HINT.search(line):
                            offenders.append((crate, rel, "CertifyIssued-ctor"))
                    elif PUBLISH.search(line):
                        offenders.append((crate, rel, "publish_certify-call"))
    return offenders


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
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
    raise SystemExit(main())
