#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""check_v340_crate_proposals.py — 门 G-97/G-98：v3.5 终局原则 2 的逐项证据守护。

WHY 需要门（不是一次性脚本）
--------------------------
总账原则 2 声称「v3.4.0 §4.1 括号内新 crate 经三态判定后全部作废或已以模块落地」，
而逐项判定表写在总账 §4 附录 4A。文档里的表一旦与源文档/代码漂移，就退化成
"主张跑在实现前面"——本仓已有 G-95/G-96（形式化名册对账）、crate_contract --check
等先例，凡是"文档声称 ↔ 代码事实"的对账都必须机检。

判据（全部可证伪，不依赖我的印象）
--------------------------------
INV-A  0 新增 crate：§4.1 括号组里的提案名，**不得**是 workspace 成员 crate。
       （§4.2 组件列允许出现既有 crate——那是"强化"项不是"新增"提案，故 INV-A
        的论域严格限定为 §4.1，与原则 2 原文口径一致。）
INV-B  表覆盖闭合：附录 4A 标记块里的判定表必须恰好覆盖全部候选名。
       缺行 ⇒ [GAP-MISS]（主张又没有证据）；多行 ⇒ [GAP-STALE]（表里有源文档
       不存在的名字，或解析口径变了）。
INV-C  证据自校验：终判为 LANDED 的行，证据里的每个 `crates/xxx/...rs:NN`
       必须真实存在且 1 <= NN <= 该文件行数 ⇒ 否则 [GAP-EV]（防编造/防失效）。
       终判为 VOID 的行必须带非空理由 ⇒ 否则 [GAP-EV]。
       终判取值只能 ∈ {IN-PLACE, LANDED, VOID} ⇒ 否则退 2（未知旗标）。

退出码方言（F33/F39b）：0 通过 / 1 判红 / 2 不可判定（源文档缺失、§4.1 定位失败、
候选名集合为空、附录块缺失、未知终判、未知旗标）。

用法：
    py -3 scripts/check_v340_crate_proposals.py            # 真树判定
    py -3 scripts/check_v340_crate_proposals.py --selftest  # 夹具（门自身有牙）
