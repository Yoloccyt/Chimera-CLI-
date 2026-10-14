#!/usr/bin/env python3
# =============================================================================
# check_write_sensitive_instrumentation.py - "写入敏感插桩覆盖门" (v2.3 §(b.5))
# =============================================================================
# Purpose: v2.3 §(b.5) 要求敏感扫描"逐入口插桩",并"新增门纳入 gate_manifest"。
#          本门核验**已承诺的 tier-1(记忆/上下文/会话平面,见决策#2)写点确已插桩**。
#
# 判据强度(v3.5 P-5 起为双层,缺一即失守):
#   [F] 文件级:清单里每个源文件必须引用 `nexus_contracts::sensitive::scan`
#       ——防"整文件插桩被无声摘除"。
#   [E] 入口级:每个 **可携带自由文本的 pub 写入口**(体内有写信号 + 签名参数类型
#       携带文本面)必须满足二者之一:体内自带 scan,或在 ENTRY_EXEMPT 里带证据豁免。
#
# WHY 必须升到入口级(实测盲区,不是推测):
#   旧判据只要 MARKER 在文件里出现一次就整文件放行。mlc-engine `l3_procedural.rs`、
#   cmt-tiering `warm.rs`、`cold.rs` 的 **批量写入口 `insert_batch` 因此藏了三个批次**
#   ——单条 `insert` 有 scan 而批量没有,文件级门一直是绿的。本门升入口级当日即抓出
#   这三处(守卫短路后新增测必翻红,夹具记录见 tmp/p5_teeth.py)。
#   教训:字符串存在性 != 覆盖面。
#
# WHY 入口集合只取 pub*(不含私有 helper):
#   "逐入口"的"入口"= crate 边界外可调用的写点。实测 repo-wiki/src/store.rs 的架构是
#   "pub 入口先 scan,再经 channel 交给写线程的私有 writer_* 落盘"——把私有 writer
#   计入判据会把 7 处"已扫描后再落盘"的正常架构误判成缺口(双扫并不更安全,只会把
#   守卫逼到下游)。代价如实登记:新增一个"签名非文本面"的 pub 写入口(如
#   `fn put(&self, id: u64)` 内部拼原文)本门看不见,须靠契约复审——不是已解决项。
#
# 退出码: 0=全部写入口已插桩或已带证据豁免 / 1=有未覆盖或豁免失效(STALE)/ 2=不可判定
#         (承诺文件读不到、函数体花括号失衡、豁免表格式非法)。
# Usage: py -3 scripts/check_write_sensitive_instrumentation.py [--selftest]
# =============================================================================
import os
import re
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

# 文件级豁免登记(理由与证据链在此显式留痕,防"静默省略"被后续读作"未甄别")。
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
#    token_evidence 表已入 REQUIRED)。v3.5 P-5 起该口径由 ENTRY_EXEMPT 的 store 行
#    机械化承载,不再只靠这段注释。
# 6) omega-learner / hcw-window / decb-governor / parliament / nexus-hook /
#    mas-sched / chimera-mas / nexus-app-server —— 零持久面:大小写敏感全信号
#    集实测零磁盘/零 DB 写(rusqlite 仅存在于 7 crate,SQL 面有界可穷举)。
MARKER = "nexus_contracts::sensitive::scan"

# ---------------------------------------------------------------------------
# [E] 入口级豁免表(每条必须有 category + 指到具体 SQL/字段的证据文本)。
#     两类 category 的机械核验强度不同:
#       UPSTREAM-GUARDED —— 本入口不扫描,但委托给同文件另一个已扫描入口。本门核验
#           三件事:委托目标存在、目标体内自带 sensitive::scan、本入口体内确实调用了
#           目标。任一失效即判红(防"上游后来不扫了"或"豁免写错位置")。
#       STRUCTURAL-WRITE —— 本入口的写语句不携带自由文本面(DDL/PRAGMA、只翻标志位、
#           按 id 删除、更新时间戳与计数)。理由必须指到具体 SQL/字段清单,否则等于
#           把真缺口写成豁免(净降防,Spec 风险登记 R-7 明写的正是这一类)。
#     静态判据看不到"字段语义将来会不会变",所以每条理由都带行号/列清单证据:
#     契约变更时必须复审本表。这是登记债,不是已完成项。
# ---------------------------------------------------------------------------
EXEMPT_CATEGORIES = ("UPSTREAM-GUARDED", "STRUCTURAL-WRITE")

