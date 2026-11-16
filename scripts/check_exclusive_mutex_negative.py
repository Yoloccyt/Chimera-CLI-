#!/usr/bin/env python3
"""exclusive 资源互斥的**负控**（变异验证）—— 证明 ExclusiveSemanticsTests 有牙齿。

WHY 需要这个门（ADR-315 W-T1 验收矩阵「负例设计」列）：
`scripts/govd/tests/test_govd.py::ExclusiveSemanticsTests` 断言「同 `exclusive`
键的门在 jobs>1 下区间不重叠」。这类**并发断言**最大的风险是**恒真**——
测试永远通过、却什么都没锁。因此必须有第二道方法（变异验证）在同一批里
证明断言真的会因为被测机制消失而失败。

本门做的事：
1. 把 `scripts/govd` 整包复制到临时目录（真实源文件零改动）；
2. 在**副本**的 `cli.py` 里把互斥判定变异掉
   （`if t.exclusive:` → `if False:`：两道同键门落回自由任务池 ⇒ 应当并行）；
3. 在**正确的包路径**下（`scripts.govd.tests.test_govd`）跑
   `ExclusiveSemanticsTests`；
4. 期望：互斥那条**必须红**，其余三条（jobs=1 阳性对照 / 跨键并行 /
   无键并行）**必须仍绿**——若它们也红，说明变异破坏面过大、测试彼此不独立。

⚠️ 实施教训（负控自身踩过的坑，必须保留此约束）：
第一次写本门时，变异副本被以 `import govd.cli` 加载，而被测测试文件用的是
`from scripts.govd import cli` —— **两套不同的模块实例**，真实包覆盖了副本，
变异从未生效，负控因此"失败"（看起来像测试恒真，其实是负控方法错了）。
⇒ 本门必须以 `scripts.govd` 包名加载副本，并在输出里打印被测模块的 `__file__`
供人工核对；断言 `cli.__file__` 指向副本路径，否则直接判失败。

用法：python scripts/check_exclusive_mutex_negative.py
退出码：0 = 负控成立（测试有牙齿）；1 = 负控不成立（测试恒真或变异未生效）。
"""
from __future__ import annotations

import pathlib
import shutil
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

# 变异锚点：cli.py 里的互斥分组判定。
# WHY 改这一行而不是删整个分组逻辑：变异面越小，越能证明"失败是由互斥机制
# 的消失直接导致"，而不是被别的副作用带崩。
MUTATION_FROM = "if t.exclusive:"
MUTATION_TO = "if False:"


def main() -> int:
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="govd-negctl-", dir=str(ROOT / "tmp")))
    try:
        sp = tmp / "scripts"
        shutil.copytree(ROOT / "scripts" / "govd", sp / "govd",
                        ignore=shutil.ignore_patterns("__pycache__"))
        init = ROOT / "scripts" / "__init__.py"
        if init.exists():
            shutil.copy2(init, sp / "__init__.py")

        cli_p = sp / "govd" / "cli.py"
        src = cli_p.read_text(encoding="utf-8")
        if MUTATION_FROM not in src:
            print("[FAIL] 变异锚点未命中：cli.py 里找不到 %r" % MUTATION_FROM)
            print("       （锚点漂移会让本门静默失效——变异没发生，测试自然通过）")
            return 1
        cli_p.write_text(src.replace(MUTATION_FROM, MUTATION_TO, 1), encoding="utf-8")
        print("[INFO] 变异已注入副本：%s" % cli_p)

        sys.path.insert(0, str(tmp))
        for name in list(sys.modules):
            if name.startswith("scripts.govd") or name == "scripts":
                del sys.modules[name]
        from scripts.govd.tests import test_govd as tg

        # 关键守卫：被测模块必须来自副本，否则变异没生效。
        cli_file = pathlib.Path(tg.cli.__file__).resolve()
        if sp not in cli_file.parents:
            print("[FAIL] 被测 cli 模块不在副本内：%s" % cli_file)
            print("       （真实包覆盖了副本 ⇒ 变异从未生效 ⇒ 本负控无意义）")
            return 1
        print("[OK] 被测 cli 确认为副本：%s" % cli_file)

        suite = unittest.TestLoader().loadTestsFromTestCase(tg.ExclusiveSemanticsTests)
        result = unittest.TextTestRunner(verbosity=0, stream=open(
            sys.stdout.fileno(), "w", closefd=False)).run(suite)

        failed_names = {str(t).split(" ")[0] for t, _ in result.failures + result.errors}
        expect_red = "test_same_exclusive_key_never_overlaps_under_jobs_two"
        expect_green = {
            "test_same_exclusive_key_serial_under_jobs_one_positive_control",
            "test_different_exclusive_keys_still_run_in_parallel",
            "test_unconstrained_gates_are_not_serialised_by_the_mutex",
        }

        problems: list[str] = []
        if expect_red not in failed_names:
            problems.append("变异后 %s 仍为绿 ⇒ 互斥断言恒真（无牙齿）" % expect_red)
        for name in sorted(expect_green):
            if name in failed_names:
                problems.append("变异后 %s 变红 ⇒ 变异破坏面过大，测试彼此不独立" % name)

        if problems:
            for p in problems:
                print("[FAIL] %s" % p)
            print("[SUM] failures=%d errors=%d" % (len(result.failures), len(result.errors)))
            return 1

        print("[OK] 负控成立：互斥断言在机制消失时确实变红，其余三条保持绿")
        print("[SUM] failures=%d errors=%d（预期 1 failure / 0 errors）"
              % (len(result.failures), len(result.errors)))
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
