#!/usr/bin/env python3
# =============================================================================
# check_tool_plane_contract.py - 工具执行面（ADR-229）四条不可退让判据的门
# =============================================================================
# WHY this gate exists
#   ADR-229 把工具执行面钉成三层倒置：契约在 L0、trait+运行时在 L7、provider 适配器在
#   L10。这个倒置是**编译期抓不到**的那类约束——往 L7 的 Cargo.toml 加一条 chimera-cli
#   依赖照样编译通过（只要不成环），往 tool_plane 里塞一个新 NexusEvent 变体也照样绿。
#   ADR 写"不许"而没有门，等于把不变量交给下一个人的记性。本门把四条判据变成可机检：
#
#   C1 provider 注册名唯一且可发现
#      L10 `tool_plane/*.rs` 里每个 `fn provider_id` 返回的字面量必须互不相同、非空，
#      且总数 >= PROVIDER_FLOOR。"0 命中"不判绿——扫描致盲正是这类门最常见的死法。
#   C2 L7 不得指向 L8-L10
#      `crates/gqep-executor/Cargo.toml` 的 [dependencies] 里出现任何上层成员名即红。
#      依赖铁律的 bash 版（check_dependency_rules.sh）看的是 `cargo metadata` 的解析结果，
#      而 feature-gated / optional 边在 metadata 里形态不同；本门只读 manifest 文本，
#      是**同一判据的第二方法**，两把尺各错各的方向时才需要互相兜底。
#   C3 执行面不得伪造事件
#      `tool_plane/` 内出现 `NexusEvent::` 即红（ADR-229 D5：可观测性走 ToolMetrics +
#      tracing；新变体须走七点位同批，属另一条路径）。
#   C4 默认 off
#      `ToolPlaneMode` 的 `#[default]` 必须贴在 `Off` 上，且 `from_env` 的兜底分支
#      `_ => ToolPlaneMode::Off` 存在。把 default 挪到 Apply 是"灰度变默认"的最短路径，
#      必须当场红，而不是等用户发现 `chimera tool call` 直接执行了。
#
# Exit dialect (shared with gate_rc): 0 全过 / 1 有发现 / 2 输入不可读或扫描致盲
#
# Usage:
#   py -3 scripts/check_tool_plane_contract.py --selftest
#   py -3 scripts/check_tool_plane_contract.py
#   py -3 scripts/check_tool_plane_contract.py --json
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

PROVIDER_FLOOR = 3  # echo / process / mcp —— 少一个就是装配面缩水
TOOL_PLANE_MIN_FILES = 4  # types/registry/scheduler/runtime(+metrics)

# L8-L10 成员（判据源：agents.md §2.1 分层表；只列本仓真实存在的上层 crate 名）
UPPER_LAYERS = {
    "parliament", "decb-governor",
    "pvl-layer", "mtpe-executor", "ssra-fusion", "nexus-subagent",
    "osa-coordinator", "kvbsr-router", "faae-router", "sesa-router", "omega-learner",
    "repo-wiki", "gsoe-evolution",
    "seccore", "qeep-protocol", "decay-engine",
    "scc-cache", "cmt-tiering", "session-store",
    "nmc-encoder", "hcw-window", "mlc-engine",
    "quest-engine", "gea-activator", "efficiency-monitor", "chimera-mas",
    "mas-sched", "nexus-hook",
    "chimera-cli", "chimera-tui", "chtc-bridge", "mcp-mesh", "csn-substitutor",
    "mca-gateway", "nexus-app-server",
}
# L7 自己与允许的下层（gqep-executor 同层 + L0-L6 + L1）——C2 的接受集
ALLOWED_IN_GQEP = {
    "gqep-executor", "nexus-core", "event-bus", "nexus-telemetry",
    "nexus-contracts", "nexus-contracts-core", "nexus-contracts-sparse",
    "nexus-contracts-safety", "nexus-contracts-seams", "nexus-contracts-mca",
    "nexus-contracts-memory",
    "seccore", "qeep-protocol", "decay-engine",  # L4：合法向下边（ADR-048/185）
}