ENTRY_EXEMPT = [
    {
        "rel": "crates/session-store/src/segment.rs",
        "fn": "append",
        "category": "UPSTREAM-GUARDED",
        "delegates_to": "append_batch",
        "reason": "append 体只做 self.append_batch(slice::from_ref(ev))(segment.rs:178),"
                  "扫描在委托目标 append_batch 里(命中即 rollback_batch + 报错);同一条 JSONL"
                  "行不会被写两次,故本入口不重复插桩。委托关系由本门机械核验。",
    },
    {
        "rel": "crates/repo-wiki/src/store.rs",
        "fn": "delete",
        "category": "STRUCTURAL-WRITE",
        "reason": "DELETE FROM entries WHERE entry_id=?1 —— 只传 Uuid,写语句不携带原文;"
                  "被删行的 content/title 早于其 insert 入口的 scan 就已落盘。",
    },
    {
        "rel": "crates/repo-wiki/src/store.rs",
        "fn": "create_anchor",
        "category": "STRUCTURAL-WRITE",
        "reason": "写线程 writer_create_anchor(store.rs:1632 起)的 INSERT 列清单只有 "
                  "anchor_id/layer/crate_name/entity_id/created_at/updated_at/is_dangling"
                  " —— 全为标识符与元数据,无用户或模型可控文本面。",
    },
    {
        "rel": "crates/repo-wiki/src/store.rs",
        "fn": "resolve_anchor",
        "category": "STRUCTURAL-WRITE",
        "reason": "以 anchor_id(Uuid) 取回**已落盘**条目;懒清理分支只把该锚点行的 "
                  "is_dangling 置真,不新增任何自由文本行。",
    },
    {
        "rel": "crates/scc-cache/src/wal.rs",
        "fn": "new",
        "category": "STRUCTURAL-WRITE",
        "reason": "建连接 + DDL/PRAGMA:wal.rs:314 的 execute 是无参数绑定的建表语句;"
                  "真正的原文载荷由 append() 写入,而 append 自带 scan。",
    },
    {
        "rel": "crates/scc-cache/src/wal.rs",
        "fn": "commit_log",
        "category": "STRUCTURAL-WRITE",
        "reason": "UPDATE wal_entries SET committed = 1 WHERE entry_id = ?1"
                  "(wal.rs:493-496) —— 只翻标志位,不写原文。",
    },
    {
        "rel": "crates/scc-cache/src/wal.rs",
        "fn": "rollback_log",
        "category": "STRUCTURAL-WRITE",
        "reason": "DELETE FROM wal_entries WHERE entry_id = ?1(wal.rs:535-537) —— "
                  "撤销预写记录,无落盘面。",
    },
    {
        "rel": "crates/cmt-tiering/src/warm.rs",
        "fn": "get",
        "category": "STRUCTURAL-WRITE",
        "reason": "UPDATE warm_capabilities SET last_accessed_at=?1, access_count=?2 "
                  "WHERE cap_id=?3(warm.rs:270-275) —— 只写时间戳与计数;content 只读不写。",
    },
    {
        "rel": "crates/cmt-tiering/src/cold.rs",
        "fn": "get",
        "category": "STRUCTURAL-WRITE",
        "reason": "UPDATE cold_capabilities 仅 SET last_accessed_at/access_count"
                  "(cold.rs:324 起的 UPDATE 语句,与 Warm 层同式);content 只读不写。",
    },
    {
        "rel": "crates/mca-gateway/src/session.rs",
        "fn": "open",
        "category": "STRUCTURAL-WRITE",
        "reason": "体内 execute_batch(session.rs:131)的是建表/索引 DDL 常量语句;"
                  "LLM 轮次原文由同文件的 append 类入口写,那些入口自带 scan。",
    },
    {
        "rel": "crates/cmt-tiering/src/experience_card_storage.rs",
        "fn": "store",
        "category": "STRUCTURAL-WRITE",
        "reason": "store 走私有 upsert_card 落 experience_cards 行:operator/execution_status "
                  "为枚举 JSON,metadata 为 CardMetadata(字段仅数值 + skills_used 技能 ID + "
                  "环境版本串,nexus-contracts/src/memory/experience_card.rs:232-243),error 面只落 "
                  "hash/type;本文件的自由文本面是 token_evidence.payload,由 store_evidence_batch "
                  "写且自带 scan(逐条跳过:无事务且 ON CONFLICT DO NOTHING 已使部分落盘成为语义)。",
    },
]

# --- 静态判据(正则;不做 AST,所以"花括号失衡即退 2"而不是猜) -------------
FN_HEAD = re.compile(
    r"^(\s*)(pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+)*fn\s+(\w+)")
