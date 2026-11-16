#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""cargo audit 口径对拍门禁（ADR-167 决策 4）—— scripts/audit_cmd.toml 为唯一机器真值源。

背景：这条命令的判据曾以散文抄写三处 —— `.github/workflows/audit.yml` 的
`Run security audit` 步骤、`AGENTS.md §7.2`、`.claude/CLAUDE.md` §2/§5。
P5-T5(2026-08-28) 把 CI 侧 `--deny warnings` 改为 `--deny unmaintained --deny
unsound` 时两份文档没跟上，照文档手跑的人会得出与 CI 不同的结论 —— 本仓反复
出现的"两份互相抄、其中一份不生效"病灶（A12)，只是这次落在安全判据上。
旧版 audit_cmd_sync.py 已做三处散文对拍，但仍有三个缺口：
  ① 真值源本身是散文，解析器要兼容 PS 反引号/反斜杠续行等书写变体，牙口弱；
  ② 目标 .md 不入库(.gitignore)，CI checkout 里看不到 ⇒ 旧脚本在 CI 只能退 2，
     从未真正接入 CI（步骤级检查缺位，等价性闭环有名无实）;
  ③ 散文互拍只能报"不一致"，回答不了"哪份是对的"。

改造（单一机器真值源，先例 I9/I10/I12 阈值表对拍模式）：
  * `scripts/audit_cmd.toml` 登记命令模板 + deny 旗标 + ignores 清单
    （顺序以 audit.yml `Run security audit` 步骤现取为准），三模式共同对拍它；
  * `--check-ci`   二方对拍（audit.yml 步骤命令 ↔ toml)，只依赖入库文件，接入 CI,
     先于实跑 cargo audit 执行（audit.yml 前置步骤）;
  * `--check-docs` 本地对拍（audit.yml 步骤 + AGENTS.md + .claude/CLAUDE.md ↔ toml),
     缺 .md 退 2 语义保留 —— 缺失静默"通过" = 永远为真的假门禁（本仓第 5 次同类病灶）;
  * `--selftest`   夹具自证：篡改命令→红、缺 ignore→红、toml 缺键/缺文件→rc=2。

用法:
    python scripts/audit_cmd_sync.py --check-ci      # CI：先于实跑 cargo audit
    python scripts/audit_cmd_sync.py --check-docs    # 本地发布前检查
    python scripts/audit_cmd_sync.py --selftest      # 夹具自证
    python scripts/audit_cmd_sync.py                 # 裸跑等价 --check-docs
                                                     # （.claude/CLAUDE.md §5 2.6 既有口径）

退出码（三态，2=不可判定≠通过）:
    0 = 对拍一致
    1 = 存在不一致（打印双方旗标明细）
    2 = 环境/用法错误 —— toml 缺键/缺文件/语法错、yml 找不到锚步骤、
        解释器无 tomllib(Python<3.11) 等，判据没真正跑完

比较语义（刻意不同，别"统一"):
    * --check-ci 严格有序：旗标 (名,值) 序列逐 token 相等。toml 即按 audit.yml
      顺序登记，序漂移即红 —— 真值源是逐字镜像，不是语义等价类;
    * --check-docs 有序多重集：只比"旗标-值"多重集，不比序。文档是给人读的散文，
      三处书写顺序本就可能不同（实测 AGENTS.md 与 audit.yml 的 ignore 顺序即不一致）。
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOML_REL = "scripts/audit_cmd.toml"
YML_REL = ".github/workflows/audit.yml"
# 步骤名是对拍锚：改名即失锚，门禁必须退 2 而不是换个地方猜（行号会漂移，步骤名不会）
CI_STEP = "Run security audit"
DOCS = ("AGENTS.md", ".claude/CLAUDE.md")

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11 —— 判据跑不了，归不可判定而非崩溃
    tomllib = None


class TruthError(Exception):
    """真值源/锚点不可用作判定（toml 缺失、缺键、语法错、yml 找不到步骤）。

    WHY 单独一个类型：这类失败必须翻译成 rc=2 并给出可读原因，而不是
    借 gate_rc 的通用崩溃路径吐 traceback —— 真值源是人手改的文件，
    报错要直接告诉他缺了什么。
    """