PROVIDER_ID_RE = re.compile(
    r"fn\s+provider_id\s*\(\s*&self\s*\)\s*->\s*&\s*str\s*\{\s*\"([^\"]*)\""
)
NEXUS_EVENT_RE = re.compile(r"NexusEvent\s*::\s*([A-Za-z0-9_]+)")
DEFAULT_ATTR_RE = re.compile(r"#\[default\]\s*(?:///[^\n]*\n\s*|//[^\n]*\n\s*|\s)*([A-Za-z0-9_]+)\s*,")
MODE_ENUM_RE = re.compile(r"pub\s+enum\s+ToolPlaneMode\s*\{(.*?)\n\}", re.S)
FROM_ENV_OFF_RE = re.compile(r"_\s*=>\s*ToolPlaneMode::Off")
DEP_SECTION_RE = re.compile(r"^\[dependencies\](.*?)(?=^\[|\Z)", re.S | re.M)
DEP_NAME_RE = re.compile(r"^\s*([A-Za-z0-9_-]+)\s*=", re.M)


def read(path):
    """返回文本；缺文件由调用方判 2（不可判定），不得当作通过。"""
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()


def rs_of(root, rel_dir):
    """递归列出目录下的 .rs（含子目录）；目录不存在返回 None 由调用方判 2。

    WHY 必须递归：`os.listdir` 只看顶层，一旦 `tool_plane/` 长出子目录，新 provider 的
    注册名与 `NexusEvent::` 构造点会**静默逃出扫描**，而"文件数地板"仍然达标 —— 门会在
    盲区里保持绿色。selftest 的子目录植入夹具把这条钉成回归锁（改回 listdir 即红）。
    """
    d = os.path.join(root, rel_dir.replace("/", os.sep))
    if not os.path.isdir(d):
        return None
    found = []
    for dirpath, dirnames, filenames in os.walk(d):
        dirnames[:] = [n for n in dirnames if n not in (".git", "target")]
        found.extend(
            os.path.join(dirpath, name) for name in filenames if name.endswith(".rs")
        )
    return sorted(found)


def check_providers(root):
    """C1：provider 名唯一、非空、数量不低于地板。"""
    files = rs_of(root, "crates/chimera-cli/src/tool_plane")
    if files is None:
        return 2, ["[UNDECIDABLE] crates/chimera-cli/src/tool_plane 目录不存在"], []
    names = []
    for path in files:
        for match in PROVIDER_ID_RE.finditer(read(path)):
            names.append((match.group(1), os.path.basename(path)))
    if not names:
        return 1, ["[C1] provider_id 扫描 0 命中 —— 夹具/正则致盲或 provider 全失"], names
    dupes = {n for n, _ in names if [x for x, _ in names].count(n) > 1}
    blanks = [n for n, _ in names if not n.strip()]
    findings = []
    if dupes:
        findings.append("[C1] provider 注册名重复: %s" % ", ".join(sorted(dupes)))
    if blanks:
        findings.append("[C1] provider 注册名为空")
    if len(names) < PROVIDER_FLOOR:
        findings.append(
            "[C1] provider 数 %d < 地板 %d（echo/process/mcp 至少三个）"
            % (len(names), PROVIDER_FLOOR)
        )
    return (1 if findings else 0), findings, names


def check_dependency_direction(root):
    """C2：L7 manifest 里不得出现 L8-L10 成员。"""
    manifest = os.path.join(root, "crates", "gqep-executor", "Cargo.toml")
    if not os.path.isfile(manifest):
        return 2, ["[UNDECIDABLE] crates/gqep-executor/Cargo.toml 不存在"], []
    body = DEP_SECTION_RE.search(read(manifest))
    if body is None:
        return 2, ["[UNDECIDABLE] Cargo.toml 里找不到 [dependencies] 段"], []
    deps = DEP_NAME_RE.findall(body.group(1))
    if not deps:
        return 2, ["[UNDECIDABLE] [dependencies] 段解析出 0 条依赖"], []
    bad = sorted(
        d for d in deps
        if d in UPPER_LAYERS and d not in ALLOWED_IN_GQEP
    )
    if bad:
        return 1, ["[C2] L7 gqep-executor 指向 L8-L10: %s" % ", ".join(bad)], deps
    return 0, [], deps


