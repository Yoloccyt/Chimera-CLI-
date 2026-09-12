#!/usr/bin/env python3
"""覆盖率门（WS-B，审计 P0 #1 的落地）。

背景（本文件存在的理由）：
  .github/workflows/coverage.yml 此前跑 cargo-tarpaulin `--fail-under 85` 却在
  job 级 `continue-on-error: true` —— 阈值算了、结论被丢（自述 RK-P45："没有任何人
  知道当前真实覆盖率"）。同时 scripts/coverage_baseline.toml（min/target 棘轮）零消费
  者，与硬编码 85 冲突。本脚本成为**唯一**消费 coverage_baseline.toml 的门，让判定
  真正生效，并把"裸奔模块"（有 crate 但零覆盖记录）从不可知变可裁决。

设计约束（沿用本仓既有门的教训，不重造轮子）：
  * 单一真值源：阈值只存在于 coverage_baseline.toml，workflow 用 `--print-fail-under`
    取数，不再写死数字（否则又是 bench_check.yml heredoc 那笔债）。
  * `--print-fail-under` 必须自拦空/非数字：python 缺位或 toml 键 typo 会打印空串，
    `--fail-under ""` 被 tarpaulin 拒但 `$( )` 展开空在 bash 常被当"无参数"→ 门变无门
    （假绿），与 slo.yml 头注"空值必须先行拦截"同族坑。
  * 逐 crate 归因用**路径前缀**（crates/<name>/），不依赖 tarpaulin 包级字段（跨版本不稳）。
  * 零覆盖记录的 crate 判红不判 100%（最阴的假绿：--exclude-files 误伤或路径漂移）。
  * workspace.members 复用 check_crate_reachability.py 的解析，绝不复制第 5 份成员表。

模式：
    --print-fail-under          # 打印整数 min（供 shell $() 取用）；异常退 2 不伪装成 0
    --validate-config           # 纯静态：两份 toml 自洽 + 棘轮单调（零 cargo/零 JSON，可进 PR/本地）
    --check DIR                 # 读 DIR/*.json（tarpaulin 或 cargo-llvm-cov 输出）→ 聚合 + 逐 crate 裁决
    --selftest                  # fixture 负控：证明门有牙（每条违规必红、干净必绿）

退出码：0 通过 / 1 违规（判红）/ 2 不可判定（缺文件/环境，绝不等于通过）。
输出保持 ASCII（Windows GBK 控制台约定）。
"""
import glob
import json
import os
import subprocess
import sys

try:
    import tomllib
except ImportError:
    print("[FAIL] coverage_gate.py requires python >= 3.11 (tomllib)")
    sys.exit(2)

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:  # noqa: BLE001
    pass

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))
BASELINE_PATH = os.path.join(HERE, "coverage_baseline.toml")
FLOOR_PATH = os.path.join(HERE, "coverage_per_crate_floor.toml")
BASELINE_REL = "scripts/coverage_baseline.toml"

# 复用可达性门的成员解析（BOM 容忍 + 以 Cargo.toml members 为权威），避免层表多源分裂。
sys.path.insert(0, HERE)
from check_crate_reachability import load_toml, workspace_members  # noqa: E402


# --------------------------------------------------------------------- config load
def load_baseline(path=BASELINE_PATH):
    """返回 dict(min, target, buffer_pp, floor_file)；缺文件 → None（不可判定）。"""
    if not os.path.isfile(path):
        return None
    data = load_toml(path)
    cov = data.get("coverage", data)  # 兼容：[coverage] 段或裸顶层键
    return {
        "min": cov.get("min_line_coverage"),
        "target": cov.get("target_line_coverage"),
        "buffer_pp": cov.get("buffer_pp", 5),
        "floor_file": cov.get("floor_file", FLOOR_PATH),
    }


