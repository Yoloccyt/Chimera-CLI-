# ADR-242 -- 回滚开关统一台账（特性开关 → 被回滚行为 → 影响范围 → 验证命令）

- 状态：Accepted（2026-11-06）
- 决策者：工具链并行化重构第四波（W4-T1）
- 关联：ADR-241（并行化深化）、ADR-240（四环链生产接线）、ADR-239（性能阈值基线与棘轮）、ADR-225（阈值双源对拍）
- 影响面：`docs/architecture/ADR-242-rollback-switch-registry.md`（本文件）、`docs/release/release_guide.md`（新增台账索引节）

## 1. 背景与问题

本次工具链重构在多个位置引入了**特性开关**（`CHIMERA_*` 环境变量）作为回滚手段：

| 开关 | 引入批次 | 回滚对象 |
|------|---------|---------|
| `CHIMERA_SYSINFO_FULL_REFRESH=1` | 在途（Wave 0） | TUI sysinfo 定向采集 → 全量采集 |
| `CHIMERA_CONFIG_EAGER=1` | 在途（Wave 0） | CLI 配置按需加载 → 强制全量加载 |
| `CHIMERA_RT_FLAVOR=auto` | 在途（Wave 0） | 默认多线程 runtime → NoContext 走 current_thread |

问题在于：**这些开关散落在代码注释里，无统一台账**。后果：

1. **不可发现** —— 交接或运维时无法快速回答「出问题该设哪个环境变量」；
2. **口径不一** —— 有的写「回滚开关」、有的写「逃生门」、有的写「兼容窗口」，读者无从判断哪些是**运行时可切**的、哪些是**构建期**的、哪些**已过期**；
3. **无验证命令** —— 开关是否真的生效、怎么确认生效，只能读代码；
4. **安全语义无声明** —— 哪些开关**绝不能**提供（否则安全能力可被降级）从未写明。

同时，全仓的 `CHIMERA_*` 变量里既有**回滚开关**，也有**普通配置项**（如 `CHIMERA_API_KEY`、`CHIMERA_MODEL__PRIMARY`）——两者混在一起时，读者容易误把配置项当回滚手段。

## 2. 决策

### 2.1 D1：建立显式台账，区分「回滚开关」与「普通配置项」

本 ADR 是**回滚开关的唯一台账**。判据：

| 类别 | 判据 | 台账位置 |
|------|------|---------|
| **回滚开关** | 存在即改变**已优化路径**的行为形态，使实现可退回改造前的形态 | 本文件 §3 |
| **兼容开关** | 提供**旧接口/旧行为**的过渡通道（如 `--no-v3-engine`） | 本文件 §3（标注兼容窗口状态） |
| **逃生门** | 提供**安全能力**的降级路径（**仅限非安全面**） | 本文件 §3（标注安全语义） |
| 普通配置项 | 业务参数（API key、模型选择、限额等） | `~/.chimera/omega.yaml` 的 14 段配置 |

**WHY 必须显式区分**：把配置项当回滚手段，会导致「出事时设了个无关环境变量，以为回滚了其实没有」——比没有台账更危险。

### 2.2 D2：台账必须现取，不登记可能漂移的计数

本 ADR 记录**开关清单**（有限、稳定的枚举），但**不登记**任何易漂计数（crate 数 / 事件变体数 / 门数）。

开关清单的现取命令见 §5。

### 2.3 D3：每个开关必须写明「被回滚行为」与「验证命令」

只有开关名而无验证命令的条目**不可验收** —— 无法证明「设了它确实回滚了」。

### 2.4 D4：安全语义的降级面**明确声明为「不提供」**

沙箱派发链的强制层（policy → sandbox → route → transport）**不提供**任何运行时关闭开关。

**WHY**：ADR-238 的安全论证建立在「顺序即安全边界，缺环无法通过类型检查构造」之上。若提供「跳过沙箱层」的环境变量，这条论证即被推翻 —— 环境变量是**部署侧可控**的，而沙箱层是**不可降级**的安全能力（`seccore` 6 条不可学习红线之一：沙箱不可降级）。

若确需排查沙箱判定问题，正确路径是**看诊断信息**（`DispatchError::Sandbox` 的错误文案已含具体拦截原因），而非关闭沙箱。

### 2.5 D5：兼容窗口过期的开关必须显式标注

`CHIMERA_NO_V3_ENGINE` 与 `--legacy-exit-code` 的兼容窗口**原计划移除但未执行**（ADR-061 / ADR-060 未修订）。台账必须标注该状态 —— 否则读者会以为它即将消失而依赖它，或以为它已被移除而困惑。

## 3. 回滚开关台账