CFG_TEST_ATTR = re.compile(r"^\s*#\[cfg\(test\)\]\s*$")
MOD_HEAD = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")
WRITE_SIGNAL = re.compile(
    r"INSERT INTO|UPSERT|ON CONFLICT|execute_batch|\bexecute\(|\.append\(|append_batch|"
    r"write_all|fs::write|writeln!|OPEN_APPEND|File::create|\bsend\(|WriteOp::|flush\("
)
SCAN_CALL = re.compile(r"sensitive::scan\s*\(")
# 签名里"能携带自由文本"的类型/结构体名代理集。实测:把 Message|Turn|Response|Prompt|Text
# 加进来后入口数不变(仍 26),保留宽集是为防新入口漏计,不是为了让旧缺口消失。
TEXTY_PARAM = re.compile(
    r"String|&\s*str\b|Vec\s*<\s*u8\s*>|&\s*\[u8\s*\]|Entry|Card|Event|Payload|Content|"
    r"Snapshot|Message|Turn|Response|Prompt|Text")


def _code_part(line):
    """去掉行内字符串字面量与行注释,只留代码部分(仅供花括号计数)。

    WHY 必须去字符串:实测两处假失衡都是测试里"字符串内带单个花括号"
    (`file_wal_replay_skips_corrupt_trailing_line` / `test_load_corrupted_file`
    要构造截断/损坏行),它们把花括号深度带偏 ⇒ 整个文件被判"失衡"而跳过。
    跳过 = 入口少计 = 静默降防,止于"退 2"仍不算安全:本门对真树的判定会变成
    不可判定。注:信号/签名判定仍用原文(SQL 就在字符串里),不能走这个口径。
    """
    out = []
    i, n = 0, len(line)
    while i < n:
        c = line[i]
        if c == "/" and i + 1 < n and line[i + 1] == "/":
            break                                        # 行注释
        if c == '"':
            raw = i > 0 and line[i - 1] == "r"
            j = i + 1
            hashes = 0
            if raw:
                while j < n and line[j] == "#":
                    hashes += 1
                    j += 1
            close = '"' + "#" * hashes if hashes else '"'
            k = line.find(close, j)
            if k == -1:                                  # 跳行字符串尾巴,保守丢弃
                break
            i = k + len(close)
            continue
        if c == "'":
            # 3 字符内闭环的是字符字面量,其余是生命周期标注(两者都无花括号)
            i += 3 if (i + 2 < n and line[i + 2] == "'") else 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


def _delta(line):
    """该行的花括号净增量(只看代码部分)。"""
    code = _code_part(line)
    return code.count("{") - code.count("}")


def _next_real_line(lines, start):
    """从 start 起跳过空行/注释/属性行,返回第一个真语句行索引(越界则返回 len)。

    写法说明:用 `enumerate` 而不是手写 `j += 1` 循环 —— G-44 入口契约门的 C1 把
    "累加变量直接 return" 当作 rc-as-counter 形状,而行号索引不承载退出码。
    本仓基线是**只减不增**台账,且它自述"修好判据本体优于登记" —— 所以改写法而不是加登记。
    """
    for j, text in enumerate(lines[start:], start):
        t = text.strip()
        if t and not t.startswith("//") and not t.startswith("#"):
            return j
    return len(lines)


def _block_end(lines, start):
    """从 start(必须含 `{`)起到它的配对 `}`。找不到配对则返回 None(调用方不跳)。"""
    if "{" not in _code_part(lines[start]):
        return None
    depth = 0
    for k in range(start, len(lines)):
        depth += _delta(lines[k])
        if depth <= 0:
            return k
    return None


def _skipped_test_regions(lines):
    """返回"只存在于 cfg(test) 编译产物里"的行号集合。

    实测教训(v3.5 P-5 第一版就踩过):`#[cfg(test)]` 不仅修饰 `mod`,也修饰
    **结构体字段与表达式**(session-store/src/segment.rs:112/115/160/162/247/274)。
    若"从 attr 行一路找到下一个 mod 头"就把中间全算作测试段,会把两个 cfg 之间的
    生产写入口(`append`/`append_batch`)吞掉——入口少计 = 判据静默降防。
    现口径:只看"真下一条"是不是 mod 头或 fn 头且同行带 `{`;否则一律不跳。
    保守方向是"多计入口"(假红)而不是"少计"(假绿)。
    """
    regions = set()
    i = 0
    while i < len(lines):
        if CFG_TEST_ATTR.match(lines[i]):
            j = _next_real_line(lines, i + 1)
            if j < len(lines) and (MOD_HEAD.match(lines[j]) or FN_HEAD.match(lines[j])):
                end = _block_end(lines, j)
                if end is not None:
                    regions.update(range(i, end + 1))
                    i = end
                    continue
        i += 1
    return regions


