#!/usr/bin/env python3
# =============================================================================
# structure_displacement_report.py - "结构位移"度量脚本 (v2.3 §(b.6))
# =============================================================================
# Purpose: v2.3 §(b.6) 要求"结构位移报告"——把 ADR / NexusEvent 变体 / Critical 通道
#          / 治理门规则 的**当前规模**分列测出,回答"系统变成了什么"。每个 Wave 结束跑
#          一次即得结构快照,纵向对比即位移量("自己吃自己的狗粮")。
#
# 纯只读静态度量(零 cargo/零网络),输出保持 ASCII(Windows GBK 控制台兼容)。
# 单一真值源仍是代码本身(types.rs / bus.rs / gate_manifest.toml / adr_index),本脚本
# 只做**读取与聚合**,不改任何基线、不做判定(pass/fail)——它是仪表而非门禁,故恒退 0。
#
# Usage: py -3 scripts/structure_displacement_report.py [--json]
# =============================================================================
import argparse
import glob
import json
import os
import re

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPTS)


def _read(path):
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.read()


def event_variants():
    """NexusEvent 变体数:types.rs 内 enum 体顶层 `Variant {` 计数(brace 感知)。"""
    text = _read(os.path.join(ROOT, "crates", "event-bus", "src", "types.rs"))
    start = text.index("pub enum NexusEvent")
    count = 0
    depth = 0
    seen_open = False
    for line in text[start:].splitlines():
        if not seen_open:
            if "{" in line:
                seen_open = True
                depth += line.count("{") - line.count("}")
            continue
        m = re.match(r"^    ([A-Z][A-Za-z0-9]*)\s*[,{]\s*$", line)
        if m:
            count += 1
        depth += line.count("{") - line.count("}")
        if depth <= 0:
            break
    return count


def critical_metrics():
    """从 bus.rs 读 CRITICAL_TOTAL 与 CRITICAL_MPSC_VARIANTS。"""
    text = _read(os.path.join(ROOT, "crates", "event-bus", "src", "bus.rs"))
    total = re.search(r"CRITICAL_TOTAL\s*:\s*usize\s*=\s*(\d+)", text)
    mpsc = re.search(r"CRITICAL_MPSC_VARIANTS\s*:\s*usize\s*=\s*(\d+)", text)
    return (int(total.group(1)) if total else -1,
            int(mpsc.group(1)) if mpsc else -1)


def gate_count():
    text = _read(os.path.join(ROOT, "scripts", "gate_manifest.toml"))
    return len(re.findall(r'(?m)^\s*id\s*=\s*"G-', text))


def adr_metrics():
    files = glob.glob(os.path.join(ROOT, "docs", "architecture", "**", "ADR-*.md"),
                      recursive=True)
    nums = []
    for f in files:
        m = re.search(r"ADR-(\d+)", os.path.basename(f))
        if m:
            nums.append(int(m.group(1)))
    return (len(nums), max(nums) if nums else -1)


def workspace_members():
    text = _read(os.path.join(ROOT, "Cargo.toml"))
    m = re.search(r"(?s)members\s*=\s*\[(.*?)\]\s*\r?\n\s*resolver", text)
    return len(re.findall(r'"crates/', m.group(1))) if m else -1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    total, mpsc = critical_metrics()
    adr_files, adr_max = adr_metrics()
    metrics = {
        "workspace_members": workspace_members(),
        "nexus_event_variants": event_variants(),
        "critical_total": total,
        "critical_mpsc_bypass": mpsc,
        "critical_broadcast_only": (total - mpsc) if (total >= 0 and mpsc >= 0) else -1,
        "governance_gates": gate_count(),
        "adr_files": adr_files,
        "adr_max_number": adr_max,
    }
    if args.json:
        print(json.dumps(metrics, ensure_ascii=False, sort_keys=True))
        return 0
    print("=== Chimera structural-displacement snapshot (v2.3 §(b.6)) ===")
    for k in sorted(metrics):
        print("  %-24s = %d" % (k, metrics[k]))
    print("  (instrument, not a gate: always exit 0; single source of truth = code)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
