# ADR-291: HCW v3 蓝图 P0 落地——能力档案契约、事件地基与有效窗口探针骨架

- 状态: Accepted
- 日期: 2026-11-12
- 批次: R9（无冲突子集，双会话同树事件见执行报告 §6.1）
- 设计蓝本: `HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md`（2026-11-04，Ω₂ 从压缩定律升级为预算定律）
- 关联: ADR-288/289（R8 前序）、ADR-290（编号已由并行会话的跨会话记忆一致性主题占用，本 ADR 顺延 291）、ADR-065/066（MCA 通道与能力协商）、ADR-198（阈值校准三态）、ADR-222（L0-A 分组）
- 搁置项: WP-C（WindowGovernor 动态定容）/WP-E（P3 桥）/WP-H（卫星态接线）/WP-D（w4）因另一会话占用 hcw-window/session-store/chimera-cli 冲突面，推迟至其提交后下一批次（执行报告 §6.1）

## D1 — 事件地基：+2 Normal 观测变体（WP-0，提交 0ddda7f4）

按蓝图 §六 新增 `ContextCapabilityUpdated`（L10→L2 能力档案回写通知）与 `WindowPlanApplied`（governor 决策留痕，双跑对照与快照回放证据锚点），均 Normal/System 观测面，不触碰 Critical 双清单与 mpsc 旁路。

- 六锁同步：`VARIANT_COUNT_LOCK` 154→156、`NORMAL_LOCK` 121→123（Critical 22 / Info 11 不变）
- ordinal 尾号续增 155/156——遵循 ADR-197「序数只增不改，避免历史序数全量位移」既有惯例（探索中发现该惯例后放弃了按声明序插入的原方案）
- TUI `event_coverage_drift_test` 穷举 match 同步（均归类 NotForTui 观测面）

## D2 — L0 能力档案契约（WP-A，提交 7e810093）

`crates/nexus-contracts-mca/src/mca/context_capability.rs`（新 leaf，712 行）：

1. **`ContextCapabilityProfile`**（`#[non_exhaustive]` + `new()` 全字段构造器，仿 EndpointSpec 先例）：provider/model/advertised_window/effective_window/effective_confidence/pricing_tiers/server_thread/max_output/long_context_notes。`model`/`long_context_notes` 用 `Box<str>` 随 crate 惯例。
2. **`CalibrationTier{ExternalPrior,OwnProbe,Inferred}`**：`as_str()` 与 serde snake_case 同形（`"external_prior"/"own_probe"/"inferred"`），与 event-bus `ContextCapabilityUpdated.confidence` 字符串约定互锁（测试锁定）。三档承载蓝图 §5.3 的数据可信度分层，为 observe→apply 治理闸提供挂点。
3. **`ThreadSupport{None,Beta{expiry_minutes},Stable}`**：`is_supported()` 对齐 `ThinkingSupport::is_supported` 先例，服务蓝图 §5.5 P3 threads gating。
4. **`PricingTier{threshold_tokens,factor_percent}` 新建而非复用 `PricingSpec`**：蓝图要的是「上下文长度门槛→倍率」的**条件计费面**（GPT-6 >272K 2×、Gemini >200K 2×），与 `PricingSpec` 的「每 token 单价+峰谷时段」全局单价面不同构；`factor_percent` 沿用 `PeakPeriod.factor_percent` 整数百分比惯例（100=1×）规避 f32 精度红线。
5. **`effective_window ≤ advertised_window` 不变量走文档约定 + 先验表测试机证**：全 crate 无运行时校验先例（L0 纯类型边界，ADR-033），校验责任落写入边界（探针回写侧）。
6. **公开先验灌入表 `external_prior_profile`**：7 主流模型按 MRCR v2 8-needle（2026-05 基准快照）落 effective_window 初始值（ExternalPrior 档）——Gemini 3.1 Pro 260K（26.3% 惨案值）、Llama 4 Scout 10M/500 万 Inferred 特例、DeepSeek 384K 输出等；海外厂商经 `ProviderId::Custom` 键入，闭集不扩（新增一等变体须 ADR 且破坏下游穷尽 match）。