def load_truth(path: Path) -> str:
    """读 audit_cmd.toml，渲染出完整命令字符串（真值源唯一出口）。

    渲染而非只取数组：template 占位符缺一个、deny/ignores 类型不对、
    渲染结果不像 cargo audit 命令 —— 全部归 TruthError(rc=2)。
    缺键的"对拍一致"是假绿，宁可不可判定。
    """
    if tomllib is None:
        raise TruthError("解释器无 tomllib(Python<3.11)，无法读真值源 toml")
    if not path.exists():
        raise TruthError(f"真值源缺失: {path}")
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8-sig"))
    except tomllib.TOMLDecodeError as exc:
        raise TruthError(f"真值源 TOML 语法错误: {exc}") from exc
    cmd = data.get("command")
    if not isinstance(cmd, dict):
        raise TruthError("真值源缺 [command] 段")
    template, deny, ignores = cmd.get("template"), cmd.get("deny"), cmd.get("ignores")
    missing = [k for k, v in (("template", template), ("deny", deny), ("ignores", ignores))
               if v is None]
    if missing:
        raise TruthError(f"真值源 [command] 缺键: {', '.join(missing)}")
    if not isinstance(template, str) or "{deny}" not in template or "{ignores}" not in template:
        raise TruthError("template 必须是同时含 {deny} 与 {ignores} 占位符的字符串")
    if not (isinstance(deny, list) and all(isinstance(x, str) for x in deny)):
        raise TruthError("deny 必须是字符串数组")
    if not (isinstance(ignores, list) and all(isinstance(x, str) for x in ignores)):
        raise TruthError("ignores 必须是字符串数组")
    deny_part = " ".join(f"--deny {d}" for d in deny)
    ignore_part = " ".join(f"--ignore {i}" for i in ignores)
    rendered = template.replace("{deny}", f" {deny_part}" if deny_part else "")
    rendered = rendered.replace("{ignores}", f" {ignore_part}" if ignore_part else "")
    rendered = re.sub(r"\s+", " ", rendered).strip()
    if not rendered.startswith("cargo audit"):
        raise TruthError(f"template 渲染结果不是 cargo audit 命令: {rendered!r}")
    return rendered


def flag_seq(cmd: str) -> list[tuple[str, str]]:
    """从 `audit` 起连续消费 `--旗标 [值]`，遇第一个非选项 token 即停。

    WHY 截断：文档常在命令后追加中文解释从句（如"…（与 audit.yml 的
    `Run security audit` 步骤逐字一致…）"），不截断会把从句当成命令。
    PS 反引号续行先抹成空白再切词；值缺失（旗标在命令尾）时值记 ""。
    返回有序 (旗标名, 值) 对 —— 有序性即 --check-ci 的严格度来源。
    """
    toks = cmd.replace("`", " ").split()
    if "audit" not in toks:
        return []
    seq: list[tuple[str, str]] = []
    i = toks.index("audit") + 1
    while i < len(toks):
        flag = toks[i].rstrip(",;.")
        if not flag.startswith("--"):
            break
        val = ""
        if i + 1 < len(toks):
            nxt = toks[i + 1].rstrip(",;.")
            if nxt and not nxt.startswith("--"):
                val = nxt
                i += 1
        seq.append((flag[2:], val))
        i += 1
    return seq


def ci_step_command(path: Path, step_name: str = CI_STEP) -> str:
    """在 workflow yml 中按步骤名定位 step，取其 run 命令（单行流式或 | 块标量）。

    刻意用步骤名而不用"文件里最后一条 cargo audit"：这个文件里只能有一个
    对拍锚，锚漂移（步骤改名/新增第二条 audit 命令）必须失锚退 2，
    而不是静默换目标制造假绿。
    """
    lines = path.read_text(encoding="utf-8-sig").splitlines()
    for idx, raw in enumerate(lines):
        if raw.strip() != f"- name: {step_name}":
            continue
        j = idx + 1
        while j < len(lines):
            s = lines[j]
            if re.match(r"^\s*run:\s*\S", s):
                return s.split("run:", 1)[1].strip()
            if re.match(r"^\s*run:\s*[|>]", s):  # 块标量：收缩进一致的非空行
                block, base = [], None
                for k in range(j + 1, len(lines)):
                    t = lines[k]
                    if not t.strip():
                        continue
                    indent = len(t) - len(t.lstrip())
                    if base is None:
                        base = indent
                    if indent < base:
                        break
                    block.append(t.strip())
                return re.sub(r"\s+", " ", " ".join(block)).strip()
            if re.match(r"^\s*- name:", s):
                break  # 走到下一个 step 还没 run：本 step 无命令，失锚
            j += 1
    raise TruthError(f"{path.name} 中找不到步骤 {step_name!r} —— 对拍失锚，不得猜")


