#!/usr/bin/env python3
"""check_bus_shard_hotspot.py -- the sharded-bus hot-shard facts are frozen and silent sends stay singleton (G-89).

Why this script exists (round-3 audit F-P3 + F-A2; plan T0.3):
    Two structural facts of the ShardedBus were only recorded in prose and were
    one careless edit away from silently becoming FALSE assumptions:

    1. HOT SHARD BY CONSTRUCTION. The shard key is FNV-1a over
       `event.type_name()` (shard.rs scan_shard region) -- instance-blind, so
       every event of the hottest type deterministically piles into ONE of the
       64 shards (the code admits it at bus.rs "同 type 事件确定性地填满同片"),
       and the 500K msg/s bench gate publishes exactly one type
       (QuestCreated). Anyone tuning shard count/capacity, or "fixing" the key,
       must confront that the baseline numbers were measured under the hot-shard
       regime. This gate freezes: key derivation mentions type_name + fnv1a,
       DEFAULT_SHARD_COUNT == 64, SHARD_CAPACITY == 256.

    2. SILENT SEND SINGLETON. shard.rs merges via `let _ = sender.send(event)`
       and bumps merged_total regardless of Err(Closed)/Err(Lagged) -- the
       equality `sharded_total == merged_total` therefore proves "entered the
       merge channel", NOT delivery (F-A2). Exactly one such line is tolerated,
       and it must sit next to its justification text. A second silent send --
       or the justification disappearing -- is a finding.

Boundary registered honestly (do not oversell the gate):
    The gate freezes TEXT MARKERS of the current algorithm. Replacing the
    algorithm will usually change those markers and then requires editing the
    EXPECT block below -- the forced review IS the gate's value; it is not a
    semantic proof that the new key is hot-shard-free.

Semantics: 0 == frozen facts hold / 1 any drift / 2 inputs unreadable.
Usage: python scripts/check_bus_shard_hotspot.py [--root crates] [--json] [--selftest]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

SHARD_FILE = "event-bus/src/shard.rs"

# ---- frozen expectations (edit ONLY together with a bench re-calibration) ----
EXPECT_KEY_MARKERS = (r"\bfnv1a\b", r"\btype_name\s*\(\s*\)")
EXPECT_CONST_LINES = (
    r"pub\s+const\s+DEFAULT_SHARD_COUNT\s*:\s*usize\s*=\s*64\s*;",
    r"pub\s+const\s+SHARD_CAPACITY\s*:\s*usize\s*=\s*256\s*;",
)
SILENT_SEND_RE = re.compile(r"let\s+_\s*=\s*sender\.send\s*\(")
SILENT_SEND_MAX = 1
# justification tokens allowed within +-6 lines of the singleton silent send;
# "不视为错误" is today's wording, "ADR-" becomes acceptable after T1.1 lands
# the Lagged counter (both directions keep the line consciously reviewed).
JUSTIFICATION_TOKENS = ("不视为错误", "ADR-")


def load_shard(crates_root: Path) -> list[str] | None:
    p = crates_root / SHARD_FILE
    try:
        return p.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return None


def evaluate(lines: list[str]) -> list[dict]:
    text = "\n".join(lines)
    findings: list[dict] = []

    for marker in EXPECT_KEY_MARKERS:
        if not re.search(marker, text):
            findings.append({
                "kind": "HOTSHARD_MARKER_GONE", "key": marker,
                "detail": "shard key algorithm markers changed -- re-derive: is the key "
                          "still instance-blind (hot shard by construction)? then update "
                          "EXPECT_* knowingly and re-run the 500K msg/s calibration",
            })
    for const_re in EXPECT_CONST_LINES:
        if not re.search(const_re, text):
            findings.append({
                "kind": "CONSTANT_DRIFT", "key": const_re,
                "detail": "frozen shard geometry constant edited -- bench baselines "
                          "(64 x 256, hot-shard single-type) no longer describe reality",
            })

    hit_idx = [i for i, l in enumerate(lines) if SILENT_SEND_RE.search(l)]
    if len(hit_idx) > SILENT_SEND_MAX:
        findings.append({
            "kind": "SILENT_SEND_PROLIFERATION",
            "key": f"{len(hit_idx)} sites",
            "detail": "more than one `let _ = sender.send(` -- every silent drop on the "
                      "merge path erodes `sharded_total == merged_total` as an evidence "
                      "equation (F-A2); route drops through shard_lagged_total instead",
        })
    for i in hit_idx:
        window = lines[max(0, i - 6):i + 3]
        if not any(any(tok in w for tok in JUSTIFICATION_TOKENS) for w in window):
            findings.append({
                "kind": "SILENT_SEND_UNJUSTIFIED", "key": f"line {i + 1}",
                "detail": f"silent send lost its justification ({'/'.join(JUSTIFICATION_TOKENS)})",
            })
    return findings


def selftest() -> int:
    good = [
        "fn pick(event: &NexusEvent) -> usize { fnv1a(event.type_name()) % DEFAULT_SHARD_COUNT }",
        "pub const DEFAULT_SHARD_COUNT: usize = 64;",
        "pub const SHARD_CAPACITY: usize = 256;",
        "// 无订阅者丢弃不视为错误",
        "let _ = sender.send(event);",
    ]
    problems: list[str] = []
    if evaluate(good):
        problems.append(f"t1 coherent fixture must be green: {evaluate(good)}")

    if not any(f["kind"] == "HOTSHARD_MARKER_GONE"
               for f in evaluate([l.replace("type_name()", "event.instance_id()") for l in good])):
        problems.append("t2 instance-aware key rewrite must be caught")
    if not any(f["kind"] == "CONSTANT_DRIFT"
               for f in evaluate([l.replace("= 64;", "= 128;") for l in good])):
        problems.append("t3 shard-count change must be caught")
    if not any(f["kind"] == "SILENT_SEND_PROLIFERATION"
               for f in evaluate(good + ["let _ = sender.send(other);"])):
        problems.append("t4 second silent send must be caught")
    if not any(f["kind"] == "SILENT_SEND_UNJUSTIFIED"
               for f in evaluate([l for l in good if "不视为错误" not in l])):
        problems.append("t5 removed justification must be caught")

    if problems:
        for p in problems:
            print(f"FAIL  {p}")
        print("SELFTEST: FAILED")
        return 1
    print("SELFTEST: PASSED (markers/geometry/silent-send, both directions)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default="crates")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    lines = load_shard(Path(args.root))
    if lines is None:
        print(f"FAIL  cannot read {SHARD_FILE}")
        return 2
    findings = evaluate(lines)
    if args.json:
        print(json.dumps({"fail": findings}, indent=2))
    else:
        print("== sharded-bus hotspot facts & silent-send singleton (G-89) ==")
        print("  OK    key markers (fnv1a x type_name), geometry 64 x 256, "
              "silent send <= 1 and justified" if not findings else "")
        for f in findings:
            print(f"  [{f['kind']}] {f['key']} -- {f['detail']}")
        if findings:
            print("\n  A frozen hot-shard fact just changed. Re-read F-P3/F-A2 in the\n"
                  "  round-3 audit before bless-ing: capacity/shard-count tuning and the\n"
                  "  500K msg/s gate were calibrated UNDER the hot-shard regime.")
            return 1
    return 1 if findings else 0


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(main))