def load_floors(path):
    """返回 (defaults_floor, exempt:set, {crate: floor})；缺文件 → (None, set(), {})。"""
    if not os.path.isfile(path):
        return None, set(), {}
    data = load_toml(path)
    defaults = data.get("defaults", {})
    floor = defaults.get("floor")
    exempt = set(defaults.get("exempt", []))
    explicit = {}
    for row in data.get("crate", []):
        if "name" in row:
            explicit[row["name"]] = row.get("floor")
    return floor, exempt, explicit


# --------------------------------------------------------------------- tarpaulin json
def crate_of(path):
    """路径前缀归因到 crate 名；非 crates/ 下的记为 '(other)'。容忍绝对路径/反斜杠。"""
    norm = str(path).replace("\\", "/")
    if "crates/" in norm:
        tail = norm.split("crates/", 1)[1]
        seg = tail.split("/", 1)[0]
        if seg:
            return seg
    return "(other)"


def parse_record(r):
    """从单条 tarpaulin JSON 记录抽 (path, covered:int, total:int)。缺信息 → (path, 0, 0)。

    tarpaulin 各版本键名漂移：filename/full_path/file；covered/uncovered 或
    total_lines+cover(0..1)。尽量取到整数 covered/total 供加权。
    """
    path = r.get("filename") or r.get("full_path") or r.get("file") or "(unknown)"
    covered = r.get("covered")
    total = None
    if covered is not None:
        uncov = r.get("uncovered", 0)
        total = int(covered) + int(uncov or 0)
        covered = int(covered)
    elif "line_coverrates" in r:  # 某些版本给每行明细
        covered = 0
        total = 0
        for lc in r.get("line_coverrates", []):
            if lc.get("covered") is None and lc.get("uncovered") is None:
                continue
            total += 1
            if lc.get("covered"):
                covered += 1
    elif "total_lines" in r:
        total = int(r["total_lines"])
        pct = r.get("cover")
        covered = int(round(total * float(pct))) if pct is not None else 0
    if total is None:
        total = 0
        covered = 0
    return path, int(covered), int(total)


def parse_llvm_files(obj):
    """cargo-llvm-cov 导出形状 → [(path, covered, total)]。

    schema: {"data":[{"files":[{"filename":...,"summary":{"lines":{"count":N,"covered":C}}}], ...}]}。
    仅取 per-file 行数据（count/covered），聚合/逐 crate 交给上层，与 tarpaulin 口径一致。
    llvm-cov 只对 workspace 成员插桩，第三方依赖（如 pulp）不插桩→不会因
    -Cinstrument-coverage 触发依赖内 const-eval 崩溃（这是改用 llvm-cov 的根因）。
    """
    out = []
    for block in obj.get("data", []):
        for f in block.get("files", []):
            fn = f.get("filename", "(unknown)")
            lines = (f.get("summary") or {}).get("lines") or {}
            out.append((fn, int(lines.get("covered", 0)), int(lines.get("count", 0))))
    return out


def load_records_from_obj(data):
    """统一入口：tarpaulin=list[record]；llvm-cov=dict with 'data' → [(path,cov,total)]."""
    recs = []
    if isinstance(data, list):
        for r in data:
            if isinstance(r, dict):
                recs.append(parse_record(r))
    elif isinstance(data, dict) and "data" in data:
        recs.extend(parse_llvm_files(data))
    return recs


def aggregate(records):
    """records: [(path, covered, total)] → (agg_pct, {crate: (covered, total)}).

    聚合用**按行加权**（sum covered / sum total），不是各文件百分比的算术平均。
    """
    per_crate = {}
    tot_cov = 0
    tot_all = 0
    for path, covered, total in records:
        c = crate_of(path)
        pc, pt = per_crate.get(c, (0, 0))
        per_crate[c] = (pc + covered, pt + total)
        tot_cov += covered
        tot_all += total
    agg = (100.0 * tot_cov / tot_all) if tot_all else 0.0
    return agg, per_crate