def split_functions(lines, skip):
    """按 fn 头 + 花括号配对切函数。返回 (funcs, imbalanced)。

    funcs      : [(name, is_public, sig, body)]
    imbalanced : 到文件尾花括号仍未闭合的函数名 —— 说明本函数的体被"读长"了,
                 后续函数可能被吞进它的体内(会漏计)。调用方必须退 2(不可判定),
                 不能把"漏计"当成"没有缺口"。
    """
    funcs, imbalanced = [], []
    i = 0
    while i < len(lines):
        if i in skip:
            i += 1
            continue
        m = FN_HEAD.match(lines[i])
        if not m or lines[i].lstrip().startswith("//"):
            i += 1
            continue
        name, is_public = m.group(3), bool(m.group(2))
        j = i
        # 无体声明先截断：trait 方法写 `fn snapshot(&self) -> X;`(分号结尾、无体)。
        # 若不把它当声明处理,解析器会一路找到下一个 `{`,然后花括号永远配不上,
        # 整文件被误判为失衡而静默跳过(=降防)。实测 chimera-tui/data/snapshot.rs。
        while j < len(lines) and "{" not in _code_part(lines[j]) \
                and not lines[j].rstrip().endswith(";"):
            j += 1
        if j >= len(lines):
            imbalanced.append(name)
            break
        if "{" not in _code_part(lines[j]):
            # 声明而非定义:无写体,不计入入口集合,直接从下一行继续扫描。
            funcs.append((name, is_public, "\n".join(lines[i:j + 1]), ""))
            i = j + 1
            continue
        sig = "\n".join(lines[i:j + 1])
        depth, k = 0, j
        while k < len(lines):
            depth += _delta(lines[k])
            if depth == 0:
                break
            k += 1
        if depth != 0:
            imbalanced.append(name)
            funcs.append((name, is_public, sig, "\n".join(lines[i:])))
            break
        funcs.append((name, is_public, sig, "\n".join(lines[i:k + 1])))
        i = k + 1
    return funcs, imbalanced


def _find_entry(table, rel, fn):
    for row in table:
        if row.get("rel") == rel and row.get("fn") == fn:
            return row
    return None


def validate_table(table, errs):
    """豁免表自身的格式闸:类别合法 + 理由足够长且带证据 + 不重复 + 委托必声明目标。"""
    seen = set()
    for row in table:
        key = "%s::%s" % (row.get("rel"), row.get("fn"))
        if key in seen:
            errs.append("豁免表重复条目: %s" % key)
        seen.add(key)
        cat = row.get("category")
        if cat not in EXEMPT_CATEGORIES:
            errs.append("%s 豁免类别非法: %r(合法: %s)"
                        % (key, cat, "/".join(EXEMPT_CATEGORIES)))
        reason = (row.get("reason") or "").strip()
        if len(reason) < 40:
            errs.append("%s 豁免理由过短/缺证据(%d 字,需指到具体 SQL/字段)"
                        % (key, len(reason)))
        if cat == "UPSTREAM-GUARDED" and not row.get("delegates_to"):
            errs.append("%s 声明 UPSTREAM-GUARDED 但未给 delegates_to" % key)
    return errs


