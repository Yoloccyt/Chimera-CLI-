"""govd CLI: list / run / verify-journal / report / resume."""
from __future__ import annotations

import argparse
import dataclasses
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
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


def resolve_jobs(explicit_jobs, profile, root=None):
    """并发度决策：返回 (jobs, 说明)。

    显式 `--jobs` 压倒一切（用户明确要 8 就给 8，不做资源压制）；缺省时取
    profile 声明值，并让本机资源上限**向下**压制它（见 policy.derive_jobs）。

    WHY 显式值不压制：资源探测是"替我决定"的兜底，不是"否决我的指令"的闸门。
    显式值一旦被压制，`--jobs` 就成了装饰旗标 —— 与 policy.toml 的
    `jobs = 4` 一样，"写了没人读"是最容易被忽略的一类假绿。
    """
    if explicit_jobs and explicit_jobs > 0:
        return int(explicit_jobs), "explicit"
    res = pol.probe_resources(root if root is not None else ROOT)
    jobs = pol.derive_jobs(profile.jobs, res.cores, res.mem_gb, res.disk_free_gb)
    note = "profile=%d" % profile.jobs
    if jobs != profile.jobs:
        note += (" capped by host resources (cores=%d mem=%.1fGiB disk_free=%.1fGiB)"
                 % (res.cores, res.mem_gb, res.disk_free_gb))
    return jobs, note



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
    """Gates always; release merges every task family, other lanes merge only the
    external rows that explicitly name them (ADR-228 §3.2).

    WHY `profiles` is required on external rows instead of merging all tasks: the
    pr ledger must keep reading as "135 light + 7 heavy skipped". If six extra
    out-of-profile tasks were loaded and then skipped, `skip=` would silently grow
    from 7 to 13 and the one number an operator watches would stop meaning
    "the disk-gated heavies".
    """
    tasks = list(mf.load(str(MANIFEST)))
    if TASKS.is_file():
        rows = mf.load_tasks(str(TASKS))
        if profile_name == "release":
            tasks.extend(rows)
        else:
            tasks.extend(r for r in rows
                         if r.source == "external" and profile_name in r.profiles)
    return mf._finalize(tuple(tasks)), pol.load(str(POLICY))


def ci_surface(tasks):
    """CI 面 = 清单里未声明 `ci_exempt` 的门。

    WHY 由清单定义而不是由 workflow 自行枚举：`ci_exempt = "<reason>"` 已经是
    "该门按设计不接 CI"（Windows 权威执行体、收口期审计项等）的唯一登记处。
    在 CI 里跑它们只会把 rc=2 或"按设计为红"读成阻塞门红。
    过滤在 `_finalize` 之后调用：needs 图的完整性校验针对完整清单，不随执行面收缩。
    """
    kept = tuple(t for t in tasks if not t.ci_exempt)
    if len(kept) != len(tasks):
        print("[CI] ci_exempt gates excluded: %d (kept %d)"
              % (len(tasks) - len(kept), len(kept)))
    return kept


def cmd_list(args):
    tasks, policy = load_all(args.profile)
    if getattr(args, "ci", False):
        tasks = ci_surface(tasks)
    role_err = pol.role_check(policy, args.role, args.profile)
    if role_err:
        print("[UNDECIDABLE] %s" % role_err)
        return 2
    profile = policy.profiles[args.profile]
    jobs, jobs_note = resolve_jobs(args.jobs, profile)
    print("[JOBS] profile=" + args.profile + " jobs=" + str(jobs) + " (" + jobs_note + ")")
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


def _prior_verdicts(journal_path):
    """task_id -> verdict recorded in an existing journal (for `--resume`).

    WHY the verdict and not just the id: a resumed run must stay as red as the
    run it resumed. Counting only "it was attempted" would let a prior
    `mismatch` silently drop out of the ledger and flip a resumed run to green.
    """
    out = {}
    p = Path(journal_path)
    if not p.exists():
        return out
    for line in p.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        ev = json.loads(line)
        if ev.get("kind") != "attempt":
            continue
        payload = ev.get("payload", {})
        tid = payload.get("task_id")
        if tid:
            out[tid] = payload.get("verdict", Verdict.UNKNOWN.value)
    return out


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


REPORT_FILES = {
    "text": ("summary.md",),
    "json": ("report.json",),
    "junit": ("junit.xml",),
    "all": ("report.json", "junit.xml", "summary.md"),
}


