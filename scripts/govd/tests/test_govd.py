"""govd self-tests (stdlib unittest). ADR-228 P3 batches I3-I5."""
from __future__ import annotations

import io
import json
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import xml.etree.ElementTree as ET
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
ROOT = Path(__file__).resolve().parents[3]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.govd import classify, cli, executor, manifest, planner, policy, report, store
from scripts.govd.model import Attempt, Profile, RunMeta, Task, Verdict

MANIFEST = ROOT / "scripts" / "gate_manifest.toml"
POLICY = ROOT / "scripts" / "govd" / "policy.toml"

def make_task(**kw):
    base = {"id": "T-1", "cmd": "{python} -c pass", "expect": 0,
            "kind": "light", "spec_ref": "test", "note": "test"}
    base.update(kw)
    return Task(**base)


class TempDirTest(unittest.TestCase):

    def setUp(self):
        base = ROOT / "tmp"
        base.mkdir(exist_ok=True)
        self.tmp = Path(tempfile.mkdtemp(prefix="govd-test-", dir=str(base)))

    def tearDown(self):
        for p in sorted(self.tmp.rglob("*"), key=lambda x: len(str(x)), reverse=True):
            if p.is_file():
                p.unlink()
            elif not any(p.iterdir()):
                p.rmdir()
        self.tmp.rmdir()

    def write(self, name, text):
        p = self.tmp / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")
        return p


class ClassifyTests(unittest.TestCase):
    ENV_CASES = (
        ("ghost-pipefail-two-line",
         "scripts/check_doc_drift.sh: line 40: set: pipefail\n: invalid option name\n",
         "bash s.sh", True),
        ("ghost-bad-interpreter", "/bin/sh: Bad interpreter.\n", "sh s.sh", True),
        ("py-crash-traceback",
         "Traceback (most recent call last):\n  File \"x.py\", line 1\n",
         "{python} x.py", True),
        ("real-finding-nosuchfile-midbody",
         "[GAP-A] a -> b violates rule\nref: No such file or directory (context only)\n",
         "bash s.sh", False),        ("real-verdict-chinese-undecidable",
         "!! 不可判定：找不到可执行 pwsh\n", "{python} x.py", False),
        ("real-verdict-normal-fail",
         "[FAIL] three-way reconciliation found gaps\n", "bash s.sh", False),
        ("shell-sig-without-error-shape",
         "note: invalid option name is not accepted here\n[FAIL] 2 gaps\n",
         "bash s.sh", False),
        ("empty-log", "\n\n", "bash s.sh", False),
    )

    VERDICT_CASES = (
        ("expect1-crash-is-not-ok", 1, 1,
         "Traceback (most recent call last):\nSyntaxError: bad decode\n", Verdict.ENV),
        ("expect1-real-red-is-ok", 1, 1,
         "[GAP] two sources disagree\n[FAIL] 2 gaps\n", Verdict.OK),
        ("expect0-green-is-ok", 0, 0, "[OK] all gates as expected\n", Verdict.OK),
        ("expect0-undecidable-is-mismatch", 2, 0,
         "[UNDECIDABLE] missing data file: scripts/x.toml\n", Verdict.MISMATCH),
    )

    def test_env_fixture_count_is_eight(self):
        self.assertEqual(len(self.ENV_CASES), 8)

    def test_env_fixtures(self):
        for name, text, cmd, want in self.ENV_CASES:
            with self.subTest(name=name):
                got = classify.env_failure_reason(text, cmd) is not None
                self.assertEqual(got, want)

    def test_verdict_order(self):
        for name, rc, exp, text, want in self.VERDICT_CASES:
            with self.subTest(name=name):
                got = classify.classify(rc, exp, text, "{python} x.py")[0]
                self.assertIs(got, want)

    def test_attempt_precedence(self):
        launch = classify.classify_attempt(0, 0, "", "x", launch_error="OSError")
        self.assertIs(launch[0], Verdict.LAUNCH)
        timeout = classify.classify_attempt(None, 0, "note", "x", timed_out=True)
        self.assertIs(timeout[0], Verdict.TIMEOUT)
        self.assertEqual(timeout[1], "timeout")

GATE = """[[gate]]
id = "%s"
spec_ref = "test"
cmd = "{python} -c pass"
expect = 0
kind = "light"
note = "test"
"""


