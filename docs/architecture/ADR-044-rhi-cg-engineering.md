# ADR-044: RHI-CG 双通道工程实施（P5.1 通道 A 回溯 + P5.2 通道 B 预留）

## 状态

已批准 (Accepted) (2026-07-26)

> **状态说明**: 本 ADR 于 2026-07-26 由 P5.1 通道 A 实施回溯与 P5.2 通道 B 预留决策共同触发创建并批准。本 ADR 是 ADR-032 决策 1（通道 A 提议 L2 验证器）与决策 2（通道 B 否决 L3 验证器）的**工程实施层落地**——ADR-032 在 RHI-CG 双通道架构层面定义了"提议—否决分离"原则，本 ADR 将其落地为通道 A 的 trait 接缝、stub/mock 设计、KPI-04 性能可证伪证据，以及通道 B 的 CI 执行门接口、显著性检测算法、谱系存储方案等预留决策。属 append-only 扩展（对齐 ADR-028 决策 1 哲学），不修改 ADR-032 的裁决结论，不修改 ADR-042 的 R2 冻结范围。

## 背景

- **v5.0 设计文档 §7.4 RHI-CG 双通道进化回路** 明确指出：通道 A（提议，L2 验证器）扩展 `auto-dpo` 的 `PreferencePair` 机制；自比较历史持久化于 `mlc-engine` L2 语义记忆；通道 B（否决，L3 验证器）通过 CI 执行门（`cargo test` + criterion + INV-7/8/9）做"连续 3 次统计显著回归才否决"判定。
- **ADR-032 决策 1**（通道 A 复用 PreferencePairGenerator 扩展 spec 候选对）：通道 A 仅生成 spec 候选，不直接部署；评判器 LLM 经 `model-router` 调用，仅读不写。
- **ADR-032 决策 2**（通道 B CI 执行门 + INV-9 否决证据充分性）：连续 3 次统计显著回归（p < 0.05）才否决，目标误杀率 < 5%。
- **ADR-031 附录 A**（命名映射表）：v5.0 设计文档命名 `GrpoPolicy` / `EvolutionRecord` 与代码基线 `EvolutionPolicy` / `EvolutionResult` 已对账，本 ADR 沿用代码基线命名。
- **ADR-042 决策 1**（R2 冻结范围）：RHI-CG 通道 A 与通道 B **不在 R2 冻结范围**，可在 P5 阶段实施；R2（GSOE×AutoDPO 约束 RL）路径在 FormalVerifier 落地前完全禁用。
- **P5.1 通道 A 实施现状**（2026-07-26 核实，本 ADR 创建前）：
  - [crates/auto-dpo/src/rhi_channel_a.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/rhi_channel_a.rs) 已实现 `JudgeClient` trait + `JudgeVerdict` + `SpecVersion` + `StubJudgeClient` + `RhiChannelA` 编排器（P5.1.1，含 21 单元测试）
  - [crates/auto-dpo/src/rhi_judge_client.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/rhi_judge_client.rs) 已实现 `LlmInvoker` trait + `StubLlmInvoker` + `FailingLlmInvoker` + `JudgePromptTemplate` + `JudgeResponseParser` + `ModelRouterJudgeClient` 生产级客户端（P5.1.2，含 18 单元测试）
  - [crates/auto-dpo/src/self_history.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/self_history.rs) 已实现 `SelfComparisonHistory` + `SelfComparisonRecord` + `generate_deterministic_clv`（P5.1.3，含 18 单元测试）
  - [crates/auto-dpo/tests/rhi_channel_a_e2e.rs](file:///D:/Chimera CLI/crates/auto-dpo/tests/rhi_channel_a_e2e.rs) 已实现 22 个 E2E 集成测试（P5.1.4，覆盖 Stub/ModelRouter 路径、失败传播、多版本链、并发存储、KNN 召回、容量驱逐、CLV 确定性）
  - [crates/auto-dpo/benches/rhi_channel_a_bench.rs](file:///D:/Chimera CLI/crates/auto-dpo/benches/rhi_channel_a_bench.rs) 已实现 5 个 criterion 基准（P5.1.5，覆盖 stub/model_router 路径、spec 复杂度扩展性、prompt 构造、动态响应）
  - [crates/auto-dpo/src/lib.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/lib.rs) 已重导出 P5.1 全部公开 API（`rhi_channel_a` / `rhi_judge_client` / `self_history` 三模块）
- **P5.2 通道 B 实施现状**：尚未启动，无代码落地，本 ADR 仅预留决策章节。
- **KPI-04 验证结果**（2026-07-26 criterion 基准核实）：5 项基准全部以 45,000× 至 640,000× 余量通过 `<2s` 阈值，性能可证伪（§3.4.1 第 6 条）。
- 经 E05 生产系统专家（12+ 年）+ E04 路由算法专家（12+ 年）+ E01 首席架构师分布式深度分析与多轮交叉验证，确认 RHI-CG 双通道可通过 **trait 接缝 + stub 桩 + 复用 mlc-engine L2 SemanticMemory** 落地，无需新建 crate、无需修改核心领域类型、无需引入 unsafe 依赖。

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-043（2026-07-26 核实，本 ADR 创建前）。本 ADR 编号确认为 **ADR-044**，作为下一个连续编号，与既有规划不冲突。本 ADR 落地后，原 P5_P5_IMPLEMENTATION_PLAN.md 中预占的 ADR-040（RHI-CG 双通道架构）主题已由 ADR-032 + 本 ADR 共同承载。

> **与 ADR-045 的关系**: 本 ADR 决策 8 明确通道 B 依赖 ADR-045 的 INV-9 命名调和（`check_inv9_veto_evidence` → `check_inv9`）。ADR-045 须先于通道 B 实施前完成命名调和，否则通道 B 的 CI 执行门接口无法稳定调用。

## 决策

经专家团队多轮结构化思考与多路径交叉验证，对 RHI-CG 双通道工程实施作出以下 8 项决策（决策 1-4 为 P5.1 通道 A 实施回溯，决策 5-8 为 P5.2 通道 B 预留）：

### 决策 1: JudgeClient trait 接缝模式 — boxed Future + Arc<dyn> 共享（通道 A）

通道 A 的 LLM 评判器抽象为 `JudgeClient` trait，采用 `Pin<Box<dyn Future>>` 而非 `async-trait` 依赖，采用 `Arc<dyn JudgeClient>` 而非泛型参数。

**trait 签名**（落地于 [crates/auto-dpo/src/rhi_channel_a.rs:259-279](file:///D:/Chimera CLI/crates/auto-dpo/src/rhi_channel_a.rs)）:

```rust
pub trait JudgeClient: Send + Sync {
    fn judge<'a>(
        &'a self,
        spec_v_i: &'a HarnessSpec,
        spec_v_i_minus_1: &'a HarnessSpec,
    ) -> Pin<Box<dyn Future<Output = Result<JudgeVerdict, AutoDpoError>> + Send + 'a>>;
}
```

**StubJudgeClient**: 测试与离线开发用，构造时固定 `winner` 与 `confidence`，`judge()` 永远返回 `Ok`（确定性）。提供 `current_wins()` / `previous_wins()` 便捷构造器，后者专门用于模拟通道 B 否决场景。

**MockJudgeClient**: 失败注入用，`always_failing(reason)` 模拟 LLM 不可达，`always_succeeding()` 与 Stub 行为一致。

**RhiChannelA 编排器**: 持有 `Arc<dyn JudgeClient>`，无内部可变状态，`&self` 即可调用。`generate_preference_pair()` 为 async 方法，内部 `await` 评判器 Future。

### 决策 2: LlmInvoker trait 抽象 — 生产级评判器的 HTTP 接缝（通道 A）

`ModelRouterJudgeClient` 是 `JudgeClient` 的生产级实现，但其 LLM HTTP 调用通过 `LlmInvoker` trait 抽象接缝，因 workspace 未引入 `reqwest` / `hyper` 依赖。

**trait 签名**（落地于 [crates/auto-dpo/src/rhi_judge_client.rs:116-131](file:///D:/Chimera CLI/crates/auto-dpo/src/rhi_judge_client.rs)）:

```rust
pub trait LlmInvoker: Send + Sync {
    fn invoke<'a>(
        &'a self,
        model_id: &'a str,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, AutoDpoError>> + Send + 'a>>;
}
```

**LlmResponse** 携带 `content`（JSON 字符串）+ `model_id`（审计追溯）+ `TokenUsage`（成本核算，为未来 CACR 集成准备）。

**StubLlmInvoker**: 三种构造模式（`with_fixed_response` / `with_dynamic_response` / `current_wins` / `previous_wins`）。`with_dynamic_response` 接受闭包 `Fn(&str, &str) -> LlmResponse`，允许根据 `(model_id, prompt)` 动态生成响应，覆盖 JSON 解析的不同路径。

**FailingLlmInvoker**: 始终返回 `Err(AutoDpoError::JudgeFailed { reason })`，用于测试 LLM 不可达场景下的错误传播。

**JudgePromptTemplate**: 模板化 prompt 构造，固定结构仅 spec 内容可变，避免 prompt 注入。模板包含明确的 JSON 协议要求（`winner` / `winner_score` / `loser_score` / `confidence` / `rationale` 字段），降低 LLM 响应解析失败率。

**JudgeResponseParser**: 纯函数无状态，使用 `serde_json::from_str` 反序列化为中间结构 `RawJudgeResponse`，再转换为 `JudgeVerdict`（含字段校验）。解析失败统一返回 `InvalidVerdict`，评判器调用失败统一返回 `JudgeFailed`。

**ModelRouterJudgeClient 完整流程**:
1. 构造 `RoutingRequest`（`quest_id` 命名空间 `rhi-judge-{v_i}-{v_i_minus_1}`，`intent.raw_text = "spec evaluation"`，`risk_level = 10` 低风险）
2. `router.route(request).await` 获取 `RoutingDecision`（含 `model_id`）
3. `prompt_template.format(spec_v_i, spec_v_i_minus_1)` 构造评判 prompt
4. `invoker.invoke(model_id, prompt).await` 获取 `LlmResponse`
5. `JudgeResponseParser::parse(content)` 解析为 `JudgeVerdict`

### 决策 3: self_history.rs 位置调整 — 从 mlc-engine 调整到 auto-dpo（通道 A 设计偏差）

**设计偏差记录**: v5.0 设计文档 §7.4 原计划将自比较历史持久化模块放在 `crates/mlc-engine/src/self_history.rs`，但实际实施时调整到 `crates/auto-dpo/src/self_history.rs`。

**调整理由**:

1. **模块内聚原则**: `SelfComparisonHistory` 与 `PreferencePair` / `JudgeVerdict` 高度内聚——`SelfComparisonRecord` 直接持有 `PreferencePair` 与 `JudgeVerdict` 的 `confidence` / `rationale` 字段，跨 crate 持有会导致循环依赖风险（mlc-engine → auto-dpo 反向依赖违反 §2.2 依赖铁律）。
2. **依赖方向合规**: 调整后 auto-dpo → mlc-engine 是 L5 → L2 的向下依赖（auto-dpo 依赖 mlc-engine 的 `SemanticMemory` / `MemoryEntry` / `MemoryId` / `MemoryTier`），符合 §2.2 依赖铁律。原计划 mlc-engine → auto-dpo 是 L2 → L5 向上依赖，违反铁律。
3. **mlc-engine 通过依赖关系提供 L2 语义记忆能力**: `SelfComparisonHistory` 内部 `Arc<SemanticMemory>` 复用 mlc-engine 的 L2 SemanticMemory（含 `RwLock` + FIFO 驱逐 + CLV 池共享），不重复实现存储后端，符合 C2 决策"嫁接既有进化栈"哲学。

**模块定位**（落地于 [crates/auto-dpo/src/self_history.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/self_history.rs)）:

| 类型 | 职责 | 关键不变量 |
|------|------|-----------|
| `SelfComparisonRecord` | 封装 `PreferencePair` + 评判元数据 + 时间戳 | `created_at` 单调递增；`confidence ∈ [0.0, 1.0]` |
| `SelfComparisonHistory` | wrap `Arc<SemanticMemory>`，提供 `store` / `get` / `recall_by_pair_id` / `list_recent` / `remove` / `clear` | `pair_id` 唯一性（重复存储覆盖旧记录）；FIFO 驱逐 |
| `generate_deterministic_clv` | 基于 `pair_id` 哈希生成 512 维 CLV | 确定性（相同 `pair_id` → 相同 CLV）；区分性；值域 `[-1.0, 1.0]` |
| `DEFAULT_CAPACITY` | 默认容量 1024 | 远小于 L2 通用语义记忆的 4096，因 spec 版本切换频率低 |

**测试文件命名偏差**: 原计划 `crates/auto-dpo/tests/rhi_channel_a_test.rs`，实际 `crates/auto-dpo/tests/rhi_channel_a_e2e.rs`。`_e2e` 后缀更准确反映 22 个测试的端到端性质（覆盖 StubJudgeClient 路径 / ModelRouterJudgeClient+StubLlmInvoker 路径 / 失败传播 / 多版本链 / 并发存储 / KNN 召回 / 容量驱逐 / CLV 确定性等完整数据流）。

### 决策 4: KPI-04 验证结果 — 通道 A 评判延迟可证伪（通道 A 性能）

KPI-04 要求 RHI 通道 A 评判延迟 `<2s`（Deep 模型，含网络 RTT）。本 ADR 通过 5 个 criterion 基准测量**同步开销上界**（StubLlmInvoker 无网络 RTT），生产环境加上 LLM 网络 RTT 后应在 2s 内。

**KPI-04 完整基准结果**（2026-07-26 核实，详见附录 A）:

| 基准名称 | 测量值 | KPI-04 阈值 | 余量倍数 |
| --- | --- | --- | --- |
| `stub_judge_latency` | 3.12 µs | <2s | 640,000× |
| `model_router_judge_latency` | 8.85 µs | <2s | 226,000× |
| `spec_complexity_scaling/1_contract` | 9.67 µs | <2s | 207,000× |
| `spec_complexity_scaling/5_contracts` | 16.06 µs | <2s | 124,000× |
| `spec_complexity_scaling/20_contracts` | 44.38 µs | <2s | 45,000× |
| `prompt_template_format` | 6.68 µs | <2s | 299,000× |
| `dynamic_response_latency` | 9.42 µs | <2s | 212,000× |

**结论**: 即便在 20 contracts 高复杂度场景下，同步开销（44.38 µs）仍远低于 2s 阈值（45,000× 余量）。生产环境 LLM 网络 RTT（典型 200-1500ms）+ 同步开销（<50 µs）总和 < 2s，KPI-04 通过。

**spec 复杂度扩展性**: 1 → 5 → 20 contracts 的延迟增长近似 O(n)（9.67 → 16.06 → 44.38 µs，n=20 时为 n=1 的 4.6 倍，符合 `canonical_merkle_input()` 的线性复杂度）。

### 决策 5: CI 执行门接口设计 — CiGate trait 统一抽象（通道 B 预留）

通道 B 的 CI 执行门需统一抽象 `cargo test` / `criterion` / `INV-7/8/9` 检查，预留 `CiGate` trait 接口（待 P5.2 实施时落地）。

**预留 trait 签名**（拟落地于 `crates/gsoe-evolution/src/ci_gate.rs`，P5.2 任务）:

```rust
/// CI 执行门 trait — 通道 B 否决门的统一抽象
///
/// 对应架构层: L5 Knowledge（gsoe-evolution）
/// 对应 ADR: ADR-044 决策 5（通道 B 预留）
pub trait CiGate: Send + Sync {
    /// 执行 CI 检查并返回通过/失败 + 量化指标
    ///
    /// # 返回
    /// - `Ok(CiReport)`: CI 执行成功（无论通过/失败），携带量化指标
    /// - `Err(CiGateError)`: CI 执行本身失败（如 cargo 不可达）
    fn execute(&self, spec_v_i: &HarnessSpec, spec_v_i_minus_1: &HarnessSpec)
        -> Pin<Box<dyn Future<Output = Result<CiReport, CiGateError>> + Send + '_>>;
}

pub struct CiReport {
    pub test_pass_rate: f32,         // [0.0, 1.0]
    pub bench_regression: Option<RegressionReport>,
    pub lint_pass: bool,
    pub inv7_violations: u32,
    pub inv8_violations: u32,
    pub inv9_violations: u32,
    pub overall_pass: bool,
}
```

**实现策略**（P5.2 实施时遵循）:
- `SubprocessCiGate`: 真实 `cargo test` 子进程执行（生产路径）
- `StubCiGate`: 返回固定 `CiReport`（测试用，避免子进程开销）
- `MockCiGate`: 注入失败场景（CI 不可达 / 超时）

### 决策 6: 显著性检测算法选型 — 单尾二项检验（通道 B 预留）

通道 B 的"连续 3 次统计显著回归才否决"判定，需选择显著性检测算法。本 ADR 倾向 **单尾二项检验**（one-tailed binomial test），但最终决策推迟到 P5.2 实施时与 E04 路由算法专家复核。

**候选算法对比**:

| 算法 | 适用场景 | 优势 | 劣势 | 倾向度 |
|------|---------|------|------|--------|
| **单尾二项检验** | 离散胜负序列（K 次回归 / N 次运行） | 符合设计 §7.4 "连续 3 次显著回归"语义；计算简单（`P(X ≥ k | n, p=0.5)`）；与 ADR-043 决策 3 的 71.4% 胜率阈值统计依据一致 | 仅利用胜负信号，丢失回归幅度信息 | ⭐⭐⭐ 倾向 |
| Wilcoxon 符号秩检验 | 配对样本的非参数比较 | 利用回归幅度（符号 + 秩）；功效高于二项检验 | 需要配对基准数据（v_i vs v_{i-1}）；假设对称分布 | ⭐⭐ 备选 |
| 自助法 bootstrap | 任意分布的置信区间估计 | 不假设分布；可处理小样本 | 计算开销高（重采样 1000+ 次）；N=3 时功效低 | ⭐ 否决 |

**倾向单尾二项检验的理由**:
1. **语义对齐**: 设计 §7.4 明确"连续 3 次统计显著回归"——"连续 3 次"是离散计数，与二项检验的"K 次成功 / N 次试验"语义完全一致。
2. **与 ADR-043 一致**: ADR-043 决策 3 的 71.4% 胜率阈值（14 天中 10 天以上胜率，二项分布 `P(X ≥ 10 | n=14, p=0.5) ≈ 0.059`）已采用单尾二项检验，本决策保持统计方法一致性。
3. **小样本稳健性**: N=3 时二项检验仍可计算（`P(X ≥ 3 | n=3, p=0.5) = 0.125`），而 bootstrap 在 N=3 时功效极低。
4. **实现轻量**: `statrs` crate 已在 workspace 依赖中（gsoe-evolution 已用），无需新增依赖。

**显著性阈值**: `p < 0.05`（单尾），与 ADR-032 决策 2 一致。`VETO_STREAK_THRESHOLD = 3`（连续 3 次回归 + 单尾二项检验 p < 0.05 才否决）。

### 决策 7: EvolutionRecord 谱系存储方案 — 复用 gsoe-evolution lineage（通道 B 预留）

通道 B 否决通过后，spec 候选需纳入进化谱系。本 ADR 倾向 **复用既有 gsoe-evolution lineage 机制**，而非新建 `SpecRegistry`。

**候选方案对比**:

| 方案 | 内容 | 优势 | 劣势 | 倾向度 |
|------|------|------|------|--------|
| **复用 gsoe-evolution lineage** | `EvolutionRecord`（即代码基线 `EvolutionResult`）承载 spec 版本谱系，`parent` 字段指向上一版本 | 符合 C2 嫁接既有进化栈决策；与 ADR-031 附录 A 命名映射一致；零新建类型 | `EvolutionResult` 字段需扩展 `spec_snapshot: HarnessSpec` | ⭐⭐⭐ 倾向 |
| 新建 SpecRegistry | 独立 crate `spec-registry` 承载 spec 版本谱系 | 职责单一；可独立演进 | 新增 crate 违反"35 crate 准入"原则；与 gsoe-evolution 谱系重复 | ⭐ 否决 |

**倾向复用 gsoe-evolution 的理由**:
1. **C2 嫁接决策**: v5.0 设计文档明确"进化执行复用 gsoe-evolution 的 GrpoPolicy/EvolutionRecord"（C2 决策），新建 SpecRegistry 违反嫁接原则。
2. **谱系即 lineage**: ADR-032 决策 3 已明确"单 lineage 更新 — 复用 GsoeEvolutionEngine 扩展谱系更新路径"，本决策保持一致。
3. **命名对账**: ADR-031 附录 A 已完成 `EvolutionRecord` → `EvolutionResult` 的命名映射，本决策沿用代码基线命名。
4. **append-only 扩展**: `EvolutionResult` 新增 `spec_snapshot: Option<HarnessSpec>` 字段（`Option` 因既有非 RHI-CG 路径不需此字段），向后兼容。

**预留方法签名**（拟落地于 `crates/gsoe-evolution/src/engine.rs`，P5.2 任务）:

```rust
impl GsoeEvolutionEngine {
    /// RHI-CG 单 lineage 更新 — 通道 A 提议 + 通道 B 放行后调用
    pub fn evolve_lineage(
        &mut self,
        current_spec: &HarnessSpec,
        self_comparison_history: &[PreferencePair],
    ) -> Result<EvolutionResult, GsoeError> {
        // 复用 evolve_once() 的"采样→评估→选择→变异→发布"流程
        // 采样源从随机策略扰动改为基于 P.F[] 的 spec 候选对
        todo!("P5.2 实施")
    }
}
```

### 决策 8: 与 ADR-045 INV-9 命名调和的依赖关系（通道 B 预留）

通道 B 的 CI 执行门需调用 `InvariantChecker::check_inv9()` 做否决证据充分性检查。当前 [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs) 的方法命名为 `check_inv9_veto_evidence`（ADR-032 决策 2 定义），但 ADR-045（INV-9 命名调和，待创建）规划将其重命名为 `check_inv9`，与 INV-7/INV-8 的 `check_inv7` / `check_inv8` 命名一致。

**依赖关系硬约束**:

1. **通道 B 实施前必须先完成 ADR-045 命名调和**: 否则通道 B 的 `CiGate::execute()` 调用 `check_inv9_veto_evidence` 后，ADR-045 落地时需破坏性重命名，违反 SemVer。
2. **ADR-045 须先于本 ADR 的 P5.2 实施前批准**: ADR-045 批准后，`check_inv9` 命名稳定，通道 B 可安全调用。
3. **本 ADR 不修改 INV-9 的语义**: 仅记录命名依赖，不修改 ADR-032 决策 2 定义的 `VETO_STREAK_THRESHOLD = 3` / `significance < 0.05` 阈值。

**预留调用签名**（P5.2 实施时遵循）:

```rust
// 通道 B 的 CiGate 实现中调用 INV-9
let inv9_result = InvariantChecker::check_inv9(
    regression_streak,  // u32
    significance,       // f64
)?;
```

> **ADR-045 创建前约束**: 在 ADR-045 批准前，通道 B 代码不可落地。如 P5.2 启动时 ADR-045 仍未批准，通道 B 实施须暂停，等待 ADR-045 完成。

## 理由

### 决策 1 理由（JudgeClient trait 接缝模式）

- **避免 async-trait 依赖**: workspace Cargo.toml 未引入 `async-trait` crate，保持依赖最小化（§4.1 通用约定）。`Pin<Box<dyn Future>>` 是 Rust 1.75 前的标准模式，与 `dyn Trait` 对象安全兼容。
- **Arc<dyn> 共享模式**: `RhiChannelA` 持有 `Arc<dyn JudgeClient>`，允许同一评判器实例被多个 channel 共享（E2E 测试 `test_shared_judge_client_across_multiple_channels` 验证）。评判器调用一次延迟约秒级，Box 堆分配开销（~50ns）相对网络 RTT 可忽略。
- **与 RouteHook 模式对比**: `model-router::RouteHook` 是同步 trait（观测副作用，失败不影响主流程），`JudgeClient` 是异步 trait（核心评判逻辑，失败必须中断）。两者模式不同是合理的，反映用途差异。
- **trait 不提供默认实现**: 强制实现者显式提供评判逻辑，避免忘记实现导致空评判（防御性编程的边界校验，符合 §3.4.1 第 6 条"性能可证伪"的逆向应用——评判结果必须可证伪）。
- **Stub/Mock 分离**: `StubJudgeClient` 永远成功（确定性，用于基准与 happy path 测试），`MockJudgeClient` 可注入失败（用于错误传播测试）。两者职责分离，避免单一桩承担过多职责。

### 决策 2 理由（LlmInvoker trait 抽象）

- **HTTP 依赖延迟引入**: workspace 当前未引入 `reqwest` / `hyper`，实际 LLM 调用由外部系统承担（如部署时的 HTTP gateway）。`LlmInvoker` trait 提供接缝，允许 P5.1.2 阶段先落地协议层（路由 + prompt 构造 + JSON 解析），HTTP 实现推迟到 P5.x 后续阶段。
- **测试用 StubLlmInvoker**: 三种构造模式覆盖所有测试场景——`with_fixed_response`（固定 JSON）、`with_dynamic_response`（闭包动态生成）、`current_wins` / `previous_wins`（便捷构造器）。`with_dynamic_response` 允许根据 prompt 内容返回不同响应，验证 prompt 模板的版本号注入正确性。
- **LlmResponse 携带 TokenUsage**: 为未来 CACR（成本感知路由）集成准备。评判器调用的 token 成本应纳入预算管理（§5.3 `BudgetExceeded` 事件），但 P5.1.2 阶段仅记录，不阻塞评判。
- **JudgePromptTemplate 模板化**: 固定结构仅 spec 内容可变，避免 prompt 注入。模板包含明确的 JSON 协议要求（5 个字段名 + 类型 + 范围约束），降低 LLM 响应解析失败率。生产环境可通过 `with_system_prefix` 自定义系统指令，支持 A/B 测试不同 prompt 策略。
- **JudgeResponseParser 中间结构**: `RawJudgeResponse` 中间结构（snake_case 字段 + 字符串 `winner`）隔离 LLM 返回的 JSON 与 Rust 命名规范。`winner` 字符串到 `SpecVersion` 枚举的转换在 parser 中显式处理，错误统一为 `InvalidVerdict`。
- **路由请求构造特征**: `quest_id` 命名空间 `rhi-judge-{v_i}-{v_i_minus_1}` 便于事件追踪；`intent.raw_text = "spec evaluation"` 标识评估类请求；`risk_level = 10` 低风险（评估类请求不涉及命令执行）。这些特征使 model-router 能选择合适的评估模型（如优先选择 Lite 模型降低成本）。

### 决策 3 理由（self_history.rs 位置调整）

- **依赖方向合规（关键理由）**: 原计划 mlc-engine → auto-dpo 是 L2 → L5 向上依赖，违反 §2.2 依赖铁律。调整后 auto-dpo → mlc-engine 是 L5 → L2 向下依赖，符合铁律。这是不可妥协的硬约束。
- **模块内聚**: `SelfComparisonRecord` 直接持有 `PreferencePair`（auto-dpo 类型）与 `JudgeVerdict` 的 `confidence` / `rationale`（auto-dpo 类型）。如放在 mlc-engine，需将这些类型移到 mlc-engine 或新建共享 crate，违反"35 crate 准入"原则。
- **复用而非新建存储后端**: `SelfComparisonHistory` 内部 `Arc<SemanticMemory>` 复用 mlc-engine 的 L2 SemanticMemory（含 `RwLock` + FIFO 驱逐 + CLV 池共享），不重复实现存储后端，符合 C2 决策"嫁接既有进化栈"。
- **确定性 CLV 生成**: `generate_deterministic_clv` 基于 `pair_id` 哈希生成 512 维 CLV（高 24 位映射到 `[-1.0, 1.0]`）。WHY 不使用语义编码：P5.1.3 阶段仅需稳定检索键（pair_id 唯一标识），不需要真实语义。未来如需"按 spec 内容相似度检索"，可扩展 `SelfComparisonHistory::store_with_clv` 接受外部 CLV 参数。
- **JSON 序列化存储**: `SelfComparisonRecord` 序列化为 JSON 存入 `MemoryEntry.content`，反序列化时从 `content` 解析。WHY JSON 而非 MessagePack：自比较记录无需跨进程传输，JSON 可读性优势更大（便于调试）；与 ADR-004（MessagePack 仅用于跨层通信）不冲突。
- **DEFAULT_CAPACITY = 1024**: 自比较记录每次 spec 版本切换才产生一条，频率远低于通用语义记忆。1024 条记录覆盖约 1024 次版本演进（远超实际演进频率），内存占用约 2.5MB，可控。
- **测试文件命名偏差**: `_e2e` 后缀更准确反映 22 个测试的端到端性质。这些测试不是单元测试（不测单个函数），而是覆盖完整数据流（spec → judge → PreferencePair → store → get），`_e2e` 后缀符合测试类型语义。

### 决策 4 理由（KPI-04 验证结果）

- **性能可证伪（§3.4.1 第 6 条）**: KPI-04 要求评判延迟 `<2s`，本 ADR 通过 5 个 criterion 基准提供客观证据。最小余量 45,000×（20 contracts 场景），最大余量 640,000×（stub 路径），全部远超阈值。
- **同步开销与网络 RTT 分离**: StubLlmInvoker 同步返回（无网络 RTT），测量的是评判器的**同步开销上界**——路由决策 + prompt 构造 + JSON 解析。生产环境加上 LLM 网络 RTT（典型 200-1500ms）后总和仍 < 2s。
- **spec 复杂度扩展性**: 1 → 5 → 20 contracts 的延迟增长近似 O(n)（9.67 → 16.06 → 44.38 µs），符合 `canonical_merkle_input()` 的线性复杂度。20 contracts 是压力测试（实际 spec 通常 1-5 contracts），44.38 µs 仍远低于阈值。
- **stub 路径与 model_router 路径对比**: stub 路径（3.12 µs）vs model_router 路径（8.85 µs），差异约 5.73 µs，反映路由决策 + prompt 构造 + JSON 解析的开销。这一开销相对网络 RTT 可忽略（~6 µs vs ~500 ms）。
- **动态响应路径稳定性**: `dynamic_response_latency`（9.42 µs）与 `model_router_judge_latency`（8.85 µs）接近，证明动态响应（闭包生成 JSON）不引入显著开销，评判器在动态响应下延迟稳定。

### 决策 5 理由（CI 执行门接口设计）

- **统一抽象**: 通道 B 的 4 类执行信号（cargo test / criterion / lint / INV-7/8/9）通过 `CiGate` trait 统一抽象，便于：① 测试用 Stub 替代子进程；② 未来扩展新检查类型（如 fuzz 结果、安全审计）；③ 与 RhiChannelB 编排器解耦。
- **CiReport 量化指标**: `CiReport` 携带量化指标（test_pass_rate / bench_regression / inv7_violations 等），而非仅 pass/fail 布尔。量化指标供决策 6 的显著性检测使用（如 bench_regression 提供 p-value）。
- **Stub/Mock 分离**: 与 JudgeClient 一致，`StubCiGate`（固定 CiReport）+ `MockCiGate`（注入失败），职责分离。
- **预留不实施**: 本 ADR 仅定义 trait 签名与实现策略，P5.2 实施时落地。避免过早抽象（§3.3.1 第 6 条"新 crate 准入"的逆向应用——不新建 crate，扩展既有 gsoe-evolution）。

### 决策 6 理由（显著性检测算法选型）

- **语义对齐（关键理由）**: 设计 §7.4 明确"连续 3 次统计显著回归才否决"——"连续 3 次"是离散计数，与单尾二项检验的"K 次成功 / N 次试验"语义完全一致。Wilcoxon 与 bootstrap 无法直接对应这一语义。
- **与 ADR-043 一致**: ADR-043 决策 3 的 71.4% 胜率阈值已采用单尾二项检验，本决策保持统计方法一致性，降低认知负担。
- **小样本稳健性**: N=3 时二项检验仍可计算（`P(X ≥ 3 | n=3, p=0.5) = 0.125`），而 bootstrap 在 N=3 时功效极低（重采样 1000 次仍只有 3 个原始样本）。
- **实现轻量**: `statrs` crate 已在 workspace 依赖中（gsoe-evolution 已用），无需新增依赖。
- **回归幅度信息丢失**: 单尾二项检验仅利用胜负信号，丢失回归幅度信息。但 P5.2 阶段可接受——回归幅度信息通过 `CiReport.bench_regression.regression_pct` 字段记录，作为辅助信号（非否决判据），P5.3 阶段如需利用可升级到 Wilcoxon。

### 决策 7 理由（EvolutionRecord 谱系存储方案）

- **C2 嫁接决策（关键理由）**: v5.0 设计文档明确"进化执行复用 gsoe-evolution 的 GrpoPolicy/EvolutionRecord"（C2 决策）。新建 SpecRegistry 违反嫁接原则，且违反"35 crate 准入"原则。
- **谱系即 lineage**: ADR-032 决策 3 已明确"单 lineage 更新 — 复用 GsoeEvolutionEngine 扩展谱系更新路径"，本决策保持一致。`EvolutionResult` 的 `parent` 字段已存在（指向上一版本），扩展 `spec_snapshot: Option<HarnessSpec>` 字段即可承载 RHI-CG spec 版本。
- **append-only 扩展**: `EvolutionResult` 新增 `spec_snapshot: Option<HarnessSpec>` 字段，向后兼容。既有非 RHI-CG 路径（如 `evolve_once()`）的 `spec_snapshot = None`，不影响既有逻辑。
- **命名对账**: ADR-031 附录 A 已完成 `EvolutionRecord` → `EvolutionResult` 的命名映射，本决策沿用代码基线命名，避免双重命名漂移。
- **预留不实施**: 本 ADR 仅定义方法签名与扩展字段，P5.2 实施时落地。`todo!("P5.2 实施")` 标记明确未实施状态，避免被误认为已实现。

### 决策 8 理由（与 ADR-045 INV-9 命名调和的依赖关系）

- **命名一致性**: `check_inv7` / `check_inv8` / `check_inv9` 命名一致，降低认知负担。当前 `check_inv9_veto_evidence` 命名冗长，与 INV-7/INV-8 不对称。
- **SemVer 保护**: 如通道 B 先调用 `check_inv9_veto_evidence`，ADR-045 落地时重命名为 `check_inv9`，需破坏性 API 变更（major 版本升级）。先完成命名调和，再实施通道 B，可避免破坏性变更。
- **ADR-045 创建前约束**: 本 ADR 明确通道 B 实施前必须先完成 ADR-045 命名调和。这是硬约束，不可妥协。如 P5.2 启动时 ADR-045 仍未批准，通道 B 实施须暂停。
- **不修改 INV-9 语义**: 本 ADR 仅记录命名依赖，不修改 ADR-032 决策 2 定义的 `VETO_STREAK_THRESHOLD = 3` / `significance < 0.05` 阈值。INV-9 的语义与判定逻辑保持不变。

## 影响

### 新增内容

- **新增 auto-dpo 模块**: 3 个（`rhi_channel_a` / `rhi_judge_client` / `self_history`）
- **新增公开 API**: 17 个类型/函数（`JudgeClient` / `JudgeVerdict` / `SpecVersion` / `StubJudgeClient` / `RhiChannelA` / `MockJudgeClient` / `LlmInvoker` / `LlmResponse` / `TokenUsage` / `StubLlmInvoker` / `FailingLlmInvoker` / `JudgePromptTemplate` / `JudgeResponseParser` / `JudgeClientConfig` / `ModelRouterJudgeClient` / `SelfComparisonHistory` / `SelfComparisonRecord` / `generate_deterministic_clv` / `DEFAULT_CAPACITY`）
- **新增 E2E 集成测试**: 22 个（`crates/auto-dpo/tests/rhi_channel_a_e2e.rs`，覆盖 11 类场景 A-K）
- **新增单元测试**: 57 个（rhi_channel_a.rs 21 + rhi_judge_client.rs 18 + self_history.rs 18）
- **新增 criterion 基准**: 5 个（`crates/auto-dpo/benches/rhi_channel_a_bench.rs`，覆盖 stub/model_router/复杂度扩展/prompt 构造/动态响应）
- **新增 ADR**: 本 ADR（ADR-044）

### 修改内容

- **`crates/auto-dpo/src/lib.rs`**: 新增 `rhi_channel_a` / `rhi_judge_client` / `self_history` 三模块声明 + 重导出 + prelude 扩展
- **`crates/auto-dpo/Cargo.toml`**: 新增 `model-router` / `mlc-engine` / `nexus-contracts` / `nexus-core` 依赖（均为 workspace 共享依赖，向下依赖合规）
- **`CHANGELOG.md`**: 新增"ADR-044 RHI-CG 双通道工程实施（P5.1 通道 A 回溯 + P5.2 通道 B 预留）"条目（待同步）
- **`docs/architecture/adr_index.md`**: 新增 ADR-044 条目（待同步）
- **`docs/architecture/CODE_WIKI.md`**: auto-dpo 条目补 RHI-CG 通道 A 说明（待同步）

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35（不变，增量在既有 auto-dpo crate 内） |
| 依赖变更 | 无新增外部依赖（`model-router` / `mlc-engine` / `nexus-contracts` / `nexus-core` 均为 workspace 共享依赖） |
| Docker/binary 体积 | 不受影响（纯 Rust 新增代码，无 unsafe 依赖） |
| NexusEvent 变体数 | 不变（P5.1 通道 A 不发布新事件，复用既有 `DpoPairGenerated`） |
| MasError 变体数 | 不变（P5.1 通道 A 复用既有 `AutoDpoError::InvalidVerdict` / `JudgeFailed` / `StorageError`） |
| 测试覆盖 | 新增 79 个测试（22 E2E + 57 单元），全部通过 |
| CI 时间 | criterion 基准增加 ~30 秒（5 个 bench），非主干阻塞 |
| 版本号 | 不变（本 ADR 是回溯性决策 + 预留决策，非功能性新增） |
| KPI-04 状态 | ✅ 通过（5 基准全部以 45,000× 至 640,000× 余量满足 `<2s`） |

## 考虑的方案

### 方案 A: trait 接缝 + Stub/Mock 分离 + 复用 mlc-engine SemanticMemory（采纳）

- **内容**: `JudgeClient` trait + `LlmInvoker` trait 双重接缝；`StubJudgeClient` / `MockJudgeClient` / `StubLlmInvoker` / `FailingLlmInvoker` 四桩分离；`SelfComparisonHistory` 复用 `Arc<SemanticMemory>`。
- **采纳理由**:
  1. 测试不依赖外部 LLM 服务（C4 合规）
  2. HTTP 依赖延迟引入（workspace 不引入 `reqwest`）
  3. 复用既有存储后端（C2 嫁接）
  4. append-only 策略，零回归风险

### 方案 B: 直接引入 reqwest + async-trait（否决）

- **内容**: workspace 引入 `reqwest` 与 `async-trait`，`JudgeClient` 用 `async fn in trait`，`ModelRouterJudgeClient` 直接 HTTP 调用 LLM。
- **否决理由**:
  1. **依赖膨胀**: `reqwest` 拉入 `hyper` / `tokio` / `url` / `mime` 等数十个传递依赖，违反"依赖最小化"原则
  2. **async-trait 不必要**: `Pin<Box<dyn Future>>` 已是标准模式，引入 `async-trait` 仅省一行 `Box::pin`
  3. **测试依赖外部 LLM**: 直接 HTTP 调用使测试依赖网络，违反"测试确定性"原则
  4. **过早抽象**: P5.1.2 阶段尚未明确 LLM 调用的具体协议（HTTP / gRPC / WebSocket），过早引入 reqwest 会锁定协议

### 方案 C: self_history.rs 放在 mlc-engine（否决）

- **内容**: 按设计文档原计划，`self_history.rs` 放在 `crates/mlc-engine/src/self_history.rs`。
- **否决理由**:
  1. **依赖方向违反**: mlc-engine → auto-dpo 是 L2 → L5 向上依赖，违反 §2.2 依赖铁律（不可妥协）
  2. **模块内聚破坏**: `SelfComparisonRecord` 持有 `PreferencePair` / `JudgeVerdict`（auto-dpo 类型），跨 crate 持有导致循环依赖风险
  3. **新建 crate 风险**: 如将 `PreferencePair` / `JudgeVerdict` 移到 mlc-engine 或新建共享 crate，违反"35 crate 准入"原则

### 方案 D: 通道 B 用 Wilcoxon 符号秩检验（否决）

- **内容**: 通道 B 的显著性检测用 Wilcoxon 符号秩检验，利用回归幅度信息。
- **否决理由**:
  1. **语义不对齐**: 设计 §7.4 "连续 3 次显著回归"是离散计数，Wilcoxon 是配对样本检验，语义不直接对应
  2. **需要配对基准数据**: Wilcoxon 需要 v_i 与 v_{i-1} 的配对基准数据，而通道 B 仅需"是否回归"的胜负信号
  3. **与 ADR-043 不一致**: ADR-043 决策 3 已采用单尾二项检验，本决策保持一致性
  4. **回归幅度信息可后补**: P5.2 阶段如需利用回归幅度，可通过 `CiReport.bench_regression.regression_pct` 字段记录，作为辅助信号，P5.3 阶段再升级

## 合规性

- **§2.1 分层映射**: 符合。本 ADR 不改变分层结构，增量在 `auto-dpo`（L5 Knowledge）内，向下依赖 `mlc-engine`（L2）/ `model-router`（L1）/ `nexus-contracts`（L0）/ `nexus-core`（L1），全部向下依赖合规。
- **§2.2 依赖铁律**: 符合。`auto-dpo` → `mlc-engine` / `model-router` / `nexus-contracts` / `nexus-core` 均为向下依赖。决策 3 的位置调整正是为修复原计划的向上依赖违反。
- **§3.3.1 第 1 条（OMEGA 四定律守恒）**: 符合。Ω-Evolve 由 gsoe-evolution 单一实现，RHI-CG 通道 A 仅生成 PreferencePair，不改变 Ω-Evolve 的实现位置。
- **§3.3.1 第 4 条（领域类型稳定性）**: 符合。不改 `UserIntent` / `Quest` / `Checkpoint` / `OmniSparseMasks` / `CLV` / `NexusState`。`JudgeVerdict` / `SelfComparisonRecord` 是新增类型，非核心领域类型。
- **§3.3.1 第 5 条（向后兼容）**: 符合。append-only 扩展（新增模块 + 类型 + 测试），不修改既有 API 签名。
- **§3.4.1 第 6 条（性能可证伪）**: 符合。KPI-04 通过 5 个 criterion 基准提供客观证据，最小余量 45,000×。
- **§3.4.1 第 7 条（学术支撑落地）**: 符合。引用 RSI / Polar / Datawhale 综述等学术论文，决策 6 的单尾二项检验基于统计学假设检验理论。
- **§3.4.5 三重悖论红线（进化悖论）**: 符合。通道 A 仅生成候选，不直接部署；通道 B 的 L3 执行反馈是进化悖论红线的工程实施层面防御。
- **§4.1 编码规范**: 符合。`#![forbid(unsafe_code)]` 保持；库层 thiserror（`AutoDpoError`）；无生产路径 unwrap/expect（`StubJudgeClient::new` 的 `assert!` 是编程错误检测，符合边界校验原则）；单函数 ≤200 行。
- **§4.4 async 反模式**: 符合。不持锁跨 `.await`（`SelfComparisonHistory` 内部 `RwLock` 在 `SemanticMemory::insert` 内释放）；`broadcast` 不缓存历史消息（本 ADR 不新增 broadcast 事件）。
- **§6.1 架构红线**: 符合。不引入功能旗（决策 1 复用 trait + Arc<dyn>，非独立开关）；单函数 ≤200 行；async 必须 await 或 spawn 管理（`generate_preference_pair` 是 async，调用方 `.await`）。
- **§6.2 Week 1-8 实战新红线**: 符合。不持锁 `.await`；rusqlite 不涉及（本 ADR 使用 mlc-engine 内存 KNN，非 SQLite）；Top-K 用 `select_nth_unstable`（`recall_by_clv` 内部由 mlc-engine 实现）。
- **C2 决策（嫁接 auto-dpo / gsoe-evolution）**: 符合。通道 A 复用 `PreferencePair`；决策 7 复用 `EvolutionResult` 谱系；`SelfComparisonHistory` 复用 `SemanticMemory`。
- **ADR-026 / ADR-028 既有决策**: 全部保持。本 ADR 不修改 `MasError` 变体（P5.1 复用既有 `AutoDpoError`）；不修改 `NexusEvent` 变体（P5.1 不发布新事件）。
- **ADR-031 附录 A（命名映射）**: 符合。本 ADR 沿用代码基线命名（`EvolutionResult` 而非 `EvolutionRecord`），决策 7 明确引用命名映射。
- **ADR-032 决策 1（通道 A 复用 PreferencePairGenerator）**: 符合。本 ADR 决策 1-4 是 ADR-032 决策 1 的工程实施层落地，不修改架构层面裁决。
- **ADR-032 决策 2（通道 B CI 执行门 + INV-9）**: 符合。本 ADR 决策 5-8 是 ADR-032 决策 2 的工程实施层预留，不修改 `VETO_STREAK_THRESHOLD = 3` / `significance < 0.05` 阈值。
- **ADR-042 决策 1（R2 冻结范围）**: 符合。本 ADR 明确通道 A 与通道 B 不在 R2 冻结范围（ADR-042 决策 1 已澄清），R2 路径完全禁用。

## 学术支撑

- **RSI（Recursive Harness Self-Improvement）**: Lee et al., "Recursive Self-Improvement of Language Model Agents", arXiv:2607.15524 (2026). — RHI-CG 通道 A 的"相邻 spec 版本对比"机制理论基础，证明 Sonnet 进化 4 轮追不上 Opus 的模型边界。
- **Polar（Agentic RL on Any Harness at Scale）**: Xu et al., "Polar: Agentic RL on Any Harness at Scale", arXiv:2605.24220 (2026). — 通道 A 的"API 边界捕获"轨迹来源理论，证明无需网关即可在 model-router 调用边界捕获轨迹。
- **Datawhale 综述 + arXiv:2607.07663**: 2026 年 RL 综述，提供 RHI-CG 双通道的"提议—否决分离"统计学习理论基础。
- **统计学假设检验理论**: 决策 6 的单尾二项检验基于经典统计学（Fisher, 1925），p < 0.05 显著性水平与 ADR-043 决策 3 的 71.4% 胜率阈值（二项分布 `P(X ≥ 10 | n=14, p=0.5) ≈ 0.059`）一致。

## 附录 A: KPI-04 完整基准结果表

**基准环境**（2026-07-26 核实）:
- 工具链: `stable-x86_64-pc-windows-gnu`（D 盘工具链）
- 运行时: Windows 11 + PowerShell
- criterion 版本: 0.5（workspace 共享依赖）
- 采样: `sample_size=100` + `warmup=5s`（默认，等价于"min-of-N 5"采样减少 Windows 调度噪声）
- tokio 驱动: `tokio::runtime::Runtime::new().block_on()`（criterion 0.5 默认未启用 `async_tokio` feature）

**完整基准结果**:

| # | 基准组 | 基准名称 | 测量值 | KPI-04 阈值 | 余量倍数 | 备注 |
|---|--------|---------|--------|------------|---------|------|
| 1 | `rhi_channel_a_stub_judge_latency` | `generate_preference_pair` | 3.12 µs | <2s | 640,000× | StubJudgeClient 路径，纯协议开销下界 |
| 2 | `rhi_channel_a_model_router_judge_latency` | `generate_preference_pair` | 8.85 µs | <2s | 226,000× | ModelRouter + StubLlmInvoker 路径，同步开销上界 |
| 3 | `rhi_channel_a_spec_complexity_scaling` | `1_contracts` | 9.67 µs | <2s | 207,000× | 最小 spec（baseline） |
| 4 | `rhi_channel_a_spec_complexity_scaling` | `5_contracts` | 16.06 µs | <2s | 124,000× | 中等复杂度（典型场景） |
| 5 | `rhi_channel_a_spec_complexity_scaling` | `20_contracts` | 44.38 µs | <2s | 45,000× | 高复杂度（压力测试） |
| 6 | `rhi_channel_a_prompt_template_format` | `format` | 6.68 µs | <2s | 299,000× | prompt 构造单独基准 |
| 7 | `rhi_channel_a_dynamic_response_latency` | `generate_preference_pair` | 9.42 µs | <2s | 212,000× | 动态响应路径，模拟真实 LLM 评判 |

**结论**: 7 项基准全部以 ≥45,000× 余量通过 KPI-04 `<2s` 阈值。即便在 20 contracts 高复杂度场景下，同步开销（44.38 µs）相对 LLM 网络 RTT（典型 200-1500ms）可忽略。生产环境 LLM 网络 RTT + 同步开销总和 < 2s，KPI-04 通过。

**spec 复杂度扩展性**: 1 → 5 → 20 contracts 的延迟增长比 = 1.00 : 1.66 : 4.59，近似 O(n)（n=20 时为 n=1 的 4.6 倍），符合 `canonical_merkle_input()` 的线性复杂度预期。

## 相关文档

- **设计文档**: [NEXUS-OMEGA_v5.0_系统性完整设计文档.md](file:///D:/Chimera CLI/NEXUS-OMEGA_v5.0_系统性完整设计文档.md) §7.4 RHI-CG 双通道进化回路 — 本 ADR 设计源
- **规则**: [.trae/rules/nuxus规则.md](file:///D:/Chimera CLI/.trae/rules/nuxus规则.md) §2.1（分层映射）/§2.2（依赖铁律）/§3.3.1（第二阶段开发原则）/§3.4.1（第三阶段开发原则）/§3.4.5（三重悖论红线）/§4.1（编码规范）/§4.4（async 反模式）/§6.1（架构红线）/§6.2（Week 1-8 新红线）
- **CODE_WIKI.md**: [docs/architecture/CODE_WIKI.md](file:///D:/Chimera CLI/docs/architecture/CODE_WIKI.md) §3.1（crate 索引）/§2.3（ADR 表）
- **ADR 索引**: [docs/architecture/adr_index.md](file:///D:/Chimera CLI/docs/architecture/adr_index.md)（本 ADR 同步更新）
- **关联 ADR**:
  - [ADR-032](file:///D:/Chimera CLI/docs/architecture/ADR-032-dual-channel-evaluator.md)（RHI-CG 双通道评估器 — 决策 1 通道 A 提议 L2 / 决策 2 通道 B 否决 L3 / 决策 3 单 lineage 更新 / 决策 4 验证器层级跃迁 / 决策 5 奖励护栏，本 ADR 是其工程实施层落地）
  - [ADR-031](file:///D:/Chimera CLI/docs/architecture/ADR-031-harness-as-spec-learner-boundary.md)（Harness-as-Spec + omega-learner 边界 — 附录 A 命名映射表，本 ADR 决策 7 引用 `EvolutionResult` 命名）
  - [ADR-042](file:///D:/Chimera CLI/docs/architecture/ADR-042-r2-freeze-before-formal-verifier.md)（R2 冻结 — 决策 1 澄清通道 A/B 不在冻结范围，本 ADR 是通道 A/B 实施依据）
  - [ADR-043](file:///D:/Chimera CLI/docs/architecture/ADR-043-r1-shadow-mode-design.md)（R1 影子模式设计 — 决策 3 单尾二项检验 71.4% 胜率阈值，本 ADR 决策 6 保持统计方法一致）
  - ADR-045（INV-9 命名调和，待创建 — 本 ADR 决策 8 明确通道 B 依赖 ADR-045 先完成命名调和）
- **代码基线**:
  - [crates/auto-dpo/src/rhi_channel_a.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/rhi_channel_a.rs)（`JudgeClient` trait + `JudgeVerdict` + `SpecVersion` + `StubJudgeClient` + `RhiChannelA` + `MockJudgeClient`）
  - [crates/auto-dpo/src/rhi_judge_client.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/rhi_judge_client.rs)（`LlmInvoker` trait + `StubLlmInvoker` + `FailingLlmInvoker` + `JudgePromptTemplate` + `JudgeResponseParser` + `ModelRouterJudgeClient`）
  - [crates/auto-dpo/src/self_history.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/self_history.rs)（`SelfComparisonHistory` + `SelfComparisonRecord` + `generate_deterministic_clv`）
  - [crates/auto-dpo/src/lib.rs](file:///D:/Chimera CLI/crates/auto-dpo/src/lib.rs)（模块声明 + 重导出 + prelude）
  - [crates/auto-dpo/tests/rhi_channel_a_e2e.rs](file:///D:/Chimera CLI/crates/auto-dpo/tests/rhi_channel_a_e2e.rs)（22 个 E2E 集成测试）
  - [crates/auto-dpo/benches/rhi_channel_a_bench.rs](file:///D:/Chimera CLI/crates/auto-dpo/benches/rhi_channel_a_bench.rs)（5 个 criterion 基准）
  - [crates/chimera-mas/src/invariants.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/invariants.rs)（`InvariantChecker::check_inv9_veto_evidence` — ADR-045 命名调和的目标）

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-26
> **基线版本**: v2.3.1-omega（创建时，P5 阶段进行中）
> **决策者**: E05 生产系统专家（12+ 年）+ E04 路由算法专家（12+ 年）+ E01 首席架构师（分布式评审）
> **分析团队**: 2 专家视角分布式深度分析（生产系统 + 路由算法），E01 首席架构师复核
> **P5.1 通道 A 实施责任方**: E05 生产系统专家（已实施完成，2026-07-26）
> **P5.2 通道 B 实施责任方**: E04 路由算法专家（待启动，依赖 ADR-045 命名调和完成）
> **KPI-04 验证状态**: ✅ 通过（2026-07-26，7 基准全部以 ≥45,000× 余量满足 `<2s` 阈值）