def report_surface(run_dir, kind):
    """Return (surfaced, missing) for the artefacts this `run --report` promised.

    WHY a guarantee rather than a switch: every run writes all three artefacts
    because the journal directory is the evidence (Spec §3.3), so turning writing
    off would only make CI lose data. What the flag *must* do is give the caller a
    post-condition: a lane that asks for junit has to be told, in the exit code,
    when that artefact is not there. Accepting `--report junit` while producing
    nothing is exactly the decorative-flag shape this batch removed elsewhere.
    """
    names = REPORT_FILES.get(kind, REPORT_FILES["all"])
    surfaced, missing = [], []
    for name in names:
        path = Path(run_dir) / name
        if path.is_file() and path.stat().st_size > 0:
            surfaced.append("%s=%s (%d bytes)" % (name, path, path.stat().st_size))
        else:
            missing.append(name)
    return surfaced, missing


def cmd_run(args):
    tasks, policy = load_all(args.profile)
    if getattr(args, "ci", False):
        tasks = ci_surface(tasks)
    role_err = pol.role_check(policy, args.role, args.profile)
    if role_err:


        print("UNDECIDABLE " + role_err)
        return 2
    refuse = _reuse_refusal(args.profile, getattr(args, "reuse_ok", False))
    if refuse:
        print("UNDECIDABLE " + refuse)
        return 2
    profile = policy.profiles[args.profile]
    # A mutating task that this profile would select must not be dropped quietly.
    # `planner.plan()` filters mutating rows out via selected_by(), which made the
    # flag check a silent skip: the guard stopped being a guard without anything
    # ever going red. Decide here, before planning, so refusing stays loud.
    blocked = [t.id for t in tasks
               if t.mutating and not args.allow_mutating
               and (not t.profiles or args.profile in t.profiles)
               and (profile.include_heavy or t.kind != "heavy")]
    if blocked:
        print("UNDECIDABLE mutating task(s) %s need --allow-mutating"
              % ",".join(sorted(blocked)))
        return 2
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


    # Only reachable when --allow-mutating was given (the case above already refused
    # everything else), so this check is about tree cleanliness, not about the flag.
    mutating = [t for t in ordered if t.mutating]
    before = pol.snapshot(ROOT)
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
    prior = _prior_verdicts(journal.events) if args.resume else {}
    # Reuse is opt-in per run and keyed on the whole manifest digest, so an edit
    # anywhere in gate_manifest.toml invalidates every cached verdict at once.
    reuse = (store.reusable_ok_runs(ROOT, digest_value, meta.env_fingerprint,
                                    exclude_run=rid)
             if getattr(args, "reuse_ok", False) else {})


    # The journal is the declared authority (ADR-228 §2.4), so provenance has to
    # live *in it*, not only in the derived report.json: "who ran which HEAD, under
    # which role, with which digest" must survive even if every derived artefact is
    # deleted. Hence the whole meta, not a hand-picked subset.
    journal.append("run_start", {**dataclasses.asdict(meta),
                                 "shard": shard_note,
                                 "selected": len(ordered), "skipped": len(skipped)})


    attempts = []
    order = {t.id: i for i, t in enumerate(tasks)}

    def execute(task):
        # 真红即停（fail-fast）：第一个 mismatch 出现后，尚未开始的门不再启动。
        #
        # WHY 取消而不是继续跑完：真红 = 仓库已经坏了，剩下的门不会改变这个结论，
        # 继续跑只是消耗机器时间；而且真红之后的门往往也是被同一处缺陷带崩的，
        # 让它们各写一份 tmp/ 证据只会制造噪声。
        #
        # WHY 已启动的门绝不 kill：它们可能正在往 tmp/ 里写证据，杀掉会留下半写的
        # 文件（"门没跑完"被读成"门跑完了但红了"）。已启动的门一律让它自然结束，
        # 它的判决照常进台账 —— 半写文件比晚一点结束贵得多。
        if stop.is_set():
            return Attempt(task.id, 0, Verdict.CANCELLED, None, 0.0,
                           "cancelled:fail-fast", cmd_digest=task.cmd_digest())
        if task.kind == "heavy" and task.disk_gb:
            # WHY 磁盘余额在执行点现取，而不是循环外采样一次：并发下越靠后的门用的
            # 是越旧的读数，于是 UNKNOWN（"这道门真的不可判"）会退化成"来不及看"。
            # 判据的正确性不允许这种混淆 —— 串行时两者恰好等价，所以旧写法从不显红。
            free_gib = shutil.disk_usage(str(ROOT)).free / 1024 ** 3
            if free_gib < task.disk_gb:
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
        attempt = _execute_one(task, policy, bash_path, journal.logs, 1)
        # 只有 mismatch 停表：env-fail / unknown 都不停（见 test_govd 的三态传播断言）。
        # WHY 只有真红停：env-fail 是环境问题（另一个门可能好着），unknown 是资源不足
        # （磁盘门槛），两者都不构成"仓库坏了"的结论，继续跑才能把"有多少门受同一处
        # 环境拖累"说清楚。mismatch 才是真红。
        #
        # `--no-fail-fast`（全量收口档）：有存量真红时也要拿到"全部门 × 全部判定"
        # 的一张完整台账 —— 否则 fail-fast 让后续门永远 CANCELLED，既看不清债务总量，
        # 也无法做 A/B 并发收益的同轮配对（判定不变性需要一个跑满的基线）。
        if attempt.verdict is Verdict.MISMATCH and not args.no_fail_fast:
            stop.set()
        return attempt

    def record(att):
        # 逐条落盘，不是攒齐再写：Ctrl-C 之后 events.jsonl 仍能证明哪些门真的跑过。
        # 旧写法 `list(pool.map(...))` 让整轮 run 在 journal 里只留一个 run_start，
        # 与 Spec §3.4「已完成事件保留」直接冲突。
        journal.attempt(att)
        attempts.append(att)
        marker = " (reused from %s)" % att.reused_from if att.reused else ""
        print("  " + att.task_id + " " + att.verdict.value + " rc=" + str(att.rc) + marker)

    pending = [t for t in ordered if t.id not in done]
    # Carried-forward verdicts: without these a resumed run would under-report
    # its own ledger (140 gates planned, N attempted, N-k "seen") and -- worse --
    # a prior mismatch would vanish. Only ids still present in this plan count.
    for task in ordered:
        if task.id in done and task.id in prior:
            attempts.append(Attempt(task.id, 0, Verdict(prior[task.id]), None, 0.0,
                                    "resumed from journal", cmd_digest=task.cmd_digest()))
            print("  " + task.id + " " + prior[task.id] + " (resumed, not re-run)")
    futs = []
    cancelled = False
    # 并发度决策：显式 `--jobs` > profile 声明值经资源压制（见 resolve_jobs）。
    jobs, jobs_note = resolve_jobs(args.jobs, profile)
    print("[JOBS] profile=" + args.profile + " jobs=" + str(jobs) + " (" + jobs_note + ")")
    # 真红即停（fail-fast）：第一个 mismatch 出现后，尚未开始的门不再启动。
    # WHY threading.Event 而不是共享变量：并发下多线程需要原子可见性；Event 是 stdlib
    # 提供的最小同步原语，比 `bool` 加锁便宜，比 `list.append` 语义明确。
    stop = threading.Event()
    # ADR-315 W-T1: resource mutual exclusion.
    #
    # WHY this is not `needs`: the manifest header records the measured fact
    # that `needs` buys only inclusion order, NOT a mutex -- with jobs>1 the
    # executor submits every pending task into the pool at once and never reads
    # `needs` again. Concurrency therefore has to be serialised on an explicit
    # resource key.
    #
    # WHY groups instead of one global lock: a single global lock would
    # serialise the whole run (defeating the point of the parallel scheduler);
    # per-key groups keep independent resources genuinely parallel while
    # guaranteeing that two gates sharing a key never overlap in time.
    #
    # The partition is by `exclusive` key first (mutual exclusion is a hard
    # constraint), then by `group` for the remaining unconstrained tasks (the
    # `concurrency` cap of a group bounds how many of its members run at once).
    exclusive_groups: dict[str, list] = {}
    free_tasks: list = []
    for t in pending:
        if t.exclusive:
            exclusive_groups.setdefault(t.exclusive, []).append(t)
        else:
            free_tasks.append(t)
    # Deterministic key order: the journal and the JUnit report must not
    # depend on dict insertion order (which follows the manifest, but a
    # resumed run could differ).
    ordered_exclusive = sorted(exclusive_groups)

    def run_exclusive_chain(key: str) -> list:
        """Run every gate sharing one resource key, strictly one after another.

        WHY sequential inside the chain even when jobs>1: the whole point of
        the key is that these gates must never overlap. Running them one at a
        time is the enforcement; running the chains themselves in parallel is
        what preserves throughput.
        """
        chain = exclusive_groups[key]
        out = []
        for task in chain:
            out.append(execute(task))
        return out

    try:
        if jobs > 1:
            with ThreadPoolExecutor(max_workers=jobs) as pool:
                # Exclusive chains are submitted as unit jobs: one future per
                # resource key, each running its chain sequentially. Free tasks
                # are submitted individually.
                chain_futs = {key: pool.submit(run_exclusive_chain, key)
                              for key in ordered_exclusive}
                free_futs = [pool.submit(execute, t) for t in free_tasks]
                for fut in as_completed(list(chain_futs.values()) + free_futs):
                    result = fut.result()
                    if isinstance(result, list):
                        for att in result:
                            record(att)
                    else:
                        record(result)
        else:
            for key in ordered_exclusive:
                for att in run_exclusive_chain(key):
                    record(att)
            for task in free_tasks:
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

    surfaced, missing = report_surface(journal.dir, getattr(args, "report", "all"))
    for line in surfaced:
        print("REPORT " + line)
    if missing:
        print("UNDECIDABLE requested --report %s but these artefacts are absent or empty: %s"
              % (getattr(args, "report", "all"), ",".join(missing)))
        return 2


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