# --------------------------------------------------------------------- core checks
def check_coverage(agg, per_crate, baseline, defaults_floor, exempt, explicit, members):
    """V1 ghost / V2 no-data / V3 agg / V5 per-crate。返回 (errors, summary_rows)。"""
    errors = []
    rows = []
    # V1: 显式 floor 行不得引用不存在的 crate（改名/删 crate 后成幽灵行）
    for name in explicit:
        if name not in members and name not in exempt:
            errors.append(f"V1-GHOST: floor row for unknown crate {name!r} (not a workspace member)")
    # V2: 每个非 exempt 成员必须至少 1 条覆盖记录（否则该 crate 零记录被当达标 = 最阴假绿）
    measured_crates = {c for c, (_cv, tl) in per_crate.items() if tl > 0}
    for m in sorted(members):
        if m in exempt:
            continue
        if m not in measured_crates:
            errors.append(f"V2-NODATA: workspace member {m!r} has ZERO coverage records (naked module?)")
    # V3: 聚合行覆盖 >= baseline.min
    mn = baseline.get("min")
    if not isinstance(mn, (int, float)):
        errors.append(f"V3: baseline.min_line_coverage not numeric: {mn!r}")
    elif agg < mn:
        errors.append(f"V3-AGG: aggregate line coverage {agg:.2f}% < min {mn}")
    # V5: 逐 crate 实测 >= 其 floor（显式优先，否则 defaults）
    for c, (cov, tl) in sorted(per_crate.items()):
        if c in exempt or tl == 0:
            continue
        pct = 100.0 * cov / tl
        floor = explicit.get(c, defaults_floor)
        rows.append((c, pct, floor))
        if isinstance(floor, (int, float)) and pct < floor:
            errors.append(f"V5-CRATE: {c} coverage {pct:.2f}% < floor {floor}")
    return errors, rows


def validate_config(baseline, defaults_floor, exempt, explicit, members):
    """--validate-config：静态自洽（零 JSON）。返回 errors。"""
    errors = []
    mn = baseline.get("min") if baseline else None
    tg = baseline.get("target") if baseline else None
    buf = baseline.get("buffer_pp") if baseline else None
    if baseline is None:
        errors.append("CFG: coverage_baseline.toml missing (undeterminable)")
        return errors
    if not isinstance(mn, (int, float)) or isinstance(mn, bool):
        errors.append(f"CFG: min_line_coverage not numeric: {mn!r}")
    if not isinstance(tg, (int, float)) or isinstance(tg, bool):
        errors.append(f"CFG: target_line_coverage not numeric: {tg!r}")
    if isinstance(mn, (int, float)) and isinstance(tg, (int, float)):
        if not (0 <= mn <= 100):
            errors.append(f"CFG: min_line_coverage out of range: {mn}")
        if mn > tg:
            errors.append(f"CFG: min({mn}) > target({tg}) (ratchet cannot exceed goal)")
    if not isinstance(buf, (int, float)) or not (3 <= buf <= 5):
        errors.append(f"CFG: buffer_pp expected 3..5, got {buf!r}")
    if not isinstance(defaults_floor, (int, float)):
        errors.append("CFG: per-crate [defaults].floor missing/non-numeric")
    # V1 ghost（不依赖 JSON 也可查：floor 行 crate ∈ members ∪ exempt）
    for name in explicit:
        if name not in members and name not in exempt:
            errors.append(f"V1-GHOST: floor row for unknown crate {name!r}")
    # 棘轮单调：min 在 git 历史中不得下调（无基线则 INFO 待命，不误红）
    ratchet_err, _ = check_min_ratchet()
    errors += ratchet_err
    return errors


