"""Report rendering: text/[SUM], JSON, JUnit XML and step summary."""
from __future__ import annotations

import dataclasses
import json
import threading
import time
from collections import Counter
from xml.sax.saxutils import escape

from .model import Verdict

FIELDS = ("ok", "mismatch", "env", "timeout", "launch", "unknown",
          "skipped", "cancelled")

SUMMARY_FIELDS = ("peak_concurrency", "total_wall_s", "serial_wall_s",
                  "parallelism_gain")


class Telemetry:
    """Run-level schedule observability: peak concurrency, wall clock, waits.

    WHY a collector fed by the scheduler instead of arithmetic over attempts:
    peak concurrency, total wall clock and exclusive-domain waiting are
    properties of the *schedule*, not of any single attempt. Only the dispatch
    points can observe them, so `cli.cmd_run` feeds this object and the report
    layer only renders the numbers (one fact, one measurement site).

    All methods are thread-safe. A caller that never feeds it still gets zeros
    from `snapshot()` -- observability must never be able to break a run.
    """

    def __init__(self, clock=time.monotonic):
        self._clock = clock
        self._lock = threading.Lock()
        self._active = 0
        self._peak = 0
        self._started = clock()
        self._ended = None
        # exclusive key -> {"enqueued": ts, "last_end": ts | None, "wait_s": s}
        self._exclusive = {}

    def gate_entered(self):
        with self._lock:
            self._active += 1
            if self._active > self._peak:
                self._peak = self._active

    def gate_left(self):
        with self._lock:
            if self._active > 0:
                self._active -= 1

    def exclusive_enqueued(self, key):
        with self._lock:
            self._exclusive.setdefault(
                key, {"enqueued": self._clock(), "last_end": None,
                      "wait_s": 0.0})

    def exclusive_gate_started(self, key):
        """Count the waiting accrued by one gate of an exclusive chain.

        First gate of the chain waits from the chain's enqueue moment; every
        later gate waits from the previous gate's end. The sum per key is the
        time the chain spent *not* executing (queueing + inter-gate gaps), which
        is exactly what a serial fallback would add on top of the gate wall
        clocks.
        """
        now = self._clock()
        with self._lock:
            row = self._exclusive.get(key)
            if row is None:
                return
            base = row["last_end"] if row["last_end"] is not None else row["enqueued"]
            waited = now - base
            if waited > 0:
                row["wait_s"] += waited

    def exclusive_gate_finished(self, key):
        now = self._clock()
        with self._lock:
            row = self._exclusive.get(key)
            if row is not None:
                row["last_end"] = now

    def stop(self):
        with self._lock:
            if self._ended is None:
                self._ended = self._clock()

    def snapshot(self, attempts):
        with self._lock:
            end = self._ended if self._ended is not None else self._clock()
            total = round(max(end - self._started, 0.0), 3)
            peak = self._peak
            waits = {k: round(v["wait_s"], 3)
                     for k, v in sorted(self._exclusive.items())}
        serial = round(sum(a.duration_s for a in attempts), 3)
        gain = round(serial / total, 3) if total > 0 else 0.0
        return {"peak_concurrency": peak, "total_wall_s": total,
                "serial_wall_s": serial, "parallelism_gain": gain,
                "exclusive_wait_s": waits}



def counts(attempts, skipped):
    c = Counter(a.verdict.value for a in attempts)
    c["skipped"] += len(skipped)
    # `reused` is a sub-count of ok, never a new verdict: the seven-state
    # vocabulary from ADR-228 §2.3 stays intact, but a gate that was not
    # executed has to be visible in the ledger instead of hiding inside ok.
    c["reused"] = sum(1 for a in attempts if a.reused)
    return c


def sum_line(attempts, skipped, total):
    c = counts(attempts, skipped)
    accounted = sum(c[f] for f in FIELDS)
    line = ("[SUM] ok=%d mismatch=%d env-fail=%d unknown=%d skip=%d -- total %d/%d"
            % (c["ok"], c["mismatch"], c["env"] + c["timeout"] + c["launch"],
               c["unknown"], c["skipped"], accounted, total))
    if c["reused"]:
        line += " reused=%d(not executed)" % c["reused"]
    if c["cancelled"]:
        line += " cancelled=%d(not started)" % c["cancelled"]
    return line


