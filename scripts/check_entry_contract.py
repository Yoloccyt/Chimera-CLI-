"""门脚本"入口契约"面检查（F32 → F38 提升为常驻门 G-43）—— 四族缺陷一次性扫描。

起因：F24（run_gate_manifest 未知旗标静默跑全部）与 F29（check_perf_redlines 未知旗标
降级成默认模式）是同族第二次命中 ⇒ "发现一处只治一处"本身是缺陷。本门把四族形态
一次性扫全库 scripts/*.py 并逐条裁决。

四族：
  C1 rc-as-counter   : return/sys.exit 的实参是**被累加过的**计数变量（F18 形态）
  C2 crash-is-red    : Python 门脚本无 __main__ 异常兜底 ⇒ 崩溃默认退 1 伪装成红（F30/F37 形态）
  C3 silent-default  : 读 argv 做旗标判断却无"未知旗标"拒收路径（F24/F29/F35 形态）
  C4 label-mismatch  : 打印 [FAIL] 的那条分支退出码是 2（标签与码相反，F31/F36 形态）

为什么必须常驻（F38 的动机）：F30/F32/F35/F36/F37 五轮都在同一张面上修，而承载判据的
探针一直躺在 gitignore 的 tmp/ 里、没有任何东西调用它 ⇒ 前八轮的修复**没有回归通道**，
任何人改坏一处都不会被发现。门本身才是"修好了"的一部分。

退出码方言（与其它门一致）：0 全绿 / 1 有未登记发现或基线 STALE / 2 不可判定
（自检不过、论域读不出、基线缺失、用法错误）。命中项一律打印语义键（不含行号）。

自检纪律（本仓 §17.3 / §64.6 立的规矩）：每族各配正样本 + 负样本 + 论域判据夹具 +
键/基线夹具，自检不过 ⇒ 拒绝输出结论并退 2。
"""
import ast
import io
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPTS = os.path.join(ROOT, "scripts")

# 计数器的判定不靠命名启发（F38 起）：只看"被 += 累加过"这一条硬事实，见 _counter_names。
EXIT2 = ("return 2", "sys.exit(2)", "exit 2")


def _counter_names(fn):
    """返回该函数内**被累加过**的整数值名字集合（= F18 形态的"计数变量"）。

    WHY 必须是"累加过"而不是"取过整数字面量"（F38 实测收窄）：
      - `gaps = [] ... return gaps` 返回容器 ⇒ 不是缺陷（下面用 cols 排除）；
      - `status = 1 ... return status` 是**状态量**（绿路径提前 return 后才走到这里），
        永远不会承载条数 ⇒ 也不是缺陷。上一轮 `check_declared_dep_usage.py` /
        `check_dep_edge_freeze.py` 各被记一条这样的假阳性，正是因为旧判据把
        "赋过整数字面量 + 名字像复数"当成了计数器（`status` 恰好以 `s` 结尾）。
      - 只有 `x += ...` 过的整数才可能把"发现条数"当退出码 —— 那才是 rc-as-counter。
      收窄后不丢任何当前在盘真命中：旧判据命中的三类里，未累加的两类按定义不承载计数。
    """
    ints, cols, accum = set(), set(), set()
    for node in ast.walk(fn):
        if isinstance(node, ast.Assign):
            val = node.value
            is_int = isinstance(val, ast.Constant) and isinstance(val.value, int)
            is_col = isinstance(val, (ast.List, ast.Dict, ast.Set, ast.ListComp,
                                       ast.DictComp, ast.SetComp, ast.Call))
            for t in node.targets:
                if isinstance(t, ast.Name):
                    if is_int:
                        ints.add(t.id)
                    elif is_col:
                        cols.add(t.id)
        elif isinstance(node, ast.AugAssign) and isinstance(node.op, ast.Add):
            if isinstance(node.target, ast.Name):
                ints.add(node.target.id)          # x += 1 => 计数器
                accum.add(node.target.id)
    return (ints - cols) & (accum - cols)


