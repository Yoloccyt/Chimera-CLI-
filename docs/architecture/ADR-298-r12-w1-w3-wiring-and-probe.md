# ADR-298: R12 W1-1 / W3-1 —— 装配内核接线（含去重面闸门）+ 索引内容一致性探针（含最后一跳接线）

- 状态: Accepted
- 日期: 2026-11-14
- 批次: R12 W1-1、W3-1（八波次计划的第二、四波；W0/W2 见 ADR-296 / ADR-297）
- 关联: ADR-296(R12 W0 度量地基) / ADR-297(W2 记忆层边界) / ADR-290 D2(段文件权威、索引加速面) / ADR-295 D4(索引自愈与 schema_version) / ADR-144(上下文装配内核)
- 影响面: `crates/mca-gateway/src/conversation_trim.rs`(+`lib.rs`, `tests/assembly_wiring.rs`)、`crates/session-store/src/tree.rs`(+`lib.rs`, `writer.rs`, `tests/index_probe.rs`)

---

## 1. 背景：两个"实现完整、零消费者"的模块

| 模块 | 事实 | 出处 |
|---|---|---|
| 上下文装配内核 `assemble_context` | 生产调用点 = 0（19 处命中 = 定义 1 + 重导出 1 + 测试 17） | ADR-296 §3、`02-现状证据` §A.6 |
| 索引自愈 `verify_and_repair_tails` | 覆盖范围仅"尾部缺口"：其文档自陈「中段随机缺行/索引整库丢失不在本探针判据内」 | `crates/session-store/src/tree.rs:1006-1010`（订正前行号） |

R12 对这两处的立场一致：**先接线、再谈优化**。本 ADR 记录接线方式与两处"接线时才发现"的裁决。

---

## 2. 决策

### D1 —— 装配内核接进 mca 会话裁剪，**单账本**架构

`crates/mca-gateway/src/conversation_trim.rs` 的 `trim_to_budget` 签名与语义保持不变（既有调用点零改动），内部按开关分派：

- `AssemblyWiring::Off`（默认）⇒ 调 `trim_to_budget_legacy`（**旧函数体一字未动**，仅更名），输出与接线前逐字节相同；
- `AssemblyWiring::On` ⇒ 先经装配内核去重，再交旧路径做预算裁决。

**关键取舍：内核只做去重，不做预算。** `assemble_context` 自带一套 token 估算（`assemble.rs::estimate_tokens` = 字符数/4 上取整），而本模块用 `token_estimate`（UTF-8 字节宽/4 floor）。两条口径在中文上相差 **3 倍**。若把本模块预算直接喂给内核：

1. 内核按另一套单位做驱逐 ⇒ 与随后旧路径的裁剪**叠加**，形成"双层账本、双倍过度裁剪"；
2. 内核的驱逐优先级（`source_value_factor=0.3`）与旧路径"工具消息恒保留"的不变量**冲突**。

故传给内核的 `max_tokens = usize::MAX`（驱逐段永不触发），**预算裁决权唯一留在旧路径**。装配内核的驱逐逻辑在此路径**未被使用**（`evicted` 恒 0），这是已知代价，前置条件是先统一两套估算口径——越出本波写入域（`crates/hcw-window/**`）。

> 开关解析是表驱动纯函数（`parse_assembly_wiring`），非法值 warn 后回退 `Off`（对齐 `action_orchestrator::resolve_action_concurrency` 的既有形态）。

### D2 —— **去重面闸门：只有工具消息参与去重**（本 ADR 的核心裁决）

子代理的首版实现让**全部角色**参与去重。复核时判定该语义错误，改为只允许 `Tool` 消息参与：

```rust
fn is_dedup_participant(role: MessageRole) -> bool {
    role == MessageRole::Tool
}
```

**WHY 不能对对话轮次去重**：

- 重复的对话消息是**不同事件**，不是同一件事的副本。用户连说两次「继续」是两次发言，模型应当看到两次输入；折叠掉就是**静默少一次发言**——这是"结果丢了"一类缺陷，且不可回溯。
- `CHIMERA_DEDUP_NORMALIZE` 默认**开启**，连"仅空白/大小写不同"的两条不同轮次也会被折成一条，把语义差异当噪声删。
- 工具消息相反：同一段工具输出被多轮重复注入是同一产物的副本，去重是净收益（装配内核的去重本就是为"注入项"设计的）。

**落地证据（这是"测试即规格"的一次完整闭环）**：改闸门后，子代理原测试 **3 条转红**（`wire_on_dedups_duplicates`、`wire_on_preserves_block_structure`、属性测试的"无重复项"断言）。按新裁决改写而非迁就旧断言：

