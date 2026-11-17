"""Profile, role, executable-whitelist and mutating policy (ADR-228 §3.5)."""
from __future__ import annotations

import os
import shlex
import subprocess
import tomllib
from dataclasses import dataclass

from .model import PROFILE_NAMES, Profile, Task


class PolicyError(ValueError):
    """Policy cannot be evaluated; the caller must exit 2."""


@dataclass(frozen=True)
class Policy:
    profiles: dict
    whitelist: tuple[str, ...]
    default_timeout_s: int
    heavy_timeout_s: int
    env_retry: int
    roles: dict
    mutable_prefixes: tuple[str, ...]

def load(path) -> Policy:
    with open(path, "rb") as fh:
        doc = tomllib.load(fh)
    raw_profiles = doc.get("profiles", {})
    profiles = {}
    for name in PROFILE_NAMES:
        row = raw_profiles.get(name)
        if not isinstance(row, dict):
            raise PolicyError("profile %r missing from policy" % name)
        try:
            jobs = int(row.get("jobs", 1))
        except (TypeError, ValueError):
            raise PolicyError("profile %r: jobs must be an int" % name)
        profiles[name] = Profile(
            name=name, include_heavy=bool(row.get("include_heavy", False)),
            jobs=max(1, jobs), allow_mutating=bool(row.get("allow_mutating", False)),
        )
    exec_row = doc.get("exec", {})
    whitelist = tuple(str(x) for x in exec_row.get("whitelist", ()))
    if not whitelist:
        raise PolicyError("exec.whitelist must not be empty")
    roles = {}
    for name, row in doc.get("roles", {}).items():
        roles[str(name)] = tuple(str(x) for x in row.get("profiles", ()))
    if not roles:
        raise PolicyError("at least one [roles.*] entry is required")
    return Policy(
        profiles=profiles, whitelist=whitelist,
        default_timeout_s=int(exec_row.get("default_timeout_s", 300)),
        heavy_timeout_s=int(exec_row.get("heavy_timeout_s", 1800)),
        env_retry=int(exec_row.get("env_retry", 1)),
        roles=roles,
        mutable_prefixes=tuple(str(x) for x in exec_row.get("mutating_prefixes", ("tmp/",))),
    )

def select(tasks, profile: Profile, allow_mutating: bool):
    """Split into (selected, skipped); skipping is a verdict, not a silence."""
    selected, skipped = [], []
    for task in tasks:
        if task.selected_by(profile.name, profile.include_heavy,
                            profile.allow_mutating and allow_mutating):
            selected.append(task)
        else:
            skipped.append(task)
    return selected, skipped


@dataclass(frozen=True)
class Resources:
    """What this host can actually run, measured once per run."""

    cores: int
    mem_gb: float
    disk_free_gb: float


def _total_memory_gb() -> float:
    """Total physical RAM in GiB (Windows: ctypes; POSIX: sysconf).

    WHY not psutil: govd is a stdlib-only package (ADR-228 default decision
    8.1, asserted by scripts/govd/tests/test_govd.py::DependencyFreedomTests),
    so a third-party dependency would break the lane that runs the scheduler.
    """
    if os.name == "nt":
        import ctypes

        class _MemStatus(ctypes.Structure):
            _fields_ = [
                ("dwLength", ctypes.c_ulong),
                ("dwMemoryLoad", ctypes.c_ulong),
                ("ullTotalPhys", ctypes.c_ulonglong),
                ("ullAvailPhys", ctypes.c_ulonglong),
                ("ullTotalPageFile", ctypes.c_ulonglong),
                ("ullAvailPageFile", ctypes.c_ulonglong),
                ("ullTotalVirtual", ctypes.c_ulonglong),
                ("ullAvailVirtual", ctypes.c_ulonglong),
                ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
            ]

        stat = _MemStatus()
        stat.dwLength = ctypes.sizeof(stat)
        ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(stat))
        return stat.ullTotalPhys / (1024 ** 3)
    try:
        pages = os.sysconf("SC_PHYS_PAGES")
        page_size = os.sysconf("SC_PAGE_SIZE")
    except (AttributeError, ValueError, OSError):
        return 0.0
    return pages * page_size / (1024 ** 3)