"""
from __future__ import annotations

import glob
import io
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATE_LIKE = re.compile(r"^[a-z][a-z0-9]+(?:-[a-z0-9]+)*$")
LAYER_HEAD = re.compile(r"^\s*L(\d\w*)\s")
BEGIN = "<!-- BEGIN v340-4.1-three-state-appendix -->"
END = "<!-- END v340-4.1-three-state-appendix -->"
EVID = re.compile(r"crates/[A-Za-z0-9_\-./]+\.rs:(\d+)")
VERDICTS = ("IN-PLACE", "LANDED", "VOID")
USAGE = "usage: check_v340_crate_proposals.py [--selftest] [--root DIR]"


def read(path):
    return io.open(path, encoding="utf-8", errors="replace").read()


def bracket_groups(text):
    """嵌套感知的 [ ... ] 抽取（最外层内容，内部嵌套原样保留）。"""
    out, depth, buf = [], 0, []
    for ch in text:
        if ch == "[":
            if depth == 0:
                buf = []
            depth += 1
            if depth > 1:
                buf.append(ch)
        elif ch == "]":
            if depth:
                depth -= 1
                if depth == 0:
                    out.append("".join(buf))
                else:
                    buf.append(ch)
        elif depth:
            buf.append(ch)
    return out


def emit(tok, layer, src, sink):
    tok = re.sub(r"\(.*?\)", "", tok).strip()
    if not tok or any(ord(c) > 127 for c in tok):
        return
    if CRATE_LIKE.match(tok) and len(tok) > 4:
        sink.append((tok, layer, src))


def parse_candidates(doc_text):
    """返回 [(name, layer, src)]；src ∈ {§4.1, §4.2}。定位失败 ⇒ None（退 2）。"""
    lines = doc_text.splitlines()
    try:
        i41 = next(i for i, l in enumerate(lines) if l.startswith("### 4.1"))
        i42 = next(i for i, l in enumerate(lines) if l.startswith("### 4.2"))
        i5 = next(i for i, l in enumerate(lines) if l.startswith("## 5."))
    except StopIteration:
        return None
    cands = []
    blocks, cur = [], None
    for l in lines[i41:i42]:
        plain = re.sub(r"[\u2500-\u25ff]", " ", l)
        m = LAYER_HEAD.match(plain.strip())
        if m:
            cur = ["L" + m.group(1)]
            blocks.append(cur)
        elif cur is not None:
            cur.append(plain)
    for blk in blocks:
        for g in bracket_groups("\n".join(blk[1:])):
            for tok in re.split(r"[·,\n]", g):
                emit(tok, blk[0], "§4.1", cands)
    for l in lines[i42:i5]:
        if not l.strip().startswith("|"):
            continue
        cells = [c.strip() for c in l.strip().strip("|").split("|")]
        if len(cells) < 6:
            continue
        for cell in (cells[0], cells[5]):
            for tok in re.findall(r"`([^`]+)`", cell):
                emit(tok.strip(), cells[1], "§4.2", cands)
    return cands


def parse_appendix(ledger_text):
    """解析附录块里的判定表 ⇒ [(name, mech, verdict, evidence)]；块缺失 ⇒ None。"""
    if BEGIN not in ledger_text or END not in ledger_text:
        return None
    seg = ledger_text[ledger_text.index(BEGIN) + len(BEGIN):ledger_text.index(END)]
    rows = []
    for l in seg.splitlines():
        if not l.startswith("| `"):
            continue
        cells = [c.strip() for c in l.strip().strip("|").split("|")]
        if len(cells) != 6 or cells[0] == "提案名":
            continue
        rows.append((cells[0].strip("`"), cells[3], cells[4], cells[5]))
    return rows


def judge(root):
    """返回 (rc, 报告行列表, 统计 dict)。"""
    rep, stats = [], {}
    docs = glob.glob(os.path.join(root, "Chimera_CLI_v3.4.0*.md"))
    ledgers = glob.glob(os.path.join(root, "Chimera架构重构与优化方案_v3.5-*.md"))
    if not docs or not ledgers:
        return 2, ["[TOOL-ERROR] 缺源文档或总账（不可判定）"], stats
    cands = parse_candidates(read(docs[0]))
    if cands is None:
        return 2, ["[TOOL-ERROR] 源文档 §4.1/§4.2/§5 定位失败（不可判定）"], stats
    names = sorted({n for n, _, _ in cands})
    stats["candidates"] = len(names)
    if not names:
        return 2, ["[TOOL-ERROR] 候选名集合为空（取证器空转即不可判定）"], stats
    only41 = {n for n, _, sr in cands if sr == "§4.1"}
    members = {os.path.basename(p) for p in glob.glob(os.path.join(root, "crates", "*"))
               if os.path.isdir(p)}
    if not members:
        return 2, ["[TOOL-ERROR] 无法解析 workspace 成员集合（不可判定）"], stats

    gaps = []
    # INV-A
    newc = sorted(only41 & members)
    stats["inv_a_new_crate"] = len(newc)
    if newc:
        gaps.append("[GAP] INV-A：§4.1 括号内提案 %s 已成为 workspace crate 成员，"
                    "违反原则 2『0 新增 crate』（消解：撤销该 crate 或走 ADR 修订原则 2）"
                    % ", ".join(newc))
    # INV-B
    rows = parse_appendix(read(ledgers[0]))
    if rows is None:
        return 2, ["[TOOL-ERROR] 总账缺附录块（BEGIN/END 标记不全，不可判定）"], stats
    tbl = {r[0] for r in rows}
    missing = sorted(set(names) - tbl)
    stale = sorted(tbl - set(names))
    stats["table_rows"], stats["inv_b_missing"], stats["inv_b_stale"] = len(rows), len(missing), len(stale)
    if missing:
        gaps.append("[GAP-MISS] INV-B：判定表缺 %d 行（主张没有逐项证据）：%s"
                    % (len(missing), ", ".join(missing)))
    if stale:
        gaps.append("[GAP-STALE] INV-B：判定表多 %d 行（源文档已无此名或口径漂移）：%s"
                    % (len(stale), ", ".join(stale)))
    # INV-C
    bad_ev, bad_verdict, unknown = [], [], []
    for name, mech, verdict, ev in rows:
        if verdict not in VERDICTS:
            unknown.append("%s=%s" % (name, verdict))
            continue
        # 任何行的 file:line 引用都得真存在（LANDED 靠它撑结论，VOID 也常引用
        # “那是另一个提案”的载体行号；写了假路径就是假证据，不分终判）
        for m in EVID.finditer(ev):
            p = os.path.join(root, m.group(0).split(".rs")[0] + ".rs")
            if not os.path.isfile(p):
                bad_ev.append("%s：证据文件不存在 %s" % (name, m.group(0)))
                continue
            n = len(read(p).splitlines())
            if not (1 <= int(m.group(1)) <= n):
                bad_ev.append("%s：证据行号越界 %s（该文件 %d 行）" % (name, m.group(0), n))
        if verdict == "LANDED":
            if not EVID.search(ev):
                bad_ev.append("%s：LANDED 却无 crates/...rs:NN 证据" % name)
        elif verdict == "VOID" and not ev:
            bad_ev.append("%s：VOID 未给理由" % name)
    stats["inv_c_bad_evidence"], stats["unknown_verdicts"] = len(bad_ev), len(unknown)
    if unknown:
        return 2, ["[TOOL-ERROR] 终判列出现未知取值（不可判定）：%s" % ", ".join(unknown)], stats
    if bad_ev:
        gaps.extend("[GAP-EV] INV-C：" + b for b in bad_ev)

    reps = ["候选名=%d（§4.1 口径 %d）｜判定表行=%d｜成员 crate=%d"
            % (len(names), len(only41), len(rows), len(members))]
    reps += gaps
    reps.append("RESULT: %s" % ("OK" if not gaps else "MISMATCH (%d)" % len(gaps)))
    return (0 if not gaps else 1), reps, stats


def _write(path, text):
    io.open(path, "w", encoding="utf-8", newline="\n").write(text)


def selftest():
    """夹具：一致⇒绿 / 新 crate 提案⇒红 / 缺行⇒红 / 多行⇒红 / 证据越界⇒红 /
    证据文件缺失⇒红 / VOID 无理由⇒红 / 未知终判⇒退 2 / 块缺失⇒退 2 /
    源文档缺§4.1⇒退 2 / 候选名解析为空⇒退 2。"""
    doc = (
        "## 4. 架构总览\n"
        "### 4.1 完整架构图\n\n```\n"
        "│ L7 Execution ——\n"
        "│  pvl-layer ·\n"
        "│  [alpha-one · beta-two ·\n"
        "│  gamma-three]  │\n"
        "│  [中文预留组] │\n```\n\n"
        "### 4.2 新增/强化组件清单\n\n"
        "| 组件 | 层 | 来源 | 功能 | 融合状态 | 落地方案 |\n"
        "|---|---|---|---|---|---|\n"
        "| `delta-four` | L6 | X | f | s | 在 `pvl-layer` 内部扩展 |\n"
        "## 5. 下一节\n"
    )
    ledger_head = "# 总账\n\n## 4 crate 总账\n\n"
    table = ("| 提案名 | 层 | 来源 | 机械判定 | 终判 | 证据 |\n"
             "|---|---|---|---|---|---|\n"
             "| `alpha-one` | L7 | §4.1 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：pvl-layer） |\n"
             "| `beta-two` | L7 | §4.1 | ②c PARTIAL | LANDED | crates/pvl-layer/src/lib.rs:1 符号 |\n"
             "| `gamma-three` | L7 | §4.1 | ③ VOID | VOID | 全库零命中 |\n"
             "| `delta-four` | L6 | §4.2 | ② MODULE | IN-PLACE | 机械判定即终判（模块文件：pvl-layer） |\n"
             "| `pvl-layer` | L6 | §4.2 | ① CRATE | IN-PLACE | 机械判定即终判（crates/pvl-layer 在籍） |\n")

    def make(ledger_table, *, crate_names=(), drop_line=None, extra=None,
             ev_oob=False, ev_missing=False, void_reason=True, verdict="LANDED",
             void_fake=False):
        d = tempfile.mkdtemp(prefix="v340st-")
        os.makedirs(os.path.join(d, "crates", "pvl-layer", "src"))
        _write(os.path.join(d, "crates", "pvl-layer", "src", "lib.rs"), "// one\n// two\n")
        for c in crate_names:
            os.makedirs(os.path.join(d, "crates", c), exist_ok=True)
        _write(os.path.join(d, "Chimera_CLI_v3.4.0_omega_x.md"), doc)
        body = ledger_table
        if drop_line:
            body = "\n".join(l for l in body.splitlines() if drop_line not in l) + "\n"
        if extra:
            body += extra
        if ev_oob:
            body = body.replace("lib.rs:1 符号", "lib.rs:99 符号")
        if ev_missing:
            body = body.replace("crates/pvl-layer/src/lib.rs:1", "crates/pvl-layer/src/ghost.rs:1")
        if not void_reason:
            body = body.replace("| VOID | 全库零命中 |", "| VOID |  |")
        if void_fake:
            body = body.replace("| VOID | 全库零命中 |",
                                "| VOID | 参 crates 他提案 crates/pvl-layer/src/ghost.rs:3 |")
        if verdict != "LANDED":
            body = body.replace("| LANDED |", "| %s |" % verdict)
        _write(os.path.join(d, "Chimera架构重构与优化方案_v3.5-全栈终局总账.md"),
               ledger_head + BEGIN + "\n" + body + END + "\n\n## 5 契约\n")
        return d

    cases = []

    def run(d):
        return judge(d)

    def cleanup(d):
        shutil.rmtree(d, ignore_errors=True)

    d = make(table)
    rc, rep, st = run(d)
    cases.append(("一致 ⇒ 绿", rc == 0, rep[-1]))
    cleanup(d)

    d = make(table, crate_names=("alpha_one", "beta-two"))
    rc, rep, st = run(d)
    cases.append(("§4.1 提案成为 crate ⇒ 红(INV-A)", rc == 1 and "INV-A" in "".join(rep), rep[-1]))
    cleanup(d)

    d = make(table, drop_line="`gamma-three`")
    rc, rep, st = run(d)
    cases.append(("表缺行 ⇒ 红(INV-B MISS)", rc == 1 and "GAP-MISS" in "".join(rep), rep[-1]))
    cleanup(d)

    d = make(table, extra="| `zeta-ghost` | L7 | §4.1 | ③ VOID | VOID | 无 |\n")
    rc, rep, st = run(d)
    cases.append(("表多行 ⇒ 红(INV-B STALE)", rc == 1 and "GAP-STALE" in "".join(rep), rep[-1]))
    cleanup(d)

    d = make(table, ev_oob=True)
    rc, rep, st = run(d)
    cases.append(("证据行号越界 ⇒ 红(INV-C)", rc == 1 and "越界" in "".join(rep), rep[-1]))
    cleanup(d)

    d = make(table, ev_missing=True)
    rc, rep, st = run(d)
    cases.append(("证据文件不存在 ⇒ 红(INV-C)", rc == 1 and "不存在" in "".join(rep), rep[-1]))
    cleanup(d)

    d = make(table, void_reason=False)
    rc, rep, st = run(d)
    cases.append(("VOID 无理由 ⇒ 红(INV-C)", rc == 1 and "未给理由" in "".join(rep), rep[-1]))
    cleanup(d)

    d = make(table, void_fake=True)
    rc, rep, st = run(d)
    cases.append(("VOID 行里的假路径也被抓 ⇒ 红", rc == 1 and "ghost.rs" in "".join(rep), rep[-1]))
    cleanup(d)

    d = make(table, verdict="MAYBE")
    rc, rep, st = run(d)
    cases.append(("未知终判 ⇒ 退 2", rc == 2, rep[-1]))
    cleanup(d)

    d = tempfile.mkdtemp(prefix="v340st-")
    os.makedirs(os.path.join(d, "crates", "pvl-layer", "src"))
    _write(os.path.join(d, "crates", "pvl-layer", "src", "lib.rs"), "// one\n")
    _write(os.path.join(d, "Chimera_CLI_v3.4.0_omega_x.md"), doc)
    _write(os.path.join(d, "Chimera架构重构与优化方案_v3.5-x.md"), "# 无附录\n")
    rc, rep, st = run(d)
    cases.append(("附录块缺失 ⇒ 退 2", rc == 2 and "附录" in "".join(rep), rep[-1]))
    cleanup(d)

    d = tempfile.mkdtemp(prefix="v340st-")
    os.makedirs(os.path.join(d, "crates", "pvl-layer", "src"))
    _write(os.path.join(d, "crates", "pvl-layer", "src", "lib.rs"), "// one\n")
    _write(os.path.join(d, "Chimera_CLI_v3.4.0_omega_x.md"), "## 4\n## 5.\n")
    _write(os.path.join(d, "Chimera架构重构与优化方案_v3.5-x.md"), BEGIN + END)
    rc, rep, st = run(d)
    cases.append(("§4.1 定位失败 ⇒ 退 2", rc == 2, rep[-1]))
    cleanup(d)

    print("check_v340_crate_proposals selftest: %d/%d" % (sum(1 for _, ok, _ in cases if ok), len(cases)))
    if not cases:
        return 2  # 夹具集合为空 ⇒ 不可判定（禁止“空集全绿”）
    bad = 0
    for name, ok, detail in cases:
        print("  %-34s %s   %s" % (name, "PASS" if ok else "FAIL", detail))
        bad += 0 if ok else 1
    print("RESULT: %s" % ("OK" if bad == 0 else "MISMATCH (%d)" % bad))
    return 0 if bad == 0 else 1


def main(argv):
    args = list(argv)
    root = ROOT
    positional = []
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--root":
            if i + 1 >= len(args):
                sys.stderr.write(USAGE + "\n")
                return 2
            root = args[i + 1]
            i += 2
            continue
        if a != "--selftest":
            positional.append(a)
        i += 1
    if positional:
        sys.stderr.write(USAGE + "\n")
        return 2
    if "--selftest" in args:
        return selftest()
    rc, rep, stats = judge(root)
    for l in rep:
        print(l)
    if rc == 2:
        sys.stderr.write(USAGE + "\n")
    return rc


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