| 测试 | 处置 |
|---|---|
| `wire_on_dedups_duplicates` | 重复对从 `User` 改为 `Tool`；断言改为"工具消息无重复" |
| `wire_on_preserves_block_structure` | 改为"重复 assistant 轮次必须**原样存活**、重复工具输出收敛"；块结构逐字节断言保留 |
| `property::wire_on_output_is_subsequence_and_deduped` | 拆成两条：充裕预算下断言"工具无重复 + 对话轮次逐条保留"；预算压力下只断言"子序列 + 长度不增" |
| **新增** `wire_on_never_dedups_conversation_turns` | 守卫用例：两次「继续」+ 两次「好的」+ 两条同文 System ⇒ 开态输出与输入**逐字节相同** |

> 拆分属性的原因（首版实测踩到）：原属性把"去重"与"预算裁剪"混在一处，`budget in 0..400` 时旧路径会**合法丢弃**消息，于是"轮次被裁剪"被误判成"轮次被去重"。分离后两条各管一事。

### D3 —— 索引内容一致性探针：三向指纹 + 只报告不误重建

`crates/session-store/src/tree.rs` 新增 `IndexContentFingerprint`（`bytes` / `rows` / `hash`，形如 `v1:<b>:<r>:<h>`）与 `probe_content_consistency(&self, data_dir)`：

- **三向比对**（子代理首版是两向，实施中自行发现会漏判"索引中段缺行"后改三向，本 ADR 认可）：`stored`（meta 里记的指纹）vs `live`（段文件实况重算）vs 索引面聚合（`COUNT(*)`/`MAX(offset)`）。
- 不一致 ⇒ 按权威源重建（`rebuild_index`，`INSERT OR IGNORE` 幂等）；缺失字段（老库）⇒ 视为 `Unknown` ⇒ 同样走重建。
- **索引越前（孤儿行）判 `IndexAhead` 只报告不重建**：补入式重建删不掉孤儿行，若每次启动都"重建"，会陷入**每次启动白扫**。该处置归 ADR-290 D5 的审计器。
- 开关：与尾部探针**同一门控** `repair_index_on_open`（关闭即一次不跑，保住既有"禁用 ⇒ 缺口保留、不干预"的语义）；探针内部另有 `CHIMERA_INDEX_PROBE=off` 独立逃生。
- 失败 fail-open（继续用现有索引 + warn），**绝不 panic**。

### D4 —— 补上"最后一跳"：探针必须在打开路径上被真正调用

子代理交付时自陈：探针函数齐备、测试全绿，但**真正启动入口在 `writer.rs::open_blocking`（其写入域之外）**，故探针实际上是"无人调用"的——那正是 R12 反复要消灭的形态。

本 ADR 由主代理补上这一跳（`crates/session-store/src/writer.rs`）：

```rust
// 与尾部探针同门控；失败不阻断打开（索引是加速面，打开可用性优先）
let _startup_content_probe: ContentProbeReport = if config.repair_index_on_open {
    match tree.probe_content_consistency(&config.data_dir) {
        Ok(report) => { log_content_probe(&report); report }
        Err(e) => { warn!(error = %e, "…失败,降级为不重建(打开不受阻)"); ContentProbeReport::default() }
    }
} else { ContentProbeReport::default() };
```

**只记日志、不存报告**：本 crate 不依赖 `tracing-subscriber`（禁新增 crate），进程内无法断言日志；需要结构化结果的调用方应直接调 `probe_content_consistency`。记日志的策略是"该告警才告警"——有重建 ⇒ `warn`（真实修复动作，运维应看见），全健康 ⇒ `debug`（避免每次启动刷 warn 变成习惯性忽略）。

**同门控的验证**：既有用例 `startup_probe_skipped_when_disabled` 在 `repair_index_on_open=false` 下断言"缺口保留"，本接线不改变该语义 ⇒ 该用例保持绿（实测确认）。

---

## 3. 验收（全部机证，逐条可复跑）

| # | 判据 | 命令 | 实测 |
|---|---|---|---|
| 1 | 装配接线：关态逐字节等价 + 守卫 + 属性 | `cargo test -p mca-gateway --test assembly_wiring --jobs 2` | **11 passed / 0 failed** |
| 2 | mca-gateway 全量回归 | `cargo test -p mca-gateway --jobs 2` | **255 + 10 + 11 + 8 + 8 + 5 + 7 + 4 + 8 全绿 / 0 failed** |
| 3 | session-store 全量回归（含接线后） | `cargo test -p session-store --jobs 2` | **75 + 7 + 1 + 1(ignored) / 0 failed** |
| 4 | clippy 零告警 | `cargo clippy -p mca-gateway -p session-store --all-targets --jobs 2 -- -D warnings` | rc=0 |
| 5 | 格式 | `cargo fmt -p session-store -- --check`；三份 mca 文件 `rustfmt --check` | rc=0（见 §4 的 fmt 边界） |

