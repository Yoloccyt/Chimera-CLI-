#!/usr/bin/env python3
"""审计"提交耦合"：找出**已跟踪文件引用了未跟踪文件**的所有位置。

WHY：CI/门禁的断链风险从来不是"代码写错"，而是"引用方入库了、被引用方没入库"。
本波次我自己就造了一例：`check_doc_consistency.ps1`（已跟踪）新增 D5 检查，读取
`scripts/mojibake_baseline.txt`（未跟踪）—— 只提前者的话，任何人拉下来跑门禁都会因
`CHANGELOG.md = 94 > cap 0` 判红。所以这条判定必须是**脚本**，不是文档里的一张表。

判定语义：
  REF_UNTRACKED  已跟踪的引用方 -> 未跟踪的被引用方        ⇒ 必须同 commit（或更早）
  BOTH_UNTRACKED 双方都未跟踪                              ⇒ 同 commit（新增批次内部自洽）
  OK             被引用方已跟踪                            ⇒ 无约束
  IGNORED_TARGET 被引用方被 .gitignore 排除（*.md，按 ADR-166 不入库）⇒ 提示级，不参与退出码
  缺输入（引用方文件不存在等）→ 退出码 2，**不视为通过**（沿用 audit_cmd_sync.py 的纪律）

退出码：0 = 无未满足耦合；1 = 存在 REF_UNTRACKED（须同批入库）；2 = 不可判定。

WHY 批量视图（ADR-236 S1）：首版对每条引用逐项起 `git ls-files --error-unmatch` /
`git check-ignore` / `git cat-file`，并为**每个裸名匹配**重走一遍 5 个目录。实测本仓
一次运行 = 子进程 732~2928 次 + `os.walk` 6362 次（累计遍历 2,622,586 个文件条目）
⇒ 单门 102.2s，占 `govd pr` 总工作量首位。现在三个 git 视图各取一次全集、
basename 索引一趟建好，判定语义逐位不变（等价性用合成仓库跑新旧两版逐字节对拍）。
"""
import io
import os
import re
import subprocess
import sys

import gate_rc  # F37：平铺脚本无 __main__ 可包 => 崩溃经 excepthook 退 2, 不借默认 1
gate_rc.install()

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)

# 扫描范围：会进入 CI/门禁的执行体
SCANNERS = [
    (".github/workflows", (".yml", ".yaml")),
    ("scripts", (".ps1", ".sh", ".py")),
]
# 被引用路径的识别：仓库相对路径 + 常见数据文件名
# 末段必须以字母数字收尾：否则 `check_perf_redlines.py.` 这类带尾点的文本会变成假依赖。
DIR_RE = r"(?:scripts|\.github|crates|docs|tests|fuzz)/[A-Za-z0-9_.\-/]*[A-Za-z0-9_]"
PATH_RE = re.compile(DIR_RE)
BARE_RE = re.compile(r"\b([A-Za-z0-9_][A-Za-z0-9_.\-]*\.(?:toml|txt|py|sh|ps1|yml|rs))\b")
# 裸名回退检索的目录与顺序 —— 与首版 resolve() 完全一致，改动顺序会改判定
BARE_ROOTS = ("scripts", ".github/workflows", "docs", "crates", "tests")


def git_lines(*args: str):
    """跑一次 git 并把 stdout 按行取回；rc 非 0 视为不可判定（退 2），不静默当空集。

    `-c core.quotePath=false` 是**等价性要求**而非美化：默认配置下 git 会把非 ASCII
    路径打成八进制转义串（本仓有中文名 md），于是"批量取全集"与首版"逐项
    `--error-unmatch`"会对同一个文件给出不同答案。bytes 收 + utf-8 解，避免本机
    控制台码（cp936）参与判定。
    """
    proc = subprocess.run(["git", "-c", "core.quotePath=false", *args],
                          capture_output=True)
    if proc.returncode != 0:
        raise RuntimeError(
            f"git {' '.join(args)} 退 {proc.returncode}: "
            f"{proc.stderr.decode('utf-8', 'replace').strip()}")
    text = proc.stdout.decode("utf-8", "replace")
    return [line for line in text.splitlines() if line]


