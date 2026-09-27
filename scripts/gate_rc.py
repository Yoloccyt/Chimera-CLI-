"""门脚本的退出码兜底 —— 崩溃必须退 2, 不得借 1 冒充"判过且红"。

WHY 单一真值源：这条守卫此前**要么没有**（16 个门脚本），**要么各写一份**（1 个）。
F20 的教训是"同一事实存两份登记表就会分叉"，守卫逻辑同理，故收在此处只写一次。

退出码方言（本仓治理门共用）：
  0 = 通过   1 = 判红   2 = 不可判定（判据没真正跑完：用法/环境错误/崩溃）

★ F33（本文件要关的那条假绿通道）：Python 未捕获异常默认退 **1**，而
  `run_gate_manifest.py:366` 的判据是 `ok = rc == g["expect"]` —— 对 `expect = 1` 的门
  （G-10 audit_phaseR_spec、G-39 check_gate_predicates_parity），一次崩溃恰好产出 rc=1，
  于是"根本没给出判定"被记成"符合期望"，且 ENV-FAIL/自检签名分支只在 `ok` 为假时才走。
  崩溃伪装成红已经够糟（收口台账会把它当成待修红消化掉），在 expect=1 的门上还直接升级成假绿。

用法（每个门脚本的 __main__ 一行）：
    import gate_rc
    if __name__ == "__main__":
        sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
"""
import os
import sys
import traceback

UNDECIDABLE = 2


def _utf8_stdio():
    """把 stdout/stderr 改指 UTF-8（幂等，失败则维持原状）。

    WHY：Windows GBK/cp936 控制台上 print 一个 '\xb5'（criterion 的 µs 单位）或 CJK
    就会 UnicodeEncodeError——实测 2026-09-25 mode_slo 因此崩溃退 2。所有 python 门
    都经由 run()/install() 这两个共同咽喉，修一处而非 37 处。
    errors=replace 保证极端环境（流已关/不可重配）下不反向引入新崩溃。
    """
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError, OSError):
            pass


def install():
    """给**平铺式**门脚本装崩溃兜底（F37）：顶层直接执行判定、没有 `__main__` 可包的那一类。

    WHY 不用 run()：run() 需要一个可包装的入口函数, 而这类脚本的逻辑就在模块顶层;
    把它们函数化是一次纯风格的大改, 风险大于收益。excepthook 是同一语义的最小实现：
    未捕获异常 ⇒ 退 2, 不再借解释器默认的 1 冒充"判过且红"（在 `expect = 1` 的门上会直接升级成假绿）。

    三条限制必须知道：① 只兜**未捕获**异常, 脚本自己 `except` 掉的不经过这里；
    ② 解释器收尾期（finalization）抛出的异常不经过这里；
    ③ `SystemExit` 不触发 excepthook —— 这正是想要的（脚本主动 `sys.exit(2)` 属方言本身）。
    文案写 stderr：门的 stdout 纯净性另有约定（结构化输出通道不得混入横幅）。
    """
    def _hook(exc, value, tb):
        traceback.print_exception(exc, value, tb)
        sys.stderr.write("[UNDECIDABLE] gate crashed before producing a verdict: "
                         "%s: %s\n" % (exc.__name__, value))
        os._exit(UNDECIDABLE)

    _utf8_stdio()
    sys.excepthook = _hook


def run(fn):
    """执行门的判定, 把未捕获异常翻译成"不可判定"。

    SystemExit 原样放行：脚本内部主动 `sys.exit(2)`（缺数据文件等）已是正确方言,
    不得被本守卫改写成别的码。KeyboardInterrupt 不在 Exception 之内, 也原样上抛
    （退 130 与 0/1/2 天然可区分）。
    """
    try:
        _utf8_stdio()
        return fn()
    except SystemExit:
        raise
    except Exception as exc:
        traceback.print_exc()
        print("[UNDECIDABLE] gate crashed before producing a verdict: "
              "%s: %s" % (type(exc).__name__, exc))
        return UNDECIDABLE


def selftest():
    """守卫自身必须有执行证据, 不能只在注释里声称兜得住。"""
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

    def boom():
        raise TypeError("probe: unsupported operand")

    check("crash becomes undecidable", run(boom), 2)
    check("verdict 0 passes through", run(lambda: 0), 0)
    check("verdict 1 passes through", run(lambda: 1), 1)

    def explicit_exit():
        sys.exit(2)

    try:
        run(explicit_exit)
        check("in-repo sys.exit(2) is re-raised, not swallowed", "no-raise", "SystemExit")
    except SystemExit as e:
        check("in-repo sys.exit(2) is re-raised, not swallowed", e.code, 2)

    # install() 的断言必须走**真子进程**：excepthook 的语义就是"进程退出码", 在进程内测不到。
    import subprocess
    here = os.path.dirname(os.path.abspath(__file__))
    probe = "import gate_rc; gate_rc.install(); raise ValueError('probe crash')"
    p = subprocess.run([sys.executable, "-c", probe], cwd=here,
                       capture_output=True, text=True)
    check("install() turns an uncaught crash into rc=2 (flat gates)", p.returncode, 2)
    check("install() prints the undecidable label on stderr",
          "[UNDECIDABLE]" in p.stderr, True)

    ok_probe = "import gate_rc; gate_rc.install(); raise SystemExit(1)"
    q = subprocess.run([sys.executable, "-c", ok_probe], cwd=here,
                       capture_output=True, text=True)
    check("install() leaves SystemExit dialect untouched (1 stays 1)", q.returncode, 1)
    print("  RESULT: %s (%d assertions)" % ("PASS" if not bad else "FAIL", n))
    return 1 if bad else 0


if __name__ == "__main__":
    # 本模块自己的"判据"就是它的自测；旗标留空, 不需要 --selftest 双形态。
    sys.exit(run(selftest))
