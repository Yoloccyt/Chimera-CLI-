//! OmniSparseCoordinator 实现 — 五维度稀疏掩码计算与事件发布
//!
//! 对应架构层:L6 Router
//! 对应创新点:OSA / Ω-Sparse(Omni-Sparse Architecture)
//!
//! # 核心职责
//! - 基于 `TaskProfile` 一次性计算五维度稀疏掩码(routing/context/memory/audit/budget)
//! - 复杂度联动稀疏化:按 `complexity_score` 四档产生不同稀疏度掩码
//! - 发布 `OmniSparseMasksComputed` 事件(携带 `mask_hash`、`sparsity`),修正 V1 违规
//! - `mask_hash` 为五维度掩码序列化的 SHA-256 hex,消费者据此去重与拉取
//!
//! # V1 违规修正
//! 原架构:OSA(L6)直接 import HCW(L2)→ 向上依赖违规
//! 修正后:OSA 发布 `OmniSparseMasksComputed` 事件,HCW 订阅消费
//! OSA 不持有 HCW 的引用,仅通过事件传递 `context_mask`
//!
//! # ADR-033 类型上提(P2-W5.2)
//! `OmniSparseMasks` / `SparseMask<T>` / 五维度 ID 类型已上提至 L0 `nexus-contracts`,
//! 本 crate 改为 re-export,消除星型耦合(L6 3 router 共享同一类型)。
//! `mask_hash` 计算逻辑保留在本 crate(L6 依赖 sha2/hex),通过 `compute_omni_mask_hash`
//! 自由函数提供,因 L0 禁止依赖 sha2/hex。
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 所有 async fn 满足 Send 约束

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::{MemoryStrategy, MemoryStrategyProvider};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::config::OsaConfig;
use crate::error::OsaError;
use crate::masks::SparseMask;
use crate::six_dimension::SixDimensionAdjuster;
use crate::tool_pruning::{PruneResult, PruneToolSchema, ToolSchemaPruner};
use crate::types::{ComplexityBand, FileId, MemoryId, OperationId, TaskId, TaskProfile, ToolId};

// OmniSparseMasks 从 L0 nexus-contracts 统一导入(ADR-033, P2-W5.2)
//
// WHY:原定义在本 crate,被 L6 3 router(kvbsr-router/faae-router/sesa-router)依赖,
// 形成星型耦合。上提至 L0 后,3 router 可直接依赖 nexus-contracts 获取同一类型,
// 消除 osa_coordinator::OmniSparseMasks ≠ nexus_contracts::OmniSparseMasks 的类型分裂。
//
// 迁移说明:
// - L0 版本移除了 `mask_hash` 缓存字段(L0 禁止依赖 sha2/hex)
// - L0 版本的 `new()` 不再返回 Result(纯构造,无哈希计算)
// - `mask_hash` 计算逻辑保留在本 crate 的 `compute_omni_mask_hash` 自由函数
// - `average_sparsity()` / `routing_ids()` 等纯计算方法保留在 L0 类型上
pub use nexus_contracts::OmniSparseMasks;

/// 计算 OmniSparseMasks 的 SHA-256 哈希(原 mask_hash 字段逻辑迁移,ADR-033 P2-W5.2)
///
/// 将五维度掩码序列化为 JSON,然后计算 SHA-256 hex 字符串。
/// 消费者(如 HCW)据此哈希去重,避免重复处理相同掩码。
///
/// WHY:L0 `nexus-contracts` 禁止依赖 `sha2` / `hex`(仅允许 serde derive),
/// 因此 `mask_hash` 计算逻辑保留在 L6 `osa-coordinator`。本函数为纯函数,
/// 相同输入产生相同输出,可在并发环境安全调用。
///
/// # 参数
/// - `masks`:从 nexus-contracts 导入的 OmniSparseMasks 实例
///
/// # 返回
/// SHA-256 哈希的 hex 字符串(64 字符),或序列化失败错误
///
/// # 错误
/// - `OsaError::MaskComputationFailed`:JSON 序列化失败(理论上不会发生,除非类型定义变化)
///
/// # 示例
/// ```
/// use nexus_contracts::{FileId, MemoryId, OmniSparseMasks, OperationId, SparseMask, TaskId, ToolId};
/// use osa_coordinator::compute_omni_mask_hash;
///
/// let masks = OmniSparseMasks::new(
///     SparseMask::full(vec![ToolId::new("t1")]),
///     SparseMask::empty(),
///     SparseMask::empty(),
///     SparseMask::empty(),
///     SparseMask::empty(),
/// );
/// let hash = compute_omni_mask_hash(&masks).unwrap();
/// assert_eq!(hash.len(), 64); // SHA-256 hex = 64 字符
/// ```
pub fn compute_omni_mask_hash(masks: &OmniSparseMasks) -> Result<String, OsaError> {
    let json = serde_json::to_string(masks)?;
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let hash = hasher.finalize();
    Ok(hex::encode(hash))
}

/// OmniSparseCoordinator — 全维稀疏协调器主结构
///
/// 基于 `TaskProfile` 一次性计算五维度稀疏掩码,发布 `OmniSparseMasksComputed` 事件。
/// 可跨 async 任务共享(Send + Sync),所有方法满足 Send 约束。
///
/// # 架构红线
/// - 不持有 HCW 的引用(修正 V1 违规),仅通过 EventBus 传递 context_mask
/// - 掩码计算为纯函数,O(N) 复杂度(N=活跃项数),无性能瓶颈
/// - 事件发布失败不阻断掩码返回(掩码是核心产出,事件是副作用)
pub struct OmniSparseCoordinator {
    /// 事件总线(基于 Arc,Clone 廉价)
    event_bus: EventBus,
    /// 协调器配置
    config: OsaConfig,
    /// S2 记忆策略提供者(可选)— 用于 memory 维度自适应记忆策略选择(Task 2)
    ///
    /// WHY Option<Arc<dyn>>: 通过 L0 trait 解耦,避免 L6→L6 直接依赖
    /// (osa-coordinator 不直接依赖 omega-learner,依赖铁律 §2.2 合规)。
    /// None 时 fallback 到 StandardTopK(k_multiplier=1.0,当前行为,向后兼容)。
    memory_strategy_provider: Option<Arc<dyn MemoryStrategyProvider>>,
    /// 最近一次成功计算的掩码快照缓存（Phase 6 D-6 占位治理）
    ///
    /// WHY: 全局函数 `five_dimension_masks()` 返回全零占位属虚假数据固化；
    /// 真实数据源是 `compute_all_masks` 的动态计算结果。同步短临界区，
    /// 无持锁跨 await（红线 §4.4-1）。
    recent_masks: Arc<Mutex<Option<OmniSparseMasks>>>,
    /// W1(§11.5): 工具 schema 裁剪器 — routing 维度使用统计二次裁剪（ADR-084）
    ///
    /// WHY Option + Mutex: 在线喂入使用统计（record_tool_step）与
    /// compute_all_masks 裁剪并发共享；短临界区无持锁跨 await。
    tool_pruner: Option<Arc<Mutex<ToolSchemaPruner>>>,
    /// W1: 工具 schema token 估算表（tokens_saved 观测指标用，缺省 0 诚实降级）
    tool_schema_tokens: HashMap<String, u32>,
    /// W1: 裁剪保留数（None = 不裁剪；运行时可变，供 W2 六维调整器 D2 下发）
    tool_keep_count: Arc<Mutex<Option<usize>>>,
    /// W1: 最近一次裁剪结果（可观测性）
    last_prune_result: Arc<Mutex<Option<PruneResult>>>,
    /// W2(§11.3): 六维动态调整器 — D1-D6 控制面纯规则反馈（ADR-084 决策 1）
    ///
    /// WHY Option + Mutex: `apply_feedback` 事件驱动并发写入与
    /// `compute_all_masks` D2 读取共享；短临界区无持锁跨 await。
    dimension_adjuster: Option<Arc<Mutex<SixDimensionAdjuster>>>,
}

