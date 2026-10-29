"""Report rendering: text/[SUM], JSON, JUnit XML and step summary."""
from __future__ import annotations

import dataclasses
import json
from collections import Counter
from xml.sax.saxutils import escape

from .model import Verdict

FIELDS = ("ok", "mismatch", "env", "timeout", "launch", "unknown",
          "skipped", "cancelled")


def counts(attempts, skipped):
    c = Counter(a.verdict.value for a in attempts)
    c["skipped"] += len(skipped)
    return c


def sum_line(attempts, skipped, total):
    c = counts(attempts, skipped)
    accounted = sum(c[f] for f in FIELDS)
    return ("[SUM] ok=%d mismatch=%d env-fail=%d unknown=%d skip=%d -- total %d/%d"
            % (c["ok"], c["mismatch"], c["env"] + c["timeout"] + c["launch"],
               c["unknown"], c["skipped"], accounted, total))


def exit_code(attempts, skipped, total):
    c = counts(attempts, skipped)
    if sum(c[f] for f in FIELDS) != total:
        return 2
    if c["mismatch"] or c["cancelled"]:
        return 1
    if c["env"] or c["timeout"] or c["launch"] or c["unknown"]:
        return 2
    return 0


def to_json(meta, attempts, skipped):
    doc = {"run": dataclasses.asdict(meta),
           "attempts": [a.to_dict() for a in attempts],
           "skipped": [t.id for t in skipped],
           "counts": dict(counts(attempts, skipped))}
    return json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False)


def to_junit(attempts, skipped):
    rows = []
    fails = 0
    for a in attempts:
        name = escape(a.task_id)
        if a.verdict is Verdict.OK:
            rows.append('    <testcase classname="govd" name="%s" time="%.3f"/>' % (name, a.duration_s))
            continue
        fails += 1
        body = escape((a.reason + chr(10) + a.log)[-4000:])
        rows.append('    <testcase classname="govd" name="%s" time="%.3f"><failure message="%s">%s</failure></testcase>' % (name, a.duration_s, escape(a.reason), body))
    for task in skipped:
        rows.append('    <testcase classname="govd" name="%s"><skipped/></testcase>' % escape(task.id))
    head = '<?xml version="1.0" encoding="utf-8"?>'
    open_tag = '<testsuite name="govd" tests="%d" failures="%d" skipped="%d">' % (len(rows), fails, len(skipped))
    return head + chr(10) + open_tag + chr(10) + chr(10).join(rows) + chr(10) + "</testsuite>" + chr(10)


def step_summary(meta, attempts, skipped, journal_sha):
    c = counts(attempts, skipped)
    lines = ["### govd run %s" % meta.run_id, "", "| verdict | n |", "|---|---|"]
    for f in FIELDS:
        if c[f]:
            lines.append("| %s | %d |" % (f, c[f]))
    lines += ["", "journal=%s" % journal_sha, "profile=%s head=%s python=%s"
              % (meta.profile, meta.head, meta.python)]
    return chr(10).join(lines) + chr(10)

