#!/usr/bin/env python3
"""Top-K sort_by 例外登记表的守护门（C9 收编，ADR-160 同族）。

背景（都是本仓踩出来的）：
  scripts/topk_sortby_freeze.txt 是 C9 全仓扫描后"判定为 GRAY/例外、暂不迁移
  到 xts_top_k 范式"的站点登记表，纪律是**只减不增**（随批次消化）。它此前
  零消费者（登记无门）——与 coverage_baseline / crate_reachability_freeze 同族
  的"有登记、无牙齿"反模式。本脚本给它装上牙齿，但**刻意不做启发式 sort_by
  扫描**（那会引入误报红，比休眠登记更糟）；只 enforce 登记表自身不退化的
  四条硬判据 + 只减不增棘轮：

    V-form    每行首 token 必须是 `<crate>/<path>.rs:<int>`（可解析、含 crate）
    V-verdict 每行必须含 {DEFER,GRAY,GREEN,MIGRATED} 之一（裁决词表）
    V-dup     站点键（crate/文件:行 去空白）不得重复
    V-stale   站点所在 crate 目录内必须真实存在该文件（防代码重构后登记腐坏）
    V-shrink  相对 git HEAD 基线，活跃站点集合不得增长（无基线则 [INFO] 跳过，
              首次提交后自动生效）

退出码（对齐 run_gate_manifest.py 语义）：
    0 = 全部通过
    1 = 存在违规（判红，阻塞合入）
    2 = 登记表缺失/不可读（不可判定，不等于通过）

输出保持 ASCII（Windows GBK 控制台约定，见 freeze.txt 头注）。
"""
import os
import re
import subprocess
import sys
import tempfile

# 控制台兜底：即便上游是 GBK 也不因写中文崩溃（本项目脚本惯例）。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:  # noqa: BLE001 - reconfigure 在老解释器/非 tty 下可能缺省
    pass

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REGISTRY_REL = os.path.join("scripts", "topk_sortby_freeze.txt")
VERDICTS = ("DEFER", "GRAY", "GREEN", "MIGRATED")

# 站点首 token 形如 `<crate>/<path>.rs:<line>`；crate/文件可含子路径（如
# hcw-window/recall/rerank.rs:832）。只要求：含 '/'、以 `.rs:<int>` 结尾。
SITE_RE = re.compile(r"^(?P<path>[^\s/]+/[^\s]+\.rs):(?P<line>\d+)$")


def parse_entries(text):
    """从登记表文本解析活跃站点。

    返回 list[(raw_site_key, path, line, verdict, whole_tokens)]。
    规则：以 '#' 开头（含缩进）与空行为注释，跳过；其余为数据行。
    """
    entries = []
    for raw in text.splitlines():
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        tokens = stripped.split()
        if not tokens:
            continue
        m = SITE_RE.match(tokens[0])
        path = m.group("path") if m else None
        line = int(m.group("line")) if m else None
        verdict = next((t for t in tokens if t in VERDICTS), None)
        entries.append((stripped, path, line, verdict, tokens))
    return entries


def check_format_and_verdict(entries):
    """V-form + V-verdict：返回错误列表。"""
    errors = []
    for site_key, path, line, verdict, tokens in entries:
        if path is None or line is None:
            errors.append(f"V-form: site token not 'crate/path.rs:<int>': {tokens[0]!r}")
        if verdict is None:
            errors.append(
                "V-verdict: no verdict in "
                + "/".join(VERDICTS)
                + f" for site: {site_key[:60]}"
            )
    return errors


def check_duplicates(entries):
    """V-dup：活跃站点键（首 token）唯一。"""
    seen = {}
    dup = []
    for site_key, _path, _line, _verdict, tokens in entries:
        key = tokens[0]
        if key in seen:
            dup.append(f"V-dup: duplicate site key {key!r}")
        seen[key] = True
    return dup


def check_staleness(entries, repo_root):
    """V-stale：站点所在 crate 目录内该文件 basename 必须真实存在。

    只校验到文件级（行号会随手改漂移，不作硬失效判据以免误红）。crate = 路径首段。
    """
    errors = []
    for site_key, path, _line, _verdict, _tokens in entries:
        if not path:
            continue  # 已由 V-form 报错
        crate = path.split("/", 1)[0]
        base = os.path.basename(path)
        crate_dir = os.path.join(repo_root, "crates", crate)
        if not os.path.isdir(crate_dir):
            errors.append(f"V-stale: crate dir missing for {path!r}: crates/{crate}")
            continue
        found = False
        for _dirpath, _dirnames, filenames in os.walk(crate_dir):
            if base in filenames:
                found = True
                break
        if not found:
            errors.append(f"V-stale: file {base!r} not found under crates/{crate}/ (site {site_key[:50]})")
    return errors


def check_shrink(current_keys, baseline_keys):
    """V-shrink：当前活跃键集相对基线不得增长。baseline 为 None 表示无基线。"""
    if baseline_keys is None:
        return [], ["V-shrink: no committed baseline for registry (untracked); ratchet self-activates after first commit"]
    added = current_keys - baseline_keys
    errors = []
    for a in sorted(added):
        errors.append(f"V-shrink: NEW registered sort_by site added (table must only shrink): {a}")
    return errors, []