class ManifestTests(TempDirTest):

    def test_real_census(self):
        tasks = manifest.load(MANIFEST)
        ids = [t.id for t in tasks]
        self.assertEqual(len(tasks), 140)
        self.assertEqual(len(set(ids)), 140)
        self.assertEqual(sum(1 for t in tasks if t.kind == "light"), 133)
        self.assertEqual(sum(1 for t in tasks if t.kind == "heavy"), 7)

    def test_digest_is_order_independent_and_sensitive(self):
        tasks = manifest.load(MANIFEST)
        self.assertEqual(manifest.digest(tasks), manifest.digest(tuple(reversed(tasks))))
        self.assertNotEqual(manifest.digest(tasks), manifest.digest(tasks[:-1]))
        self.assertEqual(len(manifest.digest(tasks)), 64)

    def test_v2_defaults_are_explicit(self):
        path = self.write("ok.toml", GATE % "G-1")
        task = manifest.load(path)[0]
        self.assertEqual(task.timeout_s, 300)
        self.assertEqual(task.retry, -1)
        self.assertFalse(task.mutating)
        self.assertEqual(task.needs, ())
        self.assertEqual(task.profiles, ())
        self.assertEqual(task.source, "gate")

    def test_invalid_manifests_are_rejected(self):
        base = GATE % "G-1"
        cases = (
            ("duplicate", base + GATE % "G-1", "duplicate gate id"),
            ("expect", base.replace("expect = 0", "expect = 3"), "expect must be"),
            ("kind", base.replace('kind = "light"', 'kind = "medium"'), "kind must be"),
            ("profile", base + 'profiles = ["staging"]\n', "unknown profile"),
            ("retry", base + "retry = 9\n", "retry must be"),
            ("timeout", base + "timeout_s = 0\n", "timeout_s must be"),
            ("needs", base + 'needs = ["G-9"]\n', "needs unknown task"),
        )
        for name, text, needle in cases:
            with self.subTest(name=name):
                path = self.write(name + ".toml", text)
                with self.assertRaises(manifest.ManifestError) as ctx:
                    manifest.load(path)
                self.assertIn(needle, str(ctx.exception))

    def test_empty_manifest_is_never_a_pass(self):
        path = self.write("empty.toml", "# no gates\n")
        with self.assertRaises(manifest.ManifestError):
            manifest.load(path)

    def test_registry_rows_need_lifecycle_and_evidence(self):
        rows = (
            {"script": "scripts/a.py", "status": "active", "owner": "x", "evidence": "y"},
            {"script": "scripts/a.py", "status": "zombie"},
            {"script": "a.py", "status": "active", "owner": "", "evidence": ""},
        )
        text = chr(10).join(manifest.validate_registry(rows))
        self.assertIn("duplicate script", text)
        self.assertIn("status must be one of", text)
        self.assertIn("under scripts/", text)
        self.assertIn("owner is required", text)


class PlannerTests(unittest.TestCase):

    def test_toposort_respects_needs_and_is_stable(self):
        a = make_task(id="A")
        b = make_task(id="B", needs=("A",))
        c = make_task(id="C")
        self.assertEqual([t.id for t in planner.toposort([b, c, a])], ["A", "B", "C"])
        self.assertEqual([t.id for t in planner.toposort([c, a])], ["C", "A"])

    def test_cycle_is_plan_error(self):
        a = make_task(id="A", needs=("B",))
        b = make_task(id="B", needs=("A",))
        with self.assertRaises(planner.PlanError) as ctx:
            planner.toposort([a, b])
        self.assertIn("needs cycle", str(ctx.exception))

    def test_shards_cover_every_task_once(self):
        tasks = [make_task(id="T-%02d" % i) for i in range(5)]
        parts = planner.shards(tasks, 4)
        self.assertEqual(len(parts), 4)
        flat = [t.id for part in parts for t in part]
        self.assertEqual(sorted(flat), sorted(t.id for t in tasks))

    def test_mutating_requires_both_profile_and_flag(self):
        tasks = [make_task(id="M-1", mutating=True), make_task(id="L-1")]
        pr = Profile("pr", False, 1, False, False)
        selected, skipped = planner.plan(tasks, pr, False)
        self.assertEqual([t.id for t in selected], ["L-1"])
        self.assertEqual([t.id for t in skipped], ["M-1"])
        rel = Profile("release", True, 1, True, False)
        self.assertEqual(len(planner.plan(tasks, rel, True)[0]), 2)
        self.assertEqual(len(planner.plan(tasks, rel, False)[0]), 1)


