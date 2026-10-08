#!/usr/bin/env python3
# =============================================================================
# check_write_sensitive_instrumentation.py - "写入敏感插桩覆盖门" (v2.3 §(b.5))
# =============================================================================
# Purpose: v2.3 §(b.5) 要求敏感扫描"逐入口插桩",并"新增门纳入 gate_manifest"。
#          本门核验**已承诺的 tier-1(记忆/上下文/会话平面,见决策#2)写点确已插桩**
#          ——即对应源文件引用 `nexus_contracts::sensitive::scan`。防止插桩被无声摘除
#          (回潮为"写了不跑/接了又拆")。tier-2(全量持久面 ~15 crate)随 Wave 2/3
#          逐点接入后追加到 REQUIRED_INSTRUMENTED 清单即可。
#
# 与 check_test_surface_liveness / 各 parity 门同范式:清单式、静态、只读、ASCII 输出。
# 退出码 0=全部插桩在位 / 1=有承诺写点缺失 / 2=配置错(清单文件读不到)。
# Usage: py -3 scripts/check_write_sensitive_instrumentation.py
# =============================================================================
import os
import sys

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPTS)

# tier-1 承诺写点(决策#2:记忆/上下文/会话平面)→ 必须引用敏感扫描的文件。
REQUIRED_INSTRUMENTED = [
    "crates/mlc-engine/src/l3_procedural.rs",       # L3 程序记忆落库
    "crates/session-store/src/segment.rs",         # 会话段 JSONL 追加
    "crates/repo-wiki/src/store.rs",               # 知识条目 insert
    "crates/quest-engine/src/checkpoint.rs",       # 核心 Checkpoint 落盘(R3-1 补入)
    "crates/scc-cache/src/wal.rs",                 # 推测缓存 WAL
    "crates/event-bus/src/critical_sink.rs",       # Critical WAL 审计落盘
    "crates/cmt-tiering/src/warm.rs",              # 能力 Warm 层 SQLite 落库
    "crates/cmt-tiering/src/cold.rs",              # 能力 Cold 层 SQLite 落库
    "crates/cmt-tiering/src/ice.rs",               # 能力 Ice 层归档落盘
    "crates/mcp-mesh/src/quantum/wal.rs",           # 2PC 事务 WAL JSONL
    # v2.3 §(b.5) tier-2(R3-1 全量持久面逐点接入):会话平面 LLM 轮次落库
    # (携带用户粘贴的 prompt/response 自由文本 = 真实敏感面)。
    "crates/mca-gateway/src/session.rs",            # MCA 会话轮次 SQLite 落库
    # === v3.4 W5-1 R-1:tier-2 全量持久面实勘接入(含自由文本且未插桩者)===
    # 推翻 ADR-194:34「mca-gateway 为唯一真实敏感文本面」——下列三处经
    # 大小写敏感全信号集实勘确认为用户/模型可控原文落盘面。
    "crates/cmt-tiering/src/experience_card_storage.rs",  # token_evidence.payload(工具入参/结果原文)
    "crates/chimera-tui/src/types.rs",                    # 退出时整只 TuiState 落 YAML(用户输入/会话事件)
    "crates/chimera-tui/src/data/snapshot.rs",            # /export CSV+JSON 落 Quest.title/Task.description
]

# 豁免登记(v3.4 §2 C-29/C-30/C-31 证据链;不计入 REQUIRED_INSTRUMENTED,
# 但理由必须在此显式留痕,防“静默省略”被后续读作“未甄别”)。
#
# 1) crates/session-store/src/tree.rs —— 条件豁免:等效覆盖-依赖 writer 编排。
#    events.payload = rmp_serde::to_vec_named(SessionEvent)(tree.rs:302)承载原文,
#    但唯一生产调用方 writer.rs:312 位于 writer.rs:276 append_batch(内含
#    segment.rs:225 scan,命中即 rollback_batch + 报错)之后,敏感事件不达 SQLite。
#    约束:TreeIndex::insert_events 为 pub,任何新调用方直连即绕过守卫,须自行插桩。
# 2) crates/repo-wiki/src/fts.rs —— 豁免:FTS 行仅由 store.rs 写线程在 scan
#    (store.rs 两条入口均在 send(WriteOp::*) 之前)之后派生写入。
# 3) crates/event-bus/src/attribution.rs —— 豁免:JSONL 字段为 ts/trigger/fingerprint/
#    relation/delta;trigger 取 event.type_name() 且被测试锁定为常量表,无事件 payload。
# 4) crates/quest-engine/src/wal.rs —— 条件豁免:孤儿源文件(lib.rs 无 mod wal
#    亦无 #[path]),不参与编译;一旦接线必须同时纳入本清单。
# 5) experience_card_storage.rs 的 experience_cards 表 —— 表级豁免:upsert_card
#    只落 error_hash/error_type,不落 ErrorSignature.error_summary(同文件的
#    token_evidence 表已入 REQUIRED)。
# 6) omega-learner / hcw-window / decb-governor / parliament / nexus-hook /
#    mas-sched / chimera-mas / nexus-app-server —— 零持久面:大小写敏感全信号
#    集实测零磁盘/零 DB 写(rusqlite 仅存在于 7 crate,SQL 面有界可穷举)。
MARKER = "nexus_contracts::sensitive::scan"


def main():
    missing = []
    unreadable = []
    for rel in REQUIRED_INSTRUMENTED:
        path = os.path.join(ROOT, *rel.split("/"))
        if not os.path.isfile(path):
            unreadable.append(rel)
            continue
        with open(path, encoding="utf-8", errors="replace") as fh:
            if MARKER not in fh.read():
                missing.append(rel)
    if unreadable:
        for rel in unreadable:
            print("[CONFIG] 承诺写点文件缺失: %s" % rel)
        return 2
    print("=== write sensitive-instrumentation coverage (v2.3 §(b.5)) ===")
    print("  required=%d instrumented=%d" % (
        len(REQUIRED_INSTRUMENTED), len(REQUIRED_INSTRUMENTED) - len(missing)))
    if missing:
        for rel in missing:
            print("  [GAP-W] %s 未引用 %s(承诺写点插桩缺失/被摘除)" % (rel, MARKER))
        print("  修法:在该写点落库前接入 nexus_contracts::sensitive::scan(参照 mlc L3)。")
        return 1
    print("  [OK] 全部承诺写点(含 tier-2 扩面)均已插桩敏感扫描")
    return 0


if __name__ == "__main__":
    sys.exit(main())
