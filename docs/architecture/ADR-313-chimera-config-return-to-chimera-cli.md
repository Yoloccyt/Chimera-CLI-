# ADR-313: `ChimeraConfig` 迁回 chimera-cli（B3 批次）

- 状态: Accepted
- 日期: 决策与落地 2026-11-16（B3 提交 `361901d5`，2026-11-16T01:17:08+08:00）；本文件落档 2026-11-18
- 批次: L1 深度重构 B3（ADR-309 明示后续项 **W4-2** 的正式收口；ADR-311 决策合订的 D2.3 子章节）
- 决策者: NEXUS-OMEGA Team
- 关联提交: `361901d5`（B3 主体）、`58ac0ca0`（ADR-311 落档 + 索引登记）、`fe6ba887`（ADR-318 改号批中的表述更正）
- 关联: ADR-311 D2.3（领域类型直连 L0 终局）、ADR-309（compute 迁出先例）、ADR-222 D1/D2（L0-B 核心基础面定位）、ADR-246（`tool_policy` 段新增）

> **落档性质**：本决策已于 2026-11-16 由 B3 提交落地，此前仅以代码注释
> （`crates/nexus-core/src/lib.rs`、`crates/chimera-cli/src/config_types.rs` 等
> 的 `B3/ADR-313` 注记）与 `adr_index.md` 索引行承载，磁盘无独立 ADR 文件。
> 本文件为**补录**，不改变任何既成事实。

---

## D1 — 背景：配置"定义"与"消费者"分居两层

B3 之前，CLI 域配置类型的物理归属与真实消费者不一致：

1. **定义在 L1**：`crates/nexus-core/src/config.rs` 承载 `ChimeraConfig` 及其
   子结构（Nexus/Quest/Mcp/Monitoring 等段）。
2. **加载逻辑在 L10**：Figment 多源合并、`~/.chimera/omega.yaml` 模板生成、
   文件初始化全部在 `crates/chimera-cli/src/config.rs`。
3. **消费者只在 L10**：L2–L9 生产代码对这批配置类型**零消费**（全仓 grep 实证，
   见 B3 提交说明）。
4. **定位冲突**：ADR-222 D1/D2 已把 L1 重定义为「L0-B 核心基础面」；
   `nexus-core` 的职责应是 `NexusState` / `CLV` / ID newtype / `PragmaCapable` /
   衰减纯函数，而不是承载 CLI 域配置。

⇒ 结果是"定义在 L1、加载在 L10、消费者只有 L10"的三分格局：为了改一个 CLI
配置字段，必须跨 crate 修改；而 `nexus-core` 为一个无人消费的类型族付出了
依赖与编译面成本。

**与 ADR-309 的同构性**：compute 面的处置结论是「迁出到独立 crate，原位置保留
shim 一个版本窗」；本批处置结论是「迁回唯一真实消费者」，因为配置类型的
消费者本就唯一、且同在 L10，无需新 crate（违 ADR-049 的 0 新增 crate 约束）。

---

## D2 — 决策

### D2.1 配置类型平移到 `chimera-cli::config_types`

- 新增 `crates/chimera-cli/src/config_types.rs`：纯类型定义（struct +
  `Default` impl + 默认值函数）。取数：`(Get-Content
  crates/chimera-cli/src/config_types.rs).Count` = **1030 行**（B3 提交说明记为
  1,028 行，按当次提交计数；两者口径差为提交后追加的注记行）。
- 实测类型面：`Select-String -Path crates/chimera-cli/src/config_types.rs
  -Pattern '^pub struct '` = **30 个 `pub struct`**（`ChimeraConfig`、
  `NexusConfig`、`QuestConfig`、`McpConfig`、`McpServerConfig`、
  `ModelRouterConfig`、`MonitoringConfig`、`ToolPolicyConfig` 等）。
  > 注：`crates/chimera-cli/src/config.rs` 的模块文档写「27 个 struct」，
  > 与现取 30 不符，属**已知文档陈旧**（登记为文档债，不在本批修正范围）。

### D2.2 调用面零 diff（re-export 保持旧路径）

`crates/chimera-cli/src/config.rs` 增加：

```rust
pub use crate::config_types::*;
```

⇒ `chimera_cli::config::X` 的既有调用路径**逐字不变**，本 crate 内部无须
批量改写 `use`。仅 7 处内部直引用改写（`mcp_tool_executor.rs` /
`tool_plane/mcp_connect.rs` / `commands/cost.rs` + 3 个测试文件，见 B3 提交说明），
以及 `lib.rs` 的 `config_types` 模块声明。

### D2.3 从 `nexus-core` 摘除

- 删除 `crates/nexus-core/src/config.rs`（现取 `Test-Path
  crates/nexus-core/src/config.rs` = `False`）。
- `crates/nexus-core/src/lib.rs`：删除 `config` 模块声明、顶层 `ChimeraConfig`
  重导出与 `prelude` 条目，就地留下 `NOTE(B3/ADR-313)` 说明去向。

### D2.4 同批衍生的两处订正