def check_no_fabricated_events(root):
    """C3：执行面不伪造 NexusEvent 变体。"""
    files = rs_of(root, "crates/gqep-executor/src/tool_plane")
    if files is None:
        return 2, ["[UNDECIDABLE] crates/gqep-executor/src/tool_plane 目录不存在"], []
    if len(files) < TOOL_PLANE_MIN_FILES:
        return 1, [
            "[C3] tool_plane 只有 %d 个 .rs（地板 %d）—— 模块被误删会让本门无物可扫"
            % (len(files), TOOL_PLANE_MIN_FILES)
        ], []
    hits = []
    for path in files:
        for match in NEXUS_EVENT_RE.finditer(read(path)):
            hits.append("%s:%s" % (os.path.basename(path), match.group(1)))
    if hits:
        return 1, [
            "[C3] 执行面伪造事件（ADR-229 D5 要求走 ToolMetrics/tracing，新变体须七点位同批）: %s"
            % ", ".join(hits)
        ], hits
    return 0, [], hits


def check_default_off(root):
    """C4：ToolPlaneMode 的 #[default] 必须贴在 Off 上，且 env 兜底是 Off。"""
    config = os.path.join(
        root, "crates", "chimera-cli", "src", "tool_plane", "config.rs"
    )
    if not os.path.isfile(config):
        return 2, ["[UNDECIDABLE] tool_plane/config.rs 不存在"], []
    text = read(config)
    enum_body = MODE_ENUM_RE.search(text)
    if enum_body is None:
        return 2, ["[UNDECIDABLE] 找不到 `pub enum ToolPlaneMode` 定义"], []
    marker = DEFAULT_ATTR_RE.search(enum_body.group(1))
    findings = []
    default_variant = marker.group(1) if marker else None
    if default_variant != "Off":
        findings.append(
            "[C4] `#[default]` 贴在 %r 上而非 Off —— 灰度被写成默认即红线"
            % default_variant
        )
    if not FROM_ENV_OFF_RE.search(text):
        findings.append(
            "[C4] from_env 缺 `_ => ToolPlaneMode::Off` 兜底 —— 非法 env 会被当成启用"
        )
    return (1 if findings else 0), findings, [default_variant]


def run_all(root):
    """四条判据合并裁决；任何一条 2 即整体不可判定（退 2），否则按红/绿。"""
    results = []
    for fn in (
        check_providers,
        check_dependency_direction,
        check_no_fabricated_events,
        check_default_off,
    ):
        results.append((fn.__name__,) + fn(root))
    findings = []
    undecidable = False
    red = False
    for name, status, msgs, _evidence in results:
        findings.extend(msgs)
        if status == 2:
            undecidable = True
        elif status == 1:
            red = True
    if undecidable:
        return 2, findings, results
    return (1 if red else 0), findings, results


# =============================================================================
# selftest：植入坏例证明每条判据真的会红（0 命中即绿的门没有价值）
# =============================================================================
def _fixture_tree(base):
    """把真实树的相关文件复制进 base，形如可独立改形的夹具。"""
    pairs = [
        "crates/chimera-cli/src/tool_plane",
        "crates/gqep-executor/src/tool_plane",
        "crates/gqep-executor/Cargo.toml",
    ]
    for rel in pairs:
        src = os.path.join(ROOT, rel.replace("/", os.sep))
        dst = os.path.join(base, rel.replace("/", os.sep))
        if os.path.isdir(src):
            shutil.copytree(src, dst)
        elif os.path.isfile(src):
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copy2(src, dst)
        else:
            raise RuntimeError("夹具源缺失: %s" % rel)


