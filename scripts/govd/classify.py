"""Environment-vs-verdict classification, ported from run_gate_manifest (F33).

Order is the criterion: an environment failure must be classified before
`rc == expect`, or an expect=1 gate that crashes at import time is recorded
as a satisfied gate. Every literal below is covered by a negative fixture.
"""
from __future__ import annotations

import re

from .model import Verdict

UNCONDITIONAL_ENV_SIGS = (
    "Bad interpreter",
    "command not found",
    "cannot execute binary file",
)
AMBIGUOUS_ENV_SIGS = (
    "invalid option name",
    "No such file or directory",
    "SyntaxError",
    "Traceback (most recent call last)",
    # Measured 2026-10-29: `cargo fmt --all -- --check` aborts on this host with
    # ERROR_INVALID_NAME (long path) plus a usage banner, and exits 1 -- the exact
    # code a real "rustfmt found diffs" verdict uses. Treated like the other
    # ambiguous signatures: env for non-shell commands (cargo/python), while a
    # bash/sh gate still needs a shell-error first line to be called env.
    "os error 206",
)
SHELL_ERROR_RE = re.compile(r"^.+: line \d+: ")


def env_failure_reason(log_text, cmd):
    """Signature proving the gate never ran, or None (first 5 lines only)."""
    lines = [l for l in log_text.splitlines() if l.strip()]
    if not lines:
        return None
    head = chr(10).join(lines[:5])
    first = lines[0]
    for sig in UNCONDITIONAL_ENV_SIGS:
        if sig in head:
            return sig
    sig = next((s for s in AMBIGUOUS_ENV_SIGS if s in head), None)
    if sig is None:
        return None
    argv0 = cmd.strip().split()[:1]
    if argv0 and argv0[0] in ("bash", "sh") and not SHELL_ERROR_RE.match(first):
        return None
    return sig


def classify(rc, expect, log_text, cmd):
    sig = env_failure_reason(log_text, cmd)
    if sig:
        return Verdict.ENV, "env:%s" % sig
    if rc == expect:
        return Verdict.OK, "rc=%d" % rc
    return Verdict.MISMATCH, "rc=%d!=%d" % (rc, expect)


def classify_attempt(rc, expect, log_text, cmd, timed_out=False, launch_error=""):
    if launch_error:
        return Verdict.LAUNCH, "launch:%s" % launch_error
    if timed_out:
        return Verdict.TIMEOUT, "timeout"
    return classify(rc, expect, log_text, cmd)
