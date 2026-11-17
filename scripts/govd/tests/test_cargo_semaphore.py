"""ADR-319 工具链批 TL-W3c: cargo 门应用层互斥（cargo_semaphore）锁。

背景：159 门中只有 4 道直接以 cargo 起头的门（G-12~G-15，恰好也是仅有的
exclusive = "cargo" 门）；其余 cargo 类门经 wrapper 间接调起，靠 target-dir
文件锁隐式等待。文件锁挡不住的是「4 个门各自拉起 4 个 rustc」的内存峰值叠加
（15.2GB 宿主内存墙前科：诊断报告 16 并发 → 0GB）。本锁在 executor 咽喉点对
argv[0] 基名为 cargo 的门加进程级 Semaphore(1)，与 .cargo/config.toml 的
jobs = 4 互补。

四条测试腿，缺一不可：
- 正控：两道 cargo 假任务并发，互斥体的 acquire/release 事件必须严格交替
  （第二道 acquire 晚于第一道 release）——锁的是「执行窗不重叠」，不是
  「调用窗不重叠」（被卡在 acquire 上的等待正是互斥在工作，从外部量调用区间
  只会把排队量成执行，仪器就失真了）；
- 负控：开关关闭时同一对任务不产生任何 semaphore 流量——证明上面的事件
  序来自本机制而不是 subprocess 自身的耗时；
- 单任务：开关打开时恰好一个 acquire/release 周期；
- 超时三态：permit 被占满时 acquire 按本门预算 1/10 封顶，超时记 UNKNOWN
  （rc=2），绝不死等（permit 泄漏的最坏形态是所有后续 cargo 门被静默吊死）。
"""
from __future__ import annotations

import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[3]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.govd import executor
from scripts.govd.model import Task, Verdict

POLICY = ROOT / "scripts" / "govd" / "policy.toml"


def make_task(**kw):
    base = {"id": "T-1", "cmd": "cargo --version", "expect": 0,
            "kind": "light", "spec_ref": "test", "note": "test"}
    base.update(kw)
    return Task(**base)


class TempDirTest(unittest.TestCase):

    def setUp(self):
        base = ROOT / "tmp"
        base.mkdir(exist_ok=True)
        self.tmp = Path(tempfile.mkdtemp(prefix="govd-sem-test-", dir=str(base)))
        # cargo --version 是规格允许的白名单探针；把本仓工具链放到 PATH 最前，
        # 保证探针解析到真实 cargo 而不是碰巧的环境残留。
        executor.augment_path()

    def tearDown(self):
        for p in sorted(self.tmp.rglob("*"), key=lambda x: len(str(x)), reverse=True):
            if p.is_file():
                p.unlink()
            elif not any(p.iterdir()):
                p.rmdir()
        self.tmp.rmdir()

    def log_for(self, task_id):
        return str(self.tmp / (task_id + ".log"))


class _SpySemaphore:
    """包一层真实 Semaphore(1)，给每次成功的 acquire/release 盖单调时戳。

    为什么必须包真锁而不是纯记录：正控要的是「两个线程真抢同一把锁」的事件序；
    纯记录器没有互斥语义，事件序恒为 acquire/acquire/release/release，
    永远测不出串行化是否生效。
    """

    def __init__(self):
        self._inner = threading.Semaphore(1)
        self.events = []  # [(kind, monotonic_ts)]
        self._lock = threading.Lock()

    def acquire(self, timeout=None):
        got = self._inner.acquire(timeout=timeout)
        if got:
            with self._lock:
                self.events.append(("acquire", time.monotonic()))
        return got

    def release(self):
        with self._lock:
            self.events.append(("release", time.monotonic()))
        self._inner.release()


