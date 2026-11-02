#!/usr/bin/env python3
# =============================================================================
# check_dev_dep_direction.py - "dev-dependencies 向上边门" (v3.5 C-52, G-93)
# =============================================================================
# Purpose: 依赖方向铁律(L(N) 只可依赖 L(N-1) 及以下)此前对 `[dev-dependencies]`
#          **机器不可见**：Check B(`check_dependency_rules.sh/.ps1`)只读 `[dependencies]`
#          段 ⇒ dev 段里的向上边只能靠"注释即治理"。v3.5 Spec 的 C-52 记录了两条真实
#          向上 dev 边(hcw-window→scc-cache L2→L3、parliament→quest-engine L8→L9),
#          并把"Check B 扩 dev-deps"列为需裁决项,**默认值 = 做,独立 id 观察一周期**。
#
# 为什么是"独立 id"而不是直接改 Check B(本轮刻意不碰那两条门)：
#   Check B 是 `.sh` + `.ps1` 双实现,有逐字等价性对账门(audit_gate_parity / G-34 族)
#   与之绑定；单侧改判据必触发 parity 红,双改则超出"观察一周期"的授权面。
#   故本门以**新增独立判据**承载同一事实面,不改 Check B 的语义。
#
# 判据：
#   边集合 = 每个 crates/*/Cargo.toml 的 dev 段(含 `[target.<cfg>.dev-dependencies]`)
#            中"被依赖者也是 workspace 成员"的边(外部依赖与自依赖不计)。
#   违规   = layer(dep) > layer(user)  (与 Check B 同向：只禁向上,同层/向下放行)。
#   层号   = **派生**自唯一权威源 `check_dependency_rules.sh::layer_of()`
#            (经 check_layer_map_parity.parse_sh；禁手抄层表,终局原则 1)。
#   存量   = `scripts/dev_dep_freeze.txt` 基线(**只减不增**)：新向上边未登记 ⇒ GAP 红;
#            登记行在盘上已不再向上 ⇒ STALE 红(必须同批删行,防豁免僵尸)。
#
# 观察期语义(为什么本门现在就是绿的)：盘上实测向上 dev 边恰 2 条,全部进基线并附理由;
#   其余 25 条同层/向下边由判据放行 ⇒ selftest T1/T7 锁住"不误伤"。观察一周期后
#   这 2 条要么消边(改 dev→prod 方向合法化/移测试),要么立 ADR 承认,不得长期滞留。
#
# 退出码：0=无未登记违规 / 1=GAP 或 STALE / 2=不可判定(权威层表读不出、成员集合为空、
#         基线文件缺失或畸形、未知旗标)。崩溃由 gate_rc 兜底退 2,绝不借 1。
# Usage：py -3 scripts/check_dev_dep_direction.py [--selftest] [--emit]
#         (--emit 只打印重生成的基线正文,须人工复核 diff 后手工落盘,防棘轮洗白)
# =============================================================================
import glob
import os
import re
import sys

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPTS)
BASELINE = os.path.join(SCRIPTS, "dev_dep_freeze.txt")
DEP_LINE = re.compile(r"^([A-Za-z0-9_-]+)\s*=")
SECTION = re.compile(r"^\[([^\]]+)\]$")


def load_layer_map(root=ROOT):
    """从唯一权威源派生 {crate: 层}。读不出 ⇒ 抛错(调用方转不可判定)。"""
    sys.path.insert(0, SCRIPTS)
    import check_layer_map_parity as _par
    sh = os.path.join(root, "scripts", "check_dependency_rules.sh")
    if not os.path.isfile(sh):
        raise RuntimeError("权威层表源文件缺失: %s" % sh)
    with open(sh, encoding="utf-8", errors="replace") as fh:
        case, _lst, _exp = _par.parse_sh(fh.read())
    if not case:
        raise RuntimeError("layer_of() case 块解析为空")
    return case


def is_dev_section(name):
    """`dev-dependencies` 与 `target.<cfg>.dev-dependencies` 都算 dev 段。

    WHY 收 target 限定形态：今天盘上 0 条(实测),但 cargo 合法语法;漏认它等于
    给未来留一条静默旁路(G-90 的 UNSUPPORTED_FORM 同族教训)。
    """
    return name == "dev-dependencies" or name.endswith(".dev-dependencies")


def parse_dev_edges(root=ROOT):
    """返回 (edges, members)。edges = set[(user, dep)]，只含内部(成员间)dev 边。"""
    manifests = sorted(glob.glob(os.path.join(root, "crates", "*", "Cargo.toml")))
    members = {os.path.basename(os.path.dirname(p)) for p in manifests}
    edges = set()
    for path in manifests:
        user = os.path.basename(os.path.dirname(path))
        section = None
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                s = line.strip()
                m = SECTION.match(s)
                if m:
                    section = m.group(1)
                    continue
                if section is None or not is_dev_section(section):
                    continue
                d = DEP_LINE.match(s)
                if not d:
                    continue
                dep = d.group(1)
                if dep in members and dep != user:
                    edges.add((user, dep))
    return edges, members


