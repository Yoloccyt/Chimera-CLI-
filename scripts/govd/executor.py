"""Subprocess execution: timeout, process-tree kill, env fingerprint."""
from __future__ import annotations

import hashlib
import os
import shlex
import shutil
import signal
import subprocess
import sys
import threading
import time
import tomllib
from pathlib import Path

from .classify import classify_attempt
from .model import Attempt, Verdict

ROOT = Path(__file__).resolve().parents[2]
def augment_path():
    """Put repo toolchain dirs first on PATH; return what was added."""
    extra = [os.path.join(str(ROOT), ".toolchain", "cargo", "bin"),
             os.path.join(str(ROOT), ".toolchain", "rustup")]
    mingw = r"D:\msys64\mingw64\bin"
    if os.path.isdir(mingw):
        extra.append(mingw)
    bash_path, _note = usable_bash()
    if bash_path:
        core = os.path.dirname(os.path.abspath(bash_path))
        def has(tool):
            return any(os.path.exists(os.path.join(core, name))
                       for name in (tool, tool + ".exe"))
        if core and all(has(t) for t in ("dirname", "awk", "grep", "head")):
            extra.append(core)
    cur = os.environ.get("PATH", "")
    add = [x for x in extra if os.path.isdir(x) and x not in cur.split(os.pathsep)]
    if add:
        os.environ["PATH"] = os.pathsep.join(add + [cur])
    return add


def usable_bash():
    """Pick a GNU bash that understands Windows paths and strips CRLF."""
    candidates = []
    found = shutil.which("bash")
    if found:
        candidates.append(found)
    if os.name == "nt":
        candidates += [r"C:\Program Files\Git\usr\bin\bash.exe",
                       r"D:\msys64\usr\bin\bash.exe"]
    rejected = []
    for path in candidates:
        if not path or not os.path.isfile(path):
            continue
        try:
            proc = subprocess.run([path, "--version"], capture_output=True,
                                  text=True, encoding="utf-8",
                                  errors="replace", timeout=20)
            out = proc.stdout or ""
        except Exception as exc:
            rejected.append("%s: %s" % (path, type(exc).__name__))
            continue
        first = (out.splitlines() or [""])[0].strip()
        if not first.startswith("GNU bash"):
            rejected.append("%s: not GNU bash (%s)" % (path, first or "no output"))
            continue
        if os.name == "nt" and "linux" in first.lower():
            rejected.append("%s: %s" % (path, first))
            continue
        return path, first
    return None, "; ".join(rejected) or "no usable bash"


def resolve_argv(cmd, bash_path=None):
    """Tokenize a manifest cmd, swap {python}, pin bash to the verified one."""
    args = shlex.split(cmd, posix=True)
    if not args:
        raise ValueError("empty cmd")
    if args[0] == "{python}":
        return [sys.executable] + args[1:]
    if args[0] in ("bash", "sh"):
        if not bash_path:
            raise LookupError("no usable GNU bash for %r" % cmd)
        return [bash_path] + args[1:]
    return args


# ADR-318 工具链批 TL-W3c：cargo 门应用层互斥。
#
# WHY 模块级：整个门禁面跑在 govd 单进程内（cli.cmd_run 的 ThreadPoolExecutor），
# 进程级 Semaphore(1) 正是「全进程同时至多一个 cargo 子进程」的正确作用域；
# 实例级或局部锁只会串住共享同一实例的线程，挡不住别的执行路径。
#
# WHY 放在 executor 而不是 cli：run_attempt 是所有门 spawn 的唯一咽喉
# （_execute_one 的每次尝试、包括重试，都从这里过），挂在这里天然覆盖重试，
# 且每次尝试各自 acquire/release —— 绝不跨重试持有，这是无死锁的前提。
_CARGO_SEMAPHORE = threading.Semaphore(1)
_SEMAPHORE_CONFIG = {"path": "", "enabled": False}


def cargo_semaphore_enabled(policy_path=None) -> bool:
    """Read [exec].cargo_semaphore from policy.toml, cached per path.

    WHY 在 executor 里直接读文件而不是走 policy.Policy：policy.py 承载类型化
    策略面，本批不能扩展它；executor 只取自己需要的那一个键。WHY 缺省 False：
    这个互斥会改变所有 cargo 门的执行时序，必须是 policy.toml 里的显式 opt-in
    —— 一份缺键的陈旧文件若被静默当成「开」，调度行为就被一份没人读的文件
    悄悄改了（「写了没人读」的假绿家族）；显式 false 则是规格承诺的回滚手段。
    WHY 按路径缓存：该值是 run 级常量，每门重解析 TOML 是纯开销。
    """
    path = str(policy_path or (ROOT / "scripts" / "govd" / "policy.toml"))
    if _SEMAPHORE_CONFIG["path"] != path:
        enabled = False
        try:
            with open(path, "rb") as fh:
                row = tomllib.load(fh).get("exec", {})
            enabled = bool(row.get("cargo_semaphore", False))
        except (OSError, tomllib.TOMLDecodeError):
            # 文件读不出 ⇒ 不启用互斥，门照常跑。semaphore 只是内存防护，
            # 它的失效形态必须是「退回现状」而不是「把整轮门禁判成不可判定」。
            enabled = False
        _SEMAPHORE_CONFIG["path"] = path
        _SEMAPHORE_CONFIG["enabled"] = enabled
    return _SEMAPHORE_CONFIG["enabled"]