def tracked_paths():
    """index 里的全部路径 —— 替代逐路径 `git ls-files --error-unmatch`。"""
    return set(git_lines("ls-files"))


def head_paths():
    """HEAD 里的全部文件路径 —— 替代逐路径 `git cat-file -e HEAD:<path>`。

    只用于判"引用方是否本次新入"，被检对象恒为文件路径，故 `ls-tree -r`（只列 blob）
    与 `cat-file -e` 在此等价。
    """
    return set(git_lines("ls-tree", "-r", "--name-only", "HEAD"))


def ignored_paths(candidates):
    """一次 `check-ignore --stdin` 判完全集。

    输出格式（`-v --non-matching`）：命中为 `<来源>:<行>:<模式>\t<路径>`，
    未命中为 `::\t<路径>` ⇒ 取末列做路径、首列 `::` 做"未忽略"标记。
    注意 git 默认**不报已被跟踪的路径**，与首版"先判 tracked 再判 ignored"的次序同构。

    WHY 用 bytes 送 stdin：`text=True` 会把 `\\n` 按宿主行尾翻成 `\\r\\n`，于是 git
    看到的是带 `\\r` 的路径、**一个都不命中** —— 本函数的第一版就是这么把
    IGNORED_TARGET 静默变成了硬耦合的，靠合成仓库的新旧对拍才抓出来。
    """
    if not candidates:
        return set()
    payload = ("\n".join(candidates) + "\n").encode("utf-8")
    proc = subprocess.run(["git", "-c", "core.quotePath=false",
                           "check-ignore", "-v", "--non-matching", "--stdin"],
                          input=payload, capture_output=True)
    # rc=1 只表示"没有一个候选被忽略"，属正常判决而非环境故障
    if proc.returncode not in (0, 1):
        raise RuntimeError(
            f"git check-ignore 退 {proc.returncode}: "
            f"{proc.stderr.decode('utf-8', 'replace').strip()}")
    ignored = set()
    for line in proc.stdout.decode("utf-8", "replace").splitlines():
        fields = line.split("\t")
        if len(fields) >= 2 and fields[0] != "::":
            ignored.add(fields[-1])
    return ignored


_bare_cache = {}


def bare_index():
    """basename -> 首个匹配路径。一趟遍历取代"每个裸名重走 5 个目录"。

    与首版同构的三点：目录顺序 = BARE_ROOTS；每个目录内取 os.walk 首次命中
    （setdefault 即"首个赢"）；只剪 `target`（首版就这么剪，多剪会改判定面）。
    """
    if not _bare_cache:
        idx = {}
        for top in BARE_ROOTS:
            for dp, dirs, fs in os.walk(top):
                dirs[:] = [x for x in dirs if x != "target"]
                for f in fs:
                    idx.setdefault(f, (dp + "/" + f).replace(os.sep, "/"))
        _bare_cache["idx"] = idx
    return _bare_cache["idx"]


def resolve(ref: str, holder: str, idx):
    """把引用文本解析成仓库相对路径；解析不到返回 None。"""
    cand = ref if ref.startswith((".github/", "scripts/", "crates/", "docs/", "tests/", "fuzz/")) \
        else os.path.join(os.path.dirname(holder), ref)
    cand = os.path.normpath(cand).replace(os.sep, "/")
    if os.path.exists(cand):
        return cand
    # 裸文件名：在常见数据目录里找（索引由一趟遍历建好）
    return idx.get(os.path.basename(ref))