impl OmniSparseCoordinator {
    /// 创建协调器,使用默认配置
    pub fn new(event_bus: EventBus) -> Self {
        Self::with_config(event_bus, OsaConfig::default())
    }

    /// 创建协调器,使用自定义配置
    ///
    /// 配置在创建时校验,非法配置返回 `OsaError::InvalidConfig`
    pub fn with_config(event_bus: EventBus, config: OsaConfig) -> Self {
        Self {
            event_bus,
            config,
            // Task 2: 默认无 S2 provider,fallback 到 StandardTopK(向后兼容)
            memory_strategy_provider: None,
            // Phase 6 D-6: 快照缓存初始为空（未计算过 → snapshot 返回 None）
            recent_masks: Arc::new(Mutex::new(None)),
            // W1(§11.5): 裁剪器默认不注入（行为与 W1 前逐位一致）
            tool_pruner: None,
            tool_schema_tokens: HashMap::new(),
            tool_keep_count: Arc::new(Mutex::new(None)),
            last_prune_result: Arc::new(Mutex::new(None)),
            // W2(§11.3): 调整器默认不注入
            dimension_adjuster: None,
        }
    }

    /// 设置记忆策略提供者(Task 2:用于集成 omega-learner S2)
    ///
    /// WHY builder 模式: OSA 构造时通常无 S2 provider(omega-learner 尚未启动),
    /// 通过 builder 方法在 S2 学习器就绪后注入,避免构造函数参数膨胀。
    /// 注入后 memory 维度将根据 task_phase 自适应选择记忆策略。
    pub fn with_memory_strategy_provider(
        mut self,
        provider: Arc<dyn MemoryStrategyProvider>,
    ) -> Self {
        self.memory_strategy_provider = Some(provider);
        self
    }

    /// W1(§11.5): 注入工具裁剪器 — routing 维度使用统计二次裁剪（Dressage）
    ///
    /// 注入后需配合 [`with_tool_keep_count`] / [`set_tool_keep_count`] 设定
    /// 保留数（未设定 = 不裁剪，保持纯相关性 Top-K 行为，向后兼容）。
    pub fn with_tool_pruner(mut self, pruner: ToolSchemaPruner) -> Self {
        self.tool_pruner = Some(Arc::new(Mutex::new(pruner)));
        self
    }

    /// W1: 注入工具 schema token 估算表（tokens_saved 观测指标用）
    ///
    /// WHY 可选: 裁剪决策依赖使用统计（频率/成功率/新近度）而非 token
    /// 估算；缺省 0 时 tokens_saved 恒 0（诚实降级，不伪造估算值）。
    pub fn with_tool_schema_tokens(mut self, tokens: HashMap<String, u32>) -> Self {
        self.tool_schema_tokens = tokens;
        self
    }

    /// W1: 设置裁剪保留数（D2.max_tools_per_step 控制面入口）
    pub fn with_tool_keep_count(self, keep: usize) -> Self {
        if let Ok(mut slot) = self.tool_keep_count.lock() {
            *slot = Some(keep);
        }
        self
    }

    /// W1: 运行时更新裁剪保留数（供 W2 六维调整器 D2 动态下发）
    ///
    /// None = 关闭裁剪（回到纯相关性 Top-K）。
    pub fn set_tool_keep_count(&self, keep: Option<usize>) {
        if let Ok(mut slot) = self.tool_keep_count.lock() {
            *slot = keep;
        }
    }

    /// W1: 工具裁剪器共享句柄（在线喂入使用统计 `record_tool_step`）
    pub fn tool_pruner_handle(&self) -> Option<Arc<Mutex<ToolSchemaPruner>>> {
        self.tool_pruner.clone()
    }

    /// W1: 最近一次裁剪结果（可观测性）
    pub fn last_prune_result(&self) -> Option<PruneResult> {
        self.last_prune_result
            .lock()
            .ok()
            .and_then(|cache| cache.clone())
    }

    /// W2(§11.3): 注入六维动态调整器（D1-D6 控制面纯规则反馈）
    ///
    /// 注入后 `compute_all_masks` 的裁剪 keep 优先级:
    /// 显式 `tool_keep_count` > 调整器当前契约 `D2.max_tools_per_step`。
    /// 需配合 [`start_dimension_adjustment_loop`] 启动事件反馈消费。
    pub fn with_dimension_adjuster(mut self, adjuster: SixDimensionAdjuster) -> Self {
        self.dimension_adjuster = Some(Arc::new(Mutex::new(adjuster)));
        self
    }

    /// W2: 六维调整器共享句柄（外部直接读取契约 / 手动喂入反馈）
    pub fn dimension_adjuster_handle(&self) -> Option<Arc<Mutex<SixDimensionAdjuster>>> {
        self.dimension_adjuster.clone()
    }