def check(root, required, table):
    """核心判据(纯函数,可被 --selftest 用合成 root 驱动)。

    返回 (rc, 输出行列表, stats)。stats 含 entries/covered/exempted/gaps/stale,
    供夹具断言"入口计数本身",而不只是 rc(只看 rc 会漏掉"少计一个入口"的失效)。
    """
    lines, gaps, stale, config = [], [], [], []
    if validate_table(list(table), config):
        lines = ["  [CONFIG] %s" % c for c in config]
        return 2, ["=== write sensitive-instrumentation coverage (v2.3 §(b.5)) ==="] + lines, {}

    entries = covered = exempted = 0
    used = set()
    for rel in required:
        path = os.path.join(root, *rel.split("/"))
        if not os.path.isfile(path):
            config.append("承诺写点文件缺失: %s" % rel)
            continue
        with open(path, encoding="utf-8", errors="replace") as fh:
            src = fh.read()
        if MARKER not in src:                                  # [F] 文件级
            gaps.append("%s :: <file> 未引用 %s(承诺写点插桩缺失/被摘除)" % (rel, MARKER))
            continue
        rows = src.splitlines()
        funcs, imbalanced = split_functions(rows, _skipped_test_regions(rows))
        if imbalanced:
            config.append("%s 函数体花括号失衡(%s)——入口集合可能漏计,判据不可信"
                          % (rel, ",".join(imbalanced)))
            continue
        bodies = dict((n, b) for (n, p, s, b) in funcs)
        for (name, is_public, sig, body) in funcs:
            if not is_public or not WRITE_SIGNAL.search(body) or not TEXTY_PARAM.search(sig):
                continue
            entries += 1
            if SCAN_CALL.search(body):
                covered += 1
                continue
            row = _find_entry(table, rel, name)
            if row is None:
                gaps.append("%s :: %s 是可携带自由文本的写入口,既无 scan 也无豁免" % (rel, name))
                continue
            used.add((rel, name))
            if row["category"] == "UPSTREAM-GUARDED":
                target = row["delegates_to"]
                if target not in bodies or not SCAN_CALL.search(bodies[target]) \
                        or not re.search(r"\b%s\s*\(" % re.escape(target), body):
                    gaps.append("%s :: %s 声明委托 %s,但委托目标缺失/自身无 scan/本入口未调用"
                                % (rel, name, target))
                    continue
            exempted += 1

    for row in table:
        if (row["rel"], row["fn"]) not in used:
            stale.append("%s :: %s 已不再是「未覆盖写入口」"
                         "(函数被删/改名,或守卫已补上)"
                         % (row["rel"], row["fn"]))

    stats = {"entries": entries, "covered": covered, "exempted": exempted,
             "gaps": len(gaps), "stale": len(stale)}
    head = "=== write sensitive-instrumentation coverage (v2.3 §(b.5)) ==="
    body = ["  [F] 文件级 required=%d" % len(required),
            "  [E] 入口级 pub写入口=%d 自带scan=%d 带证据豁免=%d 未覆盖=%d 失效豁免=%d"
            % (entries, covered, exempted, len(gaps), len(stale))]
    if config:
        return 2, [head] + body + ["  [CONFIG] %s" % c for c in config], stats
    for g in gaps:
        body.append("  [GAP-W] %s" % g)
    for s in stale:
        body.append("  [STALE] %s —— 豁免行须删除;留着就会掩盖将来「摘除守卫」的改动" % s)
    if gaps or stale:
        body.append("  修法:在该写入口落库前接入 nexus_contracts::sensitive::scan"
                    "(参照同文件单条入口),或把豁免理由补到 ENTRY_EXEMPT 并指到具体 SQL/字段。")
        return 1, [head] + body, stats
    body.append("  [OK] 每个可携带自由文本的 pub 写入口:自带 scan 或在 ENTRY_EXEMPT 带证据")
    return 0, [head] + body, stats


def main(argv):
    # 入口契约 C3(F24/F29/F35):旗标分派必须有"未知旗标拒收"路径。
    # WHY 不默认降级:把 `--self-tset` 这类拼错读成"跑真树裁决"会给出一个
    # 看似成功、实际跑错面的 rc=0 —— 同一形状在 run_gate_manifest.py 里已专门修过。
    unknown = [a for a in argv if a != "--selftest"]
    if unknown:
        print("[UNDECIDABLE] 未知参数: %s —— 不把拼错的旗标降级成默认裁决" % ",".join(unknown))
        print("用法: py -3 scripts/check_write_sensitive_instrumentation.py [--selftest]")
        return 2
    if "--selftest" in argv:
        return selftest()
    rc, out, _stats = check(ROOT, REQUIRED_INSTRUMENTED, ENTRY_EXEMPT)
    for ln in out:
        print(ln)
    return rc


# ---------------------------------------------------------------------------
# --selftest:全部在临时目录里用合成源码驱动 check(),不碰真树。
# 沿 check_topk_registry / coverage_gate / check_await_across_guard 先例:
# 门必须先证明自己会红,才配拦人。夹具既断言 rc,也断言入口计数(防"少计")。
# ---------------------------------------------------------------------------
_REL = "crates/fake/src/lib.rs"