class SemaphoreConfigTests(TempDirTest):
    """policy.toml [exec].cargo_semaphore 的读键语义与 shipped 配置锁。

    缺省 off 是本机制的纪律：互斥改变所有 cargo 门的执行时序，必须是
    policy.toml 里的显式 opt-in；显式 false 是规格承诺的回滚手段。
    """

    def _write_policy(self, body):
        path = self.tmp / "policy-case.toml"
        path.write_text(body, encoding="utf-8")
        return path

    def test_flag_true_enables(self):
        path = self._write_policy("[exec]\ncargo_semaphore = true\n")
        self.assertTrue(executor.cargo_semaphore_enabled(str(path)))

    def test_flag_false_disables(self):
        path = self._write_policy("[exec]\ncargo_semaphore = false\n")
        self.assertFalse(executor.cargo_semaphore_enabled(str(path)))

    def test_missing_key_defaults_off(self):
        path = self._write_policy("[exec]\ndefault_timeout_s = 300\n")
        self.assertFalse(executor.cargo_semaphore_enabled(str(path)))

    def test_unreadable_file_defaults_off(self):
        # 读不出 ⇒ 退回现状（不互斥），绝不让防护机制的失效形态把整轮门禁判死
        self.assertFalse(executor.cargo_semaphore_enabled(str(self.tmp / "absent.toml")))

    def test_shipped_policy_enables_the_mutex(self):
        """ shipped policy.toml 必须显式开——这条锁的是接线，不是读键逻辑。"""
        self.assertTrue(executor.cargo_semaphore_enabled(str(POLICY)))


class CargoArgvTests(unittest.TestCase):
    """argv[0] 基名判定的各形态：绝对路径 / cargo.exe / 非 cargo 工具。"""

    def test_plain_and_absolute_paths_match(self):
        for argv in (["cargo", "--version"],
                     ["cargo.exe", "--version"],
                     ["D:/toolchain/cargo/bin/cargo.exe", "--version"],
                     [r"D:\toolchain\cargo\bin\cargo.exe", "--version"],
                     ["/usr/local/cargo/bin/cargo", "--version"]):
            self.assertTrue(executor.is_cargo_argv(argv), argv)

    def test_non_cargo_heads_do_not_match(self):
        for argv in (["cargo-check", "--staged"],  # 独立工具，不是 cargo
                     ["mycargo", "--version"],
                     ["{python}", "-c", "pass"],
                     ["git", "status"],
                     []):
            self.assertFalse(executor.is_cargo_argv(argv), argv)


