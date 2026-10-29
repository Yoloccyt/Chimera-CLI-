"""Manifest loader for govd (ADR-228)."""
from __future__ import annotations

import hashlib
import json
import tomllib

from .model import PROFILE_NAMES, Task

REQUIRED = ("id", "spec_ref", "cmd", "expect", "kind", "note")
REGISTRY_STATUS = ("active", "diagnostic", "evidence", "retired")


class ManifestError(ValueError):
    """Bad manifest: the caller must exit 2, never judge a gate."""


def _default_timeout(kind: str) -> int:
    return 1800 if kind == "heavy" else 300


def _str_tuple(raw, name: str, gid: str) -> tuple[str, ...]:
    if raw is None:
        return ()
    if not isinstance(raw, list) or not all(isinstance(x, str) for x in raw):
        raise ManifestError("%s: %s must be a list of strings" % (gid, name))
    return tuple(raw)

def _task_from(g: dict) -> Task:
    gid = g.get("id")
    if not isinstance(gid, str) or not gid:
        raise ManifestError("a [[gate]] entry has no string id")
    missing = [k for k in REQUIRED if k not in g]
    if missing:
        raise ManifestError("%s: missing required field(s) %s" % (gid, missing))
    if not isinstance(g["expect"], int) or g["expect"] not in (0, 1, 2):
        raise ManifestError("%s: expect must be 0, 1 or 2" % gid)
    if g["kind"] not in ("light", "heavy"):
        raise ManifestError("%s: kind must be light|heavy" % gid)
    profiles = _str_tuple(g.get("profiles"), "profiles", gid)
    bad = [p for p in profiles if p not in PROFILE_NAMES]
    if bad:
        raise ManifestError("%s: unknown profile(s) %s" % (gid, bad))
    retry = g.get("retry", -1)
    if not isinstance(retry, int) or retry < -1 or retry > 3:
        raise ManifestError("%s: retry must be -1..3" % gid)
    timeout_s = g.get("timeout_s", _default_timeout(g["kind"]))
    if not isinstance(timeout_s, int) or timeout_s <= 0:
        raise ManifestError("%s: timeout_s must be a positive int" % gid)
    if not isinstance(g.get("mutating", False), bool):
        raise ManifestError("%s: mutating must be bool" % gid)
    return Task(
        id=gid, cmd=str(g["cmd"]), expect=g["expect"], kind=g["kind"],
        spec_ref=str(g["spec_ref"]), note=str(g["note"]),
        timeout_s=timeout_s, retry=retry, mutating=g.get("mutating", False),
        needs=_str_tuple(g.get("needs"), "needs", gid), profiles=profiles,
        owner=str(g.get("owner", "")), disk_gb=float(g.get("disk_gb", 0)),
        source=str(g.get("source", "gate")),
    )

def read_gates(path) -> list[dict]:
    with open(path, "rb") as fh:
        doc = tomllib.load(fh)
    gates = doc.get("gate", [])
    if not isinstance(gates, list) or not gates:
        raise ManifestError("no [[gate]] entries in %s" % path)
    return gates


def _finalize(tasks) -> tuple[Task, ...]:
    ids = [t.id for t in tasks]
    dup = sorted({i for i in ids if ids.count(i) > 1})
    if dup:
        raise ManifestError("duplicate gate id(s): %s" % dup)
    known = set(ids)
    for t in tasks:
        unknown = [n for n in t.needs if n not in known]
        if unknown:
            raise ManifestError("%s: needs unknown task(s) %s" % (t.id, unknown))
    return tasks


def load(path) -> tuple[Task, ...]:
    gates = read_gates(path)
    return _finalize(tuple(_task_from(g) for g in gates))


def load_tasks(path) -> tuple[Task, ...]:
    """Load [[task]] rows (rust/external/diagnostic families).

    Only the release profile merges these; they are not gates and must never
    inflate the pr/nightly accounting (ADR-228 §3.2).
    """
    with open(path, "rb") as fh:
        doc = tomllib.load(fh)
    rows = doc.get("task", [])
    if not isinstance(rows, list):
        raise ManifestError("task is not an array of tables in %s" % path)
    return _finalize(tuple(_task_from(row) for row in rows))


def cmd_digest(cmd: str) -> str:
    return hashlib.sha256(cmd.encode("utf-8")).hexdigest()[:16]


def digest(tasks) -> str:
    payload = [t.to_dict() for t in sorted(tasks, key=lambda x: x.id)]
    raw = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(raw).hexdigest()

def load_registry(path) -> tuple[dict, ...]:
    """Return [[tool]] rows. Missing file is an error, never an empty pass."""
    with open(path, "rb") as fh:
        doc = tomllib.load(fh)
    rows = doc.get("tool", [])
    if not isinstance(rows, list):
        raise ManifestError("tool must be an array of tables in %s" % path)
    return tuple(rows)


def validate_registry(rows) -> list[str]:
    problems = []
    seen = set()
    for i, row in enumerate(rows, 1):
        if not isinstance(row, dict):
            problems.append("row %d is not a table" % i)
            continue
        for key in ("script", "status", "owner", "evidence"):
            if not str(row.get(key, "")).strip():
                problems.append("row %d: %s is required" % (i, key))
        name = str(row.get("script", ""))
        if not name.startswith("scripts/"):
            problems.append("row %d: script must be repo-relative under scripts/" % i)
        if name in seen:
            problems.append("row %d: duplicate script %s" % (i, name))
        seen.add(name)
        if row.get("status") not in REGISTRY_STATUS:
            problems.append("row %d: status must be one of %s" % (i, REGISTRY_STATUS))
    return problems