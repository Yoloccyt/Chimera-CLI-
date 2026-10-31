#!/usr/bin/env python3
# =============================================================================
# check_llm_loop_defaults.py - LLM 工具循环（ADR-232）默认值与安全底线的门
# =============================================================================
# WHY this gate exists
#   ADR-232 的四条承诺全是"运行期才暴露"的那类：默认关闭、四重上限、审批默认拒绝、
#   不伪造收敛。它们一旦被改宽，编译与既有测试都不会红——`max_turns: 8` 改成 `800`
#   照样绿，`#[derive(Default)]` 加到 outcome 上照样绿，把 `Some("apply") | Some("on")`
#   扩成 `_ => true` 也照样绿（而且 llm_off_legacy 那 6 个子进程用例只证明"没开开关时
#   旧路径不变"，证明不了"开关本身仍是白名单"）。本门把这四条钉成静态判据。
#
#   L1 默认 off（白名单式启用）
#      `commands/run.rs::llm_loop_enabled` 必须是 `matches!(... Some("apply") | Some("on"))`
#      且**不得**出现 `_ =>` 兜底为 true 的写法。
#   L2 四重上限同时存在且默认非零
#      max_turns / max_tool_calls / max_repeat_tool_calls / max_total_tokens 四字段
#      在 LlmToolLoopConfig::default() 里都有非零值，且各自有 CHIMERA_LLM_MAX_* 读取点。
#   L3 停止原因不可伪装
#      `pub struct LlmLoopOutcome` 的 derive 行不得含 Default（漏落 stop 必须编译不过，
#      而不是静默等于 Final）。
#   L4 库层不 panic
#      `src/llm/*.rs` 内 unwrap()/expect( 命中数为 0；无 provider 时 composition 的
#      非 mca 分支必须返回 ConfigError（可执行提示），不是 panic、不是空回。
#   L5 事件面只复用不新增
#      `src/llm/*.rs` 构造的 NexusEvent 集合必须 ⊆ {BudgetExceeded}。
#
# Exit dialect (shared with gate_rc): 0 全过 / 1 有发现 / 2 输入不可读或扫描致盲
#
# Usage:
#   py -3 scripts/check_llm_loop_defaults.py --selftest
#   py -3 scripts/check_llm_loop_defaults.py
#   py -3 scripts/check_llm_loop_defaults.py --json
# =============================================================================
import argparse
import json
import os
import re
import shutil
import sys
import tempfile

SCRIPTS_DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPTS_DIR)

LLM_DIR = os.path.join("crates", "chimera-cli", "src", "llm")
RUN_RS = os.path.join("crates", "chimera-cli", "src", "commands", "run.rs")
CLIENT_RS = os.path.join("crates", "chimera-cli", "src", "llm", "client.rs")
TOOL_LOOP_RS = os.path.join("crates", "chimera-cli", "src", "llm", "tool_loop.rs")
COMPOSITION_RS = os.path.join("crates", "chimera-cli", "src", "composition.rs")

LIMIT_FIELDS = (
    "max_turns",
    "max_tool_calls",
    "max_repeat_tool_calls",
    "max_total_tokens",
)
LIMIT_ENVS = (
    "CHIMERA_LLM_MAX_TURNS",
    "CHIMERA_LLM_MAX_TOOL_CALLS",
    "CHIMERA_LLM_MAX_REPEAT",
    "CHIMERA_LLM_MAX_TOTAL_TOKENS",
)
ALLOWED_EVENTS = {"BudgetExceeded"}

