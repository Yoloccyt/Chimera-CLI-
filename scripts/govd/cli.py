"""govd CLI: list / run / verify-journal / report / resume."""
from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

from . import __version__
from . import manifest as mf
from . import planner
from . import policy as pol
from . import report as rp
from . import store
from .executor import augment_path, env_fingerprint, run_attempt, usable_bash
from .model import PROFILE_NAMES, Attempt, RunMeta, Verdict

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "scripts" / "gate_manifest.toml"
POLICY = ROOT / "scripts" / "govd" / "policy.toml"
TASKS = ROOT / "scripts" / "govd" / "tasks.toml"


def _git(args, default=""):
    try:
        p = subprocess.run(["git"] + args, cwd=str(ROOT), capture_output=True,
                           text=True, encoding="utf-8", errors="replace", timeout=30)
    except (OSError, subprocess.SubprocessError):
        return default
    return p.stdout.strip() if p.returncode == 0 else default


def new_run_id():
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    head = _git(["rev-parse", "--short", "HEAD"], "nohead")
    return "%s-%s-%d" % (stamp, head, os.getpid())


def meta_for(run, profile_name, digest_value, role):
    dirty = len([l for l in _git(["status", "--porcelain"]).splitlines() if l.strip()])
    actor = (os.environ.get("GITHUB_ACTOR") or os.environ.get("USERNAME")
             or os.environ.get("USER") or "unknown")
    return RunMeta(run_id=run, profile=profile_name,
                   started_at=datetime.now(timezone.utc).isoformat(timespec="seconds"),
                   head=_git(["rev-parse", "--short", "HEAD"], "nohead"), dirty=dirty,
                   actor=actor, host=socket.gethostname(),
                   python=sys.version.split()[0], manifest_digest=digest_value,
                   env_fingerprint=env_fingerprint(), role=role)


def load_all(profile_name=""):
    """Gates always; rust/external families only for release (ADR-228 §3.2)."""
    tasks = list(mf.load(str(MANIFEST)))
    if profile_name == "release":
        tasks.extend(mf.load_tasks(str(TASKS)))
    return mf._finalize(tuple(tasks)), pol.load(str(POLICY))


def cmd_list(args):
    tasks, policy = load_all(args.profile)
    role_err = pol.role_check(policy, args.role, args.profile)
    if role_err:
        print("[UNDECIDABLE] %s" % role_err)
        return 2
    profile = policy.profiles[args.profile]
    ordered, skipped = planner.plan(tasks, profile, args.allow_mutating)
    try:
        ordered, shard_note = planner.apply_shard(ordered, args.shard)
    except planner.PlanError as exc:
        print("[UNDECIDABLE] %s" % exc)
        return 2
    for t in ordered:
        print("  " + t.id + " " + t.kind + " expect=" + str(t.expect))
        print("      timeout=" + str(pol.timeout_for(t, policy)) + "s " + t.cmd)
    tail = (" shard=" + shard_note) if shard_note else ""
    print(chr(91) + "SUM" + chr(93) + " selected=" + str(len(ordered)) + " total=" + str(len(tasks)) + " skipped=" + str(len(skipped)) + tail)
    return 0
def _completed(journal_path):
    done = set()
    p = Path(journal_path)
    if not p.exists():
        return done
    for line in p.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        ev = json.loads(line)
        if ev.get("kind") == "attempt":
            done.add(ev.get("payload", {}).get("task_id"))
    return done


def _execute_one(task, policy, bash_path, logs, n):
    reason = pol.whitelist_check(policy, task.cmd)


    if reason:
        return Attempt(task.id, n, Verdict.LAUNCH, None, 0.0, "policy",
                       cmd_digest=task.cmd_digest())


    budget = pol.retry_budget(task, policy)
    while True:
        log = logs / (task.id + "." + str(n) + ".log")


        attempt = run_attempt(task, str(log), pol.timeout_for(task, policy), bash_path)
        attempt.n = n
        attempt.cmd_digest = task.cmd_digest()
        if attempt.verdict.retryable and n <= budget:
            n += 1
            continue
        return attempt


def _reuse_refusal(profile_name, reuse_ok):
    """`release` forbids reuse outright; every other profile needs the explicit flag."""
    if reuse_ok and profile_name == "release":
        return ("release profile forbids --reuse-ok: a release verdict must be "
                "produced by this run, not inherited from an earlier one "
                "(ADR-228 §3.4)")
    return ""