def _patch(base, rel, old, new):
    path = os.path.join(base, rel.replace("/", os.sep))
    text = read(path)
    if old not in text:
        # 锚点缺失 = 夹具与被测码脱钩，必须炸而不能静默改不到
        raise AssertionError("fixture anchor missing in %s: %r" % (rel, old[:40]))
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text.replace(old, new, 1))


def _write_new(base, rel, text):
    """在夹具里新建文件（连带建父目录）—— 子目录植入夹具专用。"""
    path = os.path.join(base, rel.replace("/", os.sep))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def _expect(base, want_red_ids, label):
    """在夹具上跑一次，断言"指定判据必须判红"。返回 None=夹具通过。"""
    rc, findings, results = run_all(base)
    fired = {name for name, status, _m, _e in results if status == 1}
    for want in want_red_ids:
        if want not in fired:
            return "%s: 植入缺陷未被 %s 抓到（门失去牙口）findings=%s" % (
                label, want, findings)
    if rc == 2:
        return "%s: 应为判红(1)却退 2 —— 输入被误读为不可判定" % label
    if rc != 1:
        return "%s: 植入缺陷后仍判绿(rc=%d)" % (label, rc)
    return None


def _case(checks, tmp, name, want_red_ids, label, mutate):
    """建夹具 -> 施改 -> 断言红，一次跑完只算一遍。"""
    base = os.path.join(tmp, name)
    os.makedirs(base)
    _fixture_tree(base)
    mutate(base)
    checks.append((label, *_record(_expect(base, want_red_ids, label))))


def _record(detail):
    return (detail is None, detail or "")


