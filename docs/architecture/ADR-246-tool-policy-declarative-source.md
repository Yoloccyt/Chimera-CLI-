# ADR-246: 工具策略规则声明式配置源（F-01 / W1）

- **状态**：Accepted（2026-11-07，`feat/toolchain-parallel-optimization` W1 波次）
- **上游**：已批准方案 `radiant-thunder-darwin-xenXoOpR.md` §5 W1（现势版 `docs/reports/toolchain-parallel-optimization-w7r2-panorama-plan-2026-11-07.md` §5.2）
- **关联**：ADR-238（四环链）/ ADR-240（组合根接线、D2 默认 Allow 先例）/ ADR-242 §3.5（安全层无运行时开关）/ ADR-252（bench 台账纪律）/ ADR-254（能力表）

## 1. 背景

ADR-240 把四环链（policy → sandbox → route → transport）接入组合根时，策略环的
规则集与默认动作来自 `composition::tool_runtime_policy()` 的**硬编码空集**
`(Vec::new(), PolicyAction::Allow)`（composition.rs:174-175）——策略环在生产路径
形同虚设（永远放行），用户无法声明「拒绝某工具」的策略。这是已批准方案 F-01
判定未闭环的五元组缺口。

## 2. 决策

**D1 · 配置段契约（L1 纯类型）**：`nexus-core/src/config.rs` 的 `ChimeraConfig`
新增第 15 个顶层 section：

```rust
pub struct ToolPolicyConfig {
    pub default_action: ToolPolicyAction,   // 缺省 Allow
    pub rules: Vec<ToolPolicyRuleDef>,      // 缺省空；声明顺序即优先序
}
pub enum ToolPolicyAction { Allow, Ask, Deny }      // serde snake_case + 首字母大写 alias
pub struct ToolPolicyRuleDef { pub pattern: String, pub action: ToolPolicyAction }
```

- WHY 独立定义 `ToolPolicyAction` 而非复用 `seccore::execpolicy::PolicyAction`：
  依赖铁律禁止 L1 `nexus-core` 依赖 L4 `seccore`；映射职责在 L10 组合根。
- WHY 类型名以 `Config` 结尾：`check_config_sample_parity.py` 以「类型名是否以
  Config 结尾」区分段与标量（段漏写样例判红，标量漏写只 NOTE）。首版命名
  `ToolPolicySection` 被门判为标量（14 段 + 2 标量），**门强度静默降级**；改名后
  恢复 `15 段 + 1 标量`。命名即门强度，这是本次实施的结构性教训。
- WHY `pattern_parts()` 拆分纯函数放 L1：pattern 语法 `"<program> [args_glob]"`
  （program 精确匹配大小写不敏感，省略 args_glob 等价 `*`）是纯逻辑，可独立测试；
  L0/L1 允许受控纯逻辑（ADR-033 追加节）。

**D2 · 缺省 Allow 不削减（ADR-240 D2 先例）**：配置缺省（无段/空段）时
`tool_runtime_policy()` 派生结果与硬编码时代逐位一致 `(vec![], Allow)`。零信任
白名单语义由部署方显式声明 `default_action: deny` 获得——**Deny 只有显式声明
才生效**。

**D3 · 无热加载**：配置仅在 `composition::build()` 装配期读取一次。运行期切换
策略 = 安全能力开关，ADR-242 §3.5 明确不提供。env 覆盖
（`CHIMERA_TOOL_POLICY__DEFAULT_ACTION`）走 figment 既有
`Env::prefixed("CHIMERA_").split("__")` 链，属「显式声明」，不违反缺省 Allow。

**D4 · 非法规则的两级边界语义**：
- `action` 非法值：serde 边界**硬失败**（`load()` 退 1，错误链含
  `unknown variant`）——「写了 deny 却没生效」比「加载失败」危险得多；
- `pattern` 无有效 program（空/全空白）：跳过该条 + `tracing::warn!`
  （fail-soft per-rule——一条笔误不拖垮整份策略，也不静默吞掉其余规则）。

