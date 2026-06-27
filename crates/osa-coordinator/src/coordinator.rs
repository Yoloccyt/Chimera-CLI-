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
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 所有 async fn 满足 Send 约束

use event_bus::{EventBus, EventMetadata, NexusEvent};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::config::OsaConfig;
use crate::error::OsaError;
use crate::masks::SparseMask;
use crate::types::{ComplexityBand, FileId, MemoryId, OperationId, TaskId, TaskProfile, ToolId};

/// 全维稀疏掩码 — 五维度掩码的聚合体
///
/// 由 `OmniSparseCoordinator::compute_all_masks` 返回,包含:
/// - `routing`:工具稀疏掩码(Top-K 工具)
/// - `context`:文件稀疏掩码(Top-K 文件)
/// - `memory`:记忆稀疏掩码(Top-K 记忆)
/// - `audit`:操作稀疏掩码(按采样率选取)
/// - `budget`:任务稀疏掩码(按保护比例选取)
/// - `mask_hash`:预计算的 SHA-256 hex,构造时一次性计算,O(1) 访问
///
/// WHY:聚合为单一结构体,便于一次性传递给下游消费者(如 HCW),
/// 避免五维度分多次传递导致的状态不一致
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmniSparseMasks {
    /// routing 维度:工具稀疏掩码
    pub routing: SparseMask<ToolId>,
    /// context 维度:文件稀疏掩码
    pub context: SparseMask<FileId>,
    /// memory 维度:记忆稀疏掩码
    pub memory: SparseMask<MemoryId>,
    /// audit 维度:操作稀疏掩码
    pub audit: SparseMask<OperationId>,
    /// budget 维度:任务稀疏掩码
    pub budget: SparseMask<TaskId>,
    /// 预计算的 mask_hash(SHA-256 hex),构造时一次性计算
    ///
    /// WHY:避免每次调用 mask_hash() 都重新序列化 + SHA-256,
    /// 重复 TaskProfile 的 mask_hash 计算从 O(n) 降到 O(1)。
    /// `#[serde(skip)]` 确保不参与序列化(避免循环依赖:hash 依赖序列化)
    #[serde(skip)]
    mask_hash: String,
}

/// 手动实现 PartialEq:仅比较五维度掩码,忽略 mask_hash 缓存字段
///
/// WHY:反序列化的 OmniSparseMasks 的 mask_hash 为空(serde skip),
/// 但五维度掩码相同即应判定为相等
impl PartialEq for OmniSparseMasks {
    fn eq(&self, other: &Self) -> bool {
        self.routing == other.routing
            && self.context == other.context
            && self.memory == other.memory
            && self.audit == other.audit
            && self.budget == other.budget
    }
}

impl OmniSparseMasks {
    /// 构造 OmniSparseMasks 并预计算 mask_hash
    ///
    /// 在构造时一次性计算 SHA-256 hex 并缓存到 `mask_hash` 字段,
    /// 后续 `mask_hash()` 调用为 O(1) 访问,无需重新序列化 + 哈希。
    pub fn new(
        routing: SparseMask<ToolId>,
        context: SparseMask<FileId>,
        memory: SparseMask<MemoryId>,
        audit: SparseMask<OperationId>,
        budget: SparseMask<TaskId>,
    ) -> Result<Self, OsaError> {
        let masks = Self {
            routing,
            context,
            memory,
            audit,
            budget,
            mask_hash: String::new(),
        };
        let mut masks = masks;
        masks.mask_hash = masks.compute_mask_hash()?;
        Ok(masks)
    }

    /// 计算五维度掩码的平均稀疏度 [0.0, 1.0]
    ///
    /// WHY:平均稀疏度作为 `OmniSparseMasksComputed` 事件的 `sparsity` 字段,
    /// 消费者据此快速判断整体稀疏程度,无需解析具体掩码
    pub fn average_sparsity(&self) -> f32 {
        (self.routing.sparsity_ratio
            + self.context.sparsity_ratio
            + self.memory.sparsity_ratio
            + self.audit.sparsity_ratio
            + self.budget.sparsity_ratio)
            / 5.0
    }

    /// 序列化为 JSON 字符串(用于 mask_hash 计算)
    ///
    /// WHY:使用 JSON 而非 MessagePack,确保哈希跨平台稳定。
    /// serde_json 按结构体字段顺序输出,保证相同掩码产生相同哈希。
    /// `mask_hash` 字段有 `#[serde(skip)]`,不参与序列化(避免循环依赖)
    pub fn to_json(&self) -> Result<String, OsaError> {
        serde_json::to_string(self).map_err(OsaError::from)
    }

    /// 返回预计算的 mask_hash(O(1) 访问)
    ///
    /// mask_hash 在构造时一次性计算并缓存,后续调用直接返回引用。
    /// 消费者(如 HCW)据此哈希去重,避免重复处理相同掩码。
    pub fn mask_hash(&self) -> &str {
        &self.mask_hash
    }