def _entry_names(tree):
    """`if __name__ == '__main__'` 块里出现的**所有**被直接调用的函数名。

    WHY 是集合而不是单个名字：接上 gate_rc 之后入口表达式变成
    `sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))`，"取第一个 Name 调用"会解析到
    别的名字而丢掉真正的 `main` ⇒ C1 从 3 悄悄掉到 1（假阴性，我自己修东西修出来的）。
    面检查在改完被检物之后必须重跑，正是为了抓这种"探测器跟着被测物一起变形"。
    两种接线写法都要认：`gate_rc.run(main)`（main 作为**实参**出现，不是调用）与
    `gate_rc.run(lambda: main(sys.argv[1:]))`（main 是被调用的 Name）。
    """
    names = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.If):
            continue
        if "__name__" not in ast.dump(node.test) or "__main__" not in ast.dump(node.test):
            continue
        for call in [n for n in ast.walk(node) if isinstance(n, ast.Call)]:
            if isinstance(call.func, ast.Name):
                names.add(call.func.id)
            for arg in call.args:
                if isinstance(arg, ast.Name):
                    names.add(arg.id)
    return names


def _call_graph(tree):
    g = {}
    for fn in [n for n in ast.walk(tree) if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))]:
        g[fn.name] = {c.func.id for c in ast.walk(fn)
                      if isinstance(c, ast.Call) and isinstance(c.func, ast.Name)}
    return g


def c1_rc_as_counter(src):
    """精确形态：**从入口可达**的函数里 `return <累加过的整数>`（F18 的 rc-as-counter）。

    只报"可能成为退出码且真的承载条数"的那些；容器/字符串返回、未累加的状态量
    （`status = 1 ... return status`）、不可达辅助函数一律不报（见 _counter_names）。
    """
    hits = []
    try:
        tree = ast.parse(src)
    except SyntaxError as e:
        return [("SYNTAX", e.lineno or 0, "parse failed - check skipped")]
    entries = _entry_names(tree)
    if not entries:
        return []
    graph = _call_graph(tree)
    seen, stack = set(entries), list(entries)
    while stack:
        for callee in graph.get(stack.pop(), ()):
            if callee not in seen:
                seen.add(callee)
                stack.append(callee)
    fns = {f.name: f for f in ast.walk(tree)
           if isinstance(f, (ast.FunctionDef, ast.AsyncFunctionDef))}
    for name in sorted(seen & set(fns)):
        counters = _counter_names(fns[name])
        for node in ast.walk(fns[name]):
            if isinstance(node, ast.Return) and isinstance(node.value, ast.Name):
                v = node.value.id
                if v in counters:
                    hits.append((name, node.lineno, "return %s" % v))
            if isinstance(node, ast.Call) and getattr(node.func, "attr", "") == "exit":
                if node.args and isinstance(node.args[0], ast.Name) and node.args[0].id in counters:
                    hits.append((name, node.lineno, "exit(%s)" % node.args[0].id))
    return hits



def c2_crash_is_red(src):
    """无 __main__ 兜底的 Python 门脚本: 崩溃 => 解释器退 1, 与'判定为红'同码。"""
    # 平铺门脚本（顶层直接执行）的合规形态是 gate_rc.install() 的 excepthook。
    # ★ 必须排在"无 __main__"分支**之前**：那 6 个脚本正是没有 __main__ 的，
    #   放后面就等于永远认不到（顺序即判据，与 F33 同形）。
    if "gate_rc.install()" in src:
        return []
    if 'if __name__' not in src:
        return ["no __main__ block (library or wrapper)"]
    tail = src[src.index("if __name__"):]
    if "try" in tail and "UNDECIDABLE" in tail:
        return []
    # F30 把守卫收进 gate_rc 单点后，合规形态变成"入口交给 gate_rc.run"。
    # 不认这一种 = 探针跟着被测物一起变形：接完守卫反而给 17 个已合规脚本记缺陷。
    if "gate_rc.run(" in tail:
        return []
    # 平铺门脚本（顶层直接执行）没有 __main__ 可包，合规形态是 gate_rc.install() 的 excepthook。
    # 同理必须认，否则 F37 修完又会给 6 个脚本记假阳性。
    if "gate_rc.install()" in src:
        return []
    return ["__main__ has no crash->2 guard"]