# 两个独立开关:
#   batch_guard    —— insert_batch 是否自带 scan(旧文件级判据对这两种形状都给绿)
#   delegate_guard —— append_batch(委托目标)是否自带 scan
_SAMPLE = """pub struct Store {
    pub cap: u64,
    /// 测试注入:下一次写入模拟失败(仅 cfg(test) 编译)
%(field_attr)s    fail_write: bool,
}

impl Store {
    /// 测试注入(仅测试编译存在):签名带文本参数 + 体内有写信号。
%(inject_attr)s    pub fn inject_failure(&self, content: String) -> Result<(), StoreError> {
        conn.execute("INSERT INTO t (c) VALUES (?1)", [content])?;
        Ok(())
    }

    pub fn insert(&self, e: Entry) -> Result<(), StoreError> {
        let policy = ScanPolicy::default();
        if nexus_contracts::sensitive::scan(&e.content, &policy).is_blocked() {
            return Err(StoreError("blocked".into()));
        }
        conn.execute("INSERT INTO t (c) VALUES (?1)", [e.content])?;
        Ok(())
    }

    pub fn insert_batch(&self, entries: Vec<Entry>) -> Result<(), StoreError> {
%(batch_guard)s        for e in entries {
            conn.execute("INSERT INTO t (c) VALUES (?1)", [e.content])?;
        }
        Ok(())
    }

    pub fn append_batch(&self, evs: Vec<Entry>) -> Result<(), StoreError> {
%(delegate_guard)s        for e in &evs {
            conn.execute("INSERT INTO t (c) VALUES (?1)", [e.content])?;
        }
        Ok(())
    }

    /// 单条追加:委托 append_batch(与 session-store segment.rs::append 同形状)
    pub fn append_one(&self, ev: Entry) -> Result<(), StoreError> {
        let offs = self.append_batch(vec![ev])?;
        if offs.is_empty() {
            return Err(StoreError("empty".into()));
        }
        Ok(())
    }

    pub fn delete_row(&self, entry_id: String) -> Result<(), StoreError> {
        conn.execute("DELETE FROM t WHERE id = ?1", [entry_id])?;
        Ok(())
    }

    /// 签名不带文本面(不计入入口集),但体内字符串里有单个花括号——
    /// 若括号计数被字符串带偏,本函数会"读长"并吞掉后面的函数,入口数必变。
    pub fn corrupt_probe(&self, n: u64) -> Result<(), StoreError> {
        let tail = "{corrupt";
        conn.execute("UPDATE t SET hits = ?1 WHERE id = ?2", [n, tail.len() as u64])?;
        Ok(())
    }

    pub fn touch(&self, n: u64) -> Result<(), StoreError> {
        conn.execute("UPDATE t SET hits = hits + 1 WHERE id = ?1", [n])?;
        Ok(())
    }

    fn writer_internal(&self, e: Entry) -> Result<(), StoreError> {
        conn.execute("INSERT INTO t (c) VALUES (?1)", [e.content])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    pub fn test_only_writer(content: String) {
        conn.execute("INSERT INTO t (c) VALUES (?1)", [content])?;
    }
}
"""

_GUARD = """        let policy = ScanPolicy::default();
        if nexus_contracts::sensitive::scan(&e.content, &policy).is_blocked() {
            return Err(StoreError("blocked".into()));
        }
"""


def _sample(batch_guard=True, delegate_guard=True, cfg_marks=True):
    """合成源。cfg_marks=False 时拿掉两处 `#[cfg(test)]` 属性,
    用于验证"同位置的裸 pub 写入口必须被判据抓住"(否则夹具只能证明"跳过",
    不能证明"不漏")。"""
    return _SAMPLE % {
        "batch_guard": _GUARD if batch_guard else "",
        "delegate_guard": _GUARD if delegate_guard else "",
        "field_attr": "    #[cfg(test)]\n" if cfg_marks else "",
        "inject_attr": "    #[cfg(test)]\n" if cfg_marks else "",
    }


def _exempt(fn, category, reason, delegates_to=None):
    row = {"rel": _REL, "fn": fn, "category": category, "reason": reason}
    if delegates_to:
        row["delegates_to"] = delegates_to
    return row


# 夹具理由文本长度都刻意 >40 字:短于阈值会被 validate_table 判成 CONFIG(rc=2) 而不是
# 预期的红/绿,那就会把"夹具自己写错"读成"门没牙"。C11 正是专门锁这条阈值的。
_R_DELEGATE = (
    "夹具用:声明 append_one 委托 append_batch(与 session-store segment.rs::append 同形状)。"
    "委托目标存在且自带 scan 且本入口确实调用它,三者同时成立才可豁免——本门逐条机械核验。")