def cmd_resume(args):
    """Thin surface over `run --resume`; one behaviour, one entry point.

    The profile is read back from the resumed run's report.json when the caller
    does not pass one, so a nightly run cannot be "resumed" under pr semantics
    (which would silently drop the heavy gates from the plan).
    """
    if not args.profile:
        report = ROOT / "tmp" / "govd" / "runs" / args.resume_id / "report.json"
        try:
            args.profile = json.loads(report.read_text(encoding="utf-8"))["run"]["profile"]
        except (OSError, ValueError, KeyError):
            print("UNDECIDABLE cannot read profile of run %s from report.json"
                  % args.resume_id)
            return 2
    args.resume = args.resume_id
    return cmd_run(args)


def build_parser():
    p = argparse.ArgumentParser(prog="govd")
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("list", "run"):
        s = sub.add_parser(name)
        s.add_argument("--profile", choices=PROFILE_NAMES, default="pr")
        s.add_argument("--role", default="local-dev")


        s.add_argument("--allow-mutating", action="store_true")
        s.add_argument("--jobs", type=int, default=0)
        s.add_argument("--no-fail-fast", action="store_true", dest="no_fail_fast",
                       help="keep running every gate after a mismatch (full-ledger "
                            "mode: debt census + A/B pairing against a complete run)")
        s.add_argument("--shard", default="",
                      help="run only shard k of n, e.g. 2/4 (deterministic round-robin)")
        s.add_argument("--ci", action="store_true",
                       help="run the CI surface only: drop gates declaring "
                            "ci_exempt in gate_manifest.toml (the manifest is the "
                            "single definition of what CI is supposed to run)")
        s.add_argument("--resume", default="")
        s.add_argument("--report", choices=("text", "json", "junit", "all"),
                       default="all",
                       help="guarantee these artefacts exist after the run (missing -> rc 2)")
    v = sub.add_parser("verify-journal")
    v.add_argument("run_id", nargs="?", default="")
    res = sub.add_parser("resume")
    res.add_argument("resume_id")
    res.add_argument("--profile", choices=PROFILE_NAMES, default="")
    res.add_argument("--role", default="local-dev")
    res.add_argument("--allow-mutating", action="store_true")
    res.add_argument("--jobs", type=int, default=0)
    res.add_argument("--no-fail-fast", action="store_true", dest="no_fail_fast",
                     help="same as `run --no-fail-fast`; resume must accept it or a "
                          "resumed full-ledger run would silently re-arm fail-fast")
    res.add_argument("--shard", default="")
    res.add_argument("--ci", action="store_true",
                     help="same as `run --ci`; resume must accept it or a resumed "
                          "CI-surface run would silently re-add the ci_exempt gates")
    res.add_argument("--report", choices=("text", "json", "junit", "all"), default="all")
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
    if args.cmd == "resume":
        return cmd_resume(args)
    return cmd_report(args)