ENABLE_RE = re.compile(
    r"fn\s+llm_loop_enabled\s*\(\s*\)\s*->\s*bool\s*\{(.*?)\n\}", re.S
)
WHITELIST_RE = re.compile(r"Some\(\s*\"apply\"\s*\)\s*\|\s*Some\(\s*\"on\"\s*\)")
DERIVE_LINE_RE = re.compile(
    r"#\[derive\(([^\)]*)\)\]\s*\n\s*(?:///[^\n]*\n\s*|//[^\n]*\n\s*)*pub\s+struct\s+LlmLoopOutcome"
)
UNSAFE_CALL_RE = re.compile(r"\.unwrap\(\)|\bexpect\(")
NEXUS_CTOR_RE = re.compile(r"NexusEvent\s*::\s*([A-Za-z0-9_]+)")
DEFAULT_IMPL_RE = re.compile(
    r"impl\s+Default\s+for\s+LlmToolLoopConfig\s*\{.*?fn\s+default\s*\(\s*\)\s*->\s*Self"
    r"\s*\{(.*?)\n    \}",
    re.S,
)
FIELD_DEFAULT_RE = re.compile(r"(\w+)\s*:\s*([0-9_u_a-zA-Z]+)")


def read(root, rel):
    path = os.path.join(root, rel.replace("/", os.sep).replace(os.sep + os.sep, os.sep))
    if not os.path.isfile(path):
        return None
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()


def llm_files(root):
    d = os.path.join(root, LLM_DIR)
    if not os.path.isdir(d):
        return None
    # WHY 递归：`os.listdir` 只覆盖顶层 —— llm/ 一旦长出子目录，里面的 `unwrap()`
    # 与 `NexusEvent::` 构造点会静默逃出 L4/L5，而门仍判绿。selftest 的子目录植入
    # 夹具把这条钉成回归锁（改回 listdir 即红）。返回相对路径，读盘仍走 read(root, rel)。
    found = []
    for dirpath, dirnames, filenames in os.walk(d):
        dirnames[:] = [n for n in dirnames if n not in (".git", "target")]
        for name in filenames:
            if name.endswith(".rs"):
                # rel 必须相对**传入的 root**算：夹具的 root 是 tempdir，若按 ROOT 取
                # 相对路径会得到 `../..` 开头的串，read(root, rel) 就指错文件（假绿）。
                rel = os.path.relpath(os.path.join(dirpath, name), root)
                found.append(rel.replace(os.sep, "/"))
    return sorted(found)


def check_default_off(root):
    """L1：启用是白名单，不是兜底。"""
    text = read(root, RUN_RS)
    if text is None:
        return 2, ["[UNDECIDABLE] commands/run.rs 读不到"], []
    body = ENABLE_RE.search(text)
    if body is None:
        return 2, ["[UNDECIDABLE] 找不到 `fn llm_loop_enabled()` 函数体"], []
    inner = body.group(1)
    findings = []
    if not WHITELIST_RE.search(inner):
        findings.append(
            "[L1] llm_loop_enabled 不再匹配 Some(\"apply\") | Some(\"on\") 白名单 —— "
            "默认关闭承诺被改宽"
        )
    if re.search(r"_\s*=>\s*true", inner):
        findings.append("[L1] llm_loop_enabled 出现 `_ => true` 兜底 —— 默认变成开启")
    return (1 if findings else 0), findings, [inner.strip()[:60]]


def _field_defaults(root):
    """取 LlmToolLoopConfig::default() 体里的 `字段: 字面量` 对。"""
    text = read(root, TOOL_LOOP_RS)
    if text is None:
        return None, None
    block = DEFAULT_IMPL_RE.search(text)
    if block is None:
        return text, {}
    pairs = {}
    for name, raw in FIELD_DEFAULT_RE.findall(block.group(1)):
        pairs[name] = raw
    return text, pairs