def exit_code(attempts, skipped, total):
    c = counts(attempts, skipped)
    if sum(c[f] for f in FIELDS) != total:
        return 2
    if c["mismatch"] or c["cancelled"]:
        return 1
    if c["env"] or c["timeout"] or c["launch"] or c["unknown"]:
        return 2
    return 0


def to_json(meta, attempts, skipped, telemetry=None):
    doc = {"run": dataclasses.asdict(meta),
           "attempts": [a.to_dict() for a in attempts],
           "skipped": [t.id for t in skipped],
           "counts": dict(counts(attempts, skipped))}
    # TC-D-1 observability block: additive only -- every pre-existing key keeps
    # its shape. Per-gate wall clock already lives in attempts[].duration_s.
    if telemetry is not None:
        snap = telemetry.snapshot(attempts)
        doc["summary"] = {k: snap[k] for k in SUMMARY_FIELDS}
        doc["exclusive_wait_s"] = snap["exclusive_wait_s"]
    return json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False)


def to_junit(attempts, skipped):
    rows = []
    fails = 0
    reused = 0
    for a in attempts:
        name = escape(a.task_id)
        if a.verdict is Verdict.OK and a.reused:
            # CI artifacts must not show an unexecuted gate as a pass: render it
            # as skipped so the JUnit consumer sees exactly how little ran.
            reused += 1
            rows.append('    <testcase classname="govd" name="%s" time="0.000">'
                        '<skipped message="reused-ok from %s"/></testcase>'
                        % (name, escape(a.reused_from)))
            continue
        if a.verdict is Verdict.OK:
            rows.append('    <testcase classname="govd" name="%s" time="%.3f"/>' % (name, a.duration_s))
            continue
        fails += 1
        body = escape((a.reason + chr(10) + a.log)[-4000:])
        rows.append('    <testcase classname="govd" name="%s" time="%.3f"><failure message="%s">%s</failure></testcase>' % (name, a.duration_s, escape(a.reason), body))
    for task in skipped:
        rows.append('    <testcase classname="govd" name="%s"><skipped/></testcase>' % escape(task.id))
    head = '<?xml version="1.0" encoding="utf-8"?>'
    open_tag = '<testsuite name="govd" tests="%d" failures="%d" skipped="%d">' % (len(rows), fails, len(skipped) + reused)
    return head + chr(10) + open_tag + chr(10) + chr(10).join(rows) + chr(10) + "</testsuite>" + chr(10)


def step_summary(meta, attempts, skipped, journal_sha, telemetry=None):
    c = counts(attempts, skipped)
    lines = ["### govd run %s" % meta.run_id, "", "| verdict | n |", "|---|---|"]
    for f in FIELDS:
        if c[f]:
            lines.append("| %s | %d |" % (f, c[f]))
    if c["reused"]:
        lines.append("| reused (not executed) | %d |" % c["reused"])
    lines += ["", "journal=%s" % journal_sha, "profile=%s head=%s python=%s"
              % (meta.profile, meta.head, meta.python)]
    if telemetry is not None:
        snap = telemetry.snapshot(attempts)
        lines += ["", "#### schedule telemetry", "| metric | value |", "|---|---|"]
        lines.append("| peak_concurrency | %d |" % snap["peak_concurrency"])
        lines.append("| total_wall_s | %.3f |" % snap["total_wall_s"])
        lines.append("| serial_wall_s | %.3f |" % snap["serial_wall_s"])
        lines.append("| parallelism_gain | %.3f |" % snap["parallelism_gain"])
        for key, wait_s in snap["exclusive_wait_s"].items():
            lines.append("| exclusive_wait_s.%s | %.3f |" % (key, wait_s))
    return chr(10).join(lines) + chr(10)