def selftest():
    checks = []
    tmp = tempfile.mkdtemp(prefix="toolplane-gate-")
    try:
        # 0) 负对照：原样复制必须全绿，否则"红"就无从归因
        pristine = os.path.join(tmp, "pristine")
        os.makedirs(pristine)
        _fixture_tree(pristine)
        rc, findings, _ = run_all(pristine)
        checks.append((
            "negative control: 未改动夹具应判绿",
            rc == 0,
            "rc=%d findings=%s" % (rc, findings),
        ))

        # 1) C1 重复注册名（proc 改成 echo 撞车）
        _case(checks, tmp, "dup-provider", {"check_providers"},
              "C1 植入: provider 名与 echo 撞车 -> 红",
              lambda base: _patch(
                  base, "crates/chimera-cli/src/tool_plane/process.rs",
                  '"proc"', '"echo"'))

        # 1b) 子目录里的撞名 provider —— 扫描面若不递归，它静默逃检（真实盲区）
        _case(checks, tmp, "subdir-provider", {"check_providers"},
              "C1 植入: 子目录 providers/dup.rs 撞名 -> 红（证明扫描递归）",
              lambda base: _write_new(
                  base, "crates/chimera-cli/src/tool_plane/providers/dup.rs",
                  "#[async_trait]\nimpl ToolProvider for Dup {\n"
                  '    fn provider_id(&self) -> &str {\n        "echo"\n    }\n}\n'))

        # 2) C1 provider 全消失（扫描致盲的正面对照）
        def _kill_pids(base):
            for name in ("echo.rs", "process.rs", "mcp.rs"):
                _patch(base, "crates/chimera-cli/src/tool_plane/" + name,
                       "fn provider_id", "fn provider_id_disabled")
        _case(checks, tmp, "no-provider", {"check_providers"},
              "C1 植入: provider_id 全部改名 -> 0 命中即红", _kill_pids)

        # 3) C2 加一条 L10 依赖
        _case(checks, tmp, "bad-dep", {"check_dependency_direction"},
              "C2 植入: L7 依赖 chimera-cli -> 红",
              lambda base: _patch(
                  base, "crates/gqep-executor/Cargo.toml", "[dependencies]",
                  '[dependencies]\nchimera-cli = { path = "../chimera-cli" }'))

        # 4) C3 伪造事件（植的是"构造点"而非裸 import —— 判据扫的是 `NexusEvent::`，
        #    只加 import 不该判红，否则夹具会教出门本身没有的牙）
        _case(checks, tmp, "fake-event", {"check_no_fabricated_events"},
              "C3 植入: tool_plane 构造 NexusEvent -> 红",
              lambda base: _patch(
                  base, "crates/gqep-executor/src/tool_plane/metrics.rs",
                  "use std::sync::atomic::{AtomicU64, Ordering};",
                  "use std::sync::atomic::{AtomicU64, Ordering};\n\n"
                  "#[allow(dead_code)]\n"
                  "fn planted() -> usize {\n"
                  "    let _e = NexusEvent::ToolCalled;\n    0\n}"))

        # 4b) C3 的反向边界：只加 import（无构造点）不应判红，否则门会在真正
        #     无关的改动上狼来了，红会被当成噪声
        import_only = os.path.join(tmp, "import-only")
        os.makedirs(import_only)
        _fixture_tree(import_only)
        _patch(import_only, "crates/gqep-executor/src/tool_plane/metrics.rs",
               "use std::sync::atomic::{AtomicU64, Ordering};",
               "use event_bus::NexusEvent; // import only\n"
               "use std::sync::atomic::{AtomicU64, Ordering};")
        rc_import, f_import, _ = run_all(import_only)
        checks.append((
            "C3 边界: 只有 import 无构造点 -> 不判红",
            rc_import == 0,
            "rc=%d findings=%s" % (rc_import, f_import),
        ))

        # 5) C4 默认档被挪到 Apply
        def _move_default(base):
            _patch(base, "crates/chimera-cli/src/tool_plane/config.rs",
                   "#[default]\n    Off,", "Off,")
            _patch(base, "crates/chimera-cli/src/tool_plane/config.rs",
                   "    /// Execute for real.\n    Apply,",
                   "    /// Execute for real.\n    #[default]\n    Apply,")
        _case(checks, tmp, "default-apply", {"check_default_off"},
              "C4 植入: #[default] 移到 Apply -> 红", _move_default)

        # 6) C4 兜底分支被改（非法 env 会被当成启用）
        _case(checks, tmp, "no-fallback", {"check_default_off"},
              "C4 植入: 非法 env 兜底改成 Apply -> 红",
              lambda base: _patch(
                  base, "crates/chimera-cli/src/tool_plane/config.rs",
                  "            _ => ToolPlaneMode::Off,",
                  "            _ => ToolPlaneMode::Apply,"))

        # 7) 不可判定：整目录缺失必须退 2 而不是判绿
        empty = os.path.join(tmp, "empty")
        os.makedirs(empty)
        rc, _f, _r = run_all(empty)
        checks.append(("缺失输入应退 2（不可判定），不得静默判绿", rc == 2, "rc=%d" % rc))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    failed = 0
    for name, ok, detail in checks:
        print("%-58s %s%s" % (name, "OK" if ok else "FAIL",
                              "" if ok else "  (%s)" % detail))
        if not ok:
            failed += 1
    print("[selftest] %d/%d assertions passed" % (len(checks) - failed, len(checks)))
    return 0 if failed == 0 else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description="ADR-229 tool plane contract gate")
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--root", default=ROOT)
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    rc, findings, results = run_all(args.root)
    if args.json:
        payload = {
            "rc": rc,
            "findings": findings,
            "evidence": {name: evidence for name, _s, _m, evidence in results},
        }
        print(json.dumps(payload, ensure_ascii=False))
    else:
        for name, status, msgs, _evidence in results:
            print("%-32s %s" % (name, {0: "PASS", 1: "FAIL", 2: "UNDECIDABLE"}[status]))
        for line in findings:
            print(line)
        print("[VERDICT] rc=%d (%s)" % (
            rc, {0: "tool plane 四条判据全绿", 1: "有发现", 2: "不可判定"}[rc]))
    return rc


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