class CargoMutexTests(TempDirTest):
    """互斥正控 / 负控 / 单任务 / 超时三态。"""

    def _run_concurrently(self, tasks):
        """真线程并发跑 run_attempt（底层执行函数，不经调度器）。"""
        results = {}

        def worker(task):
            results[task.id] = executor.run_attempt(
                task, self.log_for(task.id), timeout_s=300,
                bash_path=None, root=str(self.tmp))

        threads = [threading.Thread(target=worker, args=(t,)) for t in tasks]
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        return results

    def _assert_both_ok(self, results, task_ids):
        for tid in task_ids:
            self.assertIn(tid, results, "两道门都应真跑出 attempt")
            att = results[tid]
            self.assertIs(att.verdict, Verdict.OK,
                          "%s 期望 ok（cargo --version 白名单探针），实得 %s: %s"
                          % (tid, att.verdict.value, att.reason))

    def test_two_cargo_tasks_are_serialized_when_enabled(self):
        spy = _SpySemaphore()
        tasks = (make_task(id="T-A"), make_task(id="T-B"))
        with mock.patch.object(executor, "_CARGO_SEMAPHORE", spy), \
             mock.patch.object(executor, "cargo_semaphore_enabled", return_value=True):
            results = self._run_concurrently(tasks)
        self._assert_both_ok(results, ("T-A", "T-B"))
        kinds = [k for k, _ in spy.events]
        self.assertEqual(
            kinds, ["acquire", "release", "acquire", "release"],
            "两并发 cargo 门的 semaphore 事件序应为 acquire/release 严格交替，"
            "实得 %s —— 交替被破坏即互斥未生效（target/ rlib 哈希互踩的前奏）" % kinds,
        )
        ts = [t for _, t in spy.events]
        # Windows 下 time.monotonic() 粒度约 15.6ms（未开高精度定时器），
        # release 与紧随其后的 acquire 可落在同一刻度 —— 互斥性质是
        # 「acquire_second 不得早于 release_first」，不是时戳严格大于。
        self.assertTrue(all(later >= earlier for earlier, later in zip(ts, ts[1:])),
                        "事件时戳必须单调不减（%s）" % ts)
        _a1, release_first, acquire_second, _r2 = ts
        self.assertGreaterEqual(
            acquire_second, release_first,
            "第二道门的执行窗（acquire）必须不早于第一道的释放（release）："
            "互斥锁的是执行窗不重叠，墙钟窗 %s/%s" % (ts[0:2], ts[2:4]),
        )

    def test_no_semaphore_traffic_when_disabled(self):
        """负控：开关关闭时，同一对并发 cargo 门不产生任何 semaphore 事件。"""
        spy = _SpySemaphore()
        tasks = (make_task(id="T-A"), make_task(id="T-B"))
        with mock.patch.object(executor, "_CARGO_SEMAPHORE", spy), \
             mock.patch.object(executor, "cargo_semaphore_enabled", return_value=False):
            results = self._run_concurrently(tasks)
        self._assert_both_ok(results, ("T-A", "T-B"))
        self.assertEqual(
            spy.events, [],
            "开关关闭时不应有任何 acquire/release 事件，实得 %s —— "
            "semaphore 流量只该来自本机制本身" % spy.events,
        )

    def test_single_task_holds_exactly_one_permit_cycle(self):
        spy = _SpySemaphore()
        with mock.patch.object(executor, "_CARGO_SEMAPHORE", spy), \
             mock.patch.object(executor, "cargo_semaphore_enabled", return_value=True):
            att = executor.run_attempt(make_task(id="T-A"), self.log_for("T-A"),
                                       timeout_s=300, bash_path=None, root=str(self.tmp))
        self.assertIs(att.verdict, Verdict.OK, att.reason)
        self.assertEqual([k for k, _ in spy.events], ["acquire", "release"],
                         "单任务恰好一个 acquire/release 周期，实得 %s" % spy.events)

    def test_acquire_timeout_is_unknown_not_red(self):
        """permit 占满时按本门预算 1/10 封顶等待，超时记 UNKNOWN（rc=2）。"""
        held = executor._CARGO_SEMAPHORE.acquire()  # 模拟 permit 泄漏场景
        self.assertTrue(held)
        try:
            with mock.patch.object(executor, "cargo_semaphore_enabled", return_value=True):
                started = time.monotonic()
                att = executor.run_attempt(make_task(id="T-A"), self.log_for("T-A"),
                                           timeout_s=10, bash_path=None,
                                           root=str(self.tmp))
                waited = time.monotonic() - started
        finally:
            executor._CARGO_SEMAPHORE.release()
        self.assertIs(att.verdict, Verdict.UNKNOWN,
                      "acquire 超时必须三态判 UNKNOWN（不可判定≠红），实得 %s: %s"
                      % (att.verdict.value, att.reason))
        self.assertEqual(att.reason, "cargo-semaphore-timeout", att.reason)
        self.assertIsNone(att.rc)
        self.assertLess(waited, 10.0, "acquire 上限 = timeout_s/10 = 1s，不得死等")
        self.assertEqual(executor._CARGO_SEMAPHORE._value, 1,
                         "用例结束后 permit 必须归位，否则后续 cargo 门全被拖死")


if __name__ == "__main__":
    # F30 dialect: an uncaught crash must exit 2, never borrow the interpreter's 1
    # (which would read as "ran and judged red" to run_gate_manifest).
    scripts_dir = str(ROOT / "scripts")
    if scripts_dir not in sys.path:
        sys.path.insert(0, scripts_dir)
    import gate_rc
    sys.exit(gate_rc.run(lambda: unittest.main(verbosity=2)))