def load_baseline(path=None):
    """基线正文 = `user|dep` 行；# 注释与空行忽略。返回 (集合, 畸形行)。

    WHY 默认参写 `None` 而不是 `BASELINE`:默认参在定义时就绑定,拿全局变量做默认值
    会让夹具无法把基线路径换成不存在的文件(测不到 fail-closed 分支)。
    """
    path = path or BASELINE
    if not os.path.isfile(path):
        return None, ["基线文件缺失: %s" % path]
    rows, bad = set(), []
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            s = line.split("#", 1)[0].strip()
            if not s:
                continue
            parts = s.split("|")
            if len(parts) != 2 or not all(p.strip() for p in parts):
                bad.append("畸形基线行: %r" % line.rstrip("\n"))
                continue
            rows.add((parts[0].strip(), parts[1].strip()))
    return rows, bad


def judge(edges, members, layer_of, baseline):
    """纯函数判据(可被 --selftest 用合成输入驱动)。返回 (gaps, stale, undecidable)。"""
    gaps, stale, undecidable = [], [], []
    upward = set()
    for (user, dep) in sorted(edges):
        if user not in layer_of or dep not in layer_of:
            # 成员在权威层表里查不到 ⇒ 方向不可判。绝不能"查不到就当合规"。
            undecidable.append("层号缺失: %s -> %s" % (user, dep))
            continue
        if layer_of[dep] > layer_of[user]:
            upward.add((user, dep))
    for (user, dep) in sorted(upward):
        if (user, dep) not in baseline:
            gaps.append("%s(L%s) -> %s(L%s) 是向上 dev 边且未登记"
                        % (user, layer_of[user], dep, layer_of[dep]))
    for (user, dep) in sorted(baseline):
        if (user, dep) not in upward:
            why = ("盘上已不存在该 dev 边" if (user, dep) not in edges
                   else "该边已不再向上(同层或向下)")
            stale.append("%s|%s 已失效：%s ⇒ 基线行必须同批删除" % (user, dep, why))
    if not members:
        undecidable.append("crates/*/Cargo.toml 成员集合为空 —— 边集不可信")
    return gaps, stale, undecidable


def render_baseline(edges, layer_of):
    """--emit：把当前向上 dev 边按 only-decrease 台账格式打印(仅正文,供人工复核)。"""
    out = []
    for (user, dep) in sorted(edges):
        if user in layer_of and dep in layer_of and layer_of[dep] > layer_of[user]:
            out.append("%s|%s" % (user, dep))
    return out


def main(argv):
    unknown = [a for a in argv if a not in ("--selftest", "--emit")]
    if unknown:
        print("[UNDECIDABLE] 未知参数: %s —— 不把拼错的旗标降级成默认裁决" % ",".join(unknown))
        print("用法: py -3 scripts/check_dev_dep_direction.py [--selftest|--emit]")
        return 2
    if "--selftest" in argv:
        return selftest()
    try:
        layer_of = load_layer_map()
    except Exception as exc:  # noqa: BLE001 - 判据不可信一律退 2,不猜
        print("[UNDECIDABLE] 权威层表派生失败: %s" % exc)
        return 2
    edges, members = parse_dev_edges()
    baseline, bad = load_baseline(BASELINE)
    if bad:
        for b in bad:
            print("[CONFIG] %s" % b)
        return 2
    if "--emit" in argv:
        print("=== dev upward edges (only-decrease ledger body; 人工复核后手工落盘) ===")
        for row in render_baseline(edges, layer_of):
            print(row)
        print("=== 内部 dev 边共 %d 条,其中向上 %d 条 ==="
              % (len(edges), len(render_baseline(edges, layer_of))))
        return 0
    gaps, stale, undecidable = judge(edges, members, layer_of, baseline)
    print("=== dev-dependency direction gate (v3.5 C-52 / G-93) ===")
    print("  成员=%d 内部 dev 边=%d 基线登记=%d" % (len(members), len(edges), len(baseline)))
    if undecidable:
        for u in undecidable:
            print("  [UNDECIDABLE] %s" % u)
        return 2
    for g in gaps:
        print("  [GAP-D] %s" % g)
    for s in stale:
        print("  [STALE] %s" % s)
    if gaps or stale:
        print("  修法:把 dev 依赖改为同层/向下(或把该能力正式接入生产面并同批登记 "
              "dep_edge_freeze.txt)，删除边则同批删基线行；新增向上边必须先有裁决。")
        return 1
    print("  [OK] 无未登记的向上 dev-dependency 边(存量 2 条已在基线并附裁决去向)")
    return 0


