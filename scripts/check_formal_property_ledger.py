#!/usr/bin/env python3
# =============================================================================
# check_formal_property_ledger.py - "形式化属性名册 vs 实装 checker 对账门"（v3.5 C-43）
# =============================================================================
# Purpose: Spec C-43 记的是“9 属性主张 vs 枚举 4 / checker 7 / 名单 7（含 1 幽灵）”，
#          并裁决“按实装对账修名单，属纠偏非扩面”。本轮现取复核（逐文件定位到行号）：
#
#            名册（唯一 src 侧枚举 = `nexus-contracts/src/spec/formal_props.rs`
#            `EmptyFormalProvider::collect_results()`，7 个 id）
#              lineage-dag / critic-monotonicity / preference-consistency /
#              causal-consistency / learning-monotonicity / decay-consistency /
#              invariant-closure
#            实装 checker（src 内 `pub struct` 型验证器，7 个）
#              LineageChecker(gsoe/formal/lineage_checker.rs:146)
#              CriticMonotonicityChecker(gsoe/formal/critic_monotonicity.rs:27)
#              invariant_closure 模块(gsoe/formal/invariant_closure.rs)
#              DecayConsistencyChecker(decay-engine/formal.rs:96)
#              CausalConsistencyChecker(event-bus/formal.rs)
#              LearningMonotonicityChecker(omega-learner/formal.rs)
#              ConsensusSafetyChecker(parliament/formal/consensus_safety.rs:50)
#
#          ⇒ **两边各有一项对不上**：名册里的 `preference-consistency` 全库无 checker
#          （其宿主 auto-dpo 已随 ADR-172 删除），而实装的 `consensus-safety` 不在名册里。
#          ADR-052 §53/55、ADR-053 §33/56 又把“7/7 属性达成”当 R2 解冻前置证据 ⇒
#          **换名册 = 改一条已签署 R2 证据的内容**，需新 ADR，本轮不擅改。
#
#          于是本门承担“本轮能做的全部”：把这对不上变成**机器可见且不可扩大**的事实——
#          名册每行必须有 checker 或带理由豁免；checker 每行必须入名册或带理由登记；
#          已登记项一旦失效（checker 文件被删、名册已修好）即判红，不许留僵尸登记。
#
# 退出码: 0=对账闭合 / 1=GAP(未登记的错配) 或 STALE(登记失效) / 2=不可判定
#         (名册源或 checker 路径读不出、表格式非法、未知旗标)。
# Usage: py -3 scripts/check_formal_property_ledger.py [--selftest|--emit]
# =============================================================================
import os
import re
import sys

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPTS)

PROVIDER = "crates/nexus-contracts/src/spec/formal_props.rs"

# 名册 id -> checker 落点（“checker” = 该属性唯一的 src 侧验证器文件或模块文件）。
# 每条都是本轮现取的行号级证据；checker 文件被移动/删除 ⇒ 本门必红。
ROSTER = [
    ("lineage-dag", "crates/gsoe-evolution/src/formal/lineage_checker.rs"),
    ("critic-monotonicity", "crates/gsoe-evolution/src/formal/critic_monotonicity.rs"),
    ("invariant-closure", "crates/gsoe-evolution/src/formal/invariant_closure.rs"),
    ("decay-consistency", "crates/decay-engine/src/formal.rs"),
    ("causal-consistency", "crates/event-bus/src/formal.rs"),
    ("learning-monotonicity", "crates/omega-learner/src/formal.rs"),
    ("consensus-safety", "crates/parliament/src/formal/consensus_safety.rs"),
]

# 已登记的错配（**只减不增**；每修好一条就删一行）。
#   kind: NO-CHECKER = 名册有 id 但无 checker；NOT-IN-LEDGER = 实装 checker 未入名册
#   exit  : 消解它所需的动作（两条都不是“本轮可自决”的动作，故登记而非顺手改）
LEDGER_EXEMPT = [
    {
        "id": "preference-consistency",
        "kind": "NO-CHECKER",
        "reason": "名册第 3 位，但全库无验证器：其宿主 auto-dpo 已随 ADR-172 退役"
                  "（现测 `crates/*/src` 内 preference 相关 checker 0 命中）。ADR-052 §53/55 与 "
                  "ADR-053 §33/56 把“7/7 属性达成”列为 R2 解冻前置证据 ⇒ 从名册删它等于改一条"
                  "已签署证据的内容，需新 ADR（建议与 consensus-safety 同批处理）。",
        "exit": "新 ADR：删名册位或补真 checker（二者都会改变 R2 证据集内容）",
    },
    {
        "id": "consensus-safety",
        "kind": "NOT-IN-LEDGER",
        "reason": "实装验证器 `ConsensusSafetyChecker`（parliament/src/formal/consensus_safety.rs:50）"
                  "存在且属 L0 枚举 `PropertyCategory::ConsensusSafety` 四类型之一，但名册里没有它 ⇒ "
                  "“7 属性”少算了一个真验证器。加它同样改变 ADR-052/053 引用的名册内容 ⇒ 需新 ADR。",
        "exit": "新 ADR：把 consensus-safety 纳入名册（与 preference-consistency 同批）",
    },
]