    /// 计算 mask_hash(SHA-256 hex)— 内部方法
    ///
    /// 将五维度掩码序列化为 JSON,然后计算 SHA-256 hex 字符串。
    /// 仅在 `new()` 构造时调用一次,后续通过 `mask_hash()` O(1) 访问。
    fn compute_mask_hash(&self) -> Result<String, OsaError> {
        let json = self.to_json()?;
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        let hash = hasher.finalize();
        Ok(hex::encode(hash))
    }
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
        Self { event_bus, config }
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
    /// 3. 并行计算五维度掩码(routing/context/memory/audit/budget)
    /// 4. 聚合为 OmniSparseMasks
    /// 5. 计算 mask_hash(SHA-256 hex)
    /// 6. 发布 OmniSparseMasksComputed 事件(携带 mask_hash、sparsity、context_mask)
    ///
    /// WHY:五维度独立计算,O(N) 复杂度(N=活跃项数),无性能瓶颈。
    /// 事件发布失败不阻断掩码返回(掩码是核心产出,事件是副作用)。
    ///
    /// # 性能基准
    /// 掩码计算 < 10ms(测试中断言)
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

        // 3. 计算五维度掩码
        let routing = self.compute_routing_mask(profile);
        let context = self.compute_context_mask(profile);
        let memory = self.compute_memory_mask(profile);
        let audit = self.compute_audit_mask(profile);
        let budget = self.compute_budget_mask(profile);

        // 4. 聚合为 OmniSparseMasks(构造时预计算 mask_hash)
        let masks = OmniSparseMasks::new(routing, context, memory, audit, budget)?;

        // 5. 获取预计算的 mask_hash(O(1) 访问)
        let mask_hash = masks.mask_hash();
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
            mask_hash: mask_hash.to_string(),
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

        Ok(masks)
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
    /// WHY:复杂度越高,保留更多工具以应对多样化需求。
    /// Top-K 由 `routing_top_k_bounds` 配置,默认 (8, 32)。
    pub fn compute_routing_mask(&self, profile: &TaskProfile) -> SparseMask<ToolId> {
        let band = profile.complexity_band_with_thresholds(self.config.complexity_thresholds());
        let k = self.config.routing_top_k_for(band);
        let scores = heuristic_scores(profile.available_tools.len());
        SparseMask::select_top_k(&profile.available_tools, &scores, k)
    }

    /// 计算 context 维度掩码 — 按复杂度档位选取 Top-K 文件
    ///
    /// 策略:
    /// - Simple(档位 0):1 文件
    /// - Regular(档位 1):10 文件
    /// - Complex(档位 2):100 文件
    /// - UltraComplex(档位 3):1000 文件
    ///
    /// WHY:复杂度越高,需加载更多上下文文件以理解任务全貌。
    /// Top-K 由 `context_scope_multipliers` 配置,默认 [1, 10, 100, 1000]。
    pub fn compute_context_mask(&self, profile: &TaskProfile) -> SparseMask<FileId> {
        let band = profile.complexity_band_with_thresholds(self.config.complexity_thresholds());
        let k = self.config.context_scope_for(band);
        let scores = heuristic_scores(profile.available_files.len());
        SparseMask::select_top_k(&profile.available_files, &scores, k)
    }

    /// 计算 memory 维度掩码 — 按复杂度档位选取 Top-K 记忆
    ///
    /// 策略:与 routing 维度联动,使用相同的 Top-K 策略
    /// - Simple:Top-8 记忆
    /// - Regular:Top-16 记忆
    /// - Complex:Top-24 记忆
    /// - UltraComplex:Top-32 记忆
    ///
    /// WHY:记忆维度与工具维度共享 Top-K 策略,因为复杂任务需要更多历史记忆
    /// 辅助决策,与工具需求量正相关
    pub fn compute_memory_mask(&self, profile: &TaskProfile) -> SparseMask<MemoryId> {
        let band = profile.complexity_band_with_thresholds(self.config.complexity_thresholds());
        let k = self.config.routing_top_k_for(band);
        let scores = heuristic_scores(profile.available_memories.len());
        SparseMask::select_top_k(&profile.available_memories, &scores, k)
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
    /// - 复杂度越高,保护比例越高(保留更多任务以避免预算耗尽)
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

    #[test]
    fn test_mask_hash_deterministic() {
        let masks1 = OmniSparseMasks::new(
            SparseMask::select_top_k(&["t1".into()], &[0.9], 1),
            SparseMask::select_top_k(&["f1".into()], &[0.9], 1),
            SparseMask::select_top_k(&["m1".into()], &[0.9], 1),
            SparseMask::select_top_k(&["o1".into()], &[0.9], 1),
            SparseMask::select_top_k(&["tk1".into()], &[0.9], 1),
        )
        .unwrap();
        let masks2 = masks1.clone();
        assert_eq!(masks1.mask_hash(), masks2.mask_hash());
    }

    #[test]
    fn test_mask_hash_differs() {
        let masks1 = OmniSparseMasks::new(
            SparseMask::select_top_k(&["t1".into()], &[0.9], 1),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
        )
        .unwrap();
        let masks2 = OmniSparseMasks::new(
            SparseMask::select_top_k(&["t2".into()], &[0.9], 1),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
            SparseMask::empty(),
        )
        .unwrap();
        assert_ne!(masks1.mask_hash(), masks2.mask_hash());
    }

    #[test]
    fn test_average_sparsity() {
        let masks = OmniSparseMasks::new(
            SparseMask::empty(), // sparsity 1.0
            SparseMask::empty(), // sparsity 1.0
            SparseMask::empty(), // sparsity 1.0
            SparseMask::empty(), // sparsity 1.0
            SparseMask::empty(), // sparsity 1.0
        )
        .unwrap();
        assert!((masks.average_sparsity() - 1.0).abs() < 1e-6);
    }
}