def check_limits(root):
    """L2：四重上限存在、非零、且有 env 读取点。"""
    text, defaults = _field_defaults(root)
    if text is None:
        return 2, ["[UNDECIDABLE] llm/tool_loop.rs 读不到"], []
    if defaults is None or not defaults:
        return 2, ["[UNDECIDABLE] 解析不到 LlmToolLoopConfig::default() 体"], []
    findings = []
    for field in LIMIT_FIELDS:
        value = defaults.get(field)
        if value is None:
            findings.append("[L2] 缺上限字段 %s" % field)
            continue
        digits = re.sub(r"[^0-9]", "", value)
        if digits and int(digits) == 0:
            findings.append("[L2] %s 默认值为 0 —— 等于没有上限" % field)
    for env_name in LIMIT_ENVS:
        # 必须匹配**带引号的完整字面量**：裸子串匹配会让 `CHIMERA_LLM_MAX_REPEAT_X`
        # 这类改名的坏例仍被判定为"读取点在"（selftest 第 4 号夹具第一次跑时正是
        # 这样漏掉的——植入缺陷不触发即说明量具有洞，而不是说明代码没问题）。
        if '"%s"' % env_name not in text:
            findings.append("[L2] 缺 env 读取点 %s（上限不可灰度收紧）" % env_name)
    return (1 if findings else 0), findings, sorted(defaults)


def check_stop_not_maskable(root):
    """L3：outcome 不得派生 Default。"""
    text = read(root, CLIENT_RS)
    if text is None:
        return 2, ["[UNDECIDABLE] llm/client.rs 读不到"], []
    line = DERIVE_LINE_RE.search(text)
    if line is None:
        return 2, ["[UNDECIDABLE] 找不到 `pub struct LlmLoopOutcome` 的 derive 行"], []
    derives = line.group(1)
    if "Default" in derives:
        return 1, [
            "[L3] LlmLoopOutcome 派生了 Default —— 漏落 stop 会静默伪装成 Final"
        ], [derives]
    return 0, [], [derives]


def check_no_panic_no_provider(root):
    """L4：库层不 panic；无 provider 走 ConfigError。"""
    files = llm_files(root)
    if files is None:
        return 2, ["[UNDECIDABLE] crates/chimera-cli/src/llm 目录不存在"], []
    hits = []
    for rel in files:
        text = read(root, rel) or ""
        # cfg(test) 段内的 unwrap 属测试惯用法，与 E-5 口径一致：只判生产码
        body = re.split(r"#\[cfg\(test\)\]", text, 1)[0]
        for match in UNSAFE_CALL_RE.finditer(body):
            hits.append("%s:%s" % (os.path.basename(rel), match.group(0)))
    findings = []
    if hits:
        findings.append("[L4] llm 生产码出现 unwrap/expect: %s" % ", ".join(hits))
    composition = read(root, COMPOSITION_RS)
    if composition is None:
        return 2, findings + ["[UNDECIDABLE] composition.rs 读不到"], hits
    non_mca = re.search(
        r"#\[cfg\(not\(feature\s*=\s*\"mca\"\)\)\]\s*fn\s+build_llm_client_from_config"
        r".*?\n\}",
        composition,
        re.S,
    )
    if non_mca is None:
        findings.append(
            "[L4] 找不到非 mca 档 build_llm_client_from_config —— 无 provider 路径失去定义"
        )
    elif "ConfigError" not in non_mca.group(0):
        findings.append("[L4] 非 mca 档未返回 ConfigError（应给可执行提示，不能 panic/空回）")
    return (1 if findings else 0), findings, hits


def check_event_reuse_only(root):
    """L5：llm/ 只复用 BudgetExceeded，不新增变体。"""
    files = llm_files(root)
    if files is None:
        return 2, ["[UNDECIDABLE] crates/chimera-cli/src/llm 目录不存在"], []
    used = set()
    for rel in files:
        text = read(root, rel) or ""
        body = re.split(r"#\[cfg\(test\)\]", text, 1)[0]
        used.update(NEXUS_CTOR_RE.findall(body))
    extra = sorted(used - ALLOWED_EVENTS)
    if extra:
        return 1, [
            "[L5] llm 循环构造了白名单外事件 %s（新增变体须走七点位同批：types/registry/"
            "variant_count_test/doc_count_freeze/…）" % ", ".join(extra)
        ], sorted(used)
    return 0, [], sorted(used)


CHECKS = (
    check_default_off,
    check_limits,
    check_stop_not_maskable,
    check_no_panic_no_provider,
    check_event_reuse_only,
)


