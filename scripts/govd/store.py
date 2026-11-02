"""Journal store: hashed JSONL (authority) + SQLite index (rebuildable)."""
from __future__ import annotations

import hashlib
import json
import sqlite3
import time
from pathlib import Path

ZERO = "0" * 64


def _canon(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def chain_digest(prev, event):
    return hashlib.sha256((prev + _canon(event)).encode("utf-8")).hexdigest()


class Journal:
    """Append-only event log whose every line commits to the previous one."""

    def __init__(self, root, run_id):
        self.run_id = run_id
        self.dir = Path(root) / "tmp" / "govd" / "runs" / run_id
        self.logs = self.dir / "logs"
        self.events = self.dir / "events.jsonl"
        self.logs.mkdir(parents=True, exist_ok=True)
        self.prev = self._last_digest()

    def _last_digest(self):
        if not self.events.exists():
            return ZERO
        last = ZERO
        for line in self.events.read_text(encoding="utf-8").splitlines():
            if line.strip():
                last = json.loads(line).get("sha256", last)
        return last

    def append(self, kind, payload):
        event = {"ts": round(time.time(), 3), "run_id": self.run_id, "kind": kind,
                 "payload": payload, "prev_sha256": self.prev}
        event["sha256"] = chain_digest(self.prev, event)
        with self.events.open("a", encoding="utf-8", newline="") as fh:
            fh.write(_canon(event) + chr(10))
        self.prev = event["sha256"]
        return event["sha256"]

    def attempt(self, attempt):
        return self.append("attempt", attempt.to_dict())



def verify(path):
    """Recompute the chain; return (ok, events, bad_line, reason)."""
    p = Path(path)
    if not p.exists():
        return False, 0, 0, "journal missing"
    prev = ZERO
    n = 0
    for i, line in enumerate(p.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            ev = json.loads(line)
        except ValueError as exc:
            return False, n, i, "bad json: %s" % exc
        want = ev.pop("sha256", None)
        if ev.get("prev_sha256") != prev:
            return False, n, i, "prev mismatch"
        if chain_digest(prev, ev) != want:
            return False, n, i, "sha mismatch"
        prev = want
        n += 1
    return True, n, 0, "ok"


def index_path(root):
    return Path(root) / "tmp" / "govd" / "index.sqlite3"


def rebuild_index(root):
    """Rebuild the SQLite index from every journal; the JSONL stays authority."""
    base = Path(root) / "tmp" / "govd" / "runs"
    db = sqlite3.connect(str(index_path(root)))
    db.execute("CREATE TABLE IF NOT EXISTS attempts (run_id TEXT, task_id TEXT,"
               " verdict TEXT, rc INTEGER, duration_s REAL, reason TEXT, log TEXT)")
    db.execute("DELETE FROM attempts")
    rows = 0
    for ev_file in sorted(base.glob("*/events.jsonl")):
        run_id = ev_file.parent.name
        for line in ev_file.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            ev = json.loads(line)
            if ev.get("kind") != "attempt":
                continue
            p = ev.get("payload", {})
            db.execute("INSERT INTO attempts VALUES (?,?,?,?,?,?,?)",
                       (run_id, p.get("task_id"), p.get("verdict"), p.get("rc"),
                        p.get("duration_s"), p.get("reason"), p.get("log")))
            rows += 1
    db.commit()
    db.close()
    return rows


def reusable_ok_runs(root, manifest_digest, env_fingerprint, exclude_run=""):
    """Map task_id -> (run_id, cmd_digest) for gates that already passed here.

    Key = (manifest_digest, task_id, cmd_digest, env_fingerprint), per ADR-228
    §3.4. The scan reads each previous run's derived `report.json`, and a
    missing or unreadable report simply means "not reusable": reuse must never
    depend on a corrupt cache being read as a pass. Attempts that were
    themselves reused are excluded, so a reuse chain cannot grow across runs.
    """
    out = {}
    base = Path(root) / "tmp" / "govd" / "runs"
    if not base.is_dir():
        return out
    for report_file in sorted(base.glob("*/report.json")):
        run_id = report_file.parent.name
        if run_id == exclude_run:
            continue
        try:
            doc = json.loads(report_file.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        run = doc.get("run", {})
        if run.get("manifest_digest") != manifest_digest:
            continue
        if run.get("env_fingerprint") != env_fingerprint:
            continue
        for att in doc.get("attempts", []):
            if att.get("verdict") != "ok" or att.get("reused"):
                continue
            if not att.get("cmd_digest") or not att.get("task_id"):
                continue
            out[att["task_id"]] = (run_id, att["cmd_digest"])
    return out


def write_latest(root, run_id, extra=None):
    p = Path(root) / "tmp" / "govd" / "latest.json"
    p.parent.mkdir(parents=True, exist_ok=True)
    row = {"run_id": run_id, "at": round(time.time(), 3)}
    row.update(extra or {})
    p.write_text(_canon(row), encoding="utf-8")
    return p


def read_latest(root):
    p = Path(root) / "tmp" / "govd" / "latest.json"
    if not p.exists():
        return ""
    try:
        return json.loads(p.read_text(encoding="utf-8")).get("run_id", "")
    except ValueError:
        return ""


