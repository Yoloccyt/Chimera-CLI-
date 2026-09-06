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
"""
import io
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

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


def is_tracked(path: str) -> bool:
    return subprocess.run(["git", "ls-files", "--error-unmatch", path],
                          capture_output=True).returncode == 0


def is_ignored(path: str) -> bool:
    """被 .gitignore 排除：按 ADR-166 的既定策略，这类文件不参与提交耦合判定。"""
    return subprocess.run(["git", "check-ignore", "-q", "--stdin"],
                          input=path.encode(), capture_output=True).returncode == 0


def in_head(path: str) -> bool:
    return subprocess.run(["git", "cat-file", "-e", f"HEAD:{path}"],
                          capture_output=True).returncode == 0


def resolve(ref: str, holder: str):
    """把引用文本解析成仓库相对路径；解析不到返回 None。"""
    cand = ref if ref.startswith((".github/", "scripts/", "crates/", "docs/", "tests/", "fuzz/")) \
        else os.path.join(os.path.dirname(holder), ref)
    cand = os.path.normpath(cand).replace(os.sep, "/")
    if os.path.exists(cand):
        return cand
    # 裸文件名：在常见数据目录里找
    base = os.path.basename(ref)
    for d in ("scripts", ".github/workflows", "docs", "crates", "tests"):
        for dp, dirs, fs in os.walk(d):
            dirs[:] = [x for x in dirs if x != "target"]
            if base in fs:
                return (dp + "/" + base).replace(os.sep, "/")
    return None


findings = []
ignored_refs = []
unreadable = []
scanned = 0
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
                r = resolve(m.group(1), holder)
                if r:
                    refs.add(r)
            for ref in sorted(refs):
                if ref == holder or not os.path.isfile(ref):
                    continue          # 目录/不存在的都不是依赖（首版把 docs/architecture/ 当依赖）
                if is_tracked(ref):
                    continue
                if is_ignored(ref):
                    # *.md 按 ADR-166 永不入库，CI 对缺失走 warn 分支 —— 不是提交耦合
                    ignored_refs.append((holder, ref))
                    continue
                kind = "REF_UNTRACKED" if is_tracked(holder) else "BOTH_UNTRACKED"
                findings.append((kind, holder, ref, in_head(holder)))

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
    sys.exit(2)
if hard:
    print(f"  退 1 —— {len(hard)} 条硬耦合：引用方已跟踪而被引用方未跟踪，必须同 commit 或更早")
    sys.exit(1)
print(f"  退 0 —— 无 REF_UNTRACKED；另有 {len(soft)} 条同批（双方未跟踪）提示")
sys.exit(0)