_CARGO_BASENAMES = frozenset({"cargo", "cargo.exe"})


def is_cargo_argv(argv) -> bool:
    """Whether the resolved argv launches cargo: basename of the first token.

    WHY 取基名而不是比对 cmd 字符串前缀：cmd 经 shlex 解析后 argv[0] 可能是
    绝对路径（D:/.../.toolchain/cargo/bin/cargo.exe，POSIX 写法）或经 wrapper
    调起的 cargo/cargo.exe；基名比较同时覆盖这三种形态，且不会去猜 PATH。
    带扩展名才比较（cargo.exe），避免把 cargo-check 之类的独立工具误判进来。
    """
    if not argv:
        return False
    base = argv[0].replace("\\", "/").rsplit("/", 1)[-1]
    return base.lower() in _CARGO_BASENAMES


def env_fingerprint():
    """Hash the host facts that make a rerun non-comparable elsewhere."""
    parts = [sys.version.split()[0], sys.executable, os.name,
             os.environ.get("PATH", ""), os.environ.get("PYTHONUTF8", ""),
             os.environ.get("PYTHONIOENCODING", "")]
    raw = "|".join(parts).encode("utf-8", "replace")
    return hashlib.sha256(raw).hexdigest()[:16]


def read_log(path, limit=200000):
    """Log tail for classification; never raises on missing or binary junk."""
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            return fh.read(limit)
    except OSError:
        return ""


def kill_tree(proc):
    """Terminate the whole tree: cargo/bash gates spawn children that outlive kill()."""
    if proc.poll() is not None:
        return
    if os.name == "nt":
        try:
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(proc.pid)],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           timeout=30)
        except (OSError, subprocess.SubprocessError):
            pass
    else:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()


def run_attempt(task, log_path, timeout_s, bash_path=None, root=None):
    """Run one task with a hard timeout; classify instead of raising."""
    root = str(root or ROOT)
    log_path = str(log_path)
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    started = time.monotonic()
    try:
        args = resolve_argv(task.cmd, bash_path)
    except (ValueError, LookupError) as exc:
        return Attempt(task.id, 1, Verdict.LAUNCH, None, 0.0,
                       "resolve:%s" % exc, log_path)
    # ADR-318 TL-W3c：cargo 门的进程级互斥。获取超时取本门预算的 1/10 ——
    # 排队是互斥的预期形态，但等待必须有上限：一次 permit 泄漏（未来某次改动的
    # bug）会让此后所有 cargo 门无限挂起、整个 CI job 被静默吊死，那才是最难
    # 诊断的失败模式。超时按三态纪律记 UNKNOWN（rc=2，不可判定≠红），绝不用
    # 死等把调度问题伪装成绿或红。
    sem_held = False
    if cargo_semaphore_enabled() and is_cargo_argv(args):
        sem_held = _CARGO_SEMAPHORE.acquire(timeout=timeout_s / 10.0)
        if not sem_held:
            duration = time.monotonic() - started
            return Attempt(task.id, 1, Verdict.UNKNOWN, None, duration,
                           "cargo-semaphore-timeout", log_path)
    try:
        env = dict(os.environ)
        env["PYTHONIOENCODING"] = "utf-8"
        env["PYTHONUTF8"] = "1"
        kwargs = {}
        if os.name == "nt":
            kwargs["creationflags"] = 0x00000200
        else:
            kwargs["start_new_session"] = True
        try:
            with open(log_path, "wb") as lf:
                proc = subprocess.Popen(args, cwd=root, stdin=subprocess.DEVNULL,
                                        stdout=lf, stderr=subprocess.STDOUT,
                                        env=env, **kwargs)
                try:
                    rc = proc.wait(timeout=timeout_s)
                    timed_out = False
                except subprocess.TimeoutExpired:
                    kill_tree(proc)
                    rc = None
                    timed_out = True
        except OSError as exc:
            duration = time.monotonic() - started
            reason = "launch:%s" % type(exc).__name__
            return Attempt(task.id, 1, Verdict.LAUNCH, None, duration, reason, log_path)
        duration = time.monotonic() - started
        verdict, reason = classify_attempt(rc, task.expect, read_log(log_path),
                                           task.cmd, timed_out=timed_out)
        return Attempt(task.id, 1, verdict, rc, duration, reason, log_path, timed_out)
    finally:
        # 第一红线：permit 必须 finally 释放。任何一条 return（launch 失败、
        # classify 完成）或任何异常路径漏掉 release，Semaphore(1) 就永久少一个
        # permit —— 之后所有 cargo 门全部 acquire 超时，门禁面整体不可判定。
        if sem_held:
            _CARGO_SEMAPHORE.release()