def c3_silent_default(src):
    """读 argv 且以'旗标等值测试'分派, 但全文没有未知旗标拒收路径。"""
    if "sys.argv" not in src:
        return ["not a CLI entry"]
    has_flag_test = re.search(r'in\s+(argv|flags|args\b)', src) is not None
    # 拒收路径的三种本仓写法都算：显式 unknown 变量、[UNDECIDABLE] 标签、
    # usage/用法 + 紧邻退 2（check_crate_reachability 是第三种）。少认一种 = 把已合规的
    # 脚本报成缺陷，而面检查的假阳性同样有代价：它引导人去改不需要改的东西。
    has_reject = ("unknown" in src.lower()
                  or "未知" in src
                  or "UNDECIDABLE" in src
                  or (re.search(r'usage[:s]|用法', src) is not None
                      and any(t in src for t in EXIT2)))
    if has_flag_test and not has_reject:
        return ["flag dispatch without an unknown-flag reject path"]
    return []


def c4_label_mismatch(src):
    """`[FAIL]` 标签**所在分支**的退出码是 2 ⇒ 标签与码相反。

    ★ 不能按行距绑。`run_gate_manifest.py:423` 的 `[FAIL] 与期望不符` 配的是 `return 1`,
      同函数三行后的 `return 2` 属**另一个分支**（unknown/env_fail）—— 距离启发式把这条
      读成缺陷, 是纯假阳性。语义是"这条消息退出时门给什么码", 所以绑"同缩进、紧随其后的
      第一条 return/exit"。
    ★ 但"含 [FAIL]"还不够: 标签必须**真的会被打印**, 且绑定的语句必须在**语句位**。
      否则本函数自己的文档串就成了一条命中（它同时提到 `[FAIL]` 与 `return 2`）——
      这与 check_doc_symbol_claims 的 "prose 不误报" 夹具是同一条纪律: 判据要看得懂
      "门在输出"与"人在解释"的区别, 而不是看见字符串就判红。
    """
    lines = src.splitlines()
    hits = []
    for i, ln in enumerate(lines):
        stripped = ln.strip()
        if "[FAIL]" not in ln or stripped.startswith("#"):
            continue
        if not any(tok in ln for tok in ("print(", "sys.stdout.write", "sys.stderr.write")):
            continue                            # 注释/文档串里的 [FAIL] 不是门的输出
        indent = len(ln) - len(ln.lstrip())
        for j in range(i + 1, min(i + 6, len(lines))):
            body = lines[j].strip()
            if not body or body.startswith("#"):
                continue                        # 注释 / 空行
            if len(lines[j]) - len(lines[j].lstrip()) < indent:
                break                           # 已离开本分支
            if body.startswith("return") or body.startswith("sys.exit("):
                if any(tok in body for tok in EXIT2):
                    hits.append((i + 1, stripped[:60]))
                break                           # 只看第一条: 后面的是别的分支
    return hits