class PolicyTests(unittest.TestCase):

    def setUp(self):
        self.pol = policy.load(POLICY)

    def test_profiles_and_roles(self):
        self.assertEqual(sorted(self.pol.profiles), ["local", "nightly", "pr", "release"])
        self.assertEqual(self.pol.profiles["pr"].jobs, 4)
        self.assertFalse(self.pol.profiles["pr"].include_heavy)
        self.assertTrue(self.pol.profiles["nightly"].include_heavy)
        self.assertTrue(self.pol.profiles["release"].allow_mutating)
        self.assertFalse(self.pol.profiles["release"].reuse_ok)
        self.assertIsNone(policy.role_check(self.pol, "ci", "pr"))
        self.assertIn("may not run", policy.role_check(self.pol, "ci", "release"))
        self.assertIn("unknown role", policy.role_check(self.pol, "root", "pr"))

    def test_whitelist_and_tokenizer(self):
        self.assertIsNone(policy.whitelist_check(self.pol, "{python} scripts/x.py"))
        self.assertIsNone(policy.whitelist_check(self.pol, "bash scripts/x.sh"))
        self.assertIn("not whitelisted", policy.whitelist_check(self.pol, "powershell -c x"))
        self.assertIn("cannot tokenize", policy.whitelist_check(self.pol, 'python "oops'))
        self.assertIn("empty cmd", policy.whitelist_check(self.pol, "   "))

    def test_timeouts_and_retry_budget(self):
        heavy = [t for t in manifest.load(MANIFEST) if t.kind == "heavy"][0]
        self.assertEqual(heavy.timeout_s, 1800)
        self.assertEqual(policy.timeout_for(make_task(), self.pol), 300)
        self.assertEqual(policy.timeout_for(heavy, self.pol), 1800)
        self.assertEqual(policy.timeout_for(make_task(timeout_s=7), self.pol), 7)
        self.assertEqual(policy.retry_budget(make_task(), self.pol), 1)
        self.assertEqual(policy.retry_budget(make_task(retry=0), self.pol), 0)
        self.assertEqual(policy.retry_budget(make_task(retry=2), self.pol), 2)

    def test_violations_ignore_mutable_prefixes(self):
        before = {"scripts/a.py"}
        after = {"scripts/a.py", "tmp/x.log", "target/debug/y", "scripts/b.py"}
        got = policy.violations(before, after, self.pol.mutable_prefixes)
        self.assertEqual(got, ["scripts/b.py"])