    /// W2(§11.3): 启动六维调整事件订阅任务（后台 tokio task）
    ///
    /// 订阅 EventBus 全量事件,`SixDimensionAdjuster::apply_feedback` 仅响应
    /// 四个反馈变体（HcwRecallDegraded / RouterStatsReported /
    /// BudgetExceeded / EntropyBalanced），其余静默忽略（零新增事件变体,
    /// ADR-084 决策 2）。
    ///
    /// 返回 `JoinHandle` 供调用者管理任务生命周期;未绑定调整器返回 None。
    ///
    /// # Week 6 教训 - broadcast 时序
    /// `bus.subscribe()` 必须在 `tokio::spawn` 之前同步调用（§4.4-3）,
    /// 否则事件静默丢失。持锁不跨 await: recv 完成后才取锁,apply_feedback
    /// 为同步短临界区。
    pub fn start_dimension_adjustment_loop(&self) -> Option<tokio::task::JoinHandle<()>> {
        let adjuster = self.dimension_adjuster.clone()?;
        // 在 spawn 之前同步订阅,确保不会错过后续发布的事件
        let mut rx = self.event_bus.subscribe();

        Some(tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                // 无持锁跨 await: recv 完成后短临界区应用反馈
                if let Ok(mut adj) = adjuster.lock() {
                    adj.apply_feedback(&event);
                }
            }
        }))
    }

    /// 获取配置引用(用于测试与调试)
    pub fn config(&self) -> &OsaConfig {
        &self.config
    }

    /// 获取事件总线引用(用于测试与调试)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 计算全维稀疏掩码 — 一次性生成五维度掩码并发布事件
    ///
    /// 流程:
    /// 1. 校验 TaskProfile 合法性(complexity_score ∈ [0.0, 1.0])
    /// 2. 判定复杂度档位(Simple/Regular/Complex/UltraComplex)
    /// 3. 并行计算五维度掩码(routing/context/memory/audit/budget)— Task 6
    /// 4. 聚合为 OmniSparseMasks(L0 类型,无 mask_hash 缓存)
    /// 5. 计算 mask_hash(SHA-256 hex,通过 `compute_omni_mask_hash` 自由函数)
    /// 6. 发布 OmniSparseMasksComputed 事件(携带 mask_hash、sparsity、context_mask)
    ///
    /// WHY:五维度独立计算,O(N) 复杂度(N=活跃项数),无性能瓶颈。
    /// 事件发布失败不阻断掩码返回(掩码是核心产出,事件是副作用)。
    ///
    /// # Task 6: 五维度并行计算(std::thread::scope)
    ///
    /// 原实现顺序调用 5 个 compute_*_mask 方法(O(5N) 顺序开销),Task 6 改为
    /// `std::thread::scope` 并行计算,5 个维度在独立 OS 线程中同时计算。
    ///
    /// WHY thread::scope 而非 tokio::join!:compute_*_mask 是同步纯函数,
    /// thread::scope 比 async 更轻量(无需 runtime 切换,直接 OS 线程并行)。
    /// `std::thread::scope`(Rust 1.63+)是 safe API,`#![forbid(unsafe_code)]` 合规,
    /// 且 scope 保证所有派生线程在 scope 结束前 join,引用生命周期由编译器保证。
    ///
    /// WHY 纯函数前提:5 个 compute_*_mask 方法均只读 `&self` 和 `&profile`,
    /// 无 `&mut self`,无外部副作用(无 event_bus.publish),tracing 日志线程安全。
    /// `OmniSparseCoordinator` 字段(event_bus/config/memory_strategy_provider)
    /// 均为 `Send + Sync`,可在多线程间共享 `&self`。
    ///
    /// # ADR-033 迁移说明(P2-W5.2)
    /// `OmniSparseMasks::new()` 迁移至 L0 后不再返回 Result(纯构造),
    /// `mask_hash` 从缓存字段改为通过 `compute_omni_mask_hash(&masks)?` 现算。
    ///
    /// # 性能基准
    /// 掩码计算 < 10ms(测试中断言);并行版在 50 工具 + 2000 文件规模下
    /// 相比顺序版有显著延迟降低(见 benches/parallel_vs_sequential.rs)
    pub async fn compute_all_masks(
        &self,
        profile: &TaskProfile,
    ) -> Result<OmniSparseMasks, OsaError> {
        // 1. 校验 TaskProfile 合法性
        self.validate_profile(profile)?;

        // 2. 判定复杂度档位
        let band = profile.complexity_band_with_thresholds(self.config.complexity_thresholds());
        debug!(
            task_id = %profile.task_id,
            complexity = profile.complexity_score,
            band = band.as_str(),
            "开始计算全维稀疏掩码"
        );

        // 3. 并行计算五维度掩码(Task 6: std::thread::scope)
        //
        // 5 个 compute_*_mask 方法均为纯函数(只读 &self 和 &profile),无 &mut self,
        // 无外部副作用(事件发布移到并行计算之后,见步骤 6)。5 个维度在独立 OS 线程中
        // 同时计算,消解 O(5N) 顺序开销。
        //
        // WHY thread::scope 而非 tokio::join!:compute_*_mask 是同步纯函数,
        // thread::scope 直接派生 OS 线程并行,无需 async runtime 切换开销。
        // scope 保证所有派生线程在 scope 结束前 join,引用生命周期由编译器保证。
        //
        // WHY expect 而非 ? :spawn 返回的 JoinResult::join() 失败表示计算线程 panic,
        // 属于不可恢复的程序错误(非业务错误),用 expect 直接 panic 符合"内部代码信任"原则。
        // 闭包内调用的是纯函数,无外部 IO,panic 仅可能来自底层分配失败(已超出 OsaError 范畴)。
        let (mut routing, context, memory, audit, budget) = std::thread::scope(|s| {
            // 每个闭包捕获 &self 和 &profile,scope 保证引用在 scope 内有效
            // WHY 五个 spawn 而非 rayon::join:五维度计算相互独立,无需工作窃取,
            // std::thread::scope 直接派生 5 个 OS 线程,开销最低
            let r_routing = s.spawn(|| self.compute_routing_mask(profile));
            let r_context = s.spawn(|| self.compute_context_mask(profile));
            let r_memory = s.spawn(|| self.compute_memory_mask(profile));
            let r_audit = s.spawn(|| self.compute_audit_mask(profile));
            let r_budget = s.spawn(|| self.compute_budget_mask(profile));

            // 顺序 join(顺序无关,5 个线程已并行启动)
            // WHY join 顺序不影响结果:5 个线程已通过 spawn 并行启动,
            // join 顺序仅影响主线程等待顺序,不改变并行性
            let routing = r_routing
                .join()
                .expect("routing mask 计算线程 panic:纯函数不应失败,检查底层分配");
            let context = r_context
                .join()
                .expect("context mask 计算线程 panic:纯函数不应失败,检查底层分配");
            let memory = r_memory
                .join()
                .expect("memory mask 计算线程 panic:纯函数不应失败,检查底层分配");
            let audit = r_audit
                .join()
                .expect("audit mask 计算线程 panic:纯函数不应失败,检查底层分配");
            let budget = r_budget
                .join()
                .expect("budget mask 计算线程 panic:纯函数不应失败,检查底层分配");

            (routing, context, memory, audit, budget)
        });

        // W1(§11.5): routing 维度使用统计二次裁剪（Dressage 闭环）
        //
        // 终态 routing 掩码 = 相关性 Top-K 幸存者 ∩ 使用统计保留集
        // （白名单钉住 + 门控 + Top-K 补足）；未注入 pruner / 未设
        // keep_count 时为 no-op（行为与 W1 前逐位一致）。
        // 必须在 mask_hash 计算与事件发布之前——哈希与事件反映裁剪后终态。
        let _prune_result = self.apply_usage_pruning(&mut routing);

        // 4. 聚合为 OmniSparseMasks(L0 类型,纯构造不返回 Result)
        let masks = OmniSparseMasks::new(routing, context, memory, audit, budget);

        // 5. 计算 mask_hash(通过自由函数,L6 依赖 sha2/hex)
        // WHY:L0 禁止依赖 sha2/hex,哈希逻辑留在 L6
        let mask_hash = compute_omni_mask_hash(&masks)?;
        let sparsity = masks.average_sparsity();

        // SubTask 14.3:将 context 维度活跃 FileId 转换为 Vec<String> 携带在事件中
        // WHY:event-bus 在 L1 不能依赖 OSA(L6)的 FileId newtype,
        // FileId 实现了 Display trait,用 to_string() 转换为字符串形式
        let context_mask: Vec<String> = masks
            .context
            .active_ids
            .iter()
            .map(|f| f.to_string())
            .collect();

        // 6. 发布 OmniSparseMasksComputed 事件(修正 V1 违规)
        // SubTask 14.3:事件携带 context_mask,HCW 订阅后直接使用
        let event = NexusEvent::OmniSparseMasksComputed {
            metadata: EventMetadata::new("osa-coordinator"),
            // clone 避免 move:info! 宏后续仍需借用 mask_hash 做日志记录
            mask_hash: mask_hash.clone(),
            sparsity,
            context_mask,
        };
        // 事件发布失败不阻断掩码返回,仅记录告警
        if let Err(e) = self.event_bus.publish(event).await {
            tracing::warn!(
                task_id = %profile.task_id,
                error = %e,
                "OmniSparseMasksComputed 事件发布失败(不影响掩码返回)"
            );
        }

        info!(
            task_id = %profile.task_id,
            band = band.as_str(),
            mask_hash = %mask_hash,
            sparsity,
            "全维稀疏掩码计算完成,事件已发布"
        );

        // Phase 6 D-6: 写入快照缓存（真实数据源，替代全零占位）
        if let Ok(mut cache) = self.recent_masks.lock() {
            *cache = Some(masks.clone());
        }

        Ok(masks)
    }

    /// 最近一次成功计算的掩码同步快照（Phase 6 D-6 占位治理）
    ///
    /// 返回 None 表示尚未成功计算过。调用方（如 TUI 面板）应改用
    /// 本方法替代已弃用的全局函数 `five_dimension_masks()`（全零占位）。
    pub fn snapshot(&self) -> Option<OmniSparseMasks> {
        self.recent_masks
            .lock()
            .ok()
            .and_then(|cache| cache.clone())
    }

    /// W1(§11.5): routing 维度使用统计二次裁剪 — 相关性 Top-K 幸存者上
    /// 应用 Dressage 使用统计（白名单钉住 §18.3 + 门控 + Top-K 补足）
    ///
    /// 返回 None 的全部情形（防御性 no-op，不 panic）:
    /// - 未注入 pruner / 未设定 keep_count（默认，行为向后兼容）
    /// - routing 幸存者为空 / 锁中毒（跳过裁剪，掩码保持相关性 Top-K 终态）
    fn apply_usage_pruning(&self, routing: &mut SparseMask<ToolId>) -> Option<PruneResult> {
        // keep 来源优先级: 显式 tool_keep_count(W1) > 六维调整器 D2(W2) > 不裁剪
        let explicit_keep = *self.tool_keep_count.lock().ok()?;
        let keep = match explicit_keep {
            Some(explicit) => explicit,
            None => self
                .dimension_adjuster
                .as_ref()?
                .lock()
                .ok()?
                .current_contract()
                .d2_tool
                .max_tools_per_step,
        };
        let pruner_arc = self.tool_pruner.as_ref()?;
        // 中毒锁 → 跳过裁剪: 掩码保持相关性 Top-K 终态（保守回退）
        let mut pruner = pruner_arc.lock().ok()?;
        let active: Vec<ToolId> = routing.active_ids.clone();
        if active.is_empty() {
            return None;
        }
        // schema_tokens 缺省 0: 裁剪决策依赖使用统计而非 token 估算
        let available: Vec<PruneToolSchema> = active
            .iter()
            .map(|tool| {
                let name = tool.to_string();
                let schema_tokens = self.tool_schema_tokens.get(&name).copied().unwrap_or(0);
                PruneToolSchema { name, schema_tokens }
            })
            .collect();
        let result = pruner.prune_tools(&available, keep);
        // 重建 routing 掩码: 仅保留裁剪幸存者（白名单钉住项必在 kept 内）
        let kept_names: HashSet<String> = result.kept.iter().map(|t| t.name.clone()).collect();
        let survived: Vec<ToolId> = active
            .iter()
            .filter(|tool| kept_names.contains(tool.as_str()))
            .cloned()
            .collect();
        debug!(
            available = available.len(),
            kept = survived.len(),
            tokens_saved = result.tokens_saved,
            "W1 工具裁剪完成（routing 维度使用统计二次裁剪）"
        );
        *routing = SparseMask::full(survived);
        if let Ok(mut cache) = self.last_prune_result.lock() {
            *cache = Some(result.clone());
        }
        Some(result)
    }

    /// 校验 TaskProfile 合法性
    ///
    /// 校验规则:
    /// - complexity_score ∈ [0.0, 1.0]
    fn validate_profile(&self, profile: &TaskProfile) -> Result<(), OsaError> {
        if !(0.0..=1.0).contains(&profile.complexity_score) {
            return Err(OsaError::InvalidTaskProfile(format!(
                "complexity_score = {} 超出 [0.0, 1.0]",
                profile.complexity_score
            )));
        }
        Ok(())
    }
}