# --- 自检：每族若干正/负样本（正=必须命中, 负=必须不命中） --------------------
POS = {
    "C1": ("import sys\ndef m():\n    fails = 0\n    fails += 1\n    return fails\n"
           "def main():\n    return m()\n"
           "if __name__ == '__main__':\n    sys.exit(main())\n", 1),
    "C2": ("import sys\nif __name__ == '__main__':\n    sys.exit(main())\n", 1),
    "C3": ("import sys\nargv = sys.argv\nif '--selftest' in argv:\n    pass\n", 1),
    "C4": ("print('[FAIL] no input')\nreturn 2\n", 1),
}
NEG = {    # 返回**容器**不是缺陷；只有把条数当退出码才是（F18 的精确形态）
    "C1": [("import sys\ndef m():\n    gaps = []\n    gaps.append(1)\n    return gaps\n"
            "def main():\n    return 1 if m() else 0\n"
            "if __name__ == '__main__':\n    sys.exit(main())\n", 0, "container-return"),
           # 从入口不可达的计数器辅助函数, 不会成为 rc
           ("import sys\ndef helper():\n    total = 0\n    total += 1\n    return total\n"
            "def main():\n    return 0\n"
            "if __name__ == '__main__':\n    sys.exit(main())\n", 0, "unreachable"),
           ("def m():\n    fails = 0\n    return 1 if fails else 0\n", 0, "normalized"),
           # 状态量（F38 实测：check_declared_dep_usage / check_dep_edge_freeze 各一条假阳性）：
           # 绿路径已提前 return，走到这里时 status 恒为 1，不承载条数 ⇒ 不是 rc-as-counter。
           ("import sys\ndef main():\n    status = 1\n    if status:\n        print('[FAIL] x')\n"
            "    return status\n"
            "if __name__ == '__main__':\n    sys.exit(main())\n", 0, "constant-state-local")],
    "C2": [("import sys, traceback\nif __name__ == '__main__':\n"
            "    try:\n        sys.exit(main())\n    except Exception:\n"
            "        traceback.print_exc()\n        print('[UNDECIDABLE] crashed')\n"
            "        sys.exit(2)\n", 0, "guarded"),
           # 外置单点守卫（F30 定式）：合规，且 POS["C2"] 保证未接线仍被抓。
           ("import sys\nimport gate_rc\nif __name__ == '__main__':\n"
            "    sys.exit(gate_rc.run(main))\n", 0, "gate_rc-guarded"),
           # 平铺脚本（无 __main__）+ excepthook：F37 的合规形态, 顺序在"无 __main__"分支之前。
           ("import sys\nimport gate_rc\ngate_rc.install()\nprint('judged')\n",
            0, "flat-install-guarded")],
    "C3": [("import sys\nargv = sys.argv\nunknown = set(argv) - KNOWN\n"
            "if unknown:\n    print('UNDECIDABLE')\n    sys.exit(2)\n", 0, "rejects")],
    "C4": [("print('[UNDECIDABLE] no input')\nreturn 2\n", 0, "aligned"),
           # 距离 ≤3 行但**不同分支**：[FAIL] 配 return 1，return 2 属后面的另一分支。
           ("import sys\ndef main(a):\n    if a:\n        print('[FAIL] mismatch')\n"
            "        return 1\n    if a:\n        return 2\n", 0, "other-branch"),
           # prose 不误报：文档串/注释里同时写 `[FAIL]` 与 `return 2` 不是门的输出。
           # 本函数自己的 docstring 就是这个形状 —— 上一轮它给自己记了一条假阳性（F38 实测）。
           ('def f(a):\n    """判据: 看到 [FAIL] 配 return 2 就是缺陷"""\n'
            "    return 1 if a else 0\n", 0, "prose-mention")],
}
CHECKS = {"C1": c1_rc_as_counter, "C2": c2_crash_is_red,
          "C3": c3_silent_default, "C4": c4_label_mismatch}
# 第二个正样本：入口被 gate_rc 包住时**仍必须**抓到（本轮真实教训：接完守卫后 C1 从 3
# 悄悄掉到 1，就是"探测器跟着被测物一起变形"的假阴性，不是问题变少了）。
POS2 = [("C1", "import sys\nimport gate_rc\ndef main(a):\n    fails = 0\n    fails += 1\n"
                "    return fails\n"
                "if __name__ == '__main__':\n"
                "    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))\n", 1),
        ("C1", "import sys\nimport gate_rc\ndef main():\n    fails = 0\n    fails += 1\n"
               "    return fails\n"
               "if __name__ == '__main__':\n"
               "    sys.exit(gate_rc.run(main))\n", 1),
        # 分支内、且 print 跨两行：收紧后的 C4 仍必须抓到（防"修假阳性时顺手把牙磨掉"）
        ("C4", "import sys\ndef main(a):\n    if a:\n"
               "        print(f'[FAIL] no dir {a} '\n"
               "              'undeterminable')\n        return 2\n", 1)]