ID_IN_PROVIDER = re.compile(r'^\s*"([a-z][a-z-]+)"\.into\(\),?\s*$', re.M)
BLOCK_START = "impl FormalResultProvider for EmptyFormalProvider"
PUB_STRUCT = re.compile(r"^\s*pub struct \w+", re.M)


def read(rel, root=ROOT):
    path = os.path.join(root, *rel.split("/"))
    if not os.path.isfile(path):
        raise FileNotFoundError(rel)
    with open(path, encoding="utf-8", errors="replace") as fh:
        return path, fh.read()


def parse_ledger_ids(root=ROOT):
    """从唯一 src 侧枚举（EmptyFormalProvider）解析名册 id 集合。

    WHY 只取该函数体内：`"xxx".into()` 在别处也出现（PropertySpec id 等），
    全文正则会把非名册 id 混进来 ⇒ 名册集合必须是“provider 体内的那一段”。
    """
    _path, src = read(PROVIDER, root)
    start = src.find(BLOCK_START)
    if start < 0:
        raise RuntimeError("名册源缺失：%s 内找不到 %s" % (PROVIDER, BLOCK_START))
    tail = src[start:]
    end = tail.find("\n}\n")
    body = tail[:end] if end > 0 else tail
    ids = ID_IN_PROVIDER.findall(body)
    if not ids:
        raise RuntimeError("名册解析为空（provider 体被改写？不静默放行）")
    return ids


def check(root, roster, exempt):
    """核心判据（纯函数，--selftest 用合成 root 驱动）。返回 (rc, 行, 统计)。"""
    lines, gaps, stale, config = [], [], [], []
    exempt_ids = set()
    for row in exempt:
        rid, kind = row.get("id"), row.get("kind")
        if kind not in ("NO-CHECKER", "NOT-IN-LEDGER"):
            config.append("豁免类别非法: %s -> %r" % (rid, kind))
        if len((row.get("reason") or "").strip()) < 40:
            config.append("豁免理由过短/缺证据: %s" % rid)
        if not (row.get("exit") or "").strip():
            config.append("豁免必须写消解出口: %s" % rid)
        if rid in exempt_ids:
            config.append("豁免重复: %s" % rid)
        exempt_ids.add(rid)
    if config:
        return 2, ["=== formal property ledger parity (v3.5 C-43) ==="] + \
            ["  [CONFIG] %s" % c for c in config], {}

    try:
        ids = parse_ledger_ids(root)
    except Exception as exc:  # noqa: BLE001 - 读不出就是不可判定，不猜
        return 2, ["=== formal property ledger parity (v3.5 C-43) ===",
                   "  [UNDECIDABLE] %s" % exc], {}

    ledger = set(ids)
    have_checker = {}
    for (pid, rel) in roster:
        try:
            _p, src = read(rel, root)
        except FileNotFoundError:
            config.append("checker 文件缺失: %s（名册位 %s）" % (rel, pid))
            continue
        if not PUB_STRUCT.search(src):
            gaps.append("%s 的 checker 文件不含 `pub struct` 验证器: %s" % (pid, rel))
            continue
        have_checker[pid] = rel

    if config:
        return 2, ["=== formal property ledger parity (v3.5 C-43) ==="] + \
            ["  [CONFIG] %s" % c for c in config], {}

    # 名册 -> checker：每个名册 id 必须有 checker，或带理由豁免
    for pid in sorted(ledger):
        if pid not in have_checker and pid not in exempt_ids:
            gaps.append("名册位 %s 无 checker 亦无登记（新增无验证器的属性 = 又一条假证据）" % pid)
    # checker -> 名册：每个实装 checker 必须在名册，或带理由登记
    for pid in sorted(have_checker):
        if pid not in ledger and pid not in exempt_ids:
            gaps.append("实装 checker %s 未入名册亦未登记（它会让“N/N 属性”计数虚低）" % pid)
    # 失效登记：错配已不存在 ⇒ 必须删行
    for row in exempt:
        pid, kind = row.get("id"), row.get("kind")
        if kind == "NO-CHECKER":
            still = pid in ledger and pid not in have_checker
        else:
            still = pid in have_checker and pid not in ledger
        if not still:
            stale.append("%s 的登记已失效（名册与 checker 已对账，或该属性已不在名册）⇒ 必须删行" % pid)

    stats = {"ledger": len(ledger), "checkers": len(have_checker), "exempt": len(exempt)}
    head = ["=== formal property ledger parity (v3.5 C-43) ===",
            "  名册 id=%d  实装 checker=%d  登记错配=%d  未登记错配=%d  失效登记=%d"
            % (len(ledger), len(have_checker), len(exempt), len(gaps), len(stale))]
    for g in gaps:
        head.append("  [GAP-P] %s" % g)
    for s in stale:
        head.append("  [STALE] %s —— 留着就会掩盖将来重新出现的错配" % s)
    if gaps or stale:
        head.append("  修法:要么补 checker/入名册使两边一致，要么删掉失效登记行；"
                    "换名册内容触及 ADR-052/053 的 R2 解冻证据 ⇒ 需新 ADR，不得自决。")
        return 1, head, stats
    head.append("  [OK] 名册与实装 checker 对账闭合（登记的错配均仍成立）")
    return 0, head, stats