def _disk_free_gb(root) -> float:
    """Free space (GiB) of the volume that holds `root`.

    WHY not shutil.disk_usage: the executor's per-gate disk budget reads the
    same volume through shutil, and a shared code path would let the
    scheduler's one-shot probe be counted as a gate reading (the disk-budget
    test asserts "exactly one reading per disk-gated gate"). Windows goes
    through GetDiskFreeSpaceExW, POSIX through statvfs -- both stdlib-only.
    """
    path = str(root)
    if os.name == "nt":
        import ctypes

        free_to_caller = ctypes.c_ulonglong(0)
        total_bytes = ctypes.c_ulonglong(0)
        total_free = ctypes.c_ulonglong(0)
        ok = ctypes.windll.kernel32.GetDiskFreeSpaceExW(
            path, ctypes.byref(free_to_caller), ctypes.byref(total_bytes),
            ctypes.byref(total_free))
        if not ok:
            return 0.0
        return free_to_caller.value / (1024 ** 3)
    try:
        st = os.statvfs(path)
    except (OSError, AttributeError):
        return 0.0
    return st.f_bavail * st.f_frsize / (1024 ** 3)


def probe_resources(root) -> Resources:
    """Measure the host: logical cores, total RAM (GiB), free disk (GiB).

    `root` is the repository root, so the free-space figure is the free space
    of the volume that actually holds the heavy gates' build artifacts.
    """
    return Resources(cores=int(os.cpu_count() or 1),
                      mem_gb=_total_memory_gb(),
                      disk_free_gb=_disk_free_gb(root))


def derive_jobs(profile_jobs: int, cores: int, mem_gb: float,
                disk_free_gb: float) -> int:
    """Decide the worker count from the profile value and this host's capacity.

    Rules, applied top-down:
      1. an explicit `--jobs` short-circuits here (the caller owns it);
      2. the profile's declared value is the *ceiling*, never a floor;
      3. this host's capacity may only press that ceiling DOWN:
         min(floor(cores/2), 4); 2 when RAM < 16 GiB; 2 when the volume holding
         the build artifacts has < 30 GiB free (disk is the hard gate for the
         heavy gates).

    WHY downward-only: a low-spec host must never make the scheduler run
    *wider* than the profile asked for -- otherwise "the profile says 4"
    silently becomes "8 cargo sessions on a 2-core laptop". Raising the
    ceiling above the profile value would also contradict the profile
    contract (pr=4 / nightly=2 / release=1 / local=1).
    """
    ceiling = max(1, min(int(cores) // 2, 4))
    if mem_gb < 16:
        ceiling = min(ceiling, 2)
    if disk_free_gb < 30:
        ceiling = min(ceiling, 2)
    return max(1, min(int(profile_jobs), ceiling))


def role_check(policy: Policy, role: str, profile_name: str):
    allowed = policy.roles.get(role)
    if allowed is None:
        return "unknown role %r (known: %s)" % (role, sorted(policy.roles))
    if profile_name not in allowed:
        return "role %r may not run profile %r (allowed: %s)" % (
            role, profile_name, list(allowed))
    return None

def whitelist_check(policy: Policy, cmd: str):
    try:
        argv = shlex.split(cmd, posix=True)
    except ValueError as exc:
        return "cannot tokenize cmd: %s" % exc
    if not argv:
        return "empty cmd"
    head = argv[0]
    if head not in policy.whitelist:
        return "executable %r is not whitelisted (%s)" % (head, list(policy.whitelist))
    return None


def timeout_for(task: Task, policy: Policy) -> int:
    if task.timeout_s > 0:
        return task.timeout_s
    return policy.heavy_timeout_s if task.kind == "heavy" else policy.default_timeout_s


def retry_budget(task: Task, policy: Policy) -> int:
    if task.retry >= 0:
        return task.retry
    return max(0, policy.env_retry)

def snapshot(root) -> set[str]:
    """Tracked/untracked paths from `git status --porcelain` (empty on error)."""
    try:
        proc = subprocess.run(["git", "status", "--porcelain"], cwd=str(root),
                              capture_output=True, text=True, encoding="utf-8",
                              errors="replace", timeout=30)
    except (OSError, subprocess.SubprocessError):
        return set()
    if proc.returncode != 0:
        return set()
    paths = set()
    for line in (proc.stdout or "").splitlines():
        if len(line) < 4:
            continue
        rest = line[3:]
        if " -> " in rest:
            rest = rest.split(" -> ", 1)[1]
        paths.add(rest.strip().strip('"'))
    return paths


def violations(before: set[str], after: set[str], prefixes) -> list[str]:
    return sorted(p for p in after - before
                  if not any(p.startswith(x) for x in prefixes))