def _n(fn, src):
    r = fn(src)
    return len(r) if isinstance(r, list) and r and isinstance(r[0], tuple) else (1 if r else 0)


def selfcheck():
    ok = True
    for key, fn in CHECKS.items():
        src, want = POS[key]
        if _n(fn, src) != want:
            print("  [SELFTEST-POS] %s got=%d want=%d" % (key, _n(fn, src), want))
            ok = False
        for src, want, label in NEG[key]:
            if _n(fn, src) != want:
                print("  [SELFTEST-NEG] %s/%s got=%d want=%d"
                      % (key, label, _n(fn, src), want))
                ok = False
    for key, src, want in POS2:
        if _n(CHECKS[key], src) != want:
            print("  [SELFTEST-POS2] %s got=%d want=%d" % (key, _n(CHECKS[key], src), want))
            ok = False
    # 论域判据（F37）：同一条 "no __main__" 消息, 在"被清单执行的门"上必须是发现, 在库脚本上才可抑制。
    # 原实现无条件抑制 ⇒ 6 个门脚本缺崩溃守卫被读成"干净"。两侧都必须钉住, 否则这条判据没有牙。
    for msg, in_reg, want_suppress in (("no __main__ block (library or wrapper)", True, False),
                                       ("no __main__ block (library or wrapper)", False, True),
                                       ("not a CLI entry", True, True),
                                       ("not a CLI entry", False, True),
                                       ("__main__ has no crash->2 guard", True, False),
                                       ("[FAIL] x", False, False)):
        if is_non_program_finding(msg, in_reg) != want_suppress:
            print("  [SELFTEST-SCOPE] msg=%r in_registry=%s got=%s want=%s"
                  % (msg[:22], in_reg, is_non_program_finding(msg, in_reg), want_suppress))
            ok = False
    # 键稳定性（F2 解药自身也要有夹具）：同一语义、不同行号/空白 ⇒ 必须同键。
    # 若不钉这条，基线会在一次无关的上移下移后同时报"新增未登记"和"STALE"，
    # 而这正是 lock_await / dep_edge_freeze 行号键表反复付出的零语义价值仪式。
    k_a = make_key("x.py", "C1", "main:L42", "return   status")
    k_b = make_key("x.py", "C1", "main:L777", "return status")
    if k_a != k_b:
        print("  [SELFTEST-KEY] line-sensitive key: %r != %r" % (k_a, k_b))
        ok = False
    # 基线三向拆分：漏一项就会把"豁免垃圾桶"当成正常工作。
    hit = {k_a, "y.py::C3::reads argv without unknown-flag rejection"}
    supp, unres, stale = diff_baseline(hit, {k_a, "old.py::C1::gone::return 1"})
    if (supp, unres, stale) != ([k_a], ["y.py::C3::reads argv without unknown-flag rejection"],
                                ["old.py::C1::gone::return 1"]):
        print("  [SELFTEST-BASE] three-way split wrong: %r" % ((supp, unres, stale),))
        ok = False
    return ok



def load_registered_scripts():
    """论域 = 清单真的当**程序**执行的脚本名。

    WHY 必须从清单取而不是从内容猜：同一个文件在"库"与"程序"两种身份下，
    入口契约的结论正好相反（F37）。读法照生产门的口径 `utf-8-sig`（本仓 manifest 有 BOM 前科）。
    """
    path = os.path.join(SCRIPTS, "gate_manifest.toml")
    try:
        import tomllib
        doc = tomllib.loads(io.open(path, encoding="utf-8-sig").read())
    except Exception as exc:                       # noqa: BLE001 - 取不到论域就必须停, 不得静默当"全都不是门"
        print("[UNDECIDABLE] 无法读取 gate_manifest 论域: %s —— 拒绝出数" % exc)
        return None
    names = set()
    for g in doc.get("gate", []):
        for m in re.finditer(r"([\w\-/.]+\.py)", g.get("cmd", "")):
            names.add(os.path.basename(m.group(1)))
    return names