def main(argv):
    unknown = [a for a in argv if a not in ("--selftest", "--emit")]
    if unknown:
        print("[UNDECIDABLE] 未知参数: %s —— 不把拼错的旗标降级成默认裁决" % ",".join(unknown))
        print("用法: py -3 scripts/check_formal_property_ledger.py [--selftest|--emit]")
        return 2
    if "--selftest" in argv:
        return selftest()
    rc, out, _stats = check(ROOT, ROSTER, LEDGER_EXEMPT)
    for ln in out:
        print(ln)
    return rc


# ---------------------------------------------------------------------------
# --selftest：合成 root + 合成名册源，逐条锁“会红”的形状。
# 夹具数由本门自报（写死数字就会漂，同 G-92 的教训）。
# ---------------------------------------------------------------------------
_PROVIDER_TMPL = """pub struct EmptyFormalProvider;
impl FormalResultProvider for EmptyFormalProvider {
    fn collect_results(&self) -> Vec<(String, VerificationResult)> {
        vec![
%s
        ]
    }
}
"""


def _ids_block(ids):
    """必须与真源**同形**（id 独占一行）：否则名册解析读到 0 条，
    夹具会把“自己写错”当成“门退 2”而误判为有牙。"""
    return "\n".join(
        '            (\n                "%s".into(),\n'
        '                VerificationResult::Skipped {\n                    reason: "no data".into(),\n'
        '                },\n            ),' % i for i in ids)