def check_min_ratchet(baseline_rel=BASELINE_REL):
    """用 git 历史断言 min_line_coverage 单调非减。无历史 → ([], [INFO])。"""
    try:
        log = subprocess.run(
            ["git", "-C", ROOT, "log", "--format=%H", "--", baseline_rel],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
        )
    except FileNotFoundError:
        return [], ["R1: git unavailable; min-ratchet self-activates after commit"]
    commits = log.stdout.split() if log.returncode == 0 else []
    if not commits:
        return [], ["R1: no committed baseline for coverage_baseline.toml; ratchet self-activates after first commit"]
    vals = []
    for c in reversed(commits):  # 从旧到新
        show = subprocess.run(
            ["git", "-C", ROOT, "show", f"{c}:{baseline_rel}"],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
        )
        if show.returncode != 0:
            continue
        try:
            data = tomllib.loads(show.stdout)
        except Exception:  # noqa: BLE001
            continue
        cov = data.get("coverage", data)
        v = cov.get("min_line_coverage")
        if isinstance(v, (int, float)):
            vals.append(v)
    if not vals:
        return [], ["R1: no readable historical min; ratchet self-activates after first commit"]
    if vals[-1] != load_baseline()["min"]:
        # 工作区当前值应等于最新提交值或更高（未提交上调待校准，放行但提示）
        if vals[-1] > load_baseline()["min"]:
            return [f"R1-RATCHET-DOWN: min {load_baseline()['min']} < last committed {vals[-1]}"], []
    for a, b in zip(vals, vals[1:]):
        if b < a:
            return [f"R1-RATCHET-DOWN: historical min decreased {a} -> {b}"], []
    return [], [f"R1: min ratchet monotonic over {len(vals)} committed values (last={vals[-1]})"]


# --------------------------------------------------------------------- modes
def mode_print_fail_under():
    b = load_baseline()
    if b is None:
        return 2
    mn = b.get("min")
    if not isinstance(mn, (int, float)) or isinstance(mn, bool) or not (0 <= mn <= 100):
        print("", end="")  # 空输出，让调用方拦（勿打印非数字污染 $()）
        sys.stderr.write(f"[FAIL] baseline min not numeric/in-range: {mn!r}\n")
        return 2
    print(int(mn))
    return 0


def mode_validate_config():
    members = workspace_members(ROOT)
    b = load_baseline()
    df, ex, exp = load_floors(os.path.join(HERE, "coverage_per_crate_floor.toml"))
    errors = validate_config(b, df, ex, exp, members)
    for e in _last_infos:
        print(f"[INFO] {e}")
    if errors:
        print(f"[FAIL] coverage config invalid ({len(errors)}):")
        for e in errors:
            print(f"   - {e}")
        return 1
    print("[OK] coverage config self-consistent (baseline/floor/ratchet)")
    return 0