def doc_commands(path: Path) -> list[str]:
    """抽取散文文件里的 `cargo audit` 命令（含 YAML 反斜杠 / PS 反引号续行）。

    跳过：纯注释行（描述的是历史变更）与 Markdown 表格行（叙述而非命令）。
    每处文件的最后一条命令视为该文件的当前态（文档 §2 在前、清单在后）。
    """
    out: list[str] = []
    lines = path.read_text(encoding="utf-8-sig").splitlines()
    i = 0
    while i < len(lines):
        raw = lines[i].strip()
        if not raw.lstrip("|").startswith("#") and "cargo audit" in raw \
                and not raw.startswith("|"):
            buf = raw
            while re.search(r"[\\`]$", buf) and i + 1 < len(lines):
                i += 1
                buf = re.sub(r"[\\`]$", "", buf).strip() + " " + lines[i].strip()
            if "--deny" in buf or "--ignore" in buf:
                out.append(re.sub(r"\s+", " ", buf))
        i += 1
    return out


def fmt_flags(pairs: list[tuple[str, str]]) -> str:
    """失败信息用：("deny","unmaintained") → "--deny unmaintained"。"""
    return " ".join(f"--{k}" + (f" {v}" if v else "") for k, v in pairs)


def check_ci(root: Path) -> int:
    """二方对拍：audit.yml `Run security audit` 步骤命令 ↔ toml（严格有序）。"""
    truth = load_truth(root / TOML_REL)
    yml = root / YML_REL
    if not yml.exists():
        print(f"[UNDECIDABLE] required file not found: {YML_REL} "
              "(该项比对未执行，不得视为通过)", file=sys.stderr)
        return 2
    want = flag_seq(truth)
    got = flag_seq(ci_step_command(yml))
    if got != want:
        print("[FAIL] audit.yml `Run security audit` 步骤与真值源不一致:", file=sys.stderr)
        print(f"  yml : {fmt_flags(got)}", file=sys.stderr)
        print(f"  toml: {fmt_flags(want)}", file=sys.stderr)
        return 1
    n_ig = sum(1 for k, _ in want if k == "ignore")
    print(f"[PASS] audit.yml `{CI_STEP}` 步骤与 {TOML_REL} 逐字等价 "
          f"(deny={len(want) - n_ig}, ignore={n_ig})")
    return 0


def check_docs(root: Path) -> int:
    """本地对拍：audit.yml 步骤 + AGENTS.md + .claude/CLAUDE.md ↔ toml（有序多重集）。

    缺任一 .md 退 2：.md 不入库，CI checkout 看不到；若在缺失时静默通过，
    就等于造出永远为真的假门禁。
    """
    truth = load_truth(root / TOML_REL)
    want = sorted(flag_seq(truth))
    current: dict[str, list[tuple[str, str]]] = {}
    yml = root / YML_REL
    if not yml.exists():
        print(f"[UNDECIDABLE] required file not found: {YML_REL}", file=sys.stderr)
        return 2
    current[YML_REL] = flag_seq(ci_step_command(yml))
    for rel in DOCS:
        path = root / rel
        if not path.exists():
            # 大小写不敏感兜底：仓库内该文件历史上出现过 agents.md / AGENTS.md 两种写法
            alt = root / rel.lower()
            if alt.exists():
                path = alt
            else:
                print(f"[UNDECIDABLE] required file not found: {rel} "
                      "(该项比对未执行，不得视为通过)", file=sys.stderr)
                return 2
        cmds = doc_commands(path)
        if not cmds:
            print(f"[FAIL] no `cargo audit` command parsed from {rel} "
                  "(解析失败即为门禁失效，不得静默放过)", file=sys.stderr)
            return 1
        current[rel] = flag_seq(cmds[-1])
    bad = [rel for rel, got in current.items() if sorted(got) != want]
    if bad:
        print("[FAIL] 以下来源与真值源口径不一致:", file=sys.stderr)
        print(f"  toml({TOML_REL}): {fmt_flags(want)}", file=sys.stderr)
        for rel in bad:
            print(f"  {rel}: {fmt_flags(current[rel])}", file=sys.stderr)
        return 1
    n_ig = sum(1 for k, _ in want if k == "ignore")
    print(f"[PASS] {len(current)} 处来源与 {TOML_REL} 等价 "
          f"(deny={len(want) - n_ig}, ignore={n_ig})")
    return 0