_R_DELETE = (
    "夹具用:delete_row 只把 entry_id 传进 DELETE 语句,写语句不携带任何原文,"
    "故可按 STRUCTURAL-WRITE 豁免;补这条是为了把判定点闭合到唯一目标上。")
_R_BATCH = (
    "夹具用:假设批量入口只写上游已扫描过的内容。本行只为凑齐入口集合的全部处置项,"
    "理由长度需过 validate_table 阈值,所以刻意写得长一点。")
_R_GHOST = (
    "夹具用:本行指向一个不存在的函数——幽灵豁免必须判红(STALE),否则豁免表会随重构烂掉。"
    "C4 的其余判定点已由 _MIN_EXEMPT 闭合,红只能来自这一条。")

# 合成源里"必然需要处置"的三条(批量/委托/删除),一次性凑齐便得绿。
_FULL_EXEMPT = [
    _exempt("insert_batch", "STRUCTURAL-WRITE", _R_BATCH),
    _exempt("append_one", "UPSTREAM-GUARDED", _R_DELEGATE, delegates_to="append_batch"),
    _exempt("delete_row", "STRUCTURAL-WRITE", _R_DELETE),
]
# 批量入口已自带 scan 的夹具只需这两条。
_MIN_EXEMPT = [
    _exempt("append_one", "UPSTREAM-GUARDED", _R_DELEGATE, delegates_to="append_batch"),
    _exempt("delete_row", "STRUCTURAL-WRITE", _R_DELETE),
]