def mode_check(dir_path):
    if not os.path.isdir(dir_path):
        print(f"[FAIL] coverage dir not found: {dir_path}")
        return 2
    files = sorted(glob.glob(os.path.join(dir_path, "*.json")))
    if not files:
        print(f"[FAIL] no coverage JSON in {dir_path} (tarpaulin produced nothing)")
        return 2
    records = []
    for fp in files:
        try:
            with open(fp, encoding="utf-8", errors="replace") as f:
                data = json.load(f)
        except (json.JSONDecodeError, OSError) as e:
            print(f"[WARN] skip unparsable {os.path.basename(fp)}: {e}")
            continue
        if isinstance(data, list):
            records.extend(load_records_from_obj(data))
        elif isinstance(data, dict):
            records.extend(load_records_from_obj(data))
    if not records:
        print("[FAIL] no usable coverage records (key-name contract broken? print first JSON to summary)")
        return 2
    b = load_baseline()
    if b is None:
        print("[FAIL] baseline missing (undeterminable)")
        return 2
    df, ex, exp = load_floors(b["floor_file"] if os.path.isabs(b["floor_file"]) else os.path.join(ROOT, b["floor_file"]))
    members = workspace_members(ROOT)
    agg, per_crate = aggregate(records)
    errors, rows = check_coverage(agg, per_crate, b, df, ex, exp, members)
    mn = b.get("min")
    # 机器可读摘要 + 上调建议（floor(agg)-buffer），供 step summary 与人工校准
    print(f"AGG={agg:.2f} MIN={mn} CRATES={len([c for c,(_x,t) in per_crate.items() if t>0])}")
    # 落盘 AGG 供 coverage.yml 的趋势步读取（不经管道，避免 tee 掩盖 --check 退出码）
    try:
        with open(os.path.join(dir_path, "agg.env"), "w", encoding="ascii") as f:
            f.write(f"AGG={agg:.2f}\nMIN={mn}\n")
    except OSError:
        pass
    for c, pct, floor in rows:
        flag = "OK " if (not isinstance(floor, (int, float)) or pct >= floor) else "LOW"
        print(f"  [{flag}] {c}: {pct:.2f}% (floor={floor})")
    if isinstance(mn, (int, float)) and agg > mn + 2 * b.get("buffer_pp", 5):
        print(f"[RAISE-CANDIDATE] agg {agg:.2f} > min+2*buffer; consider min = {int((agg // 1) - b.get('buffer_pp', 5))}")
    if errors:
        print(f"[FAIL] coverage gate violations ({len(errors)}):")
        for e in errors:
            print(f"   - {e}")
        return 1
    print("[OK] coverage gate green (aggregate + per-crate floors hold)")
    return 0