impl OmniSparseCoordinator {
    /// 计算 routing 维度掩码 — 按复杂度档位选取 Top-K 工具
    ///
    /// 策略:
    /// - Simple(档位 0):Top-8 工具
    /// - Regular(档位 1):Top-16 工具
    /// - Complex(档位 2):Top-24 工具
    /// - UltraComplex(档位 3):Top-32 工具
    ///
    /// 评分来源:
    /// - `profile.routing_scores = Some(vec)` 时用真实评分做 Top-K(基于相关性)
    /// - `profile.routing_scores = None` 时 fallback 到 `heuristic_scores`(前 K 个)
    ///
    /// WHY:复杂度越高,保留更多工具以应对多样化需求。
    /// Top-K 由 `routing_top_k_bounds` 配置,默认 (8, 32)。
    /// 评分字段让上游可注入语义相关性分数,实现真正的 Top-K 而非"前 K 个"。
    pub fn compute_routing_mask(&self, profile: &TaskProfile) -> SparseMask<ToolId> {
        let band = profile.complexity_band_with_thresholds(self.config.complexity_thresholds());
        let k = self.config.routing_top_k_for(band);
        // 评分来源:优先用 profile.routing_scores,None 时 fallback 到 heuristic_scores
        // WHY:heuristic_scores 用索引负相关评分使 Top-K 退化为"前 K 个",
        // profile 携带真实评分时用真实评分,实现基于相关性的 Top-K
        let heuristic = heuristic_scores(profile.available_tools.len());
        let scores = profile.routing_scores.as_ref().unwrap_or(&heuristic);
        SparseMask::select_top_k(&profile.available_tools, scores, k)
    }