1. **B5 遗漏订正**：`crates/event-bus/src/lib.rs` 的 `PRODUCERS` 3→2
   （`TokenLedgerRecorded` 退役后派生面未同步，`crate_contract --check` 抓到）。
   这是 ADR-312 交付面的补漏，不是 B3 的内容，但作为同批原子提交的一部分落地。
2. **panorama 六锁纪律**：`crates/chimera-cli/tests/panorama_adjudication_locks.rs`
   的 `variant_count_lock_unchanged` 156→155，与 ADR-312 的变体锁保持一致。

### D2.5 派生工具的配置源探测

`scripts/check_config_sample_parity.py` 的"定义文件"由**单点硬编码**改为
**有序候选探测**：

```
crates/chimera-cli/src/config_types.rs → crates/nexus-core/src/config.rs（夹具）→ crates/chimera-cli/src/config.rs
```

selftest 11 断言全过，真实裁决逐字等值（G-68/G-69 绿）。

---

## D3 — 后果

### 正面

- **层职责归位**：`nexus-core` 回到「核心基础面」本义；CLI 域配置定义与
  唯一真实消费者同 crate，改字段不再跨层。
- **调用面零成本**：re-export 保住了 `chimera_cli::config::X` 路径，
  L10 内部与测试无需大范围改写。
- **与 ADR-311 D2.3 的终局一致**：L1 只保留 `CLV` / `NexusState` / ID newtype /
  `PragmaCapable` / 衰减纯函数等核心面。

### 负面与破例说明

1. **破坏式公共路径变更**：`nexus_core::config::*`（含 `nexus_core::ChimeraConfig`
   与 prelude 条目）失效。
   - **破例依据**：ADR-311 D2.1 的零消费判据——L2–L9 生产代码对该类型族
     grep 实证零消费；测试消费不作为保留理由。
   - **precedent**：ADR-309（compute 面迁出，`nexus_core::compute::*` 经 shim
     过渡后退役）与 ADR-222 D2（L0 契约拆分后旧路径失效）属同类先例。
   - **与 ADR-309 的差异（须写明）**：ADR-309 为**仍在生产消费**的路径保留了
     一个版本窗 shim；本批迁走的是**零生产消费**路径，故**不设 shim**，
     直接删除（判据同 ADR-311 D2.1 决策 3）。
2. **文档陈旧（已知债）**：`crates/chimera-cli/src/config.rs` 模块文档写
   「27 个 struct」，实测 30；`config.rs` 中"15 个顶层 section"等描述亦需
   随字段演进复核。本 ADR 只登记，不修改代码文档。
3. **派生工具的多候选探测**：`check_config_sample_parity.py` 的候选顺序若被
   后人改动，可能让夹具路径（`nexus-core/src/config.rs`）重新成为真值源；
   selftest 11 断言是唯一防线。

---

## D4 — 回滚

- **原子回滚**：`git revert 361901d5`。
  该 commit 内同时包含：`config_types.rs` 新增、`config.rs` re-export、
  `nexus-core` 摘除、`check_config_sample_parity.py` 候选探测、event-bus
  `PRODUCERS` 订正与 panorama 锁同步——一次 revert 即整体回到 B3 之前。
- **不可拆回滚**：只 revert `nexus-core/src/config.rs` 的删除而不撤
  `chimera-cli/src/config_types.rs`，会产生两处 `ChimeraConfig` 定义
  （E0428/E0252），不是有效中间态；这与 ADR-222 D6 的原子纪律一致。
- 无不可逆点：全部为 git 跟踪文件，历史完整保留。

---

## D5 — 证据索引

| 事实 | 文件 + 符号（行号仅辅助） |
|---|---|
| 配置类型新宿主 | `crates/chimera-cli/src/config_types.rs` + `ChimeraConfig`（:13 起模块文档，`:912` 附近为 `tool_policy` 段注记） |
| re-export 零 diff | `crates/chimera-cli/src/config.rs` + `pub use crate::config_types::*;`（:22） |
| 模块声明 | `crates/chimera-cli/src/lib.rs` + `config_types` 模块文档（:72） |
| L1 摘除声明 | `crates/nexus-core/src/lib.rs` + `NOTE(B3/ADR-313)`（:51-53）/ prelude 注记（:84） |
| 派生工具候选探测 | `scripts/check_config_sample_parity.py` + `B3/ADR-313` 注记（:47） |
| B5 遗漏订正 | `crates/event-bus/src/lib.rs` + `PRODUCERS`（:5） |
| panorama 变体锁同步 | `crates/chimera-cli/tests/panorama_adjudication_locks.rs` + `variant_count_lock_unchanged`（:126） |
| 配置样例等值门 | `scripts/check_config_sample_parity.py` + `G-68/G-69`（由 ADR-246 引入的 `tool_policy` 段） |

**取数命令（本文所有数字的来源）**：

```powershell
# 类型面现取（应为 30）
(Select-String -Path crates/chimera-cli/src/config_types.rs -Pattern '^pub struct ').Count
# 文件行数现取
(Get-Content crates/chimera-cli/src/config_types.rs).Count
# 旧宿主确已不在盘（应为 False）
Test-Path crates/nexus-core/src/config.rs
# 改动面
git show 361901d5 --stat
```