# ---------------------------------------------------------------------------
# --selftest：全部用合成输入驱动 judge()/parse_dev_edges(),不碰真树。
# 每条都注明"判据改坏时本例会怎么红"——门必须先证明自己会红。
# ---------------------------------------------------------------------------
def selftest():
    import shutil
    import tempfile

    layers = {"a-core": 1, "b-mid": 2, "c-top": 3, "same-1": 2, "same-2": 2}
    BL_EMPTY = set()
    failed = []

    def check(name, cond, detail=""):
        print("  [%s] %s%s" % ("OK" if cond else "FAIL", name, ("  <- " + detail) if detail and not cond else ""))
        if not cond:
            failed.append(name)

    # T1 全为向下/同层边 ⇒ 无违规(误报防线：25 条同层/向下边不得被判红)
    edges = {("c-top", "b-mid"), ("b-mid", "a-core"), ("same-1", "same-2")}
    gaps, stale, und = judge(edges, set(layers), layers, BL_EMPTY)
    check("T1 同层/向下边不误伤", not gaps and not stale and not und, str(gaps))

    # T2 向上边未登记 ⇒ GAP(反向牙齿：删掉未登记判定这条必红)
    edges = {("a-core", "c-top")}
    gaps, stale, und = judge(edges, set(layers), layers, BL_EMPTY)
    check("T2 新向上边未登记必红", len(gaps) == 1 and not stale, str(gaps))

    # T3 登记后放行
    gaps, stale, und = judge(edges, set(layers), layers, {("a-core", "c-top")})
    check("T3 已登记存量放行", not gaps and not stale, str(gaps + stale))

    # T4 基线幽灵行(边已不存在)⇒ STALE 必红
    gaps, stale, und = judge(set(), set(layers), layers, {("a-core", "c-top")})
    check("T4 幽灵基线必红", len(stale) == 1 and not gaps, str(stale))

    # T5 基线行对应的边已不再向上 ⇒ 同样 STALE 必红(否则"改好了"仍留着豁免)
    edges = {("c-top", "a-core")}
    gaps, stale, und = judge(edges, set(layers), layers, {("a-core", "c-top")})
    check("T5 已消边未删行必红", len(stale) == 1, str(stale))

    # T6 成员在权威层表查不到 ⇒ 不可判定(绝不"查不到就当合规")
    edges = {("a-core", "ghost-dep")}
    gaps, stale, und = judge(edges, {"a-core", "ghost-dep"}, layers, BL_EMPTY)
    check("T6 层号缺失退不可判定", len(und) == 1, str(und))

    # T7 成员集合为空 ⇒ 不可判定(防止 glob 失效被读成"零违规")
    gaps, stale, und = judge(set(), set(), layers, BL_EMPTY)
    check("T7 成员集为空退不可判定", len(und) == 1, str(und))

    # T8/T9/T10 解析层：真临时目录造 Cargo.toml,验 dev 段抽取与形态覆盖
    d = tempfile.mkdtemp(prefix="c52fix")
    try:
        def mk(crate, text):
            p = os.path.join(d, "crates", crate)
            os.makedirs(p, exist_ok=True)
            with open(os.path.join(p, "Cargo.toml"), "w", encoding="utf-8") as fh:
                fh.write(text)

        mk("a-core", "[package]\nname = \"a-core\"\n"
                     "[dependencies]\nnexus-contracts = { path = \"../nexus-contracts\" }\n"
                     "[dev-dependencies]\nb-mid = { path = \"../b-mid\" }\n"
                     "serde_json = \"1\"\n"
                     "[target.'cfg(windows)'.dev-dependencies]\nc-top = \"1\"\n")
        mk("b-mid", "[package]\nname = \"b-mid\"\n[dev-dependencies]\nb-mid = { path = \".\" }\n")
        mk("c-top", "[package]\nname = \"c-top\"\n[build-dependencies]\nb-mid = \"1\"\n")
        mk("nexus-contracts", "[package]\nname = \"nexus-contracts\"\n")
        edges, members = parse_dev_edges(d)
        check("T8 dev 段内部边抽出且外部依赖不计",
              ("a-core", "b-mid") in edges and "serde_json" not in {x for _, x in edges}
              and ("a-core", "c-top") in edges, str(sorted(edges)))
        check("T9 target 限定 dev 段不得静默漏检", ("a-core", "c-top") in edges,
              str(sorted(edges)))
        check("T10 build-dependencies/自依赖不计入 dev 边",
              ("b-mid", "b-mid") not in edges and ("c-top", "b-mid") not in edges,
              str(sorted(edges)))
    finally:
        shutil.rmtree(d, ignore_errors=True)

    # T11 未知旗标 ⇒ 退 2(入口契约 C3：不降级为默认裁决)
    check("T11 未知旗标退 2", main(["--silently-typo"]) == 2)
    # T12 权威层表读不出 ⇒ 退 2(把 load_layer_map 换成坏 root)
    saved = load_layer_map
    try:
        globals()["load_layer_map"] = lambda root=ROOT: (_ for _ in ()).throw(
            RuntimeError("fixture: 层表不可用"))
        check("T12 层表派生失败退 2", main([]) == 2)
    finally:
        globals()["load_layer_map"] = saved
    # T13 基线缺失 ⇒ 退 2(fail-closed，不许"没有台账=没有违规")
    saved_baseline = BASELINE
    try:
        globals()["BASELINE"] = os.path.join(SCRIPTS, "dev_dep_freeze_missing.txt")
        check("T13 基线缺失退 2", main([]) == 2)
    finally:
        globals()["BASELINE"] = saved_baseline

    print("  夹具 %d 项,失败 %d" % (13, len(failed)))
    return 0 if not failed else 1


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