**D5 · 映射职责在 L10 组合根**：`tool_runtime_policy(&ChimeraConfig)` 签名化
（原无参），L1 类型 → L4 `ExecPolicyRule`/`PolicyAction` 的映射集中于此；
`build_with_sharding` 调用点传 `config`。

**D6 · bench 组名含计数 → 同步改名**：`config_concurrency_bench` 的组名
`14_sections_*` 硬编码段数，第 15 段落地后改名 `15_sections_*`（getter/并发
task 同步 15）；`perf_thresholds.toml` 与 `bench_check.yml` heredoc 的
first-match 关键词 `["14_sections"]` **双源同步**改为 `["15_sections"]`——否则
归类落空即自造 hnsw_tiered 同型幽灵门（N-3 教训）。阈值值 `config_concurrency
= 3ms` 不变（第 15 次 extract_inner 为 µs 级增量，余量充分；只收紧不放宽纪律
未被触碰）。

**D7 · LazyConfig 同批扩展**：第 15 个 getter + `to_chimera_config` 条目同批
（穷举结构字面量缺字段即编译失败，编译器强制两账一致）。

## 3. 否决方案

| 方案 | 否决理由 |
|------|---------|
| A. 直接复用 `seccore::execpolicy::PolicyAction` 作配置类型 | L1→L4 向上依赖违反依赖铁律；serde 形态（snake_case + alias）属配置边界关切，不应泄漏进 L4 |
| B. 非法 pattern 也硬失败整份配置 | 一条 pattern 笔误拖垮全部 15 段配置加载，比例失当；warn+skip 保持其余规则生效且失败可见 |
| C. 运行期热加载 tool_policy | 运行期切换策略属安全能力开关，ADR-242 §3.5 明确不提供 |
| D. 保留组名 `14_sections_*` 仅追加第 15 getter | 组名失实；且 15 getter/14 task 的半吊子形态更难排查 |
| E. 配置类型命名 `ToolPolicySection` | parity 门以类型名后缀区分段/标量，该名被判标量使漏段只 NOTE 不红——门强度静默降级（本次实测抓到并已改名） |

## 4. 后果

**正向**：策略环首次拥有声明式规则源；四环 policy→sandbox→route→transport 中
policy 环从「形同虚设」转为「用户可控」；P1（MCP 工具执行器）可直接用
`pattern: "mcp.*"` 类规则设防（program 精确匹配下按工具名逐个声明）。

**代价 / 边界**：`ChimeraConfig` 15 段（Figment 全量 extract 多解析一段，µs 级）；
`config_concurrency` 组键名变更使旧 criterion 历史不再可比（同轮对照基准不受影响）。

## 5. 测试证据（2026-11-07 本机实测）

```text
cargo test -p chimera-cli --test tool_policy_config   # 8 passed / 0 failed（新增三层测试）
cargo test -p nexus-core --lib config                 # 4 passed / 0 failed
cargo test -p chimera-cli --test tool_runtime_wiring  # 17 passed（含签名适配后全绿）
cargo test -p chimera-cli --test config_lazy          # 18 passed / exec_plan_wiring 7 passed
python scripts/check_config_sample_parity.py          # 15 根级键 / 16 字段（15 段+1 标量）rc=0
python scripts/check_perf_redlines.py invariants      # PASS（I1 79/79、I9 79 键，关键词改值不增减键）
cargo check -p chimera-cli --benches                  # Finished dev profile（组名改名后编译绿）
```

## 6. 回退

还原 `nexus-core/src/config.rs` 三类型与字段、`composition.rs` 签名与映射体、
`chimera-cli/src/config.rs` 模板段与 LazyConfig 条目，重跑
`check_config_sample_parity.py --write`；bench 组名与关键词逆同步回
`14_sections`。回退后回到 ADR-240 时代硬编码空集语义（安全层无开关，无降级面）。
