"""Core data-model types for the govd scheduler (ADR-228).

Verdicts are the seven-state vocabulary from the ADR; the aggregate exit code
is still the repository dialect 0/1/2. Nothing here touches the filesystem.
"""
from __future__ import annotations

import hashlib
from dataclasses import dataclass
from enum import Enum

PROFILE_NAMES = ("pr", "nightly", "release", "local")


class Verdict(str, Enum):
    OK = "ok"
    MISMATCH = "mismatch"
    ENV = "env"
    TIMEOUT = "timeout"
    LAUNCH = "launch"
    UNKNOWN = "unknown"
    SKIPPED = "skipped"
    CANCELLED = "cancelled"

    @property
    def retryable(self) -> bool:
        return self in (Verdict.ENV, Verdict.TIMEOUT, Verdict.LAUNCH)

    @property
    def rc(self) -> int:
        if self is Verdict.MISMATCH:
            return 1
        if self in (Verdict.OK, Verdict.SKIPPED):
            return 0
        return 2


@dataclass(frozen=True)
class Task:
    id: str
    cmd: str
    expect: int
    kind: str
    spec_ref: str
    note: str
    timeout_s: int = 0
    retry: int = -1
    mutating: bool = False
    needs: tuple[str, ...] = ()
    profiles: tuple[str, ...] = ()
    owner: str = ""
    disk_gb: float = 0.0
    source: str = "gate"

    def selected_by(self, profile_name: str, include_heavy: bool,
                    allow_mutating: bool) -> bool:
        if self.kind == "heavy" and not include_heavy:
            return False
        if self.profiles and profile_name not in self.profiles:
            return False
        if self.mutating and not allow_mutating:
            return False
        return True

    def cmd_digest(self) -> str:
        """Digest of the command string -- the third leg of the `--reuse-ok` key.

        WHY hash the cmd instead of trusting the id: a gate can be edited in
        place (same id, new predicate). Reuse must then be invalidated, or the
        scheduler would report yesterday's verdict for today's gate -- the same
        laundering class F33 guards against, only arriving through the cache side.
        """
        return hashlib.sha256(self.cmd.encode("utf-8")).hexdigest()

    def to_dict(self) -> dict:
        return {
            "id": self.id, "cmd": self.cmd, "expect": self.expect,
            "kind": self.kind, "spec_ref": self.spec_ref, "note": self.note,
            "timeout_s": self.timeout_s, "retry": self.retry,
            "mutating": self.mutating, "needs": list(self.needs),
            "profiles": list(self.profiles), "owner": self.owner,
            "disk_gb": self.disk_gb, "source": self.source,
        }


@dataclass(frozen=True)
class Profile:
    name: str
    include_heavy: bool
    jobs: int
    allow_mutating: bool
    reuse_ok: bool

@dataclass
class Attempt:
    task_id: str
    n: int
    verdict: Verdict
    rc: int | None
    duration_s: float
    reason: str
    log: str = ""
    timed_out: bool = False
    cmd_digest: str = ""
    reused: bool = False
    reused_from: str = ""

    def to_dict(self) -> dict:
        return {
            "task_id": self.task_id, "attempt": self.n,
            "verdict": self.verdict.value, "rc": self.rc,
            "duration_s": round(self.duration_s, 3), "reason": self.reason,
            "log": self.log, "timed_out": self.timed_out,
            "cmd_digest": self.cmd_digest, "reused": self.reused,
            "reused_from": self.reused_from,
        }


@dataclass
class RunMeta:
    run_id: str
    profile: str
    started_at: str
    head: str
    dirty: int
    actor: str
    host: str
    python: str
    manifest_digest: str
    env_fingerprint: str = ""
    role: str = "local-dev"
    status: str = "running"
    # No free-form `extra` bag: it was never written and never read, yet it
    # appeared in every run_start event and report.json as `{}`. A field that
    # promises extensibility while nothing fills it is decorative surface -- the
    # same category this batch removed for parsed-but-unread CLI flags.