# --------------------------------------------------------------------------- selftest
def mode_selftest():
    """证明门有牙齿：合成 members/baseline/floors/records，逐条断言违规必红、干净必绿。"""
    print("=== selftest: coverage gate must have teeth ===")
    fails = []

    def expect(name, cond):
        print(f"  [{'ok' if cond else 'FAIL'}] {name}")
        if not cond:
            fails.append(name)

    members = {"core", "sec", "empty", "cli"}
    baseline = {"min": 50, "target": 85, "buffer_pp": 5}
    defaults = 45
    exempt = {"cli"}
    explicit = {"core": 60, "sec": 65}

    # 手工算 agg：按行加权的聚合覆盖率
    def agg_of(pc):
        tc = sum(c for c, _ in pc.values())
        tt = sum(t for _, t in pc.values())
        return (100.0 * tc / tt) if tt else 0.0

    clean = {"core": (90, 100), "sec": (80, 100), "empty": (70, 100)}
    a = agg_of(clean)
    e, _ = check_coverage(a, clean, baseline, defaults, exempt, explicit, members)
    expect("selftest-1 clean coverage green", e == [])
    # 2) 低于 min → V3 红
    lowmin = {"core": (10, 100), "sec": (5, 100), "empty": (20, 100)}
    e, _ = check_coverage(agg_of(lowmin), lowmin, baseline, defaults, exempt, explicit, members)
    expect("selftest-2 aggregate below min is fatal", any("V3-AGG" in x for x in e))
    # 3) 单 crate 低于 floor 而聚合达标 → V5 红（证明"聚合掩盖单模块裸奔"被抓住）
    hid = {"core": (55, 100), "sec": (80, 100), "empty": (80, 100)}
    e, _ = check_coverage(agg_of(hid), hid, baseline, defaults, exempt, explicit, members)
    expect("selftest-3 crate below floor while agg passes is fatal", any("V5-CRATE" in x for x in e))
    # 4) 成员零覆盖记录 → V2 红（裸奔模块）
    nodata = {"core": (90, 100), "sec": (90, 100)}  # 'empty' 缺席
    e, _ = check_coverage(agg_of(nodata), nodata, baseline, defaults, exempt, explicit, members)
    expect("selftest-4 member with zero records is fatal", any("V2-NODATA" in x for x in e))
    # 5) 幽灵 floor 行（不存在的 crate）→ V1 红
    ghost = {"core": (90, 100), "sec": (90, 100), "empty": (90, 100)}
    e, _ = check_coverage(agg_of(ghost), ghost, baseline, defaults, exempt, {"core": 60, "ghost_crate": 70}, members)
    expect("selftest-5 ghost floor row is fatal", any("V1-GHOST" in x for x in e))
    # 6) min 非数字 → V3 红（不可判定不当通过）
    e, _ = check_coverage(agg_of(clean), clean, {"min": "oops", "target": 85, "buffer_pp": 5}, defaults, exempt, explicit, members)
    expect("selftest-6 non-numeric min is fatal", any("V3" in x for x in e))
    # 7) validate-config：min>target → 红
    e = validate_config({"min": 90, "target": 85, "buffer_pp": 5}, defaults, exempt, explicit, members)
    expect("selftest-7 min>target is fatal in config validation", any("CFG" in x for x in e))
    # 8) validate-config：buffer 越界 → 红
    e = validate_config({"min": 50, "target": 85, "buffer_pp": 20}, defaults, exempt, explicit, members)
    expect("selftest-8 bad buffer_pp is fatal", any("buffer_pp" in x for x in e))
    # 9) crate_of 归因：绝对路径 / 反斜杠 / 非 crates 前缀
    expect("selftest-9a crate attribution from rel path", crate_of("crates/core/src/lib.rs") == "core")
    expect("selftest-9b crate attribution from windows abs path", crate_of(r"D:\repo\crates\sec\src\a.rs") == "sec")
    expect("selftest-9c non-crates path -> other", crate_of("build.rs") == "(other)")
    # 10) parse_record 覆盖 uncovered 缺失/仅 cover 两种形态
    p, c, t = parse_record({"filename": "crates/core/src/x.rs", "covered": 8, "uncovered": 2})
    expect("selftest-10a parse covered/uncovered", (c, t) == (8, 10))
    p, c, t = parse_record({"filename": "crates/core/src/x.rs", "total_lines": 20, "cover": 0.5})
    expect("selftest-10b parse total_lines+cover", (c, t) == (10, 20))
    # 11) llvm-cov dict 形状→统一解析（新引擎回归基线；无此则 --check 报“无可用记录”退 2）
    llvm = {"data": [{"files": [
        {"filename": "crates/core/src/lib.rs", "summary": {"lines": {"count": 100, "covered": 90}}},
        {"filename": "crates/sec/src/a.rs", "summary": {"lines": {"count": 100, "covered": 80}}},
    ]}]}
    lr = load_records_from_obj(llvm)
    expect("selftest-11 llvm-cov dict parsed to records",
           sorted(lr) == sorted([("crates/core/src/lib.rs", 90, 100), ("crates/sec/src/a.rs", 80, 100)]))
    # 12) llvm-cov 空 summary/缺键 → 不崩，归 (path,0,count)
    lr2 = load_records_from_obj({"data": [{"files": [{"filename": "crates/x/src/a.rs"}]}]})
    expect("selftest-12 llvm-cov missing summary tolerated", lr2 == [("crates/x/src/a.rs", 0, 0)])

    print("=== selftest result:", "ALL PASS" if not fails else f"{len(fails)} FAIL", "===")
    return 0 if not fails else 1


_last_infos = []  # module-level scratch for INFO lines surfaced by modes


def main(argv):
    global _last_infos
    if "--selftest" in argv:
        return mode_selftest()
    if "--print-fail-under" in argv:
        return mode_print_fail_under()
    if "--validate-config" in argv:
        r, infos = check_min_ratchet()
        _last_infos = infos
        return mode_validate_config()
    if "--check" in argv:
        idx = argv.index("--check")
        target = argv[idx + 1] if idx + 1 < len(argv) else "coverage"
        return mode_check(target)
    print(__doc__)
    print("usage: coverage_gate.py [--print-fail-under | --validate-config | --check DIR | --selftest]")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