def collect_records():
    """扫一遍执行体，产出 (holder, ref) 候选对 —— 保持首版的遍历与排序，
    使后续输出顺序逐位不变。返回 (records, scanned, unreadable)。"""
    records = []
    unreadable = []
    scanned = 0
    idx = bare_index()
    for top, exts in SCANNERS:
        if not os.path.isdir(top):
            unreadable.append(f"{top}/ 目录不存在")
            continue
        for dp, dirs, fs in os.walk(top):
            dirs[:] = [x for x in dirs if x not in ("target", "__pycache__")]
            for f in sorted(fs):
                if not f.endswith(exts):
                    continue
                holder = (dp + "/" + f).replace(os.sep, "/")
                try:
                    text = io.open(holder, encoding="utf-8", errors="replace").read()
                except OSError as e:
                    unreadable.append(f"{holder}: 读取失败 {e}")
                    continue
                scanned += 1
                refs = set(PATH_RE.findall(text))
                # 裸名只在本目录/脚本目录能解析时才计入，避免把注释里的示例当依赖
                for m in BARE_RE.finditer(text):
                    r = resolve(m.group(1), holder, idx)
                    if r:
                        refs.add(r)
                for ref in sorted(refs):
                    if ref == holder or not os.path.isfile(ref):
                        continue      # 目录/不存在的都不是依赖（首版把 docs/architecture/ 当依赖）
                    records.append((holder, ref))
    return records, scanned, unreadable


def classify(records, tracked, ignored, head):
    """纯判定：候选对 + 三个集合 -> (findings, ignored_refs)。

    抽成纯函数是为了让 `--selftest` 能在不碰 git 的前提下植入反例（首版把 git 调用
    与判定交织在一起，任何自测都只能重跑真树，测不到分类逻辑本身）。
    """
    findings = []
    ignored_refs = []
    for holder, ref in records:
        if ref in tracked:
            continue
        if ref in ignored:
            # *.md 按 ADR-166 永不入库，CI 对缺失走 warn 分支 —— 不是提交耦合
            ignored_refs.append((holder, ref))
            continue
        kind = "REF_UNTRACKED" if holder in tracked else "BOTH_UNTRACKED"
        findings.append((kind, holder, ref, holder in head))
    return findings, ignored_refs


def emit(findings, ignored_refs, unreadable, scanned):
    """按首版的文本与顺序打印，返回退出码。"""
    print(f"扫描执行体 {scanned} 个（workflows + scripts）")
    print(f"发现未满足耦合 {len(findings)} 条\n")
    hard = [x for x in findings if x[0] == "REF_UNTRACKED"]
    soft = [x for x in findings if x[0] == "BOTH_UNTRACKED"]
    for kind, holder, ref, held_in_head in hard:
        tag = "已在 HEAD" if held_in_head else "本次新入"
        print(f"  [{kind}] {holder}（{tag}）-> {ref}（未跟踪）")
    for kind, holder, ref, _ in soft:
        print(f"  [{kind}] {holder} -> {ref}（双方未跟踪，须同批）")

    if ignored_refs:
        print(f"\n[IGNORED_TARGET] {len(ignored_refs)} 条引用指向被 .gitignore 排除的文档"
              "（按 ADR-166 永不入库，非提交耦合，不参与退出码）：")
        for holder, ref in ignored_refs[:6]:
            print(f"    {holder} -> {ref}")
        if len(ignored_refs) > 6:
            print(f"    ... 其余 {len(ignored_refs) - 6} 条同性质")

    if unreadable:
        print("\n不可判定项（不得当作通过）：")
        for u in unreadable:
            print("  ", u)

    print("\n结论：")
    if unreadable and not findings:
        print("  退 2 —— 输入不全，耦合关系不可判定")
        return 2
    if hard:
        print(f"  退 1 —— {len(hard)} 条硬耦合：引用方已跟踪而被引用方未跟踪，必须同 commit 或更早")
        return 1
    print(f"  退 0 —— 无 REF_UNTRACKED；另有 {len(soft)} 条同批（双方未跟踪）提示")
    return 0