def selftest():
    import shutil
    import tempfile

    def in_dir(text, fn):
        d = tempfile.mkdtemp(prefix="g84fix")
        try:
            p = os.path.join(d, *_REL.split("/"))
            os.makedirs(os.path.dirname(p))
            with open(p, "w", encoding="utf-8") as fh:
                fh.write(text)
            return fn(d)
        finally:
            shutil.rmtree(d, ignore_errors=True)

    def rc_of(root, required, table):
        return check(root, required, table)[0]

    cases = []

    def add(name, expect_rc, source, table, expect_entries=None, expect_contains=None,
            expect_absent=None):
        """夹具断言四件事:退出码、入口计数、诊断文本含/不含指定标记。

        WHY 断言诊断文本:只看 rc 无法区分"因为哪个判据红"。C4/C5/C10/C11 若只断言
        rc,就会把"夹具写错导致的 2"读成"门有牙"。这正是本轮第一版踩过的坑。
        """
        def probe(root):
            rc, out, stats = check(root, [_REL], table)
            text = "\n".join(out)
            got_entries = stats.get("entries")
            why = []
            if rc != expect_rc:
                why.append("rc=%s 期望 %s" % (rc, expect_rc))
            if expect_entries is not None and got_entries != expect_entries:
                why.append("entries=%s 期望 %s" % (got_entries, expect_entries))
            if expect_contains is not None and expect_contains not in text:
                why.append("诊断文本缺标记 %r" % expect_contains)
            if expect_absent is not None and expect_absent in text:
                why.append("诊断文本不应出现 %r" % expect_absent)
            ok = not why
            detail = "rc=%s, entries=%s" % (rc, got_entries)
            if expect_contains:
                detail += ", 含标记=%s" % (expect_contains in text)
            if expect_absent:
                detail += ", 无 %s=%s" % (expect_absent, expect_absent not in text)
            if why:
                detail += " <- " + "; ".join(why)
            return (ok, detail)
        cases.append((name, source, table, probe))

    # C1 反向核心:批量入口无守卫且无豁免 ⇒ 必红(旧文件级判据在这里给绿)
    add("C1 未覆盖写入口必红(旧判据盲区)", 1, _sample(batch_guard=False), [],
        expect_entries=5, expect_contains="insert_batch")
    # C2 三条带证据豁免 ⇒ 绿;入口数必须恰为 5(insert/insert_batch/append_batch/
    #    append_one/delete_row)。touch(n: u64) 签名无文本面不计数,私有 writer_internal
    #    非 pub 不计数,#[cfg(test)] 段不计数。锁死计数才防得住"少计入口伪装成通过"。
    add("C2 带证据豁免放行", 0, _sample(batch_guard=False), _FULL_EXEMPT,
        expect_entries=5)
    # C3 真插桩到位 ⇒ 绿(不依赖豁免)
    add("C3 自带 scan 放行", 0, _sample(batch_guard=True), _MIN_EXEMPT,
        expect_entries=5)
    # C4 幽灵豁免必红(指向不存在的函数),且红必须来自 STALE 而非别处
    add("C4 幽灵豁免必红", 1, _sample(batch_guard=True),
        _MIN_EXEMPT + [_exempt("no_such_entry", "STRUCTURAL-WRITE", _R_GHOST)],
        expect_contains="已不再是")
    # C5 委托豁免的牙齿:委托目标自身丢了 scan ⇒ 必红
    add("C5 委托目标失守必红", 1, _sample(batch_guard=True, delegate_guard=False),
        _MIN_EXEMPT, expect_contains="委托目标缺失/自身无 scan/本入口未调用")
    # C6 非文本面写入口不误伤:touch(n: u64) 不得计入入口集合(入口数=5 而非 6)
    add("C6 非文本面签名不误计", 0, _sample(batch_guard=True), _MIN_EXEMPT,
        expect_entries=5)
    # C7 #[cfg(test)] mod 内的 pub fn 不得算生产写入口(否则入口数=6)
    add("C7 测试段 pub 写入口不计数", 0, _sample(batch_guard=True), _MIN_EXEMPT,
        expect_entries=5)
    # C8 私有 helper 不计入入口集合(writer_internal 有写信号 + texty 签名但非 pub)
    add("C8 私有写 helper 不误判", 0, _sample(batch_guard=True), _MIN_EXEMPT,
        expect_entries=5)
    # C12 cfg(test) 面不计入:结构体字段上的 #[cfg(test)] 不得吞掉后续生产入口,
    #     挂在 fn 上的 #[cfg(test)] 也不得把那个 fn 当作生产写入口。
    add("C12 cfg(test) 字段/函数不污染入口集", 0, _sample(batch_guard=True), _MIN_EXEMPT,
        expect_entries=5)
    # C13 同一位置的**裸 pub 写入口**必须被抓住(去掉 cfg 标记 ⇒ 红且点名)。
    #     WHY 需要这一条:C12 只能证明"跳过",C13 才能证明"不漏"——两条合起来才是双向。
    add("C13 去 cfg 标记后同位置必红", 1, _sample(batch_guard=True, cfg_marks=False),
        _MIN_EXEMPT, expect_entries=6, expect_contains="inject_failure")
    # C14 字符串内的花括号不得带偏函数边界(真树曾因此把两个文件误判为失衡):
    #     corrupt_probe 体内有 "{corrupt" —— 若计数被它带偏,入口数会变且报失衡。
    add("C14 字符串括号不带偏函数边界", 0, _sample(batch_guard=True), _MIN_EXEMPT,
        expect_entries=5, expect_absent="失衡")
    # C10 非法豁免类别 ⇒ 退 2(表自身格式错,不允许静默放行)
    add("C10 非法豁免类别退 2", 2, _sample(batch_guard=True), [
        _exempt("insert_batch", "TRUST_ME",
                "夹具用:类别非法但理由长度合规,用以证明类别校验独立生效而非靠长度连带触发。"),
    ], expect_contains="豁免类别非法")
    # C11 理由过短 ⇒ 退 2(防"豁免=拍脑袋")
    add("C11 豁免理由过短退 2", 2, _sample(batch_guard=False), [
        _exempt("insert_batch", "STRUCTURAL-WRITE", "夹具用:太短。"),
    ], expect_contains="理由过短")
    # C9 承诺文件缺失 ⇒ 退 2(不可判定,不等于"没问题")
    d_missing = tempfile.mkdtemp(prefix="g84miss")
    shutil.rmtree(d_missing, ignore_errors=True)
    cases.append(("C9 承诺文件缺失退 2", None, [],
                  lambda _root: (rc_of(d_missing, [_REL], []) == 2,
                                 "rc=%s(期望 2)" % rc_of(d_missing, [_REL], []))))

    # C15 未知旗标必退 2(入口契约 C3 的牙齿):不能把拼错的旗标降级成默认裁决。
    cases.append(("C15 未知旗标退 2(不降级为跑全量)", None, [],
                  lambda _root: (main(["--self-tset"]) == 2,
                                 "rc=%s(期望 2)" % main(["--self-tset"]))))

    print("=== G-84 selftest:入口级插桩门双向牙齿 ===")
    failed = 0
    for name, source, _table, probe in cases:
        if source is None:
            ok, detail = probe("")
        else:
            ok, detail = in_dir(source, probe)
        print("  [%s] %s  %s" % ("OK" if ok else "FAIL", name, detail))
        if not ok:
            failed += 1
    print("  夹具 %d 项,失败 %d" % (len(cases), failed))
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    import gate_rc  # crash exits 2, never borrows 1 (F32/F33)
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