def load_baseline_keys(registry_rel, repo_root):
    """从 git HEAD 读登记表基线活跃键集；未跟踪/无该文件 → None（无基线）。"""
    try:
        out = subprocess.run(
            ["git", "-C", repo_root, "show", f"HEAD:{registry_rel}"],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
        )
    except FileNotFoundError:
        return None  # 无 git，视为无基线（不误红）
    if out.returncode != 0:
        return None  # 该路径尚未提交，无 HEAD 版本
    base_entries = parse_entries(out.stdout)
    return {e[4][0] for e in base_entries}


def validate(registry_path, repo_root, baseline_keys):
    """跑全部判据，返回 (errors, infos, active_count)。"""
    try:
        with open(registry_path, "r", encoding="utf-8", errors="replace") as f:
            text = f.read()
    except OSError:
        return None, None, None  # 交由调用方退 2

    entries = parse_entries(text)
    current_keys = {e[4][0] for e in entries}
    errors = []
    errors += check_format_and_verdict(entries)
    errors += check_duplicates(entries)
    errors += check_staleness(entries, repo_root)
    shrink_errs, shrink_infos = check_shrink(current_keys, baseline_keys)
    errors += shrink_errs
    return errors, shrink_infos, len(entries)


def mode_check(argv):
    registry = argv[0] if argv else os.path.join(ROOT, REGISTRY_REL)
    if not os.path.isfile(registry):
        print(f"[FAIL] registry not found: {registry}")
        return 2
    baseline = load_baseline_keys(REGISTRY_REL.replace(os.sep, "/"), ROOT)
    errors, infos, count = validate(registry, ROOT, baseline)
    if errors is None:
        print("[FAIL] registry unreadable (undeterminable, not a pass)")
        return 2
    for i in infos:
        print(f"[INFO] {i}")
    if errors:
        print(f"[FAIL] topk registry violations ({len(errors)}), {count} active entries:")
        for e in errors:
            print(f"   - {e}")
        return 1
    print(f"[OK] topk registry clean: {count} active entries, format/dup/staleness/shrink all hold")
    return 0


# --------------------------------------------------------------------------- selftest
def mode_selftest():
    """证明门有牙齿：对构造的 fixture 断言各违规必红、干净必绿、增长必红。"""
    print("=== selftest: topk registry gate must have teeth ===")
    tmp = tempfile.mkdtemp(prefix="topk_gate_selftest_")
    fails = []

    def expect(name, cond):
        print(f"  [{'ok' if cond else 'FAIL'}] {name}")
        if not cond:
            fails.append(name)

    def run(text):
        reg = os.path.join(tmp, "reg.txt")
        with open(reg, "w", encoding="utf-8") as f:
            f.write(text)
        errs, _infos, _cnt = validate(reg, ROOT, baseline_keys=None)
        return errs

    # 造一个真实存在的 crate 文件路径供 clean/stale 用：nexus-core 必有 lib.rs
    good_site = "nexus-core/lib.rs:1"
    # 1) clean：一行合法数据 → 无错（无基线时 shrink 只出 INFO 不出 ERR）
    clean = "# comment\n   \n" + good_site + "   some_fn   GRAY   reason here\n"
    expect("selftest-1 clean registry has zero errors", run(clean) == [])
    # 2) 幽灵文件（crate 存在但文件不存在）→ V-stale 必红
    ghost = "nexus-core/definitely_not_a_real_file_zzz.rs:1   f   GRAY   x\n"
    expect("selftest-2 ghost file is flagged stale", any("V-stale" in e for e in run(ghost)))
    # 3) 缺失裁决词 → V-verdict 必红
    noverdict = good_site + "   some_fn   reason_no_verdict\n"
    expect("selftest-3 missing verdict is flagged", any("V-verdict" in e for e in run(noverdict)))
    # 4) 首 token 非 crate/path.rs:line → V-form 必红
    badform = "bareword   some_fn   GRAY   x\n"
    expect("selftest-4 malformed site token is flagged", any("V-form" in e for e in run(badform)))
    # 5) 重复键 → V-dup 必红
    dup = good_site + " a GRAY x\n" + good_site + " b DEFER y\n"
    expect("selftest-5 duplicate site key is flagged", any("V-dup" in e for e in run(dup)))
    # 6) 增长：基线为空集，当前有一活跃键 → V-shrink 必红
    reg = os.path.join(tmp, "reg2.txt")
    with open(reg, "w", encoding="utf-8") as f:
        f.write(good_site + "   f   GRAY   x\n")
    grow_errs, _ = check_shrink({good_site}, baseline_keys=set())
    expect("selftest-6 growth vs baseline is fatal", any("V-shrink" in e for e in grow_errs))
    # 7) 收缩：基线含两键，当前只剩其一 → 不报增长
    shrink_ok, _ = check_shrink({good_site}, baseline_keys={good_site, "other/x.rs:2"})
    expect("selftest-7 shrink is allowed", shrink_ok == [])

    print("=== selftest result:", "ALL PASS" if not fails else f"{len(fails)} FAIL", "===")
    return 0 if not fails else 1


def main(argv):
    if "--selftest" in argv:
        return mode_selftest()
    rest = [a for a in argv if not a.startswith("--")]
    return mode_check(rest)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
