#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""COE 观察期机器检查 —— 把 slo.yml / perf_redline.yml 头注里的观察期切换条件
从散文变成每日可执行判定 (W6b, 2026-11-10)。

WHY 存在这个文件:
  slo.yml 头注(:19-28)与 perf_redline.yml 头注写明了观察期切阻塞的三个条件,
  但此前**没有任何机器检查** —— 这是 P10 病灶「永远观察」: 条件早已满足也会
  无人发现, `continue-on-error: true` 可以无限期陪跑, 观察期变成事实上的
  永久豁免。本门把三个观察面做成三态判定, 漏翻转(条件满足而 workflow 未改)
  才是红。

观察面与判定参数(真值源是两个 workflow 的头注, 改条件先改头注再同步这里):
  面 1  slo.yml 条件① : 自 2026-08-31 起满 14 天 且 nightly 采样 >= 20 次
  面 2  slo.yml 条件② : A1 集连续 10 次全绿 且 retry 命中 = 0 (二者先到, 即 ①② 取或)
  面 3  perf_redline.yml : 连续 2 晚全绿且无 SKIP(rc=2 不算绿)
  附加  任一 workflow retry 命中率 > 10% -> 按 RK-P28 建议退回 manual-only
        (建议只写结论不改变退出码; 不得靠放宽阈值消掉 retry)

数据口径与已知近似(诚实登记, 不是隐藏局限):
  - 数据源 = `gh run list --workflow <file> --limit 40
    --json conclusion,createdAt,attempt` 的 workflow 运行级结论;
  - "retry 命中"以 attempt > 1 计(workflow 级重跑); job 级 rerun 的 attempt
    同样递增, 但 GitHub 不单独暴露, 故这是本数据源能做到的最近近似;
  - "nightly 采样"以锚点后可见 run 总数为代理(nightly cron 占绝对多数,
    tag/dispatch 触发是额外样本而非缺漏); --limit 40 截断窗口,
    锚点早于窗口时读数为 40, 对 ">= 20" 判据只会更宽松不会更严;
  - 全绿只认 conclusion == "success"; skipped/failure/timed_out 一律截断连绿;
  - SKIP 的 workflow 级代理 = conclusion == "skipped" (头注原文的 rc=2 是
    check_perf_redlines.py 的 job 级语义, workflow 结论里不可见)。

退出码方言(与 gate_rc 全仓门一致):
  0 = 判定完成(含"观察期继续"与"条件满足且已翻转"两种绿灯)
  1 = 漏翻转红: 至少一个观察面条件已满足, 对应 workflow 的观察 job 仍是
      continue-on-error: true
  2 = 不可判定(判据没真正跑完): gh 缺席 / 网络或 gh 报错 / 返回 JSON 缺字段 /
      workflow 文件里找不到观察 job 的 continue-on-error 行

用法:
  python scripts/check_coe_observability.py            # 判定(默认)
  python scripts/check_coe_observability.py --dry-run  # 只打印将判结论, 永不因此红
  python scripts/check_coe_observability.py --limit 60 # 扩大采样窗口
  python scripts/check_coe_observability.py --selftest # 夹具证牙