**依赖方向**（蓝图 §5.2 铁律落地）：mca-gateway（L10）经 `new()` 写入、hcw-window（L2）经事件只读，HCW 永不感知 L10；零新增依赖（L0 白名单 G-99 不动）。

## D3 — TractPlan 进程级缓存与 ffmpeg memoize（WP-G，提交 5895f7eb）

- `OnceLock<Mutex<HashMap<PathBuf, Arc<TractPlan>>>>` 按完整模型路径键缓存——**拒绝单条目方案**（`NmcEncoder::new` 按序加载 4 个不同模型，单条目同序互相挤占、二构造起全 miss）；按完整路径键使「model_dir 变更必须重建」失效语义天然成立
- `is_ffmpeg_available` 改 `OnceLock<bool>` memoize（部署期静态资产假设，doc 注明）
- 收益诚实评估：当下生产收益≈0（encoder 零外部生产构造点）；真实收益 = bench 每进程省 8 次 ONNX 解析编译 + 未来接线兜底

## D4 — 有效窗口探针骨架（WP-B，mca-gateway）

`src/capability_probe.rs`（新，~560 行 + 11 单测）：

1. **三态闸门 `ProbeMode{Off,Observe,Apply}`，默认 Observe**（蓝图 §5.3 原文「默认 observe」裁决，专测锁定）。Off 档在触碰执行器前拒绝（计数器机证零调用）。
2. **`EffectiveWindowProbe::calibrate(profile, probe)`**：四档 `{0.25,0.5,0.75,0.9}×标称` 逐档测量检索准确率 → 80% 截点线性插值拟合（首个下穿对；全档 ≥0.8 保守取最大档 / 全档 <0.8 保守取最小档；非单调噪声跳过）。拟合算法与执行器解耦（`Fn(u32) -> Future<Result<f32>>` 注入，单测 mock 衰减函数 `1-0.5×(w/1M)` 验证 400K 截点）；真实 5 针检索探针（最便宜 flash 通道+缓存命中）属组合根装配面，本包不接。
3. **成本护栏**：`MIN_PROBE_INTERVAL = 1h` 限频（检查+置位一次锁内完成，Mutex 临界区不跨 await，C7）。
4. **observe 档语义**：profile 共享引用传入 + 模块零 L0 写入面（编译期保证「不改写 resident 定容输入」）；测量结果发布 `ContextCapabilityUpdated` + tracing 留档；Apply 档同流程、report.mode 标记可回写，**转 apply 需独立治理裁决**（RSI 三件套守门，蓝图 §5.3/§七）。
5. **`ProviderDegraded` 补生产方**：health.rs 新增独立 `DegradedPublisher`（`publish_degraded_if_any(bus)` 由 gateway 周期调用）——**不挂 `record()`**（高频同步路径硬塞 async publish 违反 C7/反模式 7）。去重 = 状态沿（rising edge）：`DashSet` 记录已宣布通道，首次恶化才发布、恢复清沿可再触发（三场景专测）。EWMA 基建与 `capability.rs::negotiate` 接缝零改动。

验收：mca-gateway 9 test target 全绿（lib 252 passed 含新增 14）+ clippy/fmt/`--all-features` check 干净。

## 验收

- 事件：event-bus 全 target 0 failed，TUI 穷举 4/0（六锁 156/123 机器绿）
- L0：47 unit + 3 doctest 0 failed，clippy/fmt 干净，零新增依赖
- nmc：157/0 与 R8 基线逐值一致，下游三 crate check 绿
- 探针（WP-B 回填）

## 回滚

每 WP 独立 commit 可单独 revert；事件变体纯增量（revert WP-0 即回 154 锁）；先验表为纯函数无状态；缓存层对 API 透明（字段私有，签名未变）。