class StoreTests(TempDirTest):

    def test_chain_roundtrip_and_verify(self):
        j = store.Journal(self.tmp, "r1")
        j.append("run_start", {"profile": "pr"})
        j.attempt(Attempt("T-1", 1, Verdict.OK, 0, 0.5, "rc=0"))
        j.append("run_end", {"counts": {"ok": 1}})
        ok, n, bad, reason = store.verify(j.events)
        self.assertTrue(ok, reason)
        self.assertEqual((n, bad), (3, 0))

    def test_tamper_payload_is_detected(self):
        j = store.Journal(self.tmp, "r2")
        j.append("run_start", {"profile": "pr"})
        doc = json.loads(j.events.read_text(encoding="utf-8").splitlines()[0])
        doc["payload"]["profile"] = "release"
        line = json.dumps(doc, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        j.events.write_text(line + chr(10), encoding="utf-8")
        ok, n, bad, reason = store.verify(j.events)
        self.assertFalse(ok)
        self.assertEqual((n, bad), (0, 1))
        self.assertIn("sha mismatch", reason)

    def test_bad_json_and_missing_journal(self):
        p = self.write("j.jsonl", "{not json}" + chr(10))
        ok, n, bad, reason = store.verify(p)
        self.assertFalse(ok)
        self.assertEqual((n, bad), (0, 1))
        self.assertIn("bad json", reason)
        ok, n, bad, reason = store.verify(self.tmp / "nope.jsonl")
        self.assertFalse(ok)
        self.assertEqual(reason, "journal missing")

    def test_index_rebuild_and_latest_pointer(self):
        for rid in ("r4", "r5"):
            j = store.Journal(self.tmp, rid)
            j.attempt(Attempt("T-1", 1, Verdict.OK, 0, 0.1, "rc=0"))
            j.attempt(Attempt("T-2", 1, Verdict.MISMATCH, 1, 0.2, "rc=1!=0"))
        self.assertEqual(store.rebuild_index(self.tmp), 4)
        store.write_latest(self.tmp, "r5", {"status": "done"})
        self.assertEqual(store.read_latest(self.tmp), "r5")
        (self.tmp / "tmp" / "govd" / "latest.json").unlink()
        self.assertEqual(store.read_latest(self.tmp), "")


class ReportTests(unittest.TestCase):

    def attempts(self):
        return [Attempt("T-1", 1, Verdict.OK, 0, 0.1, "rc=0"),
                Attempt("T-2", 1, Verdict.MISMATCH, 1, 0.2, "rc=1!=0")]

    def test_counts_and_sum_line(self):
        line = report.sum_line(self.attempts(), [make_task(id="T-3"), make_task(id="T-4")], 4)
        self.assertIn("ok=1 mismatch=1", line)
        self.assertIn("skip=2", line)
        self.assertIn("total 4/4", line)

    def test_exit_code_dialect(self):
        one_ok = [Attempt("T-1", 1, Verdict.OK, 0, 0.1, "")]
        self.assertEqual(report.exit_code(one_ok, [], 1), 0)
        self.assertEqual(report.exit_code(self.attempts(), [], 2), 1)
        for verdict in (Verdict.ENV, Verdict.TIMEOUT, Verdict.LAUNCH, Verdict.UNKNOWN):
            row = [Attempt("T-1", 1, verdict, None, 0.1, "")]
            self.assertEqual(report.exit_code(row, [], 1), 2, verdict)
        self.assertEqual(report.exit_code(one_ok, [], 2), 2)

    def test_junit_is_well_formed(self):
        xml = report.to_junit(self.attempts(), [make_task(id="T-3")])
        root = ET.fromstring(xml)
        self.assertEqual(root.tag, "testsuite")
        self.assertEqual(root.get("tests"), "3")
        self.assertEqual(root.get("failures"), "1")
        self.assertEqual(root.get("skipped"), "1")

    def test_json_and_step_summary(self):
        meta = RunMeta("r1", "pr", "2026-10-29T00:00:00Z", "abc1234", 0,
                       "ci", "host", sys.version.split()[0], "d" * 64)
        doc = json.loads(report.to_json(meta, self.attempts(), []))
        self.assertEqual(doc["run"]["run_id"], "r1")
        self.assertEqual(doc["counts"]["ok"], 1)
        summary = report.step_summary(meta, self.attempts(), [], "sha")
        self.assertIn("govd run r1", summary)
        self.assertIn("| mismatch | 1 |", summary)


class ExecutorTests(TempDirTest):

    def test_resolve_python_token(self):
        self.assertEqual(executor.resolve_argv("{python} -c pass"),
                         [sys.executable, "-c", "pass"])

    def test_resolve_bash_is_pinned_to_verified_path(self):
        args = executor.resolve_argv("bash x.sh --flag", bash_path=r"C:\Git\bash.exe")
        self.assertEqual(args[0], r"C:\Git\bash.exe")
        self.assertEqual(args[1:], ["x.sh", "--flag"])

    def test_resolve_rejects_empty_and_missing_bash(self):
        with self.assertRaises(LookupError):
            executor.resolve_argv("bash x.sh")
        with self.assertRaises(ValueError):
            executor.resolve_argv("   ")

    def test_fingerprint_is_16_hex(self):
        fp = executor.env_fingerprint()
        self.assertEqual(len(fp), 16)
        int(fp, 16)

    def test_run_ok(self):
        log = self.tmp / "ok.log"
        att = executor.run_attempt(make_task(), str(log), 60)
        self.assertIs(att.verdict, Verdict.OK, att.reason)
        self.assertEqual(att.rc, 0)

    def test_red_against_expect_one(self):
        task = make_task(expect=1)
        att = executor.run_attempt(task, str(self.tmp / "m.log"), 60)
        self.assertIs(att.verdict, Verdict.MISMATCH)
        self.assertEqual(att.rc, 0)
        self.assertEqual(att.reason, "rc=0!=1")

    def test_hard_budget(self):
        task = make_task(cmd='{python} -c "import time; time.sleep(30)"')
        att = executor.run_attempt(task, str(self.tmp / "t.log"), 1)
        self.assertEqual(att.verdict, Verdict.TIMEOUT)
        self.assertIsNone(att.rc)
        self.assertTrue(att.timed_out)
        self.assertLess(att.duration_s, 20)

    def test_missing_launcher(self):
        task = make_task(cmd="ghostcmd-xyz --help")
        att = executor.run_attempt(task, str(self.tmp / "l.log"), 5)
        self.assertEqual(att.verdict, Verdict.LAUNCH)
        self.assertIsNone(att.rc)

    def test_missing_log_returns_empty(self):
        self.assertEqual(executor.read_log(self.tmp / "nope.log"), "")


class CliTests(TempDirTest):

    def test_list_pr_profile(self):
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = cli.main(["list", "--profile", "pr"])
        out = buf.getvalue()
        self.assertEqual(rc, 0)
        self.assertIn("selected=133 total=140 skipped=7", out)

    def test_list_rejects_role_profile_pair(self):
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = cli.main(["list", "--profile", "release", "--role", "ci"])
        self.assertEqual(rc, 2)
        self.assertIn("UNDECIDABLE", buf.getvalue())

    def test_module_entrypoint(self):
        proc = subprocess.run([sys.executable, "-m", "scripts.govd", "list",
                               "--profile", "pr"], cwd=str(ROOT), capture_output=True,
                              text=True, encoding="utf-8", errors="replace", timeout=120)
        self.assertEqual(proc.returncode, 0, proc.stderr[-2000:])
        self.assertIn("selected=133 total=140 skipped=7", proc.stdout)


class SelectionTests(unittest.TestCase):

    def test_task_profile_restriction(self):
        task = make_task(profiles=("release",))
        self.assertFalse(task.selected_by("pr", True, True))
        self.assertTrue(task.selected_by("release", True, True))

    def test_completed_reads_attempt_events_only(self):
        import tempfile as tf
        base = ROOT / "tmp"
        run = Path(tf.mkdtemp(prefix="govd-test-", dir=str(base)))
        try:
            j = store.Journal(run, "r9")
            j.append("run_start", {})
            j.attempt(Attempt("T-1", 1, Verdict.OK, 0, 0.1, ""))
            self.assertEqual(cli._completed(j.events), {"T-1"})
            self.assertEqual(cli._completed(run / "missing.jsonl"), set())
        finally:
            for f in sorted(run.rglob("*"), key=lambda x: len(str(x)), reverse=True):
                if f.is_file():
                    f.unlink()
                elif not any(f.iterdir()):
                    f.rmdir()
            run.rmdir()

class ReuseTests(TempDirTest):
    """--reuse-ok keys on (manifest digest, task id, cmd digest, env fingerprint)."""

    def write_report(self, run_id, digest, env, attempts):
        run = self.tmp / "tmp" / "govd" / "runs" / run_id
        run.mkdir(parents=True)
        (run / "report.json").write_text(json.dumps({
            "run": {"manifest_digest": digest, "env_fingerprint": env},
            "attempts": attempts}), encoding="utf-8")
        return run

    def test_cmd_digest_tracks_the_command_not_the_id(self):
        before = make_task(id="T-1", cmd="{python} scripts/x.py")
        after = make_task(id="T-1", cmd="{python} scripts/x.py --new-predicate")
        self.assertNotEqual(before.cmd_digest(), after.cmd_digest())

    def test_reuse_requires_matching_digest_and_env(self):
        task = make_task(id="T-1")
        self.write_report("r1", "d" * 64, "env-a",
                          [{"task_id": "T-1", "verdict": "ok",
                            "cmd_digest": task.cmd_digest()}])
        self.assertEqual(store.reusable_ok_runs(self.tmp, "d" * 64, "env-a"),
                         {"T-1": ("r1", task.cmd_digest())})
        self.assertEqual(store.reusable_ok_runs(self.tmp, "e" * 64, "env-a"), {})
        self.assertEqual(store.reusable_ok_runs(self.tmp, "d" * 64, "env-b"), {})
        self.assertEqual(store.reusable_ok_runs(self.tmp, "d" * 64, "env-a",
                                                exclude_run="r1"), {})

    def test_reuse_ignores_reused_sources_and_unreadable_reports(self):
        task = make_task(id="T-1")
        self.write_report("r1", "d" * 64, "env-a", [
            {"task_id": "T-1", "verdict": "ok",
             "cmd_digest": task.cmd_digest(), "reused": True},
            {"task_id": "T-2", "verdict": "ok"},
            {"task_id": "T-3", "verdict": "mismatch", "cmd_digest": "f" * 64},
        ])
        self.assertEqual(store.reusable_ok_runs(self.tmp, "d" * 64, "env-a"), {})
        broken = self.write_report("r2", "d" * 64, "env-a", [])
        (broken / "report.json").write_text("{not json", encoding="utf-8")
        self.assertEqual(store.reusable_ok_runs(self.tmp, "d" * 64, "env-a"), {})

    def test_release_profile_refuses_reuse(self):
        self.assertIn("release", cli._reuse_refusal("release", True))
        self.assertEqual(cli._reuse_refusal("pr", True), "")
        self.assertEqual(cli._reuse_refusal("release", False), "")

    def test_report_marks_reused_separately(self):
        att = Attempt("T-1", 0, Verdict.OK, 0, 0.0, "reused-ok from r1",
                      cmd_digest="a" * 64, reused=True, reused_from="r1")
        self.assertIn("ok=1", report.sum_line([att], [], 1))
        self.assertIn("reused=1(not executed)", report.sum_line([att], [], 1))
        self.assertIn("reused", report.counts([att], []))
        xml = report.to_junit([att], [])
        self.assertIn('skipped message="reused-ok from r1"', xml)
        self.assertEqual(ET.fromstring(xml).get("skipped"), "1")


class ReportCliTests(TempDirTest):
    """`govd report --format` must really surface the artefacts it advertises."""

    def make_run(self, rid="r1"):
        run = self.tmp / "tmp" / "govd" / "runs" / rid
        store.Journal(self.tmp, rid).append("run_start", {"run_id": rid})   # run/ + logs/ + events.jsonl
        (run / "report.json").write_text(json.dumps(
            {"run": {"run_id": rid}, "attempts": [], "skipped": [], "counts": {}}),
            encoding="utf-8")
        (run / "junit.xml").write_text("<testsuite/>", encoding="utf-8")
        (run / "summary.md").write_text("summary-for-" + rid + chr(10), encoding="utf-8")
        return run

    def run_report(self, argv, fmt="text"):
        args = cli.build_parser().parse_args(argv + ["--format", fmt])
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = cli.cmd_report(args, root=self.tmp)
        return rc, buf.getvalue()

    def test_report_formats_pick_their_own_artefact(self):
        run = self.make_run("r1")
        rc, out = self.run_report(["report", "r1"], "text")
        self.assertEqual(rc, 0)
        self.assertIn("summary-for-r1", out)
        rc, out = self.run_report(["report", "r1"], "json")
        self.assertEqual(rc, 0)
        self.assertEqual(json.loads(out)["run"]["run_id"], "r1")
        rc, out = self.run_report(["report", "r1"], "junit")
        self.assertEqual(rc, 0)
        self.assertIn("<testsuite/>", out)
        rc, out = self.run_report(["report", "--run-id", "r1"], "all")
        self.assertEqual(rc, 0)
        self.assertIn("summary-for-r1", out)
        self.assertIn("<testsuite/>", out)

    def test_report_refuses_missing_journal_and_missing_artefact(self):
        self.make_run("r1")
        rc, out = self.run_report(["report", "nope"], "text")
        self.assertEqual(rc, 2)
        self.assertIn("UNDECIDABLE", out)
        (self.tmp / "tmp" / "govd" / "runs" / "r1" / "report.json").unlink()
        rc, out = self.run_report(["report", "r1"], "json")
        self.assertEqual(rc, 2)
        self.assertIn("missing artefact", out)

    def test_report_subparser_rejects_unknown_format(self):
        buf, err = io.StringIO(), io.StringIO()
        with self.assertRaises(SystemExit) as cm, redirect_stdout(buf), redirect_stderr(err):
            cli.build_parser().parse_args(["report", "r1", "--format", "yaml"])
        self.assertEqual(cm.exception.code, 2)
        self.assertIn("invalid choice", err.getvalue())


class ShardTests(unittest.TestCase):
    """--shard k/n: deterministic partitioning, or a refusal -- never a silent subset."""

    def tasks(self, n, needs=()):
        return [make_task(id="T-%d" % (i + 1), needs=needs if i == 0 else ())
                for i in range(n)]

    def test_parse_shard_accepts_only_valid_k_over_n(self):
        self.assertIsNone(planner.parse_shard(""))
        self.assertEqual(planner.parse_shard("2/4"), (2, 4))
        for bad in ("5/4", "0/4", "abc", "2/0", "1"):
            with self.assertRaises(planner.PlanError, msg=bad):
                planner.parse_shard(bad)

    def test_shards_partition_without_overlap_and_cover_everything(self):
        tasks = self.tasks(10)
        parts = planner.shards(tasks, 3)
        self.assertEqual([len(p) for p in parts], [4, 3, 3])
        ids = [t.id for p in parts for t in p]
        self.assertEqual(sorted(ids), sorted(t.id for t in tasks))
        self.assertEqual(len(set(ids)), len(ids))

    def test_apply_shard_refuses_plans_that_declare_needs(self):
        with self.assertRaises(planner.PlanError):
            planner.apply_shard(self.tasks(3, needs=("T-9",)), "1/2")

    def test_apply_shard_reports_an_empty_shard_instead_of_hiding_it(self):
        selected, note = planner.apply_shard(self.tasks(2), "3/5")
        self.assertEqual(selected, [])
        self.assertEqual(note, "3/5")
        whole, noteshard = planner.apply_shard(self.tasks(2), "")
        self.assertEqual(len(whole), 2)
        self.assertEqual(noteshard, "")

    def test_shard_flag_is_parsed_by_cli(self):
        args = cli.build_parser().parse_args(
            ["list", "--profile", "pr", "--shard", "1/4"])
        self.assertEqual(args.shard, "1/4")


class ResumeLedgerTests(TempDirTest):
    """A resumed run must carry the prior verdicts, not just the prior ids."""

    def test_prior_verdicts_reads_verdict_not_just_id(self):
        journal = store.Journal(self.tmp, "r9")
        journal.attempt(Attempt("T-1", 1, Verdict.OK, 0, 0.1, "rc=0"))
        journal.attempt(Attempt("T-2", 1, Verdict.MISMATCH, 1, 0.1, "rc=1!=0"))
        self.assertEqual(cli._prior_verdicts(journal.events),
                         {"T-1": "ok", "T-2": "mismatch"})
        self.assertEqual(cli._prior_verdicts(self.tmp / "missing.jsonl"), {})

    def test_resume_carried_verdicts_keep_the_run_red(self):
        journal = store.Journal(self.tmp, "r10")
        journal.attempt(Attempt("T-1", 1, Verdict.MISMATCH, 1, 0.1, "rc=1!=0"))
        carried = Attempt("T-1", 0, Verdict(cli._prior_verdicts(journal.events)["T-1"]),
                         None, 0.0, "resumed from journal")
        self.assertEqual(report.exit_code([carried], [], 1), 1)

    def test_resume_subparser_defers_profile_to_the_journal(self):
        args = cli.build_parser().parse_args(["resume", "r1"])
        self.assertEqual(args.cmd, "resume")
        self.assertEqual(args.resume_id, "r1")
        self.assertEqual(args.profile, "")   # "" = read it back from report.json


class TaskFamilySelectionTests(unittest.TestCase):
    """Which lane sees which task family (ADR-228 §3.2)."""

    def ids(self, profile):
        tasks, _policy = cli.load_all(profile)
        return {t.id for t in tasks if t.source != "gate"}

    def test_pr_lane_sees_no_task_families(self):
        pr_ids = self.ids("pr")
        self.assertEqual(pr_ids, set(), "pr must stay 133 light + 7 heavy gates only")

    def test_nightly_gets_external_rows_only(self):
        nightly = self.ids("nightly")
        self.assertTrue(nightly, "E-01..E-03 must be scheduled by nightly")
        self.assertTrue(all(i.startswith("E-") for i in nightly), sorted(nightly))
        self.assertFalse(any(i.startswith("R-") for i in nightly),
                         "rust rows stay release-only (they shell out to cargo)")

    def test_release_gets_every_family(self):
        release = self.ids("release")
        self.assertTrue(any(i.startswith("R-") for i in release), sorted(release))
        self.assertTrue(any(i.startswith("E-") for i in release), sorted(release))


class CancellationTests(TempDirTest):
    """Spec §3.4: Ctrl-C cancels honestly, and the journal stays resumable.

    Three claims, none of which the unit-level tests covered: the run must exit 2
    (never "all green"), finished attempts must already be on disk (not buffered
    until the end), and `--resume` of that same run must carry the earlier verdict
    and execute only what never ran.
    """

    def setUp(self):
        super().setUp()
        self.tasks = (make_task(id="T-1"), make_task(id="T-2"))
        self.policy = policy.load(str(POLICY))

    def run_args(self, extra=()):
        return cli.build_parser().parse_args(["run", "--profile", "local", *extra])

    def events(self, rid):
        path = self.tmp / "tmp" / "govd" / "runs" / rid / "events.jsonl"
        return [json.loads(ln) for ln in path.read_text(encoding="utf-8").splitlines()]

    def _interrupt_one_of_two(self):
        """Run T-1 to completion, interrupt on T-2; return the run id."""
        ok = Attempt("T-1", 1, Verdict.OK, 0, 0.05, "rc=0")
        with mock.patch.object(cli, "ROOT", self.tmp), \
             mock.patch.object(cli, "load_all", lambda profile="": (self.tasks, self.policy)), \
             mock.patch.object(policy, "snapshot", lambda root: set()), \
             mock.patch.object(cli, "_execute_one",
                               side_effect=[ok, KeyboardInterrupt()]):
            buf = io.StringIO()
            with redirect_stdout(buf):
                rc = cli.cmd_run(self.run_args())
        return rc, buf.getvalue()

    def test_interrupt_keeps_journal_and_exits_undecidable(self):
        rc, out = self._interrupt_one_of_two()
        self.assertEqual(rc, 2, out)
        self.assertIn("CANCELLED", out)
        rid = store.read_latest(self.tmp)
        kinds = [e["kind"] for e in self.events(rid)]
        self.assertIn("cancelled", kinds)
        # run_end still lands so the ledger is closed, but the cancelled event is
        # what proves which gates genuinely ran.
        self.assertIn("run_end", kinds)
        cancelled = [e for e in self.events(rid) if e["kind"] == "cancelled"][0]
        self.assertEqual(cancelled["payload"]["done"], ["T-1"])
        report = json.loads((self.tmp / "tmp" / "govd" / "runs" / rid / "report.json")
                            .read_text(encoding="utf-8"))
        self.assertEqual([a["task_id"] for a in report["attempts"]], ["T-1"])
        return rid

    def test_resume_of_a_cancelled_run_only_fills_the_gap(self):
        rid = self.test_interrupt_keeps_journal_and_exits_undecidable()
        ok2 = Attempt("T-2", 1, Verdict.OK, 0, 0.05, "rc=0")
        with mock.patch.object(cli, "ROOT", self.tmp), \
             mock.patch.object(cli, "load_all", lambda profile="": (self.tasks, self.policy)), \
             mock.patch.object(policy, "snapshot", lambda root: set()), \
             mock.patch.object(cli, "_execute_one", return_value=ok2):
            buf = io.StringIO()
            with redirect_stdout(buf):
                rc = cli.cmd_run(self.run_args(["--resume", rid]))
        out = buf.getvalue()
        self.assertEqual(rc, 0, out)
        self.assertIn("T-1 ok (resumed, not re-run)", out)
        # One attempt event for T-1 in the first run, one for T-2 in the second:
        # the resume must not re-execute the finished gate.
        self.assertEqual(len([e for e in self.events(rid)
                              if e["kind"] == "attempt"]), 2)


if __name__ == "__main__":
    # F30 dialect: an uncaught crash must exit 2, never borrow the interpreter's 1
    # (which would read as "ran and judged red" to run_gate_manifest).
    scripts_dir = str(ROOT / "scripts")
    if scripts_dir not in sys.path:
        sys.path.insert(0, scripts_dir)
    import gate_rc
    sys.exit(gate_rc.run(lambda: unittest.main(verbosity=2)))