def selftest() -> int:
    """夹具自证：真树零接触，全部在临时目录里造假树（gate_rc.tmpdir, atexit 自清）。

    夹具覆盖任务规格三件套：篡改命令→红、缺 ignore→红、toml 缺键→rc=2；
    另加四个补强用例：多旗标→红、序漂移→红(--check-ci 严格度)、
    文档序乱多重集同→绿(--check-docs 语义自证)、缺 .md→rc=2。
    """
    import gate_rc  # 只在 selftest 需要：tmp 夹具目录的登记与清理

    GOOD_CMD = ("cargo audit --deny unmaintained --deny unsound "
                "--ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 "
                "--ignore RUSTSEC-2026-0253 --ignore RUSTSEC-2024-0436 "
                "--ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2025-0119")
    SCRAMBLED_CMD = ("cargo audit --deny unmaintained --deny unsound "
                     "--ignore RUSTSEC-2025-0119 --ignore RUSTSEC-2025-0141 "
                     "--ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2026-0253 "
                     "--ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2026-0190")
    GOOD_TOML = """[command]
template = "cargo audit{deny}{ignores}"
deny = ["unmaintained", "unsound"]
ignores = [
    "RUSTSEC-2026-0190",
    "RUSTSEC-2026-0002",
    "RUSTSEC-2026-0253",
    "RUSTSEC-2024-0436",
    "RUSTSEC-2025-0141",
    "RUSTSEC-2025-0119",
]
"""
    GOOD_DOC = f"散文段落\n\n{GOOD_CMD}\n"
    CASES = ("golden", "tampered", "drop-ignore", "extra-flag", "order-drift",
             "toml-missing-key", "toml-missing-file", "step-renamed",
             "docs-scrambled", "docs-missing-key", "docs-absent")

    n = 0
    bad = 0

    def check(label: str, got: int, want: int) -> None:
        nonlocal n, bad
        n += 1
        if got != want:
            bad += 1
            print(f"  [FAIL] {label} got rc={got} want rc={want}", file=sys.stderr)
        else:
            print(f"  [PASS] {label} -> rc={got}")

    def rc_of(fn) -> int:
        """复刻 main 的 TruthError→2 翻译，夹具里直接调 check_* 时同语义。"""
        try:
            return fn()
        except TruthError:
            return 2

    def build(case: str, yml_cmd: str = GOOD_CMD, toml_text: str = GOOD_TOML,
              step: str = "Run security audit", with_docs: bool = True,
              doc_text: str = GOOD_DOC) -> Path:
        base = Path(gate_rc.tmpdir(prefix="audit-cmd-sync-")) / case
        (base / ".github/workflows").mkdir(parents=True)
        (base / "scripts").mkdir(parents=True)
        (base / ".claude").mkdir(parents=True)
        (base / ".github/workflows/audit.yml").write_text(
            "jobs:\n  audit:\n    steps:\n"
            f"      - name: {step}\n        run: {yml_cmd}\n", encoding="utf-8")
        if toml_text is not None:
            (base / "scripts/audit_cmd.toml").write_text(toml_text, encoding="utf-8")
        if with_docs:
            (base / "AGENTS.md").write_text(doc_text, encoding="utf-8")
            (base / ".claude/CLAUDE.md").write_text(doc_text, encoding="utf-8")
        return base

    drop_last = GOOD_CMD.rsplit(" ", 2)[0]  # 去掉末尾 "--ignore RUSTSEC-2025-0119"
    toml_no_deny = GOOD_TOML.replace('deny = ["unmaintained", "unsound"]\n', "")

    check("golden: yml == toml -> rc=0",
          rc_of(lambda: check_ci(build("golden"))), 0)
    check("篡改命令(yml 改一条 RUSTSEC id) -> rc=1",
          rc_of(lambda: check_ci(build("tampered",
                                       yml_cmd=GOOD_CMD.replace("RUSTSEC-2026-0190",
                                                                "RUSTSEC-2026-9999")))), 1)
    check("缺 ignore(yml 少末尾一条) -> rc=1",
          rc_of(lambda: check_ci(build("drop-ignore", yml_cmd=drop_last))), 1)
    check("多旗标(yml 追加 --deny warnings) -> rc=1",
          rc_of(lambda: check_ci(build("extra-flag",
                                       yml_cmd=GOOD_CMD + " --deny warnings"))), 1)
    check("序漂移(yml ignore 倒序, 多重集相同) --check-ci 严格有序 -> rc=1",
          rc_of(lambda: check_ci(build("order-drift", yml_cmd=SCRAMBLED_CMD))), 1)
    check("toml 缺 deny 键 -> rc=2",
          rc_of(lambda: check_ci(build("toml-missing-key", toml_text=toml_no_deny))), 2)
    check("toml 文件缺失 -> rc=2",
          rc_of(lambda: check_ci(build("toml-missing-file", toml_text=None))), 2)
    check("yml 步骤改名失锚 -> rc=2",
          rc_of(lambda: check_ci(build("step-renamed", step="Run audit"))), 2)
    check("文档 golden -> rc=0",
          rc_of(lambda: check_docs(build("golden"))), 0)
    check("文档 ignore 倒序(多重集相同) --check-docs 序不敏感 -> rc=0",
          rc_of(lambda: check_docs(build("docs-scrambled",
                                         doc_text=f"散文\n\n{SCRAMBLED_CMD}\n"))), 0)
    check("文档缺一条 ignore -> rc=1",
          rc_of(lambda: check_docs(build("docs-missing-key",
                                         doc_text=f"散文\n\n{drop_last}\n"))), 1)
    check("文档缺失(.md 不入库场景) -> rc=2",
          rc_of(lambda: check_docs(build("docs-absent", with_docs=False))), 2)

    print(f"  RESULT: {'PASS' if not bad else 'FAIL'} ({n} assertions, "
          f"{len(CASES)} fixture trees)")
    return 1 if bad else 0