def cmd_run(args):
    tasks, policy = load_all(args.profile)
    role_err = pol.role_check(policy, args.role, args.profile)
    if role_err:


        print("UNDECIDABLE " + role_err)
        return 2
    refuse = _reuse_refusal(args.profile, getattr(args, "reuse_ok", False))
    if refuse:
        print("UNDECIDABLE " + refuse)
        return 2
    profile = policy.profiles[args.profile]
    ordered, skipped = planner.plan(tasks, profile, args.allow_mutating)
    try:
        ordered, shard_note = planner.apply_shard(ordered, args.shard)
    except planner.PlanError as exc:
        print("UNDECIDABLE " + str(exc))
        return 2
    # A shard is a partial run, so its ledger must be shard-local: `skipped`
    # (the heavy gates the profile never selected) is reported by shard 1 only,
    # otherwise four CI jobs would each claim the whole profile's accounting.
    owns_skipped = not shard_note or shard_note.split("/")[0] == "1"
    reported_skipped = skipped if owns_skipped else []


    mutating = [t for t in ordered if t.mutating]
    before = pol.snapshot(ROOT)
    if mutating and not args.allow_mutating:
        print("UNDECIDABLE mutating tasks need allow-mutating")
        return 2


    if mutating and before:
        print("UNDECIDABLE dirty tree; mutating tasks need a clean tree")
        return 2
    augment_path()
    bash_path, bash_note = usable_bash()
    print("bash=" + str(bash_path) + " :: " + bash_note)


    rid = args.resume or new_run_id()
    journal = store.Journal(ROOT, rid)
    digest_value = mf.digest(tasks)
    meta = meta_for(rid, args.profile, digest_value, args.role)
    done = _completed(journal.events) if args.resume else set()
    # Reuse is opt-in per run and keyed on the whole manifest digest, so an edit
    # anywhere in gate_manifest.toml invalidates every cached verdict at once.
    reuse = (store.reusable_ok_runs(ROOT, digest_value, meta.env_fingerprint,
                                    exclude_run=rid)
             if getattr(args, "reuse_ok", False) else {})


    journal.append("run_start", {"run_id": rid, "profile": args.profile,
                                 "head": meta.head, "dirty": meta.dirty,
                                 "shard": shard_note,
                                 "selected": len(ordered), "skipped": len(skipped)})


    free = shutil.disk_usage(str(ROOT)).free / 1024 ** 3
    attempts = []
    order = {t.id: i for i, t in enumerate(tasks)}

    def execute(task):
        if task.kind == "heavy" and task.disk_gb and free < task.disk_gb:
            return Attempt(task.id, 1, Verdict.UNKNOWN, None, 0.0, "disk",
                           cmd_digest=task.cmd_digest())
        hit = reuse.get(task.id)
        if hit and hit[1] == task.cmd_digest():
            # The gate did not run here. It is reported as ok because the operator
            # asked for it, but it is stamped reused/reused_from, counted separately
            # in [SUM], and rendered as <skipped/> in JUnit so no consumer can read
            # an unexecuted gate as fresh evidence.
            return Attempt(task.id, 0, Verdict.OK, 0, 0.0,
                           "reused-ok from " + hit[0], cmd_digest=task.cmd_digest(),
                           reused=True, reused_from=hit[0])
        return _execute_one(task, policy, bash_path, journal.logs, 1)

    def record(att):
        # 逐条落盘，不是攒齐再写：Ctrl-C 之后 events.jsonl 仍能证明哪些门真的跑过。
        # 旧写法 `list(pool.map(...))` 让整轮 run 在 journal 里只留一个 run_start，
        # 与 Spec §3.4「已完成事件保留」直接冲突。
        journal.attempt(att)
        attempts.append(att)
        marker = " (reused from %s)" % att.reused_from if att.reused else ""
        print("  " + att.task_id + " " + att.verdict.value + " rc=" + str(att.rc) + marker)

    pending = [t for t in ordered if t.id not in done]
    futs = []
    cancelled = False
    try:
        if args.jobs and args.jobs > 1:
            with ThreadPoolExecutor(max_workers=args.jobs) as pool:
                futs = [pool.submit(execute, t) for t in pending]
                for fut in as_completed(futs):
                    record(fut.result())
        else:
            for task in pending:
                record(execute(task))
    except KeyboardInterrupt:
        cancelled = True
        for fut in futs:
            fut.cancel()
        journal.append("cancelled", {"done": sorted(a.task_id for a in attempts)})
        print("CANCELLED: kept %d/%d finished attempt(s) in the journal"
              % (len(attempts), len(pending)))

    # 汇总按 manifest 序，不看完成先后：并发不能让报告顺序变成调度噪声。
    attempts.sort(key=lambda a: order.get(a.task_id, 10 ** 9))


    after = pol.snapshot(ROOT)
    extra = pol.violations(before, after, policy.mutable_prefixes)
    if extra:
        journal.append("policy_violation", {"paths": extra})
        print("POLICY-VIOLATION " + ",".join(extra))


    (journal.dir / "report.json").write_text(rp.to_json(meta, attempts, reported_skipped), encoding="utf-8")
    (journal.dir / "junit.xml").write_text(rp.to_junit(attempts, reported_skipped), encoding="utf-8")
    sha = journal.append("run_end", {"counts": dict(rp.counts(attempts, reported_skipped))})


    summary = rp.step_summary(meta, attempts, reported_skipped, sha)
    if shard_note:
        summary = summary.replace("profile=" + args.profile,
                                  "profile=" + args.profile + " shard=" + shard_note)
    (journal.dir / "summary.md").write_text(summary, encoding="utf-8")
    step = os.environ.get("GITHUB_STEP_SUMMARY")
    if step:
        with open(step, "a", encoding="utf-8") as fh:
            fh.write(summary)


    store.write_latest(ROOT, rid, {"status": "done"})
    store.rebuild_index(ROOT)
    expected = len(ordered) + len(reported_skipped)
    print(rp.sum_line(attempts, reported_skipped, expected))
    print("run_id=" + rid)
    rc = rp.exit_code(attempts, reported_skipped, expected)
    if cancelled:
        rc = 2  # 中断不是红门，但绝不能被读成「全绿」
    if extra:
        rc = 2
    return rc


