#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""文档死引用门 (R7-docs / P12 病灶处置).

Why this gate exists
--------------------
P12 已确诊:代码侧有机器门、文档侧无门,退役无文档同步回路——代码删了
`nexus-core/src/types.rs`,CODE_WIKI/AGENTS.md 还在引用,直到人工巡检才暴露。
本门把「文档引用的 `crates/` 路径必须存在于磁盘」变成可执行判据:
扫描 docs/**/*.md + AGENTS.md + README.md,抽取 `crates/<crate>/...` 形式的
文件引用(*.rs 或含 . 的尾段,可带 :line 后缀)与目录引用,验证磁盘存在性。

白名单(两类不计,防误报)
------------------------
1. 代码围栏(``` / ~~~ fenced block)内的引用——示例代码天然会写不存在的示意路径;
2. 已登记退役——scripts/*.txt 台账 REMOVED 行内出现的 crates/ 路径 token,
   以及 scripts/retired_crates.txt 登记的整 crate 引用(退役登记节的有意
   历史记录不算漂移)。
   ★ 实测声明(2026-11-16):全仓 REMOVED 行仅 crate_reachability_freeze.txt
   6 行,均为叙述性注释、不携带可解析路径 token;故白名单的实际承载者是
   retired_crates.txt。REMOVED 行解析仍保留——台账日后带路径 token 即自动生效。

退出码三态(本仓门方言)
----------------------
  0 = 无死引用   1 = 检出死引用   2 = 不可判定(用法错误/扫描崩溃/白名单台账缺失)

--selftest: 临时目录构造假 docs 树,夹具 = 1 死引用 + 1 活引用 + 1 代码围栏内
死引用 + 1 台账退役 crate 引用 + 1 REMOVED 行白名单引用 + 1 行号后缀活引用,
断言:检出死引用(rc=1)、活引用不误报、围栏/白名单条目不报。

输出: stdout 纯 ASCII 标签([DEAD]/[OK]/[FAIL]/[PASS]/[UNDECIDABLE]),
与项目既有门一致;注释/docstring 中文。
"""

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import gate_rc  # noqa: E402  (崩溃兜底 + 退出码三态方言)

# 引用 token:crates/<crate>/<至少一段路径>;crate 段必须以字母起首(workspace
# 目录名与 Cargo 包名均如此;可挡 "38 crates/9954/10836" 这类自然语言误报)。
# 字符集覆盖路径段,不含 Markdown 转义反斜杠(扫描前对行做 \_ → _ 反转义,
# 见 iter_refs)。
REF_RE = re.compile(r"crates/[A-Za-z][A-Za-z0-9_-]*(?:/[A-Za-z0-9_.-]+)+")
# 行号/列号后缀(:37 / :1-10 / :37:8),可叠多个。
LINE_SUFFIX_RE = re.compile(r"(?::\d+(?:-\d+)?)+$")
# 尾随标点(英文/Markdown 边界 + 中文全角),循环剥离;不含路径合法字符。
TRAILING_STRIP = ")]}>;,;:,.`'\"|*_- -、。，；：）"
# 退役 crate 登记表行:<crate> [# 注释](非 # 起首行的首 token)。
RETIRED_LEDGER = os.path.join(HERE, "retired_crates.txt")
# 盲扫下限:低于此引用数视为扫描器失明(而非干净),拒绝判绿。
REF_FLOOR = 20

SCAN_DOC_ROOTS = ("docs",)
SCAN_ROOT_FILES = ("AGENTS.md", "README.md")


def _clean_token(tok):
    """剥离行号后缀/glob/尾随标点,返回规范化引用路径(不动路径本体)。"""
    tok = tok.strip()
    while True:
        nxt = LINE_SUFFIX_RE.sub("", tok)
        # glob 结尾 /** 或 **:先剥星再剥斜杠
        while nxt.endswith("*"):
            nxt = nxt[:-1]
        if nxt.endswith("/"):
            nxt = nxt.rstrip("/")
        stripped = nxt.rstrip(TRAILING_STRIP)
        if stripped:
            nxt = stripped
        if nxt == tok:
            break
        tok = nxt
    return tok


def iter_refs(line):
    """一行(非围栏内)中的规范化 crates/ 引用列表。"""
    # Markdown 表格会把 _ 转义为 \_;反转义后再抽取,避免 token 被反斜杠截断
    # (如 command\_palette.rs 会被切成不存在的 crates/.../command 目录误报)。
    scan = line.replace("\\_", "_")
    out = []
    for m in REF_RE.finditer(scan):
        tok = _clean_token(m.group(0))
        if REF_RE.fullmatch(tok):
            out.append(tok)
    return out


def _fence_update(line, state):
    """围栏状态机:(是否在内, 围栏字符, 开围栏长度)。CommonMark 规则:闭围栏
    同字符且长度不小于开围栏。"""
    in_fence, ch, ln = state
    m = re.match(r"^\s*(`{3,}|~{3,})", line)
    if not m:
        return state
    mark = m.group(1)
    if not in_fence:
        return (True, mark[0], len(mark))
    if mark[0] == ch and len(mark) >= ln:
        return (False, "", 0)
    return state


def load_whitelist(scripts_dir):
    """(refs, crates, errors):REMOVED 行路径 token 集合 + 退役 crate 集合。"""
    refs = set()
    crates = set()
    errors = []
    if not os.path.isdir(scripts_dir):
        return refs, crates, ["missing scripts dir: %s" % scripts_dir]
    ledger = os.path.join(scripts_dir, os.path.basename(RETIRED_LEDGER))
    if not os.path.isfile(ledger):
        errors.append("missing retired ledger: %s" % ledger)
    else:
        with open(ledger, "r", encoding="utf-8", errors="replace") as fh:
            for raw in fh:
                line = raw.split("#", 1)[0].strip()
                if not line:
                    continue
                crates.add(line.split()[0])
    for name in sorted(os.listdir(scripts_dir)):
        if not name.endswith(".txt"):
            continue
        path = os.path.join(scripts_dir, name)
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                for raw in fh:
                    if "REMOVED" not in raw:
                        continue
                    for tok in REF_RE.finditer(raw):
                        refs.add(_clean_token(tok.group(0)))
        except OSError as exc:
            errors.append("unreadable ledger %s: %s" % (name, exc))
    return refs, crates, errors


def _whitelisted(tok, wl_refs, wl_crates):
    if tok in wl_refs:
        return True
    crate = tok.split("/")[1]
    return crate in wl_crates


def scan(root, wl_refs, wl_crates):
    """(dead, total_refs, scanned_files, errors)。

    dead: [(doc, line_no, ref)] 按 (doc, line_no, ref) 排序。
    """
    docs = []
    for d in SCAN_DOC_ROOTS:
        base = os.path.join(root, d)
        if not os.path.isdir(base):
            continue
        for dirpath, _dirs, names in os.walk(base):
            for name in sorted(names):
                if name.endswith(".md"):
                    docs.append(os.path.join(dirpath, name))
    for name in SCAN_ROOT_FILES:
        p = os.path.join(root, name)
        if os.path.isfile(p):
            docs.append(p)
    docs.sort()

    dead = []
    total = 0
    errors = []
    for path in docs:
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                lines = fh.read().splitlines()
        except OSError as exc:
            errors.append("unreadable doc %s: %s" % (rel, exc))
            continue
        state = (False, "", 0)
        for no, line in enumerate(lines, 1):
            state = _fence_update(line, state)
            if state[0]:
                continue
            for tok in iter_refs(line):
                total += 1
                if _whitelisted(tok, wl_refs, wl_crates):
                    continue
                if os.path.isfile(os.path.join(root, tok)):
                    continue
                if os.path.isdir(os.path.join(root, tok)):
                    continue
                dead.append((rel, no, tok))
    dead.sort()
    return dead, total, len(docs), errors


def judge(root, ref_floor=REF_FLOOR):
    """(rc, report_lines)——真树与夹具共用同一判据。"""
    wl_refs, wl_crates, wl_errors = load_whitelist(os.path.join(root, "scripts"))
    if wl_errors:
        return 2, ["[UNDECIDABLE] whitelist ledger: %s" % "; ".join(wl_errors)]
    try:
        dead, total, n_docs, errors = scan(root, wl_refs, wl_crates)
    except Exception as exc:  # noqa: BLE001 - 任何扫描失败都不可判定,不静默判绿
        return 2, ["[UNDECIDABLE] scan failed: %s: %s" % (type(exc).__name__, exc)]
    if errors:
        return 2, ["[UNDECIDABLE] %s" % e for e in errors]
    if total < ref_floor:
        return 2, ["[UNDECIDABLE] blind scan: %d ref(s) < floor %d"
                   % (total, ref_floor)]
    report = []
    for rel, no, tok in dead:
        report.append("[DEAD] %s:%d: %s" % (rel, no, tok))
    if dead:
        report.append("[FAIL] doc dead-ref gate: %d dead ref(s) in %d doc(s) "
                      "(scanned %d docs, %d refs)"
                      % (len(dead), len({d[0] for d in dead}), n_docs, total))
        return 1, report
    report.append("[OK] doc dead-ref gate: scanned %d docs, %d refs, 0 dead"
                  % (n_docs, total))
    return 0, report


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)


