"""Profile, role, executable-whitelist and mutating policy (ADR-228 §3.5)."""
from __future__ import annotations

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
            reuse_ok=bool(row.get("reuse_ok", False)),
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