    /// 计算 context 维度掩码 — 按复杂度档位选取 Top-K 文件
    ///
    /// 策略:
    /// - Simple(档位 0):1 文件
    /// - Regular(档位 1):10 文件
    /// - Complex(档位 2):100 文件
    /// - UltraComplex(档位 3):1000 文件
    ///
    /// 评分来源:
    /// - `profile.context_scores = Some(vec)` 时用真实评分做 Top-K(基于相关性)
    /// - `profile.context_scores = None` 时 fallback 到 `heuristic_scores`(前 K 个)
    ///
    /// WHY:复杂度越高,需加载更多上下文文件以理解任务全貌。
    /// Top-K 由 `context_scope_multipliers` 配置,默认 [1, 10, 100, 1000]。
    /// 评分字段让上游可注入语义相关性分数,实现真正的 Top-K 而非"前 K 个"。
    pub fn compute_context_mask(&self, profile: &TaskProfile) -> SparseMask<FileId> {
        let band = profile.complexity_band_with_thresholds(self.config.complexity_thresholds());
        let k = self.config.context_scope_for(band);
        // 评分来源:优先用 profile.context_scores,None 时 fallback 到 heuristic_scores
        // WHY:heuristic_scores 用索引负相关评分使 Top-K 退化为"前 K 个",
        // profile 携带真实评分时用真实评分,实现基于相关性的 Top-K
        let heuristic = heuristic_scores(profile.available_files.len());
        let scores = profile.context_scores.as_ref().unwrap_or(&heuristic);
        SparseMask::select_top_k(&profile.available_files, scores, k)
    }

    /// 计算 memory 维度掩码 — 按复杂度档位选取 Top-K 记忆(Task 2: S2 自适应)
    ///
    /// 策略:
    /// - 基础 Top-K 由复杂度档位决定(与 routing 联动:Simple=8/Regular=16/Complex=24/UltraComplex=32)
    /// - Task 2 集成 S2: 若注入了 `memory_strategy_provider`,根据 `task_phase`
    ///   自适应选择记忆策略,用策略的 `k_multiplier()` 调整基础 K:
    ///   - MinimalRecall(Initial): K × 0.5(快速响应,减少噪声)
    ///   - StandardTopK(默认): K × 1.0(当前行为,向后兼容)
    ///   - QueryReformulation(Stuck): K × 1.5(多角度查询,扩大召回)
    ///   - AggressivePruning(LongRun): K × 0.25(长跑抑制噪声累积)
    ///   - TimeFocused: K × 1.0(K 不变,差异在时间过滤而非数量)
    /// - 未注入 provider 时,fallback 到 StandardTopK(K × 1.0,当前行为)
    ///
    /// 评分来源:
    /// - `profile.memory_scores = Some(vec)` 时用真实评分做 Top-K(基于相关性)
    /// - `profile.memory_scores = None` 时 fallback 到 `heuristic_scores`(前 K 个)
    ///
    /// WHY S2 集成: 三重悖论"记忆悖论"修复 — 固定 top-k 召回在任务阶段切换时
    /// 产生"幽灵记忆",S2 通过 task_phase 驱动的自适应 k_multiplier 使记忆策略
    /// 随任务阶段动态调整。
    pub fn compute_memory_mask(&self, profile: &TaskProfile) -> SparseMask<MemoryId> {
        let band = profile.complexity_band_with_thresholds(self.config.complexity_thresholds());
        let base_k = self.config.routing_top_k_for(band);

        // Task 2: S2 自适应记忆策略 — 根据 task_phase 调整基础 Top-K
        // WHY: 三重悖论"记忆悖论"修复,记忆策略随任务阶段自适应
        let strategy = self.select_memory_strategy(profile);
        let adjusted_k = apply_k_multiplier(base_k, strategy.k_multiplier());

        // 评分来源:优先用 profile.memory_scores,None 时 fallback 到 heuristic_scores
        // WHY:heuristic_scores 用索引负相关评分使 Top-K 退化为"前 K 个",
        // profile 携带真实评分时用真实评分,实现基于相关性的 Top-K
        let heuristic = heuristic_scores(profile.available_memories.len());
        let scores = profile.memory_scores.as_ref().unwrap_or(&heuristic);
        SparseMask::select_top_k(&profile.available_memories, scores, adjusted_k)
    }

    /// 根据 task_phase 选择记忆策略(Task 2: S2 桥接)
    ///
    /// - 注入了 `memory_strategy_provider` 时:调用 provider 根据 phase 选择策略
    /// - 未注入 provider 时:fallback 到 `StandardTopK`(k_multiplier=1.0,向后兼容)
    /// - `profile.task_phase = None` 时:用 `MemoryTaskPhase::default()`(Initial)
    ///
    /// WHY 分离为独立方法: 便于测试验证策略选择逻辑,且 compute_memory_mask
    /// 关注 Top-K 选择,策略选择是正交关注点
    fn select_memory_strategy(&self, profile: &TaskProfile) -> MemoryStrategy {
        match &self.memory_strategy_provider {
            // C4 合规: 未注入 provider 时 fallback 到 StandardTopK(编译进二进制的 const)
            None => MemoryStrategy::StandardTopK,
            // S2 集成: 调用 provider 根据 task_phase 选择策略
            // phase 为 None 时用 MemoryTaskPhase::default()(Initial,保守召回)
            Some(provider) => {
                let phase = profile.task_phase.unwrap_or_default();
                provider.select_strategy(phase)
            }
        }
    }

