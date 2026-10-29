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
# The repository dialect: 0 pass, 1 judged red, 2 undecidable -- and G-138 enforces
# `expect in {0,1,2}` for every row. So any other exit code cannot be a verdict:
# it is the shell's "could not run it" (126/127), a Windows fastfail (0xC0000409),
# or an RPC/console failure under load. Measured 2026-10-29 in one polluted pr run:
# rc=3221225773 / 2147516553 / 127 were all filed as `mismatch` -- four fake red
# gates that pass when re-run alone.
VERDICT_CODES = (0, 1, 2)


def abnormal_exit_reason(rc, expect):
    """Why this exit code is not a verdict, or None.

    Two kinds, both environment rather than repository state:
      * a code outside the 0/1/2 dialect -> the gate never reached a verdict
      * rc == 2 with expect != 2 -> the gate itself declared 不可判定. Recording
        that as `mismatch` files "I could not decide" as "the repository is
        broken", which is the same category error in the other direction.
    """
    if rc is None:
        return None
    if rc not in VERDICT_CODES:
        return "abnormal-exit:rc=%d" % rc
    if rc == 2 and expect != 2:
        return "gate-declared-undecidable"
    return None


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
    abnormal = abnormal_exit_reason(rc, expect)
    if abnormal:
        return Verdict.ENV, abnormal
    if rc == expect:
        return Verdict.OK, "rc=%d" % rc
    return Verdict.MISMATCH, "rc=%d!=%d" % (rc, expect)


def classify_attempt(rc, expect, log_text, cmd, timed_out=False, launch_error=""):
    if launch_error:
        return Verdict.LAUNCH, "launch:%s" % launch_error
    if timed_out:
        return Verdict.TIMEOUT, "timeout"
    return classify(rc, expect, log_text, cmd)