> **现取命令**（开关全集，含普通配置项）：
> ```powershell
> git grep -oh 'CHIMERA_[A-Z0-9_]*' -- "*.rs" "*.ps1" "*.sh" "*.yml" | Sort-Object -Unique
> ```
> 下表只收录**回滚/兼容/逃生**类开关；其余为普通配置项。

### 3.1 并行化回滚（本次重构的核心）

| 开关 | 作用 | 回滚到的行为 | 影响范围 | 验证命令 |
|------|------|-------------|---------|---------|
| `CHIMERA_NO_PARALLEL_WIKI=1` | 关闭 repo-wiki `search_batch` 的并行搜索 | 串行搜索 | `crates/repo-wiki/src/vector.rs` | `cargo test -p repo-wiki` |
| `CHIMERA_NO_PARALLEL_DECAY=1` | 关闭 decay-engine 的并行衰减 | 串行衰减 | `crates/decay-engine/src/engine.rs` | `cargo test -p decay-engine` |
| `CHIMERA_NO_PARALLEL_FAAE=1` | 关闭 faae-router 并行 | 串行路由 | `crates/faae-router/src/config.rs` | `cargo test -p faae-router` |
| `CHIMERA_NO_PARALLEL_GSOE=1` | 关闭 gsoe-evolution 并行适应度 | 串行适应度 | `crates/gsoe-evolution/src/fitness_parallel.rs` | `cargo test -p gsoe-evolution` |
| `CHIMERA_NO_PARALLEL_HCW=1` | 关闭 hcw-window 并行 | 串行窗口 | `crates/hcw-window/src/config.rs` | `cargo test -p hcw-window` |
| `CHIMERA_NO_PARALLEL_NMC=1` | 关闭 nmc-encoder 并行 | 串行编码 | `crates/nmc-encoder/src/config.rs` | `cargo test -p nmc-encoder` |
| `CHIMERA_NO_PARALLEL_OSA=1` | 关闭 osa-coordinator 并行 | 串行掩码 | `crates/osa-coordinator/src/config.rs` | `cargo test -p osa-coordinator` |
| `CHIMERA_MCP_SERIAL_COMMIT=1` | 关闭 MCP 2PC 的并行 commit 阶段 | 串行 commit | `crates/mcp-mesh/src/mesh.rs` | `cargo test -p mcp-mesh` |
| `CHIMERA_NO_SYNC_OUTPUT=1` | 强制 TUI 同步输出关闭（逃生门） | `ForcedOff` 探测回退链 | `crates/chimera-tui/src/engine/sync_probe.rs` | `cargo test -p chimera-tui` |

### 3.2 性能与启动路径回滚

| 开关 | 作用 | 回滚到的行为 | 影响范围 | 验证命令 |
|------|------|-------------|---------|---------|
| `CHIMERA_SYSINFO_FULL_REFRESH=1` | TUI sysinfo 恢复全量采集（枚举全进程表/磁盘） | 定向采集（CPU 列表 + 内存 + 自身 PID） | `crates/chimera-tui/src/panels/sysinfo.rs` | `cargo test -p chimera-tui sysinfo` |
| `CHIMERA_CONFIG_EAGER=1` | 强制全量加载配置（14 段 Figment 三源合并） | 按需加载（D5：4 类命令免解析） | `crates/chimera-cli/src/main.rs` | `cargo test -p chimera-cli` |
| `CHIMERA_RT_FLAVOR=auto` | NoContext 命令走 current_thread runtime | 默认多线程 runtime | `crates/chimera-cli/src/main.rs` | `cargo bench -p chimera-cli --bench cli_parse` |
| `CHIMERA_LAZY_CONFIG=1` | 启用 LazyConfig pilot（只解析 `model_router` section） | 全量配置解析 | `crates/chimera-cli/src/commands/cost.rs` | `cargo test -p chimera-cli config` |
| `CHIMERA_NO_HNSW_MLC=1` | 全盘关闭 mlc-engine 的近似层 | 回到精确检索 | `crates/mlc-engine/src/l2_semantic.rs` | `cargo test -p mlc-engine` |

### 3.3 测试与诊断（非生产路径）

| 开关 | 作用 | 影响范围 | 验证命令 |
|------|------|---------|---------|
| `CHIMERA_TEST_TIMEOUT_SCALE` | 测试超时缩放（CI 快轨注入 0.1，clamp `[0.01, 1.0]`） | 全仓测试超时 | `cargo test --workspace` |
| `CHIMERA_PERF_SCALE` | 性能断言的延迟预算缩放（缺省 1.0） | 各 crate 的延迟预算测试 | `cargo test -p <crate>` |
| `CHIMERA_PANIC_TEST=1` | 故意触发 panic 以验证 panic hook | `crates/chimera-cli/src/main.rs` | `cargo test -p chimera-cli` |
| `CHIMERA_HNSW_BENCH_FULL=1` | hnsw 基准跑全量数据集 | 基准数据量 | `cargo bench -p repo-wiki` |