def _mk(root, rel, text):
    path = os.path.join(root, *rel.split("/"))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def selftest():
    import shutil
    import tempfile

    ok_row = {"id": "ghost-prop", "kind": "NO-CHECKER",
              "reason": "夹具用：名册里有它但没有 checker；理由长度需过表自身的格式闸，"
                        "所以这里刻意写得比阈值长一些，以免把“夹具写错”读成“门没牙”。",
              "exit": "夹具用：补 checker 或删名册位（两条出口都写在字段里）"}
    ok_row2 = {"id": "unlisted-checker", "kind": "NOT-IN-LEDGER",
               "reason": "夹具用：有 checker 但不在名册；同样必须写满理由长度阈值，"
                         "好让本夹具的红只可能来自判据本身而不是格式噪声。",
               "exit": "夹具用：入名册或消 checker"}
    roster = [("alpha-prop", "crates/x/src/a.rs"), ("beta-prop", "crates/x/src/b.rs"),
              ("unlisted-checker", "crates/x/src/c.rs")]

    def root_with(ids, structs=("alpha-prop", "beta-prop", "unlisted-checker")):
        d = tempfile.mkdtemp(prefix="fplfix")
        _mk(d, PROVIDER, _PROVIDER_TMPL % _ids_block(ids))
        for name in ("alpha-prop", "beta-prop", "unlisted-checker"):
            rel = {"alpha-prop": "crates/x/src/a.rs", "beta-prop": "crates/x/src/b.rs",
                   "unlisted-checker": "crates/x/src/c.rs"}[name]
            body = "pub struct %sChecker;\n" % name.replace("-", "_") if name in structs else "// 无 pub struct\n"
            _mk(d, rel, body)
        return d

    cases = []

    def add(name, ids, table, want_rc, want_marker=None, structs=("alpha-prop", "beta-prop", "unlisted-checker")):
        d = root_with(ids, structs)
        try:
            rc, out, stats = check(d, roster, table)
        finally:
            shutil.rmtree(d, ignore_errors=True)
        text = "\n".join(out)
        why = []
        if rc != want_rc:
            why.append("rc=%s 期望 %s" % (rc, want_rc))
        if want_marker and want_marker not in text:
            why.append("诊断文本缺标记 %r" % want_marker)
        cases.append((name, "rc=%s, ledger=%s, checkers=%s%s"
                      % (rc, stats.get("ledger"), stats.get("checkers"),
                         (" <- " + "; ".join(why)) if why else ""), not why))

    # C1 无未登记错配 ⇒ 绿（roster 里的 unlisted-checker 已按 NOT-IN-LEDGER 登记，
    #     与真树同形；若不给登记，本例应红——那正是 C2 断言的判据）
    add("C1 对账一致且登记有效 ⇒ 绿",
        ["alpha-prop", "beta-prop"], [ok_row2], want_rc=0)
    # C2 名册新增一个没有 checker 也没有登记的 id ⇒ 必红
    add("C2 新名册位无 checker 未登记必红",
        ["alpha-prop", "beta-prop", "ghost-prop"], [],
        want_rc=1, want_marker="GAP-P")
    # C3 同上但带理由登记 ⇒ 绿（登记是消噪的唯一合法途径）
    add("C3 带理由登记放行",
        ["alpha-prop", "beta-prop", "ghost-prop"], [ok_row, ok_row2], want_rc=0)
    # C4 登记失效（名册里已无该 id）⇒ 必红，防僵尸登记掩盖复发
    add("C4 失效登记必红",
        ["alpha-prop", "beta-prop"], [ok_row, ok_row2],
        want_rc=1, want_marker="STALE")
    # C5 checker 文件里没有 pub struct（验证器被掏空）⇒ 必红
    add("C5 checker 无 pub struct 必红",
        ["alpha-prop", "beta-prop"], [ok_row2],
        want_rc=1, structs=("alpha-prop", "beta-prop"))
    # C6 checker 文件消失 ⇒ 退 2（读不出不等于“没违规”）
    d = root_with(["alpha-prop", "beta-prop"])
    os.remove(os.path.join(d, "crates", "x", "src", "c.rs"))
    rc6, out6, _ = check(d, roster, [ok_row2])
    shutil.rmtree(d, ignore_errors=True)
    cases.append(("C6 checker 文件缺失退 2", "rc=%s" % rc6, rc6 == 2))
    # C7 名册源缺失 ⇒ 退 2
    d = tempfile.mkdtemp(prefix="fplmiss")
    rc7, _, _ = check(d, roster, [])
    shutil.rmtree(d, ignore_errors=True)
    cases.append(("C7 名册源读不出退 2", "rc=%s" % rc7, rc7 == 2))
    # C8 豁免缺 exit 出口 ⇒ 退 2（登记必须写“怎么消解”）
    bad = dict(ok_row)
    bad.pop("exit")
    d = root_with(["alpha-prop", "beta-prop", "ghost-prop"])
    rc8, out8, _ = check(d, roster, [bad, ok_row2])
    shutil.rmtree(d, ignore_errors=True)
    cases.append(("C8 登记缺消解出口退 2", "rc=%s 含出口标记=%s"
                  % (rc8, "消解出口" in "\n".join(out8)),
                  rc8 == 2 and "消解出口" in "\n".join(out8)))
    # C9 未知旗标 ⇒ 退 2
    cases.append(("C9 未知旗标退 2", "rc=%s" % (r9 := main(["--porcelain"])), r9 == 2))
    # C10 真树判定：本轮事实（名册 7、checker 7、错配 2 且均已登记）应为绿
    rc10, out10, s10 = check(ROOT, ROSTER, LEDGER_EXEMPT)
    cases.append(("C10 真树对账闭合（错配已登记）",
                  "rc=%s, ledger=%s, checkers=%s" % (rc10, s10.get("ledger"), s10.get("checkers")),
                  rc10 == 0 and s10.get("ledger") == 7 and s10.get("checkers") == 7))

    print("=== G-95 selftest：形式化名册与实装 checker 对账门 ===")
    failed = 0
    for name, detail, ok in cases:
        print("  [%s] %s  %s" % ("OK" if ok else "FAIL", name, detail))
        if not ok:
            failed += 1
    print("  夹具 %d 项,失败 %d" % (len(cases), failed))
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