def is_non_program_finding(msg, in_registry):
    """区分"判据不适用"与"真缺陷"。

    - C3 的 `not a CLI entry`：该脚本根本不读 argv ⇒ 无"未知旗标静默降级"可言，**任何情况下都不是缺陷**
      （把它当缺陷会给 4 个已接守卫的登记门记假阳性 —— 我上一轮就犯了这个，见报告 §69.3）。
    - C2 的 `no __main__ block`：库/工具脚本无所谓；但**被清单当程序执行**的平铺门脚本，
      没有 `__main__` 就没有崩溃守卫 ⇒ 真缺陷（F37）。
    """
    if msg.startswith("not a CLI"):
        return True
    if msg.startswith("no __main__"):
        return not in_registry
    return False


BASELINE = os.path.join(SCRIPTS, "entry_contract_baseline.txt")
KNOWN_FLAGS = {"--selftest", "--emit-keys"}


def normalize_frag(s):
    """规范化语句指纹：去掉行号（`L123` / `:L123`）并折叠空白。

    WHY 行号必须消失：F2 病 —— 以行号为键的冻结表在代码上下平移一行时会同时产生
    "新键未登记"与"旧键 STALE"两条失败，每次都得人工重读语义才能确认"语义未变"。
    """
    t = re.sub(r":?\bL\d+\b", " ", s)
    return re.sub(r"\s+", " ", t).strip()


def make_key(fname, kind, *parts):
    """基线键 = `file::族::函数名::规范化语句`（不含行号，见 normalize_frag）。"""
    return "::".join([fname, kind] + [normalize_frag(p) for p in parts if p])


def diff_baseline(keys, baseline):
    """命中集合 vs 基线 -> (被抑制, 未决, STALE) 三向拆分。

    WHY 三向而不是两向：只报"新增未登记"会让基线腐烂成单向的豁免垃圾桶 ——
    已被修好的项留在表里没人发现。口径与 G-11/可达性棘轮一致：只减不增 + 失配必报。
    """
    hit = set(keys)
    suppressed = sorted(k for k in hit if k in baseline)
    unresolved = sorted(k for k in hit if k not in baseline)
    stale = sorted(k for k in baseline if k not in hit)
    return suppressed, unresolved, stale


def load_baseline():
    """读基线。返回 (键集合, 缺理由清单, 重复清单)；文件缺失/不可读返回 None（不可判定）。

    WHY 缺理由与重复都算红（不是"警告"）：豁免通道一旦可以匿名，就会被用来藏下一次真缺陷
    （F24/F29 的"静默降级"是同族形态）。
    """
    if not os.path.isfile(BASELINE):
        print("[UNDECIDABLE] baseline missing: %s" % BASELINE)
        print("        没有基线就无法区分「新增发现」与「已定性假阳性」⇒ 拒绝出数。")
        print("        重建: python scripts/check_entry_contract.py --emit-keys")
        return None
    try:
        lines = io.open(BASELINE, encoding="utf-8-sig").read().splitlines()
    except Exception as exc:                        # noqa: BLE001 - 读不到基线就是不可判定
        print("[UNDECIDABLE] baseline unreadable: %s" % exc)
        return None
    keys, no_reason, dup = set(), [], []
    for i, line in enumerate(lines, 1):
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        key, sep, reason = s.partition("|")
        key = key.strip()
        if not sep or not reason.strip():
            no_reason.append(key or "<line %d>" % i)
        if key in keys:
            dup.append(key)
        keys.add(key)
    return keys, no_reason, dup