### 3.4 兼容窗口（**已过期，当前保留**）

| 开关 | 作用 | 状态 | 影响范围 |
|------|------|------|---------|
| `CHIMERA_NO_V3_ENGINE=1` | TUI 回退到 ratatui 渲染路径（替代 `--no-v3-engine` flag） | 兼容窗口**原计划 v2.11.0-omega 移除，未执行**；ADR-061 未修订，当前保留 | `crates/chimera-cli/src/cli.rs` |
| `CHIMERA_LEGACY_EXIT_CODE`（`--legacy-exit-code` flag） | 所有错误统一返回退出码 1（v2.8.0 行为） | 兼容窗口**原计划 v2.11.0-omega 移除，未执行**；ADR-060 未修订，当前保留 | `crates/chimera-cli/src/main.rs` |

### 3.5 明确**不提供**的开关（安全语义不可降级）

| 不提供的东西 | 原因 |
|-------------|------|
| 跳过/关闭沙箱层的开关 | `seccore` 不可学习红线「沙箱不可降级」；ADR-238 的「顺序即安全边界」安全论证 |
| 关闭策略层（policy）的开关 | 同上；策略是审批前置，绕过即等于隐式放行 |
| 让路由层 miss 落到默认执行器的开关 | ADR-238 §1 明列该失败模式：「未注册工具被默认执行，等同于隐式放行」 |
| 让四环链「缺环也能构造」的开关 | ADR-238 D2：「忘了挂沙箱」必须**无法**通过编译期/构造期 |

**排查沙箱判定问题的正确路径**：读 `DispatchError::Sandbox` 的错误文案（已含具体拦截原因：注入 / 提权 / 沙箱逃逸 / 数据泄露 / 篡改 / 滥用），而非关闭沙箱。

## 4. 后果

### 4.1 正向

- 回滚路径**可发现、可验证、可交接**（每项含验证命令）。
- 开关类别清晰（回滚 / 兼容 / 逃生 / 普通配置），不会误用。
- 安全语义的不可降级面**显式声明**（§3.5），杜绝「加个环境变量关掉沙箱」的提议。

### 4.2 负向与代价

- 台账需随新开关引入而更新 —— 这是**有意的维护成本**：新增开关而不登记，等于该开关不可交接。
- 本 ADR 是一次**文档面**变更（`*.md` 被 `.gitignore` 排除，本文件与其他 ADR 一样不入 git 跟踪）—— 见 §5 的说明。

## 5. 失效条件与维护

### 5.1 开关清单漂移的检测

新增 `CHIMERA_*` 开关而不登记本台账时，下列检查会提示（**注**：这些检查针对**性能与并行化**类开关，不覆盖全部 `CHIMERA_*`）：

```powershell
# 现取全部 CHIMERA_* 变量（清单核对用）
git grep -oh 'CHIMERA_[A-Z0-9_]*' -- "*.rs" "*.ps1" "*.sh" "*.yml" | Sort-Object -Unique

# 并行化回滚开关的现取（与本台账 §3.1 逐条核对）
git grep -n 'CHIMERA_NO_PARALLEL_' -- "*.rs"
```

**本台账不设机械门禁**：机械判定「某个 `CHIMERA_*` 是否属于回滚开关」需要语义判断（同名变量在不同 crate 可能是不同语义），机械规则会误判。**人工核对 + 现取命令**是本项目的既有纪律（见 `.trae/rules/nuxus规则.md` 附录 §C 的文档纪律）。

### 5.2 git 跟踪状态说明

`docs/architecture/*.md` 与 `docs/release/*.md` 受 `.gitignore` 的 `*.md` 规则排除，**不入 git 跟踪**（ADR 索引 `adr_index.md` 同此）。这是项目的既有状态（`AGENTS.md` / `.claude/CLAUDE.md` 已被基线快照纳入跟踪，但多数 ADR 文档未纳入）。

**后果**：本 ADR 与 `release_guide.md` 的台账节**不会随 git 分发**。新克隆仓库的协作者无法获得回滚台账 —— 这是**已知且未修复**的缺口，修复需改 `.gitignore` 的豁免规则（属独立变更，不在本次重构范围）。

## 6. 与 `docs/release/release_guide.md` 的关系

`release_guide.md` 面向**使用者与运维**，本文面向**开发者与架构决策**。前者新增一节指向本 ADR（索引），不复制表格内容 —— 避免双源。