def run_all(root):
    results = []
    for fn in CHECKS:
        results.append((fn.__name__,) + fn(root))
    findings = []
    undecidable = False
    red = False
    for _name, status, msgs, _evidence in results:
        findings.extend(msgs)
        if status == 2:
            undecidable = True
        elif status == 1:
            red = True
    if undecidable:
        return 2, findings, results
    return (1 if red else 0), findings, results


# =============================================================================
# selftest：植入坏例 + 边界 + 致盲对照
# =============================================================================
# 夹具面 = 「必需单文件」+「llm/ 目录实况（递归）」。
# WHY 不硬枚举 llm/ 的文件：枚举会把夹具覆盖率钉死在抄写那一刻 —— 新增一个
# `llm/*.rs` 真在生产扫描面里，却不在夹具里，于是"植入项全红、真树也红"而夹具
# 永远测不到新文件。目录实况 + 必需集则相反：多出来的自动进扫描面，缺必需集
# （判据依赖的文件）就响亮报错而不是静默少测。
REQUIRED_FIXTURES = (RUN_RS, COMPOSITION_RS, TOOL_LOOP_RS, CLIENT_RS)


def _fixture_tree(base):
    missing = [
        rel for rel in REQUIRED_FIXTURES
        if not os.path.isfile(os.path.join(ROOT, rel))
    ]
    if missing:
        raise AssertionError("required fixture sources missing: %s" % missing)
    llm_rel = llm_files(ROOT) or []
    if not llm_rel:
        raise AssertionError("llm 目录扫描 0 命中 —— 夹具会测一条不存在的路径")
    for rel in sorted(set(llm_rel) | {RUN_RS, COMPOSITION_RS}):
        src = os.path.join(ROOT, rel)
        dst = os.path.join(base, rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy2(src, dst)


def _write_new(base, rel, text):
    """在夹具里新建文件（连带建父目录）—— 子目录植入夹具专用。"""
    path = os.path.join(base, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def _patch(base, rel, old, new):
    path = os.path.join(base, rel)
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        text = handle.read()
    if old not in text:
        raise AssertionError("fixture anchor missing in %s: %r" % (rel, old[:48]))
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text.replace(old, new, 1))


def _append(base, rel, text):
    path = os.path.join(base, rel)
    with open(path, "a", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def _expect_red(base, want, label):
    rc, findings, results = run_all(base)
    fired = {name for name, status, _m, _e in results if status == 1}
    for name in want:
        if name not in fired:
            return "%s: 植入缺陷未被 %s 抓到（门失去牙口）findings=%s" % (label, name, findings)
    if rc == 2:
        return "%s: 应判红(1)却退 2" % label
    if rc != 1:
        return "%s: 植入缺陷后仍判绿(rc=%d)" % (label, rc)
    return None


def _case(checks, tmp, name, label, want, mutate):
    base = os.path.join(tmp, name)
    os.makedirs(base)
    _fixture_tree(base)
    mutate(base)
    detail = _expect_red(base, want, label)
    checks.append((label, detail is None, detail or ""))


def _case_green(checks, tmp, name, label, mutate):
    """反向边界：这类改动**不该**判红，否则门会在无关改动上狼来了。"""
    base = os.path.join(tmp, name)
    os.makedirs(base)
    _fixture_tree(base)
    mutate(base)
    rc, findings, _ = run_all(base)
    checks.append((label, rc == 0, "rc=%d findings=%s" % (rc, findings)))


def selftest():
    checks = []
    tmp = tempfile.mkdtemp(prefix="llm-loop-gate-")
    try:
        pristine = os.path.join(tmp, "pristine")
        os.makedirs(pristine)
        _fixture_tree(pristine)
        rc, findings, _ = run_all(pristine)
        checks.append((
            "negative control: 未改动夹具应判绿", rc == 0,
            "rc=%d findings=%s" % (rc, findings)))

        _case(checks, tmp, "default-on", "L1 植入: 白名单扩成 _ => true -> 红",
              {"check_default_off"}, lambda b: _patch(
                  b, RUN_RS, 'Some("apply") | Some("on")', 'Some(_) | None => true'))

        _case(checks, tmp, "limit-zero", "L2 植入: max_turns 默认改 0 -> 红",
              {"check_limits"}, lambda b: _patch(
                  b, TOOL_LOOP_RS, "max_turns: 8,", "max_turns: 0,"))

        _case(checks, tmp, "limit-gone", "L2 植入: 删掉一个 env 读取点 -> 红",
              {"check_limits"}, lambda b: _patch(
                  b, TOOL_LOOP_RS, '"CHIMERA_LLM_MAX_REPEAT",', '"CHIMERA_LLM_MAX_REPEAT_X",'))

        _case(checks, tmp, "derive-default", "L3 植入: outcome 派生 Default -> 红",
              {"check_stop_not_maskable"}, lambda b: _patch(
                  b, CLIENT_RS, "#[derive(Debug, Clone)]\npub struct LlmLoopOutcome",
                  "#[derive(Debug, Clone, Default)]\npub struct LlmLoopOutcome"))

        _case(checks, tmp, "unwrap", "L4 植入: llm 生产码出现 unwrap -> 红",
              {"check_no_panic_no_provider"}, lambda b: _patch(
                  b, os.path.join(LLM_DIR, "tools.rs"),
                  "pub fn tool_decls", "pub fn tool_decls_x(v: Option<u8>) -> u8 { v.unwrap() }\n\npub fn tool_decls"))

        _case(checks, tmp, "subdir-unwrap",
              "L4 植入: llm/inner/leak.rs 里的 unwrap -> 红（证明扫描递归）",
              {"check_no_panic_no_provider"}, lambda b: _write_new(
                  b, os.path.join(LLM_DIR, "inner", "leak.rs"),
                  "fn leak(v: Option<u8>) -> u8 { v.unwrap() }\n"))

        _case_green(checks, tmp, "cfg-test-unwrap",
                    "L4 边界: cfg(test) 段里的 unwrap 不判红（与 E-5 口径一致）",
                    lambda b: _append(
                        b, os.path.join(LLM_DIR, "tools.rs"),
                        "\n#[cfg(test)]\nmod planted_tests {\n"
                        "    #[test]\n    fn a() { let _ = Some(1u8).unwrap(); }\n}\n"))

        _case(checks, tmp, "new-event", "L5 植入: 构造白名单外事件 -> 红",
              {"check_event_reuse_only"}, lambda b: _patch(
                  b, TOOL_LOOP_RS, "NexusEvent::BudgetExceeded {",
                  "NexusEvent::ToolCallReported;\n        let _e = NexusEvent::BudgetExceeded {"))

        empty = os.path.join(tmp, "empty")
        os.makedirs(empty)
        rc, _f, _r = run_all(empty)
        checks.append(("缺失输入应退 2（不可判定），不得静默判绿", rc == 2, "rc=%d" % rc))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    failed = 0
    for name, ok, detail in checks:
        print("%-62s %s%s" % (name, "OK" if ok else "FAIL",
                              "" if ok else "  (%s)" % detail))
        if not ok:
            failed += 1
    print("[selftest] %d/%d assertions passed" % (len(checks) - failed, len(checks)))
    return 0 if failed == 0 else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description="ADR-232 LLM loop defaults gate")
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--root", default=ROOT)
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    rc, findings, results = run_all(args.root)
    if args.json:
        print(json.dumps({
            "rc": rc,
            "findings": findings,
            "evidence": {name: evidence for name, _s, _m, evidence in results},
        }, ensure_ascii=False))
    else:
        for name, status, msgs, _e in results:
            print("%-32s %s" % (name, {0: "PASS", 1: "FAIL", 2: "UNDECIDABLE"}[status]))
        for line in findings:
            print(line)
        print("[VERDICT] rc=%d (%s)" % (
            rc, {0: "LLM 循环默认值与底线全绿", 1: "有发现", 2: "不可判定"}[rc]))
    return rc


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