    /// 计算 audit 维度掩码 — 按复杂度档位与风险等级选取操作
    ///
    /// 策略:
    /// - Simple:采样率 10%(复杂度默认)
    /// - Regular:采样率 50%
    /// - Complex:采样率 100%(全审计)
    /// - UltraComplex:采样率 100%(全审计 + 实时告警)
    ///
    /// 风险等级调整:实际采样率取复杂度档位默认值与风险等级配置值的最大值(更保守)
    ///
    /// WHY:高风险任务需更密集审计,即使复杂度低也应提高采样率。
    /// 例如:Simple 档位 + Critical 风险 → max(0.1, 1.0) = 1.0(全审计)
    pub fn compute_audit_mask(&self, profile: &TaskProfile) -> SparseMask<OperationId> {
        let band = profile.complexity_band_with_thresholds(self.config.complexity_thresholds());
        let complexity_rate = complexity_audit_rate(band);
        let risk_rate = self.config.audit_rate_for(profile.risk_level.as_index());
        // 取最大值(更保守):复杂度与风险任一高则提高采样率
        let audit_rate = complexity_rate.max(risk_rate);

        let total = profile.recent_operations.len();
        if total == 0 {
            return SparseMask::empty();
        }
        // 计算保留数量,至少 1 个(若 audit_rate > 0)
        let k = if audit_rate >= 1.0 {
            total
        } else {
            ((total as f32) * audit_rate).ceil() as usize
        };
        let k = k.min(total);
        let scores = heuristic_scores(profile.recent_operations.len());
        SparseMask::select_top_k(&profile.recent_operations, &scores, k)
    }

    /// 计算 budget 维度掩码 — 按保护比例与复杂度选取任务
    ///
    /// 策略:
    /// - 保护比例 = threshold × (0.5 + complexity × 0.5)
    /// - 复杂度越高,保护比例越高(保留更多任务预算用于并行执行)
    /// - 保留数量 = ceil(active_tasks.len() × protection_ratio)
    ///
    /// WHY:复杂任务消耗更多预算,需保留更多活跃任务以并行执行,
    /// 避免预算耗尽导致任务中断。简单任务预算充足,可只保留高优先级任务。
    pub fn compute_budget_mask(&self, profile: &TaskProfile) -> SparseMask<TaskId> {
        let total = profile.active_tasks.len();
        if total == 0 {
            return SparseMask::empty();
        }
        // 保护比例:复杂度越高,保留越多任务(降低稀疏度)
        // protection = threshold × (0.5 + complexity × 0.5)
        // complexity=0 → protection=threshold×0.5(默认 0.4,保留 40%)
        // complexity=1 → protection=threshold×1.0(默认 0.8,保留 80%)
        // WHY:复杂任务预算紧张,保留更多任务以并行执行;简单任务预算充足,稀疏化
        let protection =
            self.config.budget_protection_threshold * (0.5 + profile.complexity_score * 0.5);
        let k = ((total as f32) * protection).ceil() as usize;
        let k = k.clamp(1, total);
        let scores = heuristic_scores(profile.active_tasks.len());
        SparseMask::select_top_k(&profile.active_tasks, &scores, k)
    }

    /// 计算上下文 token 预算 — 复杂度档位 → 可用 token 数（ADR-069 Token 效率优化）
    ///
    /// 与 `compute_budget_mask`（任务级稀疏）互补：
    /// - `compute_budget_mask`: 决定保留哪些任务（TaskId 维度）
    /// - `compute_token_budget`: 决定上下文窗口可用 token 数（供 HCW trim_to_budget 消费）
    ///
    /// 复杂度越高，分配更多 token 预算（复杂任务需要更多上下文）。
    /// BudgetExceeded 事件触发时，调用方可降低 budget_ratio 实现紧急裁剪。
    pub fn compute_token_budget(
        &self,
        complexity: ComplexityBand,
        base_context_window: u32,
        budget_ratio: f32,
    ) -> u32 {
        // 复杂度档位 → 上下文利用率（简单任务不需填满窗口）
        let utilization = match complexity {
            ComplexityBand::Simple => 0.25,
            ComplexityBand::Regular => 0.50,
            ComplexityBand::Complex => 0.75,
            ComplexityBand::UltraComplex => 1.0,
        };
        let raw = (base_context_window as f32) * utilization * budget_ratio.clamp(0.0, 1.0);
        (raw as u32).max(1024) // 最少 1K token，避免过度裁剪
    }
}

/// 按复杂度档位返回默认 audit 采样率
///
/// 对应架构手册四档分级:
/// - Simple:10%
/// - Regular:50%
/// - Complex:100%
/// - UltraComplex:100%
fn complexity_audit_rate(band: ComplexityBand) -> f32 {
    match band {
        ComplexityBand::Simple => 0.1,
        ComplexityBand::Regular => 0.5,
        ComplexityBand::Complex => 1.0,
        ComplexityBand::UltraComplex => 1.0,
    }
}

/// 生成启发式评分向量:索引越小,评分越高(前 K 个为 Top-K)
///
/// WHY:SubTask 13.10 — TaskProfile 暂未携带五维度评分,用索引负相关评分作为启发式,
/// 使 Top-K 退化为前 K 个(保持与旧签名相同的行为),且确保 `select_nth_unstable_by`
/// 产生确定的顺序(相同输入 → 相同输出,保证 `mask_hash` 一致性)。
/// 未来可在 TaskProfile 中添加各维度的评分字段,实现真正的 Top-K。
fn heuristic_scores(len: usize) -> Vec<f32> {
    if len == 0 {
        return Vec::new();
    }
    (0..len).map(|i| 1.0 - (i as f32 / len as f32)).collect()
}