def cmd_verify_journal(args):
    rid = args.run_id or store.read_latest(ROOT)
    if not rid:
        print("UNDECIDABLE no run id")
        return 2
    path = ROOT / "tmp" / "govd" / "runs" / rid / "events.jsonl"
    ok, n, bad, reason = store.verify(path)


    print("journal=" + str(path))
    print("events=" + str(n) + " bad_line=" + str(bad) + " reason=" + reason)
    return 0 if ok else 2


def cmd_report(args, root=None):
    """Print a past run's artefacts; `--format` picks which, default text.

    `root` is a parameter (not just the module ROOT) so the format selection is
    testable without touching the repository's real journal directory.
    """
    base = (Path(root) if root else ROOT) / "tmp" / "govd" / "runs"
    rid = getattr(args, "run_id_pos", "") or args.run_id or store.read_latest(root or ROOT)
    if not rid:
        print("UNDECIDABLE no run id")
        return 2
    path = base / rid / "events.jsonl"
    if not path.exists():
        print("UNDECIDABLE journal missing")
        return 2
    fmt = getattr(args, "format", "text")


    rows = store.rebuild_index(root or ROOT)
    if fmt in ("text", "all"):
        print("indexed=" + str(rows) + " run=" + rid)
        print((base / rid / "summary.md").read_text(encoding="utf-8"))
    for kind, name in (("json", "report.json"), ("junit", "junit.xml")):
        if fmt not in (kind, "all"):
            continue
        artefact = base / rid / name
        if not artefact.exists():
            print("UNDECIDABLE missing artefact: " + name)
            return 2
        sys.stdout.write(artefact.read_text(encoding="utf-8"))
    return 0


def build_parser():
    p = argparse.ArgumentParser(prog="govd")
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("list", "run"):
        s = sub.add_parser(name)
        s.add_argument("--profile", choices=PROFILE_NAMES, default="pr")
        s.add_argument("--role", default="local-dev")


        s.add_argument("--allow-mutating", action="store_true")
        s.add_argument("--jobs", type=int, default=0)
        s.add_argument("--shard", default="",
                      help="run only shard k of n, e.g. 2/4 (deterministic round-robin)")
        s.add_argument("--resume", default="")
        s.add_argument("--report", choices=("text", "json", "junit", "all"),
                       default="all", help="CI shadow compatibility")
    v = sub.add_parser("verify-journal")
    v.add_argument("run_id", nargs="?", default="")
    # --reuse-ok belongs to `run` only: `list` must not silently accept a flag it
    # ignores (the F24/F29 silent-default family that check_entry_contract C3 judges).
    sub.choices["run"].add_argument("--reuse-ok", action="store_true",
                                    help="skip gates already ok under the same manifest/cmd/env fingerprint")
    r = sub.add_parser("report")
    r.add_argument("run_id_pos", nargs="?", default="")
    r.add_argument("--run-id", default="")
    r.add_argument("--format", choices=("text", "json", "junit", "all"), default="text",
                   help="which artefact of the run to print")
    return p


def main(argv):
    args = build_parser().parse_args(argv)
    if args.cmd == "list":
        return cmd_list(args)
    if args.cmd == "run":
        return cmd_run(args)
    if args.cmd == "verify-journal":
        return cmd_verify_journal(args)
    return cmd_report(args)