**关键断言清单（代码里的可执行规格）**：

| 用例 | 钉住什么 |
|---|---|
| `wire_off_equals_legacy`（+ 关态属性测试） | 关态与旧路径**逐字节相同**（`json_bytes` 比较） |
| `wire_on_never_dedups_conversation_turns` | 对话轮次恒保留（D2 哨兵） |
| `wire_on_dedups_duplicates` | 工具消息重复收敛 1 条且保序 |
| `wire_on_preserves_block_structure` | `Thinking.signature` / `ToolUse.id` 逐字节存活（P4 状态守恒，丢了会被 Anthropic 400） |
| `property::wire_on_under_budget_pressure_keeps_shape` | 任意预算下输出仍是输入子序列 |
| `hash_roundtrip_stable` / `mismatch_triggers_rebuild_and_event` / `restart_after_partial_write_is_consistent` / `corrupt_segment_fails_open_without_panic` | 指纹稳定、不一致重建、半写重启可判、坏段 fail-open 不 panic |
| `probe_off_keeps_legacy_behavior` | 关探针 ⇒ 零重建（含"两向指纹会漏判"的回归护栏） |

---

## 4. 已知边界（诚实登记，不掩盖）

| # | 边界 | 影响 | 处置 |
|---|---|---|---|
| B1 | **两套 token 口径未统一**（内核 字符/4 ceil vs 模块 字节宽/4 floor，中文差 3 倍） | 内核的驱逐逻辑在本路径**未被使用** | 前置：统一真值源（越出本波写入域）；在此之前"让内核接管驱逐"是空话 |
| B2 | `source` 维度退化 | L0 `AffinityMessage` 无 provenance 字段 ⇒ `ContextSource::{Memory,Retrieval}` 在此路径不可达，去重键的 source 维不增加判别力 | 不靠"名字嗅探"补齐（原则 P8）；登记待 L0 加字段 |
| B3 | `is_pinned` 目前是**死信息** | 装配侧驱逐不触发，`pinned` 只是语义声明 | 内核接管驱逐时需一次显式裁决（"钉选段仍可驱逐" vs "工具消息恒保留"） |
| B4 | 写路径每会话每 flush 多一次 `readdir` + 逐段 `stat` + 索引区间扫描 | flush 墙钟会上升，**未 bench** | 已登记，重开条件：`session_store_bench` 的 flush 指标出现回退 |
| B5 | `data_dir` 由 `TreeIndex::open` 父目录推断 | 树库与段文件分目录时会误判并**白重建**（幂等非破坏） | 组合根约定；越界使用时登记 |
| B6 | 未覆盖：索引整库丢失、等长原地改字节（bit rot）；"重建失败"分支仅间接覆盖 | 探针判据有盲区 | 归 ADR-290 D5 审计器与后续波次 |
| B7 | `mca-gateway` 的 `adapters.rs` / `session.rs` 在 **HEAD 就存在 rustfmt 漂移**（非本批引入） | `cargo fmt -p mca-gateway` 会顺带重写这两个文件 | 本批只对三份改动文件 `rustfmt --check`（rc=0）；漂移修复需单独授权 |

---

## 5. 回滚

| 改动 | 回滚方式 |
|---|---|
| 装配接线 | `CHIMERA_ASSEMBLY_WIRED` 未设/`off` 即旧路径（默认态）；代码回滚 = 单 commit revert（`trim_to_budget_legacy` 原样在盘，收回分派即可） |
| 索引内容探针 | `CHIMERA_INDEX_PROBE=off` 或 `repair_index_on_open=false`；代码回滚 = 单 commit revert（`writer.rs` 的那跳与 `tree.rs` 的探针分开提交，可单独退） |

**注意**：回滚 D2 的闸门（改回全角色去重）会同时需要改回 3 条测试——这是刻意的，**回退必须显式**。

---

## 6. 减法（本波明确不做）

| 不做 | 理由 |
|---|---|
| 在装配面接管预算驱逐 | 两套 token 口径未统一（B1），接管即双账本 |
| 对对话轮次去重 | 语义错误（D2）；两次「继续」是两次发言 |
| 新增 `IndexDegraded` 事件变体 | 全仓无此变体；新增事件变体需六锁同步 + ordinal 尾号，收益不足以立此成本 |
| 让孤儿行（`IndexAhead`）也触发重建 | 补入式重建删不掉孤儿行 ⇒ 每次启动白扫（D3） |
| 修 `adapters.rs` / `session.rs` 的 fmt 漂移 | 越出本波范围（B7），需单独授权 |
| 给 `is_pinned` 造消费者 | 驱逐未启用，造出来的消费者是伪消费者（U-10 形态） |