/// 应用 S2 策略的 k_multiplier 调整基础 Top-K(Task 2)
///
/// 公式: `adjusted_k = ceil(base_k × multiplier)`,最小为 1(当 base_k ≥ 1 时)
///
/// WHY 独立函数: 将策略调整逻辑与 Top-K 选择逻辑分离,便于单元测试验证
/// k_multiplier 的正确应用,且 compute_memory_mask 关注掩码生成而非数值计算。
///
/// WHY ceil 而非 floor: AggressivePruning(multiplier=0.25)在 base_k=8 时
/// 得到 2.0,ceil=2 与 floor=2 相同;但 base_k=7 时得到 1.75,ceil=2 保留
/// 更多记忆(保守策略偏向召回),floor=1 可能过度剪枝。
///
/// # 参数
/// - `base_k`: 基础 Top-K(由复杂度档位决定,8/16/24/32)
/// - `multiplier`: S2 策略的调整因子(0.25/0.5/1.0/1.5)
///
/// # 返回
/// 调整后的 K 值,至少为 1(当 base_k ≥ 1 时),不超过 base_k × 2(合理上界)
fn apply_k_multiplier(base_k: usize, multiplier: f32) -> usize {
    if base_k == 0 {
        return 0;
    }
    // f32 全程保持 f32(§4.4 教训 6: f32 禁止隐式转 f64 比较)
    let adjusted = (base_k as f32) * multiplier;
    // ceil 向上取整,最小为 1
    let k = adjusted.ceil() as usize;
    k.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AffectedScope, RiskLevel, TaskType, TimePressure};

    /// 构造测试用 TaskProfile
    fn make_profile(complexity: f32, risk: RiskLevel) -> TaskProfile {
        TaskProfile {
            task_id: "t-1".into(),
            task_type: TaskType::Read,
            complexity_score: complexity,
            risk_level: risk,
            time_pressure: TimePressure::Low,
            affected_scope: AffectedScope::Local,
            available_tools: (0..50).map(|i| ToolId::new(format!("tool-{i}"))).collect(),
            available_files: (0..2000)
                .map(|i| FileId::new(format!("file-{i}")))
                .collect(),
            available_memories: (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect(),
            recent_operations: (0..100)
                .map(|i| OperationId::new(format!("op-{i}")))
                .collect(),
            active_tasks: (0..10).map(|i| TaskId::new(format!("task-{i}"))).collect(),
            // 评分字段默认 None:测试 fallback 到 heuristic_scores 的行为
            routing_scores: None,
            context_scores: None,
            memory_scores: None,
            // Task 2: task_phase 默认 None,测试 fallback 到 Initial 的行为
            task_phase: None,
        }
    }

    #[test]
    fn test_complexity_audit_rate() {
        assert!((complexity_audit_rate(ComplexityBand::Simple) - 0.1).abs() < 1e-6);
        assert!((complexity_audit_rate(ComplexityBand::Regular) - 0.5).abs() < 1e-6);
        assert!((complexity_audit_rate(ComplexityBand::Complex) - 1.0).abs() < 1e-6);
        assert!((complexity_audit_rate(ComplexityBand::UltraComplex) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_validate_profile_valid() {
        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);
        let profile = make_profile(0.5, RiskLevel::Medium);
        assert!(coord.validate_profile(&profile).is_ok());
    }

    #[test]
    fn test_validate_profile_invalid_complexity() {
        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);
        let profile = make_profile(1.5, RiskLevel::Low);
        let err = coord.validate_profile(&profile).unwrap_err();
        assert!(matches!(err, OsaError::InvalidTaskProfile(_)));
    }

    /// ADR-033 P2-W5.2:验证 compute_omni_mask_hash 的确定性(相同掩码 → 相同哈希)
    ///
    /// 迁移后 mask_hash 不再是 OmniSparseMasks 的缓存字段,
    /// 而是通过 `compute_omni_mask_hash` 自由函数现算。
    /// 相同的 OmniSparseMasks 实例应产生相同的哈希。
    #[test]
    fn test_mask_hash_deterministic() {
        let masks1 = OmniSparseMasks::new(
            SparseMask::select_top_k(&["t1".into()], &[0.9], 1),
            SparseMask::select_top_k(&["f1".into()], &[0.9], 1),
            SparseMask::select_top_k(&["m1".into()], &[0.9], 1),
            SparseMask::select_top_k(&["o1".into()], &[0.9], 1),
            SparseMask::select_top_k(&["tk1".into()], &[0.9], 1),
        );
        let masks2 = masks1.clone();
        let hash1 = compute_omni_mask_hash(&masks1).unwrap();
        let hash2 = compute_omni_mask_hash(&masks2).unwrap();
        assert_eq!(hash1, hash2, "相同掩码的哈希应一致");
    }

    /// ADR-033 P2-W5.2:验证不同掩码产生不同哈希
    #[test]
    fn test_mask_hash_differs() {
        let masks1 = OmniSparseMasks::new(
            SparseMask::select_top_k(&["t1".into()], &[0.9], 1),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
        );
        let masks2 = OmniSparseMasks::new(
            SparseMask::select_top_k(&["t2".into()], &[0.9], 1),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
        );
        let hash1 = compute_omni_mask_hash(&masks1).unwrap();
        let hash2 = compute_omni_mask_hash(&masks2).unwrap();
        assert_ne!(hash1, hash2, "不同掩码的哈希应不同");
    }

    /// ADR-033 P2-W5.2:验证 average_sparsity(从 L0 类型继承)
    #[test]
    fn test_average_sparsity() {
        let masks = OmniSparseMasks::new(
            SparseMask::empty(), // sparsity 1.0
            SparseMask::empty(), // sparsity 1.0
            SparseMask::empty(), // sparsity 1.0
            SparseMask::empty(), // sparsity 1.0
            SparseMask::empty(), // sparsity 1.0
        );
        assert!((masks.average_sparsity() - 1.0).abs() < 1e-6);
    }

    /// ADR-033 P2-W5.2:验证 compute_omni_mask_hash 返回 64 字符的 hex 字符串
    #[test]
    fn test_compute_omni_mask_hash_returns_hex_64_chars() {
        let masks = OmniSparseMasks::new(
            SparseMask::select_top_k(&["t1".into()], &[0.9], 1),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
        );
        let hash = compute_omni_mask_hash(&masks).unwrap();
        assert_eq!(hash.len(), 64, "SHA-256 hex 应为 64 字符");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "哈希应为纯 hex 字符"
        );
    }

    // ============================================================
    // Task 2: apply_k_multiplier 单元测试
    // ============================================================

    #[test]
    fn test_apply_k_multiplier_standard() {
        // StandardTopK: multiplier=1.0,K 不变
        assert_eq!(apply_k_multiplier(8, 1.0), 8);
        assert_eq!(apply_k_multiplier(16, 1.0), 16);
        assert_eq!(apply_k_multiplier(32, 1.0), 32);
    }

    #[test]
    fn test_apply_k_multiplier_minimal_recall() {
        // MinimalRecall: multiplier=0.5,K 减半
        assert_eq!(apply_k_multiplier(8, 0.5), 4);
        assert_eq!(apply_k_multiplier(16, 0.5), 8);
    }

    #[test]
    fn test_apply_k_multiplier_aggressive_pruning() {
        // AggressivePruning: multiplier=0.25,K 四分之一
        assert_eq!(apply_k_multiplier(8, 0.25), 2);
        assert_eq!(apply_k_multiplier(16, 0.25), 4);
    }

    #[test]
    fn test_apply_k_multiplier_query_reformulation() {
        // QueryReformulation: multiplier=1.5,K 扩大
        assert_eq!(apply_k_multiplier(8, 1.5), 12);
        assert_eq!(apply_k_multiplier(16, 1.5), 24);
    }

    #[test]
    fn test_apply_k_multiplier_ceil_behavior() {
        // ceil 向上取整: 7 × 0.5 = 3.5 → ceil = 4
        assert_eq!(apply_k_multiplier(7, 0.5), 4);
        // 7 × 0.25 = 1.75 → ceil = 2
        assert_eq!(apply_k_multiplier(7, 0.25), 2);
    }

    #[test]
    fn test_apply_k_multiplier_minimum_one() {
        // base_k ≥ 1 时,adjusted_k 至少为 1
        // 1 × 0.25 = 0.25 → ceil = 1
        assert_eq!(apply_k_multiplier(1, 0.25), 1);
    }

    #[test]
    fn test_apply_k_multiplier_zero_base() {
        // base_k = 0 时返回 0(空候选集场景)
        assert_eq!(apply_k_multiplier(0, 1.0), 0);
        assert_eq!(apply_k_multiplier(0, 0.5), 0);
    }

    // ============================================================
    // Task 2: select_memory_strategy 单元测试
    // ============================================================

    /// Mock provider: 返回固定策略(用于测试策略注入)
    struct MockFixedStrategyProvider {
        strategy: MemoryStrategy,
    }

    impl nexus_contracts::MemoryStrategyProvider for MockFixedStrategyProvider {
        fn select_strategy(&self, _phase: nexus_contracts::MemoryTaskPhase) -> MemoryStrategy {
            self.strategy
        }
    }

    #[test]
    fn test_select_strategy_no_provider_falls_back_to_standard() {
        // 未注入 provider → fallback 到 StandardTopK
        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);
        let profile = make_profile(0.5, RiskLevel::Medium);
        let strategy = coord.select_memory_strategy(&profile);
        assert_eq!(strategy, MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_select_strategy_with_provider_returns_provider_strategy() {
        // 注入 mock provider(返回 AggressivePruning)→ 返回 AggressivePruning
        let bus = EventBus::new();
        let provider: Arc<dyn nexus_contracts::MemoryStrategyProvider> =
            Arc::new(MockFixedStrategyProvider {
                strategy: MemoryStrategy::AggressivePruning,
            });
        let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
        let profile = make_profile(0.5, RiskLevel::Medium);
        let strategy = coord.select_memory_strategy(&profile);
        assert_eq!(strategy, MemoryStrategy::AggressivePruning);
    }

    #[test]
    fn test_select_strategy_none_phase_uses_default_initial() {
        // task_phase = None 时用 MemoryTaskPhase::default()(Initial)
        // mock provider 记录收到的 phase,验证 None → Initial
        use std::sync::Mutex as StdMutex;

        struct PhaseRecordingProvider {
            received_phase: StdMutex<Option<nexus_contracts::MemoryTaskPhase>>,
        }

        impl nexus_contracts::MemoryStrategyProvider for PhaseRecordingProvider {
            fn select_strategy(&self, phase: nexus_contracts::MemoryTaskPhase) -> MemoryStrategy {
                *self.received_phase.lock().unwrap() = Some(phase);
                MemoryStrategy::StandardTopK
            }
        }

        let bus = EventBus::new();
        let provider = Arc::new(PhaseRecordingProvider {
            received_phase: StdMutex::new(None),
        });
        let provider_weak = Arc::downgrade(&provider);
        let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
        let profile = make_profile(0.5, RiskLevel::Medium);
        // task_phase = None(默认)
        let _ = coord.select_memory_strategy(&profile);

        // 验证 provider 收到的是 Initial(MemoryTaskPhase::default())
        let received = provider_weak.upgrade().unwrap();
        let recorded = received.received_phase.lock().unwrap();
        assert_eq!(*recorded, Some(nexus_contracts::MemoryTaskPhase::Initial));
    }

    // ============================================================
    // Task 2: compute_memory_mask 集成测试(有/无 provider)
    // ============================================================

    #[test]
    fn test_compute_memory_mask_no_provider_uses_base_k() {
        // 未注入 provider → StandardTopK(k_multiplier=1.0)→ memory mask 大小 = base_k
        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);
        // Regular 档位(complexity=0.4 ∈ [0.25, 0.5))→ base_k = 16
        let mut profile = make_profile(0.4, RiskLevel::Medium);
        profile.available_memories = (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect();
        let mask = coord.compute_memory_mask(&profile);
        // base_k=16,k_multiplier=1.0 → adjusted_k=16
        assert_eq!(mask.active_ids.len(), 16);
    }

    #[test]
    fn test_compute_memory_mask_with_aggressive_pruning_reduces_k() {
        // 注入 provider(返回 AggressivePruning)→ k_multiplier=0.25 → adjusted_k=4
        let bus = EventBus::new();
        let provider: Arc<dyn nexus_contracts::MemoryStrategyProvider> =
            Arc::new(MockFixedStrategyProvider {
                strategy: MemoryStrategy::AggressivePruning,
            });
        let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
        // Regular 档位(complexity=0.4 ∈ [0.25, 0.5))→ base_k = 16
        let mut profile = make_profile(0.4, RiskLevel::Medium);
        profile.available_memories = (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect();
        let mask = coord.compute_memory_mask(&profile);
        // base_k=16,k_multiplier=0.25 → 16×0.25=4.0 → ceil=4
        assert_eq!(mask.active_ids.len(), 4);
    }

    #[test]
    fn test_compute_memory_mask_with_query_reformulation_increases_k() {
        // 注入 provider(返回 QueryReformulation)→ k_multiplier=1.5 → adjusted_k=24
        let bus = EventBus::new();
        let provider: Arc<dyn nexus_contracts::MemoryStrategyProvider> =
            Arc::new(MockFixedStrategyProvider {
                strategy: MemoryStrategy::QueryReformulation,
            });
        let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
        // Regular 档位(complexity=0.4 ∈ [0.25, 0.5))→ base_k = 16
        let mut profile = make_profile(0.4, RiskLevel::Medium);
        profile.available_memories = (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect();
        let mask = coord.compute_memory_mask(&profile);
        // base_k=16,k_multiplier=1.5 → 16×1.5=24.0 → ceil=24
        assert_eq!(mask.active_ids.len(), 24);
    }

    #[test]
    fn test_compute_memory_mask_with_minimal_recall_halves_k() {
        // 注入 provider(返回 MinimalRecall)→ k_multiplier=0.5 → adjusted_k=8
        let bus = EventBus::new();
        let provider: Arc<dyn nexus_contracts::MemoryStrategyProvider> =
            Arc::new(MockFixedStrategyProvider {
                strategy: MemoryStrategy::MinimalRecall,
            });
        let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
        // Regular 档位(complexity=0.4 ∈ [0.25, 0.5))→ base_k = 16
        let mut profile = make_profile(0.4, RiskLevel::Medium);
        profile.available_memories = (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect();
        let mask = coord.compute_memory_mask(&profile);
        // base_k=16,k_multiplier=0.5 → 16×0.5=8.0 → ceil=8
        assert_eq!(mask.active_ids.len(), 8);
    }
}
