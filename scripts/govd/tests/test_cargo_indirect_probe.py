"""W4N N2：间接 cargo 探测（cargo_indirect_registry）锁。

背景：semaphore（ADR-318 TL-W3c）只锁 argv[0] 基名为 cargo 的**直接**门；
wrapper 门（python/bash/pwsh 包 cargo）从互斥面漏出，「wrapper 门里藏了几个
cargo 子进程」此前无人量过。本探针在 run_attempt 咽喉点对 wrapper argv 做静态
扫描（cargo + {build,check,test,clippy,bench,doc} 邻接对），命中即追加登记
tmp/govd/cargo_indirect_registry.json（JSONL）。

三条纪律（与 executor.py 注释同一口径）：
- 纯观测位：不改 rc、不强制互斥、任何探测故障全吞 —— 观测面不得反噬门判决；
- env GOVD_CARGO_INDIRECT_PROBE=1 显式 opt-in，默认关零开销（与 cargo_semaphore
  的开关纪律一致：缺键的陈旧环境绝不能被静默当成「开」）；
- 登记格式 JSONL 每行独立对象（并发安全，读侧按行解析）。

测试腿：
- 正控：python 门 argv 显式携带 cargo test，探针开 → 登记一条且门判决不受影响；
- 负控：纯 python 门（探针开）不登记 + wrapper 门（探针默认关）不登记 ——
  两条负腿缺一，都无法区分「探针逻辑失效」与「探针根本没开」。
"""
from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[3]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.govd import executor
from scripts.govd.model import Task, Verdict


def make_task(**kw):
    base = {"id": "T-1", "cmd": "{python} -c pass", "expect": 0,
            "kind": "light", "spec_ref": "test", "note": "test"}
    base.update(kw)
    return Task(**base)


class TempDirTest(unittest.TestCase):

    def setUp(self):
        base = ROOT / "tmp"
        base.mkdir(exist_ok=True)
        self.tmp = Path(tempfile.mkdtemp(prefix="govd-probe-test-", dir=str(base)))

    def tearDown(self):
        for p in sorted(self.tmp.rglob("*"), key=lambda x: len(str(x)), reverse=True):
            if p.is_file():
                p.unlink()
            elif not any(p.iterdir()):
                p.rmdir()
        self.tmp.rmdir()

    def log_for(self, task_id):
        return str(self.tmp / (task_id + ".log"))

    def read_registry(self, path):
        lines = Path(path).read_text(encoding="utf-8").splitlines()
        return [json.loads(line) for line in lines if line.strip()]


class DetectorShapeTests(unittest.TestCase):
    """detect_indirect_cargo 的形状覆盖：邻接对、-c 载荷、无子命令不命中。"""

    def test_wrapper_shapes(self):
        cases = (
            # (argv, expected_subcmd_or_None)
            (["python", "wrapper.py", "cargo", "test"], "test"),
            (["python.exe", "wrapper.py", "cargo", "bench"], "bench"),
            (["bash", "scripts/run.sh", "cargo", "clippy"], "clippy"),
            (["pwsh", "-c", "cargo build"], "build"),
            (["bash", "-c", "cargo check --workspace"], "check"),
            # 负形状：非 wrapper 头 / cargo 无子命令 / 子命令不在清单 / 纯 python
            (["cargo", "test"], None),
            (["python", "wrapper.py", "cargo", "--version"], None),
            (["python", "wrapper.py", "cargo", "run"], None),
            (["python", "-c", "pass"], None),
            (["python", "wrapper.py"], None),
        )
        for argv, want in cases:
            self.assertEqual(executor.detect_indirect_cargo(argv), want,
                             "argv=%r 期望 %r" % (argv, want))


class ProbeIntegrationTests(TempDirTest):
    """run_attempt 端到端：探针 env 门控 + 登记内容 + 门判决不受观测面影响。"""

    def _run_probe(self, task, enabled):
        registry = str(self.tmp / "registry.json")
        if enabled:
            env_cm = mock.patch.dict(os.environ, {"GOVD_CARGO_INDIRECT_PROBE": "1"})
        else:
            env_cm = mock.patch.dict(os.environ)
        with env_cm as env:
            if not enabled:
                env.pop("GOVD_CARGO_INDIRECT_PROBE", None)
            with mock.patch.object(executor, "_CARGO_INDIRECT_REGISTRY", registry):
                att = executor.run_attempt(task, self.log_for(task.id),
                                           timeout_s=60, bash_path=None,
                                           root=str(self.tmp))
        return att, registry

    def test_indirect_cargo_registered_when_enabled(self):
        """正控：python 门 argv 显式携带 cargo test，探针开 → 登记且 rc 不受影响。"""
        task = make_task(id="T-probe-pos", cmd="{python} -c pass cargo test")
        att, registry = self._run_probe(task, enabled=True)
        self.assertIs(att.verdict, Verdict.OK,
                      "观测位不得改变门判决，实得 %s: %s" % (att.verdict.value, att.reason))
        records = self.read_registry(registry)
        self.assertEqual(len(records), 1, "恰一条登记，实得 %r" % records)
        rec = records[0]
        self.assertEqual(rec["gate_id"], "T-probe-pos", rec)
        self.assertEqual(rec["matched_cargo_subcmd"], "test", rec)
        self.assertIn("cargo", rec["argv"], rec)
        self.assertIn("test", rec["argv"], rec)
        self.assertTrue(rec.get("run_ts"), "run_ts 必须非空，实得 %r" % rec)

    def test_no_registration_for_pure_python_and_when_disabled(self):
        """负控双 leg：纯 python 门（探针开）不登记；wrapper 门（探针默认关）不登记。"""
        pure, registry_a = self._run_probe(make_task(id="T-probe-neg-a"),
                                           enabled=True)
        self.assertIs(pure.verdict, Verdict.OK, pure.reason)
        self.assertFalse(os.path.exists(registry_a),
                         "纯 python 门不产生任何 cargo 流量，不允许登记")

        wrapped, registry_b = self._run_probe(
            make_task(id="T-probe-neg-b", cmd="{python} -c pass cargo test"),
            enabled=False)
        self.assertIs(wrapped.verdict, Verdict.OK, wrapped.reason)
        self.assertFalse(os.path.exists(registry_b),
                         "默认关（env 未置 1）时零登记 —— 缺省绝不能被静默当成开")


if __name__ == "__main__":
    # F30 dialect: an uncaught crash must exit 2, never borrow the interpreter's 1
    # (which would read as "ran and judged red" to run_gate_manifest).
    scripts_dir = str(ROOT / "scripts")
    if scripts_dir not in sys.path:
        sys.path.insert(0, scripts_dir)
    import gate_rc
    sys.exit(gate_rc.run(lambda: unittest.main(verbosity=2)))