def scan(registered):
    """扫 scripts/*.py，只产事实不裁决。返回 (findings, 扫描文件数)。

    finding = (file, kind, key, msg)，其中 msg 的**未加注解前缀**供论域判据使用。
    """
    out = []
    files = sorted(f for f in os.listdir(SCRIPTS) if f.endswith(".py"))
    for f in files:
        src = io.open(os.path.join(SCRIPTS, f), encoding="utf-8-sig").read()
        in_reg = f in registered
        raw = []
        for fn_name, ln, frag in c1_rc_as_counter(src):
            raw.append(("C1", "%s -> %s" % (fn_name, frag),
                        make_key(f, "C1", fn_name, frag),
                        "  (行号 %d 仅供人看, 不进键)" % ln))
        for msg in c2_crash_is_red(src):
            raw.append(("C2", msg, make_key(f, "C2", msg),
                        "  <== 被清单当程序执行, 缺 __main__ 即无崩溃守卫 (F37)"
                        if in_reg else ""))
        for msg in c3_silent_default(src):
            raw.append(("C3", msg, make_key(f, "C3", msg), ""))
        for ln, frag in c4_label_mismatch(src):
            raw.append(("C4", frag, make_key(f, "C4", frag), "  (行号 %d 仅供人看)" % ln))
        for kind, msg, key, annot in raw:
            if is_non_program_finding(msg, in_reg):
                continue
            out.append((f, kind, key, msg + annot))
    return out, len(files)


def cli(argv):
    # F24/F29 同族入口契约：未知旗标必须"没跑任何判据"并退 2，不得当作"没给旗标"顺手跑真扫描。
    unknown = sorted({a for a in argv if a.startswith("-") and a not in KNOWN_FLAGS})
    if unknown:
        print("[UNDECIDABLE] unknown flag(s): %s" % ", ".join(unknown))
        print("  usage: check_entry_contract.py [--selftest | --emit-keys]")
        return 2

    if "--selftest" in argv:
        print("=== selftest: entry-contract scanner must have teeth ===")
        ok = selfcheck()
        print("=== selftest result:", "ALL PASS" if ok else "FAIL", "===")
        return 0 if ok else 1

    print("=== entry-contract sweep (F32/F37/F38): selfcheck first ===")
    if not selfcheck():
        print("[UNDECIDABLE] scanner failed its own fixtures - refusing to report")
        return 2
    print("  selfcheck: 4 families x (positive + negative) + 论域判据 + 键/基线夹具 = OK")

    registered = load_registered_scripts()
    if registered is None:
        return 2
    findings, n_files = scan(registered)
    print("  论域: 清单执行 %d 个 .py; 目录内 .py %d 个; 命中 %d 条"
          % (len(registered), n_files, len(findings)))

    if "--emit-keys" in argv:
        for _f, _k, key, _m in sorted(findings):
            print(key)
        return 0

    loaded = load_baseline()
    if loaded is None:
        return 2
    baseline, no_reason, dup = loaded
    keys = [k for _f, _k, k, _m in findings]
    supp, unres, stale = diff_baseline(keys, baseline)

    for f, kind, key, msg in findings:
        print("[%s] %s::%s :: %s" % ("SUPPRESSED" if key in baseline else "FINDING",
                                     f, kind, msg))
    for k in stale:
        print("[STALE] %s  (基线里有、盘上已无匹配 ⇒ 该删, 留着就是豁免腐烂)" % k)
    for k in no_reason:
        print("[NO-REASON] %s  (豁免必须写理由)" % k)
    for k in dup:
        print("[DUP] %s" % k)

    totals = {}
    for f, kind, key, msg in findings:
        if key not in baseline:
            totals[kind] = totals.get(kind, 0) + 1
    print("unresolved by family: %s   suppressed=%d  stale=%d  no_reason=%d  dup=%d"
          % (totals, len(supp), len(stale), len(no_reason), len(dup)))

    if unres or stale or no_reason or dup:
        print("VERDICT: RED (%d unresolved / %d stale / %d no-reason / %d dup)"
              % (len(unres), len(stale), len(no_reason), len(dup)))
        print("  修法是改脚本(把 rc 当状态而非计数、补崩溃守卫、补未知旗标拒收、把标签写对),"
              " 不是往基线里加一行。")
        return 1
    print("VERDICT: GREEN (%d findings, all baseline-covered)" % len(findings))
    return 0


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: cli(sys.argv[1:])))