MODES = ("--check-ci", "--check-docs", "--selftest")
USAGE = ("usage: python scripts/audit_cmd_sync.py "
         "[--check-ci | --check-docs | --selftest]\n"
         "  裸跑等价 --check-docs；三态退出码 0=一致 1=不一致 2=不可判定")


def main(argv: list[str]) -> int:
    # F35 同族：未知旗标/多模式并发不再静默忽略 —— 失败方向必须是拒绝执行，
    # 不是降级执行后给一份没请求的判定。
    modes = [a for a in argv if a in MODES]
    unknown = [a for a in argv if a not in MODES]
    if unknown or len(modes) > 1:
        print(USAGE, file=sys.stderr)
        parts = []
        if unknown:
            parts.append(f"未知参数: {' '.join(unknown)}")
        if len(modes) > 1:
            parts.append(f"模式重复: {' '.join(modes)}")
        print(f"[UNDECIDABLE] 用法错误({'；'.join(parts)}) —— 未执行任何比对",
              file=sys.stderr)
        return 2
    mode = modes[0] if modes else "--check-docs"  # 裸跑兼容 .claude/CLAUDE.md §5 2.6 既有口径
    try:
        if mode == "--selftest":
            return selftest()
        if mode == "--check-ci":
            return check_ci(ROOT)
        return check_docs(ROOT)
    except TruthError as exc:
        print(f"[UNDECIDABLE] {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    import gate_rc  # 只在入口需要：崩溃必须退 2, 不得借 1 冒充"判过且红"（F32/F33）
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