def selftest():
    """夹具证牙:死引用必检出,活引用/围栏/白名单必不误报。"""
    n = 0
    bad = 0

    def check(label, got, want):
        nonlocal n, bad
        n += 1
        if got != want:
            bad += 1
            print("  [FAIL] %s got=%r want=%r" % (label, got, want))
        else:
            print("  [PASS] %s -> %r" % (label, got))

    tmp = gate_rc.tmpdir(prefix="doc-dead-refs-")
    # 活文件 + 退役 crate 白名单台账 + REMOVED 行台账
    _write(os.path.join(tmp, "crates", "aaa", "src", "lib.rs"), "pub struct A;\n")
    _write(os.path.join(tmp, "scripts", "retired_crates.txt"),
           "# 退役 crate 登记表\nzzz-old # 退役测试\n")
    _write(os.path.join(tmp, "scripts", "demo_freeze.txt"),
           "# demo row\n"
           "# something REMOVED 2026-01-01: crates/aaa/src/ledger_gone.rs deleted\n")
    # docs 夹具:1 死引用、1 活引用、1 行号活引用、1 围栏内死引用、
    # 1 退役 crate 引用、1 REMOVED 行白名单引用
    _write(os.path.join(tmp, "docs", "guide.md"), "\n".join([
        "live: crates/aaa/src/lib.rs",
        "dead: crates/aaa/src/gone.rs",
        "line-suffix live: crates/aaa/src/lib.rs:42",
        "```rust",
        "fenced dead (must not report): crates/aaa/src/fenced_gone.rs",
        "```",
        "retired crate (must not report): crates/zzz-old/src/lib.rs",
        "ledger whitelisted (must not report): crates/aaa/src/ledger_gone.rs",
        "",
    ]))
    # 干净夹具(只有活引用)
    _write(os.path.join(tmp, "docs", "clean.md"),
           "live only: crates/aaa/src/lib.rs\n")

    rc, lines = judge(tmp, ref_floor=1)
    check("fixture with one dead ref is red", rc, 1)
    check("dead ref is named",
          any("crates/aaa/src/gone.rs" in ln for ln in lines), True)
    check("live ref not reported",
          any("lib.rs" in ln and "[DEAD]" in ln for ln in lines), False)
    check("line-suffix live ref not reported",
          any("lib.rs:42" in ln for ln in lines), False)
    check("fenced dead ref not reported",
          any("fenced_gone.rs" in ln for ln in lines), False)
    check("retired crate ref not reported",
          any("zzz-old" in ln for ln in lines), False)
    check("REMOVED-line ledger ref not reported",
          any("ledger_gone.rs" in ln for ln in lines), False)

    os.remove(os.path.join(tmp, "docs", "guide.md"))
    rc, lines = judge(tmp, ref_floor=1)
    check("clean fixture is green", rc, 0)
    check("clean report is [OK]",
          any(ln.startswith("[OK]") for ln in lines), True)

    rc, lines = judge(os.path.join(tmp, "nonexistent"), ref_floor=1)
    check("missing tree is undecidable (blind scan)", rc, 2)

    print("  RESULT: %s (%d assertions)" % ("PASS" if not bad else "FAIL", n))
    return 1 if bad else 0


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    if argv:
        print("usage: check_doc_dead_refs.py [--selftest]")
        return 2
    rc, report = judge(ROOT)
    for line in report:
        print(line)
    if rc == 2:
        print("[UNDECIDABLE] doc dead-ref gate cannot decide")
    return rc


if __name__ == "__main__":
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