旗标约定与 F29 一致: 未知旗标由 argparse 拒绝并退 2, 不静默降级。
崩溃守卫在 scripts/gate_rc.py (F30: 未捕获异常退 2 而非 1)。
判据行全部 ASCII; 散文行中文(gate_rc 已把 stdio 钉为 UTF-8)。
"""
import argparse
import contextlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import date, datetime, timedelta, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))
SLO_FILE = os.path.join(ROOT, ".github", "workflows", "slo.yml")
PERF_FILE = os.path.join(ROOT, ".github", "workflows", "perf_redline.yml")

# ---- 观察面参数(照抄头注, 见 docstring) ------------------------------------
SLO_JOB = "slo-release-gate"
PERF_JOB = "perf-contract-exec"
SLO_ANCHOR = "2026-08-31"          # slo.yml 头注条件①的锚点
SLO_MIN_DAYS = 14                  # 条件①: 满 14 天
SLO_MIN_SAMPLES = 20               # 条件①: nightly 采样 >= 20
SLO_STREAK = 10                    # 条件②: A1 集连续 10 次全绿
PERF_STREAK = 2                    # perf_redline: 连续 2 晚全绿无 SKIP
RETRY_WARN_RATE = 0.10             # RK-P28: retry 命中率 > 10% 建议 manual-only
FETCH_LIMIT = 40                   # gh 采样窗口(规格给定值)
GH_TIMEOUT_S = 30                  # gh 子进程超时(network hang -> 不可判定)

# selftest 可置 None 模拟 gh 缺席; 真跑时由 main() 填 shutil.which("gh")。
_GH_PATH = None


class Undecidable(Exception):
    """判据没真正跑完(数据/环境/解析失败), 调用方必须退 2 而不是猜。"""


def gh_available():
    return _GH_PATH is not None


def fetch_runs(workflow_file, limit):
    """调 gh 取 workflow 运行列表(最新在前); 任何失败抛 Undecidable。

    argv 形状是规格钉定的契约(--workflow <file> --limit N
    --json conclusion,createdAt,attempt), selftest 会对它做逐 token 断言。
    """
    argv = ["gh", "run", "list", "--workflow", os.path.basename(workflow_file),
            "--limit", str(limit), "--json", "conclusion,createdAt,attempt"]
    try:
        proc = subprocess.run(argv, capture_output=True, text=True,
                              timeout=GH_TIMEOUT_S)
    except FileNotFoundError as exc:                       # gh 二进制缺失
        raise Undecidable("gh executable vanished: %s" % exc)
    except subprocess.TimeoutExpired:
        raise Undecidable("gh run list timed out after %ds" % GH_TIMEOUT_S)
    if proc.returncode != 0:
        raise Undecidable("gh exited %d: %s"
                          % (proc.returncode, proc.stderr.strip()[:300]))
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise Undecidable("gh returned non-JSON: %s" % exc)
    if not isinstance(data, list):
        raise Undecidable("gh JSON is not a list")
    for row in data:
        if not isinstance(row, dict):
            raise Undecidable("gh row is not an object: %r" % row)
        for key in ("conclusion", "createdAt", "attempt"):
            if key not in row:
                raise Undecidable("gh row missing %r: %r" % (key, row))
    return data


def analyze(runs):
    """对 gh 返回(最新在前)做连绿/retry 统计。

    连绿从最新一条往回数, 遇到第一个非 success 即断 —— skipped 与 failure
    同权截断, 这正是头注「全绿且无 SKIP」的机器形。
    """
    n = len(runs)
    retry_runs = sum(1 for r in runs if int(r["attempt"]) > 1)
    streak = 0
    streak_retry = 0
    for r in runs:
        if r["conclusion"] != "success":
            break
        streak += 1
        if int(r["attempt"]) > 1:
            streak_retry += 1
    return {
        "n": n,
        "samples_since_anchor": sum(
            1 for r in runs if str(r["createdAt"])[:10] >= SLO_ANCHOR),
        "retry_runs": retry_runs,
        "retry_rate": (retry_runs / n) if n else 0.0,
        "streak": streak,
        "streak_retry": streak_retry,
    }


def slo_faces(stat):
    """slo.yml 的两个观察面 -> [(面名, 是否到期)]。"""
    days = (datetime.now(timezone.utc).date()
            - date.fromisoformat(SLO_ANCHOR)).days
    cond1 = days >= SLO_MIN_DAYS and stat["samples_since_anchor"] >= SLO_MIN_SAMPLES
    cond2 = stat["streak"] >= SLO_STREAK and stat["streak_retry"] == 0
    return days, [("cond1(days>=14 && samples>=20)", cond1),
                  ("cond2(streak>=10 && retry==0)", cond2)]


def perf_faces(stat):
    """perf_redline.yml 的一个观察面。"""
    return [("cond(streak>=2 no-skip)", stat["streak"] >= PERF_STREAK)]


_COE = re.compile(r"continue-on-error:\s*(\S+)")


def _job_block(job, text):
    return re.compile(r"(?ms)^  %s:\n(.*?)(?=^  \S|\Z)" % re.escape(job)).search(text)


def observing(path, job):
    """True = 该 job 仍是 continue-on-error: true(观察期未翻转)。

    找不到 job 块或块内没有 continue-on-error 行 -> Undecidable:
    翻转状态判不出来时宁可退 2, 也不把「漏翻转」误记为「已翻转」。
    """
    if not os.path.exists(path):
        raise Undecidable("workflow file missing: %s" % path)
    with open(path, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    m = _job_block(job, text)
    if not m:
        raise Undecidable("job %r not found in %s" % (job, os.path.basename(path)))
    coe = _COE.search(m.group(1))
    if not coe:
        raise Undecidable("job %r has no continue-on-error line in %s"
                          % (job, os.path.basename(path)))
    return coe.group(1) == "true"


def run_check(dry_run, limit, slo_file, perf_file):
    """判定主体: 打印结构化结论, 返回三态退出码。selftest 直接调本函数。"""
    if not gh_available():
        print("[UNDECIDABLE] gh CLI not found on PATH "
              "(install https://cli.github.com or fix PATH)")
        return 2
    try:
        slo_runs = fetch_runs(slo_file, limit)
        perf_runs = fetch_runs(perf_file, limit)
    except Undecidable as exc:
        print("[UNDECIDABLE] data source failed: %s" % exc)
        return 2
    try:
        slo_obs = observing(slo_file, SLO_JOB)
        perf_obs = observing(perf_file, PERF_JOB)
    except Undecidable as exc:
        print("[UNDECIDABLE] %s" % exc)
        return 2

    print("[INFO] COE observability check (W6b) data=gh run list limit=%d" % limit)
    slo_stat = analyze(slo_runs)
    days, faces = slo_faces(slo_stat)
    print("[slo] fetched=%d samples_since_%s=%d days_elapsed=%d "
          "streak=%d retry_in_streak=%d retry_rate=%.1f%%(%d/%d)"
          % (slo_stat["n"], SLO_ANCHOR, slo_stat["samples_since_anchor"],
             days, slo_stat["streak"], slo_stat["streak_retry"],
             slo_stat["retry_rate"] * 100, slo_stat["retry_runs"], slo_stat["n"]))
    for name, due in faces:
        print("[slo] %s: %s" % (name, "MET" if due else "not-met"))
    slo_due = any(due for _, due in faces)
    perf_stat = analyze(perf_runs)
    pfaces = perf_faces(perf_stat)
    print("[perf] fetched=%d streak=%d retry_rate=%.1f%%(%d/%d)"
          % (perf_stat["n"], perf_stat["streak"],
             perf_stat["retry_rate"] * 100, perf_stat["retry_runs"], perf_stat["n"]))
    for name, due in pfaces:
        print("[perf] %s: %s" % (name, "MET" if due else "not-met"))
    perf_due = any(due for _, due in pfaces)

    for tag, stat in (("slo", slo_stat), ("perf", perf_stat)):
        if stat["retry_rate"] > RETRY_WARN_RATE:
            print("[WARN] %s retry_rate=%.1f%% (>10%%): RK-P28 建议退回 "
                  "manual-only 并在登记表写理由(不得放宽阈值)" % (tag, stat["retry_rate"] * 100))

    print("[flip] slo.yml %s continue-on-error=%s"
          % (SLO_JOB, "true(observing)" if slo_obs else "false(blocking)"))
    print("[flip] perf_redline.yml %s continue-on-error=%s"
          % (PERF_JOB, "true(observing)" if perf_obs else "false(blocking)"))

    missed = []
    if slo_due and slo_obs:
        missed.append("slo.yml A1(%s)" % SLO_JOB)
    if perf_due and perf_obs:
        missed.append("perf_redline.yml(%s)" % PERF_JOB)
    if dry_run:
        print("[DRY-RUN] slo_due=%s perf_due=%s missed_flips=%d (dry-run 不判, 恒退 0)"
              % (slo_due, perf_due, len(missed)))
        return 0
    if missed:
        for face in missed:
            print("[FAIL] MISSED_FLIP: %s 观察面条件已满足但 continue-on-error "
                  "仍为 true —— 按头注切 false 并落档(RK-P 台账 / ADR-278)" % face)
        print("RESULT: FAIL (missed_flips=%d)" % len(missed))
        return 1
    state = "观察期继续" if (slo_obs or perf_obs) else "已全部阻塞态"
    print("[OK] 无漏翻转 (%s)" % state)
    print("RESULT: PASS")
    return 0


# ---------------------------------------------------------------------------
# --selftest 夹具
# ---------------------------------------------------------------------------
def _canned(n_total, n_days_back=0, red_at=None, skip_at=None, retry_every=0):
    """造 gh 形 JSON: 最新在前。red_at/skip_at 是给定下标(0=最新)的截断物。

    retry_every=k>0 时每第 k 条 attempt=2(workflow 级重跑), 其余 1。
    """
    base = datetime.now(timezone.utc)
    rows = []
    for i in range(n_total):
        conclusion = "success"
        if red_at is not None and i == red_at:
            conclusion = "failure"
        if skip_at is not None and i == skip_at:
            conclusion = "skipped"
        attempt = 2 if (retry_every and i % retry_every == 0) else 1
        rows.append({
            "conclusion": conclusion,
            "createdAt": (base - timedelta(days=n_days_back + i))
            .strftime("%Y-%m-%dT%H:%M:%SZ"),
            "attempt": attempt,
        })
    return rows


def _write_wf(path, job, coe):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("jobs:\n  %s:\n    name: fixture\n    continue-on-error: %s\n"
                 % (job, "true" if coe else "false"))


def selftest():
    """四个规格夹具 + 两个反向牙齿, 全部进程内 monkeypatch(等价于假 gh 注入
    PATH, 且免 Windows PATHEXT/.exe 麻烦); 网络失败路径必须走真 subprocess
    异常翻译, 不能只测假函数。
    """
    import gate_rc

    # fetch_runs/_GH_PATH 的夹具替换必须改模块全局(run_check 在调用时解析全局名),
    # 漏写 global 会造出同名局部变量, 夹具静默失效。
    global fetch_runs, _GH_PATH

    checks = []

    def record(name, ok):
        checks.append((name, ok))
        print("  [%s] %s" % ("PASS" if ok else "FAIL", name))

    def run_silent(**kw):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = run_check(kw.get("dry_run", False), kw.get("limit", FETCH_LIMIT),
                           kw.get("slo_file", SLO_FILE),
                           kw.get("perf_file", PERF_FILE))
        return rc, buf.getvalue()

    saved_fetch = fetch_runs
    saved_gh = _GH_PATH
    tmp = gate_rc.tmpdir(prefix="coe-observe-")
    try:
        _GH_PATH = "/fake/gh-for-selftest"

        # 夹具 1(规格): 连续 10 绿 + 已翻转文件 -> 不红(退 0)。
        green_slo = _canned(40)
        green_perf = _canned(6)
        slo_flip = os.path.join(tmp, "slo-flip.yml")
        perf_flip = os.path.join(tmp, "perf-flip.yml")
        _write_wf(slo_flip, SLO_JOB, coe=False)
        _write_wf(perf_flip, PERF_JOB, coe=False)
        fetch_runs = lambda wf, limit: (green_slo, green_perf)[
            0 if "slo" in os.path.basename(wf) else 1]
        rc, out = run_silent(slo_file=slo_flip, perf_file=perf_flip)
        record("规格1 连续10绿+已翻转 -> rc=0(不红)", rc == 0)
        record("规格1 输出含结论行", "RESULT: PASS" in out)

        # 夹具 2(规格): 伪造 1 红截断连绿 + 采样压到 20 以下(否则条件①
        # 独立成立, 断言会通过得无牙) + perf 顶部 SKIP -> 全不到期,
        # 即便用真实未翻转文件也不红。
        red_slo = _canned(12, red_at=0)
        skip_perf = _canned(5, skip_at=0)
        fetch_runs = lambda wf, limit: (red_slo, skip_perf)[
            0 if "slo" in os.path.basename(wf) else 1]
        rc, out = run_silent()
        record("规格2 伪造1红+SKIP -> 阻断到期, rc=0", rc == 0)
        record("规格2 1红确实截断连绿(streak=0)", "[slo] fetched=12" in out
               and "streak=0 " in out)

        # 夹具 3(规格): retry 命中率 25% (>10%) -> RK-P28 manual-only 建议,
        # 建议不抬退出码(其余面刻意不成立: 采样 12<20, 连绿 12<... 顶条 attempt=2
        # 也使 streak_retry>0 灭掉条件②; perf 用已翻转文件吸收到期)。
        retry_slo = _canned(12, retry_every=4)   # i=0,4,8 共 3 条 attempt=2
        fetch_runs = lambda wf, limit: (retry_slo, green_perf)[
            0 if "slo" in os.path.basename(wf) else 1]
        rc, out = run_silent(slo_file=slo_flip, perf_file=perf_flip)
        record("规格3 retry>10% -> 建议 manual-only 且仍 rc=0",
               rc == 0 and "manual-only" in out)
        record("规格3 retry_rate 读数=25.0%", "retry_rate=25.0%" in out)

        # 夹具 4(规格): gh 缺席 -> rc=2(不可判定, 不是通过)。
        _GH_PATH = None
        rc, out = run_silent(slo_file=slo_flip, perf_file=perf_flip)
        record("规格4 gh缺席 -> rc=2", rc == 2 and "[UNDECIDABLE]" in out)
        _GH_PATH = "/fake/gh-for-selftest"

        # 反向牙齿 A: 与夹具 1 同数据但文件未翻转 -> 漏翻转必红(退 1)。
        slo_obs_f = os.path.join(tmp, "slo-obs.yml")
        perf_obs_f = os.path.join(tmp, "perf-obs.yml")
        _write_wf(slo_obs_f, SLO_JOB, coe=True)
        _write_wf(perf_obs_f, PERF_JOB, coe=True)
        fetch_runs = lambda wf, limit: (green_slo, green_perf)[
            0 if "slo" in os.path.basename(wf) else 1]
        rc, out = run_silent(slo_file=slo_obs_f, perf_file=perf_obs_f)
        record("牙齿A 条件满足+未翻转 -> rc=1(MISSED_FLIP)",
               rc == 1 and "MISSED_FLIP" in out)

        # 反向牙齿 B: --dry-run 即使漏翻转也退 0。
        rc, out = run_silent(dry_run=True, slo_file=slo_obs_f, perf_file=perf_obs_f)
        record("牙齿B dry-run 漏翻转仍 rc=0 且注明", rc == 0 and "[DRY-RUN]" in out)

        # 反向牙齿 C: 网络失败(真 subprocess 异常) -> rc=2。
        real_run = subprocess.run
        def _boom(*a, **kw):
            raise subprocess.TimeoutExpired(cmd=a[0], timeout=GH_TIMEOUT_S)
        subprocess.run = _boom
        fetch_runs = saved_fetch
        try:
            rc, out = run_silent(slo_file=slo_flip, perf_file=perf_flip)
        finally:
            subprocess.run = real_run
        record("牙齿C gh 超时 -> rc=2", rc == 2 and "[UNDECIDABLE]" in out)

        # 反向牙齿 D: argv 契约 + JSON 解析走真 fetch_runs(录屏 subprocess.run)。
        captured = {}
        def _recorder(argv, **kw):
            captured["argv"] = argv
            payload = (green_slo, green_perf)[
                0 if "slo" in os.path.basename(argv[argv.index("--workflow") + 1]) else 1]
            return subprocess.CompletedProcess(argv, 0, json.dumps(payload), "")
        subprocess.run = _recorder
        try:
            rows = saved_fetch("slo.yml", 40)
        finally:
            subprocess.run = real_run
        want_argv = ["gh", "run", "list", "--workflow", "slo.yml", "--limit", "40",
                     "--json", "conclusion,createdAt,attempt"]
        record("牙齿D gh argv 契约逐 token 相符", captured.get("argv") == want_argv)
        record("牙齿D 解析行数与首条字段", len(rows) == 40
               and rows[0]["conclusion"] == "success" and "attempt" in rows[0])

        # 反向牙齿 E: 缺 continue-on-error 行的 job 块 -> 不可判定。
        bad_wf = os.path.join(tmp, "bad.yml")
        os.makedirs(os.path.dirname(bad_wf), exist_ok=True)
        with open(bad_wf, "w", encoding="utf-8") as fh:
            fh.write("jobs:\n  %s:\n    name: no-coe-line\n" % SLO_JOB)
        fetch_runs = lambda wf, limit: green_slo if "slo" in os.path.basename(wf) \
            else green_perf
        rc, out = run_silent(slo_file=bad_wf, perf_file=perf_flip)
        record("牙齿E 无 continue-on-error 行 -> rc=2", rc == 2)
    finally:
        fetch_runs = saved_fetch
        _GH_PATH = saved_gh

    bad = [n for n, ok in checks if not ok]
    print("  RESULT: %s (%d assertions)" % ("FAIL" if bad else "PASS", len(checks)))
    if bad:
        for n in bad:
            print("  [FAIL] %s" % n)
        return 1
    print("[SELFTEST] 四规格夹具 + 五反向牙齿全过: 漏翻转才红, 不误判")
    return 0


def main():
    global _GH_PATH
    _GH_PATH = shutil.which("gh")
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true",
                    help="只打印将判结论, 永不因此红")
    ap.add_argument("--limit", type=int, default=FETCH_LIMIT,
                    help="gh run list 采样窗口(默认 40)")
    ap.add_argument("--slo-file", default=SLO_FILE)
    ap.add_argument("--perf-file", default=PERF_FILE)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    return run_check(args.dry_run, args.limit, args.slo_file, args.perf_file)


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
