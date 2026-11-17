"""W6/W7 后续：把全仓注释里指向已拆分/已删除文件的路径引用归一化。

WHY 必须做：拆分把 `event-bus/src/bus.rs` 变成 `bus/` 子模块树、把 `payloads.rs`
变成 `payloads/` 子模块树、把 `nexus-compute/src/hts.rs` 变成 `hts/` 子模块树。
全仓仍有约 60 处注释写着「见 `bus.rs::is_critical_mpsc_event`」「`bus.rs:272-274`」
这类引用 —— 读者按图索骥会扑空（路径已不存在），且行号已整体位移。

处置原则（对齐本仓既有约定「勿引行号，会漂移」，见
nexus-contracts/src/spec/harness_spec.rs 的同类注释）：
  1. **带符号的路径引用**（`bus.rs::<symbol>`）→ 精确改到该符号**现在所在的文件**；
  2. **不带符号的行号引用**（`bus.rs:123-456`）→ 改到承载该话题的现行文件，
     并**去掉行号**（行号在文件级拆分后必然漂移，留着即错）；
  3. 纯散文里的 `bus.rs` → `bus/mod.rs`（总线本体所在处）。

映射表按「符号 → 现文件」给出，未知符号保守落到 `bus/mod.rs`。
"""
import io
import os
import re
import sys

ROOT = r"D:\Chimera CLI"

# 符号 -> 现文件（相对 crates/event-bus/src/）
SYMBOL_FILE = {
    "is_critical_mpsc_event": "bus/mod.rs",
    "CRITICAL_MPSC_VARIANTS": "bus/mod.rs",
    "CRITICAL_TOTAL": "bus/mod.rs",
    "LANE_FORBIDDEN_SHARD": "bus/mod.rs",
    "DEFAULT_CAPACITY": "bus/mod.rs",
    "EventBus": "bus/mod.rs",
    "send_critical_mpsc": "bus/subscribe.rs",
    "subscribe_critical_events": "bus/subscribe.rs",
    "subscribe": "bus/subscribe.rs",
    "serialize_msgpack": "bus/codec.rs",
    "deserialize_msgpack": "bus/codec.rs",
    "serialize_json": "bus/codec.rs",
    "deserialize_json": "bus/codec.rs",
    "publish": "bus/publish.rs",
    "publish_blocking": "bus/publish.rs",
    "publish_batch": "bus/publish.rs",
    "publish_critical": "bus/publish.rs",
    "publish_critical_blocking": "bus/publish.rs",
    "publish_batch_blocking": "bus/publish.rs",
    "publish_claim": "bus/publish.rs",
    "publish_certify": "bus/publish.rs",
    "publish_declaim": "bus/publish.rs",
    "dispatch_batch": "bus/publish.rs",
    "dispatch_one_impl": "bus/publish.rs",
    "enable_sharding": "bus/shard_ctl.rs",
    "try_enable_sharding": "bus/shard_ctl.rs",
    "shutdown_sharding": "bus/shard_ctl.rs",
    "sharding_enabled": "bus/shard_ctl.rs",
    "shadow_stats": "bus/shard_ctl.rs",
    "try_shard_publish": "bus/shard_ctl.rs",
}

# 带符号的引用：bus.rs::<symbol>  /  bus.rs::<symbol>::<item>
PAT_SYM = re.compile(r"bus\.rs::([A-Za-z_][A-Za-z0-9_]*)")
# 不带符号的行号引用：bus.rs:123 或 bus.rs:123-456 / bus.rs:123/456
PAT_NUM = re.compile(r"bus\.rs:(\d+)(?:\s*[-–]\s*(\d+))?(?:/(\d+))?")
# 纯散文：bus.rs
PAT_PLAIN = re.compile(r"(?<![A-Za-z0-9_/])bus\.rs(?![A-Za-z0-9_])")

# 只处理 .rs 文件里的注释行（保守：按行判定是否含注释符号）
def is_comment_line(line):
    s = line.strip()
    return s.startswith("//") or s.startswith("///") or s.startswith("//!") or "///" in s or "//" in s


changed_files = []
total_hits = 0
for dirpath, dirnames, filenames in os.walk(ROOT):
    dirnames[:] = [d for d in dirnames if d not in (".git", "target", "tmp", "node_modules")]
    for fn in filenames:
        if not fn.endswith(".rs"):
            continue
        path = os.path.join(dirpath, fn)
        with io.open(path, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
        if "bus.rs" not in text and "payloads.rs" not in text and "hts.rs" not in text:
            continue
        lines = text.split("\n")
        hits = 0
        for i, line in enumerate(lines):
            if not is_comment_line(line):
                continue
            orig = line
            # 1) 带符号
            def repl_sym(m):
                sym = m.group(1)
                return SYMBOL_FILE.get(sym, "bus/mod.rs") + "::" + sym
            line = PAT_SYM.sub(repl_sym, line)
            # 2) 行号引用 -> 去行号，落到 bus/mod.rs
            line = PAT_NUM.sub(lambda m: "bus/mod.rs", line)
            # 3) 纯散文
            line = PAT_PLAIN.sub("bus/mod.rs", line)
            # 4) payloads.rs -> payloads/mod.rs（facade，类型转出点）
            line = line.replace("payloads.rs", "payloads/mod.rs")
            # 5) hts.rs -> hts/mod.rs
            line = line.replace("nexus-compute/src/hts.rs", "nexus-compute/src/hts/mod.rs")
            line = line.replace("compute/hts.rs", "compute/hts/mod.rs")
            if line != orig:
                lines[i] = line
                hits += 1
        if hits:
            with io.open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write("\n".join(lines))
            rel = os.path.relpath(path, ROOT).replace("\\", "/")
            changed_files.append((rel, hits))
            total_hits += hits

for rel, n in sorted(changed_files):
    print("%-62s %d 行" % (rel, n))
print("共修改 %d 个文件、%d 行注释" % (len(changed_files), total_hits))
