"""Subprocess execution: timeout, process-tree kill, env fingerprint."""
from __future__ import annotations

import hashlib
import os
import shlex
import shutil
import signal
import subprocess
import sys
import time
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