def selftest():
    """植入反例驱动分类器：四类判定各一条，缺任一条即自检红。

    WHY 不只跑真树：本仓此刻是干净树，真跑输出为 0 条 —— "0 缺陷"与"分类器失能"
    在输出上完全同形，只有植入正例能区分（同一课在 I10/I11 上又还过一次）。
    """
    t = "tracked.txt"
    u = "untracked.txt"
    m = "notes.md"
    holder_tracked = "scripts/a.py"
    holder_new = "scripts/b.py"
    records = [
        (holder_tracked, u),      # 1 硬耦合
        (holder_tracked, t),      # 2 被引用方已跟踪 -> 无约束
        (holder_tracked, m),      # 3 被 .gitignore 排除 -> 提示级
        (holder_new, u),          # 4 双方未跟踪 -> 同批提示
        (holder_new, t),          # 5 引用方未跟踪但被引用方已跟踪 -> 无约束
    ]
    tracked = {t, holder_tracked}
    ignored = {m}
    head = {t, holder_tracked}    # holder_new 不在 HEAD（本次新入）
    findings, ignored_refs = classify(records, tracked, ignored, head)

    checks = [
        ("REF_UNTRACKED 恰 1 条且指向 untracked",
         [f for f in findings if f[0] == "REF_UNTRACKED"] ==
         [("REF_UNTRACKED", holder_tracked, u, True)]),
        ("BOTH_UNTRACKED 恰 1 条且 tag 为本次新入",
         [f for f in findings if f[0] == "BOTH_UNTRACKED"] ==
         [("BOTH_UNTRACKED", holder_new, u, False)]),
        ("已跟踪的被引用方不产 findings（两条都被过滤）", len(findings) == 2),
        ("ignored 落提示级且计数为 1", ignored_refs == [(holder_tracked, m)]),
        ("输入不全时退 2（无 findings）", emit([], [], ["x/ 目录不存在"], 0) == 2),
        ("有硬耦合时退 1", emit([("REF_UNTRACKED", holder_tracked, u, True)], [], [], 0) == 1),
        ("干净时退 0 并保留同批提示计数",
         emit([("BOTH_UNTRACKED", holder_new, u, False)], [], [], 0) == 0),
        # holder 用 workflows 下的路径：`scripts/gate_manifest.toml` 这条相对解析必然落空，
        # 于是只有 bare_index() 生效才能解析出来 —— 同时验到 BARE_ROOTS 的优先序。
        ("裸名索引真能解析（gate_manifest.toml -> scripts/，且优先于 docs/crates）",
         resolve("gate_manifest.toml", ".github/workflows/ci.yml", bare_index())
         == "scripts/gate_manifest.toml"),
        ("解析不到的裸名返回 None 而不是猜一个",
         resolve("definitely_not_a_real_file_xyz.txt", ".github/workflows/ci.yml", bare_index()) is None),
    ]
    failed = [name for name, ok in checks if not ok]
    for name, ok in checks:
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}")
    if failed:
        print(f"RESULT: FAIL {failed} ({len(failed)} of {len(checks)} assertions broke)")
        return 1
    print(f"RESULT: PASS (all {len(checks)} assertions held)")
    return 0


def main():
    if "--selftest" in sys.argv[1:]:
        return selftest()
    records, scanned, unreadable = collect_records()
    candidates = sorted({ref for _, ref in records})
    try:
        tracked = tracked_paths()
        head = head_paths()
        ignored = ignored_paths(candidates)
    except RuntimeError as error:
        print(f"[UNDECIDABLE] git 视图取不到，不得当作通过：{error}")
        return 2
    findings, ignored_refs = classify(records, tracked, ignored, head)
    return emit(findings, ignored_refs, unreadable, scanned)


if __name__ == "__main__":
    sys.exit(main())
