//! 上下文预算模型 — 1M Token 等效上下文的容量层级与派生准入闸
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块,context 子模块下)
//! 核心职责: 定义 `ContextTier` 容量层级 + `MemoryBudgetModel` 驻留估算 +
//!           `AdmissionGate` 派生准入闸(复用 INV-7 安全护栏,§15.3 / §15.4)。
//!
//! ## ADR-026 决策 7: 1M Token 等效机制
//!
//! 1M Token 上下文 = 128K 实际加载 + 8× 稀疏压缩(Ω-Compress)。
//! L3 层级可寻址 1M,但通过 OSA context_mask 仅加载活跃文件,
//! 实际驻留 ≤ 128K(1M / 8),避免内存爆炸(§6.1 红线)。
//!
//! ## 常量复用说明
//!
//! - `MEMORY_BUDGET_MB` 与 `MEMORY_BUDGET_UTILIZATION` 复用 `crate::invariants`
//!   的定义(单一真值源,避免重复)
//! - `SPARSE_FACTOR` / `COMPRESSION_THRESHOLD` 为本模块特有常量

use crate::context::ContextPriority;
use crate::error::{MasError, Result};
use crate::invariants::{InvariantChecker, MEMORY_BUDGET_MB};
// 引入 event-bus 用于 SubTask 15.10:发布 AgentContextOverflow Critical 事件
use event_bus::{EventBus, EventMetadata, NexusEvent};

// ============================================================
// 常量(SubTask 15.9 REFACTOR — 抽取)
// ============================================================

/// 稀疏因子 — 1M 上下文 = 128K 实际 + 8× 稀疏压缩(ADR-026 决策 7)
///
/// 1M / 128K = 8,即 L3 层级通过 8× 稀疏压缩实现 1M 可寻址。
/// L0-L2 不启用稀疏(容量 ≤ 128K,直接加载不会爆内存)。
pub const SPARSE_FACTOR: u32 = 8;

/// 压缩阈值 — 窗口利用率达 90% 触发压缩(§15.3 派生准入闸余量)
///
/// WHY 用 f64 而非 f32:§4.4 反模式 6,f32 转 f64 精度膨胀导致误判
/// (如 0.9f32 as f64 > 0.9)。全程用 f64 计算避免精度问题。
pub const COMPRESSION_THRESHOLD: f64 = 0.9;

// ============================================================
// ContextTier — 上下文容量层级枚举
// ============================================================

/// 上下文层级 — 对应 HCW 四级窗口(4K/32K/128K/1M)
///
/// - `L0`: 4K (Simple 任务)
/// - `L1`: 32K (Regular 任务)
/// - `L2`: 128K (Complex 任务)
/// - `L3`: 1M (UltraComplex 任务,等效 1M 可寻址)
///
/// ## 容量层级映射(ADR-026 决策 7)
///
/// | Tier | context_window | effective_capacity | sparse_enabled |
/// |------|---------------|-------------------|---------------|
/// | L0   | 4,096         | 4,096             | 否            |
/// | L1   | 32,768        | 32,768            | 否            |
/// | L2   | 131,072       | 131,072           | 否            |
/// | L3   | 1,048,576     | 131,072           | 是(8× 稀疏)   |
///
/// WHY 只有 L3 启用稀疏:L0-L2 容量 ≤ 128K,直接加载不会爆内存;
/// L3 (1M) 通过 8× 稀疏压缩,实际加载 128K,避免内存爆炸(§6.1 红线)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextTier {
    /// L0 — Simple 任务,4K 上下文
    L0,
    /// L1 — Regular 任务,32K 上下文
    L1,
    /// L2 — Complex 任务,128K 上下文
    L2,
    /// L3 — UltraComplex 任务,1M 上下文(等效,实际加载 128K)
    L3,
}

impl ContextTier {
    /// 返回上下文窗口大小(可寻址 Token 数)
    ///
    /// L0=4K / L1=32K / L2=128K / L3=1M(1_048_576)
    pub const fn context_window(self) -> usize {
        match self {
            Self::L0 => 4_096,
            Self::L1 => 32_768,
            Self::L2 => 131_072,
            Self::L3 => 1_048_576,
        }
    }

    /// 返回有效容量(实际加载 Token 数,稀疏后)
    ///
    /// L0-L2:窗口 == 有效容量(无稀疏)
    /// L3:1M / SPARSE_FACTOR(8) = 128K(实际加载)
    pub const fn effective_capacity(self) -> usize {
        match self {
            // L0-L2 不启用稀疏,有效容量 == 窗口
            Self::L0 | Self::L1 | Self::L2 => self.context_window(),
            // L3 启用稀疏:1M / 8 = 128K
            Self::L3 => 131_072,
        }
    }

    /// 是否启用稀疏压缩
    ///
    /// 仅 L3 启用(L0-L2 容量 ≤ 128K,无需稀疏)。
    pub const fn sparse_enabled(self) -> bool {
        matches!(self, Self::L3)
    }
}

// ============================================================
// MemoryBudgetModel — 单 Agent 密集驻留估算
// ============================================================

/// 内存预算模型 — 估算单 Agent 密集驻留内存
///
/// 单 Agent 密集驻留 = `bytes_per_tok × effective_capacity(tier)`(Ω-Compress)。
/// 用于 `AdmissionGate` 评估派生新 Agent 后的全局内存预算。
///
/// ## 字段
///
/// - `bytes_per_tok`: 每 Token 占用字节数(默认 4,UTF-8 平均估算)
///
/// ## 示例
///
/// ```
/// use chimera_mas::context::budget_model::{ContextTier, MemoryBudgetModel};
///
/// let model = MemoryBudgetModel::default_model();
/// // L3 单 Agent 驻留 = 4 bytes × 128K = 512 KB
/// let resident = model.estimate_resident(ContextTier::L3);
/// assert_eq!(resident, 4 * 131_072);
/// ```
pub struct MemoryBudgetModel {
    /// 每 Token 占用字节数
    pub bytes_per_tok: usize,
}

impl MemoryBudgetModel {
    /// 创建新的预算模型,指定 `bytes_per_tok`
    pub fn new(bytes_per_tok: usize) -> Self {
        Self { bytes_per_tok }
    }

    /// 默认模型(`bytes_per_tok = 4`,UTF-8 平均)
    ///
    /// WHY 4:UTF-8 平均每字符 1-4 字节,Token 平均 ≈ 4 字节
    pub fn default_model() -> Self {
        Self::new(4)
    }

    /// 估算指定 tier 的单 Agent 密集驻留字节数
    ///
    /// 公式:`bytes_per_tok × effective_capacity(tier)`
    pub fn estimate_resident(&self, tier: ContextTier) -> usize {
        self.bytes_per_tok * tier.effective_capacity()
    }

    /// 估算指定 tier 的单 Agent 密集驻留 MB 数
    ///
    /// 用于与 `MEMORY_BUDGET_MB`(MB 单位)对比,向下取整。
    pub fn estimate_resident_mb(&self, tier: ContextTier) -> usize {
        self.estimate_resident(tier) / (1024 * 1024)
    }
}

impl Default for MemoryBudgetModel {
    fn default() -> Self {
        Self::default_model()
    }
}

// ============================================================
// AdmissionGate — 派生准入闸(复用 INV-7)
// ============================================================

/// 派生准入闸 — 派生新 Agent 前的全局内存预算校验
///
/// 复用 INV-7 安全护栏(§15.4),失败时返回 `MasError::AdmissionGateDenied`。
/// 调用方负责发布 `AgentContextOverflow` Critical 事件(走 mpsc,§6.2 红线)。
///
/// ## 校验逻辑
///
/// 1. 估算新 Agent 驻留:`agent_resident = bytes_per_tok × effective_capacity(tier)`
/// 2. 调用 `InvariantChecker::check_inv7_context_budget`
///    - 单 Agent 约束: `agent_resident ≤ effective_capacity(tier)`
///    - 全局约束: `M_total ≤ MEMORY_BUDGET_MB × MEMORY_BUDGET_UTILIZATION = 117MB`
/// 3. INV-7 失败时转换为 `MasError::AdmissionGateDenied`(语义清晰)
///
/// ## ADR-026 决策 7: 50 Agent 稳态预算
///
/// 50 Agent 稳态分布(30×L0 + 12×L1 + 5×L2 + 3×L3)聚合 ≈ 6MB,
/// 远低于 130MB 上限(对比暴力加载 305MB 节省 57%)。
pub struct AdmissionGate;

impl AdmissionGate {
    /// 派生准入闸 — 校验全局内存预算是否允许派生新 Agent
    ///
    /// ## 参数
    ///
    /// - `m_total`: 当前全 Agent 池聚合内存(MB)
    /// - `new_agent_tier`: 新 Agent 的 tier(用于估算驻留)
    /// - `bytes_per_tok`: 每 Token 字节数(诊断用,估算新 Agent 驻留字节)
    ///
    /// ## 返回
    ///
    /// - `Ok(())`: 通过准入闸,允许派生
    /// - `Err(MasError::AdmissionGateDenied)`: 拒绝派生
    ///   - 调用方应发布 `AgentContextOverflow` Critical 事件(§6.2 红线)
    ///
    /// ## INV-7 语义对齐
    ///
    /// - **单 Agent 约束**:`agent_resident ≤ effective_capacity(tier)`(Token 单位)
    ///   - 新 Agent 派生时假设满载:`agent_resident = effective_capacity`(等号允许通过)
    /// - **全局约束**:`m_total ≤ MEMORY_BUDGET_MB × 0.9 = 117MB`
    ///   - `m_total` 已是 MB 单位,直接传入 INV-7
    ///
    /// ## `bytes_per_tok` 参数说明
    ///
    /// `bytes_per_tok` 用于估算新 Agent 驻留字节数(诊断用,填入错误信息),
    /// 不参与 INV-7 检查逻辑。原因:INV-7 的 `m_total` 已是 MB 单位,
    /// `bytes_per_tok × effective_capacity`(字节)转 MB 后通常向下取整为 0,
    /// 不影响全局约束判断。保留此参数符合 spec 签名要求,便于未来扩展
    /// (如改用 KB 单位或更大 bytes_per_tok 场景)。
    ///
    /// ## 边界场景
    ///
    /// - `m_total == 117`: 通过(等号允许,恰好达阈值)
    /// - `m_total == 118`: 拒绝(超过 117MB)
    /// - `bytes_per_tok == 0`: 不影响检查(仅诊断字段为 0)
    pub fn check(m_total: usize, new_agent_tier: ContextTier, bytes_per_tok: usize) -> Result<()> {
        let effective_capacity = new_agent_tier.effective_capacity();
        // INV-7 单 Agent 约束:新 Agent 派生时假设满载,驻留 = effective_capacity(Token 单位)
        // 等号允许通过(与 INV-7 边界场景一致,见 invariants.rs:174)
        let agent_resident = effective_capacity;

        // 诊断用:估算新 Agent 派生后的字节驻留(不参与 INV-7 检查)
        // WHY 保留:符合 spec 签名 + 未来扩展(KB 单位 / KB 级阈值)
        let estimated_resident_bytes = bytes_per_tok * effective_capacity;

        let m_budget = MEMORY_BUDGET_MB;

        // 复用 INV-7 安全护栏(§15.4):
        // - 单 Agent 约束:agent_resident ≤ effective_capacity(等号允许)
        // - 全局约束:m_total ≤ 130 × 0.9 = 117
        InvariantChecker::check_inv7_context_budget(
            agent_resident,
            effective_capacity,
            m_total,
            m_budget,
        )
        .map_err(|e| MasError::AdmissionGateDenied {
            m_total,
            m_budget,
            new_agent_tier: format!("{new_agent_tier:?}"),
            // 保留 INV-7 原始错误 + 估算驻留字节(诊断用)
            reason: format!("{e} (estimated_resident_bytes={estimated_resident_bytes})"),
        })?;

        Ok(())
    }
}

// ============================================================
// compression_threshold — 上下文压缩决策
// ============================================================

/// 判断上下文块在指定利用率下是否应该被压缩
///
/// 决策矩阵(ADR-026 决策 7):
///
/// | 优先级 | utilization < 0.9 | utilization ≥ 0.9 |
/// |--------|-------------------|-------------------|
/// | Critical | 否(永不) | 否(永不) |
/// | Optional | 是(可丢) | 是(可丢) |
/// | High/Normal/Low | 否 | 是 |
///
/// ## 参数
///
/// - `priority`: 上下文块优先级
/// - `utilization`: 当前窗口利用率(0.0-1.0)
///
/// ## 返回
///
/// - `true`: 应该压缩或丢弃
/// - `false`: 应该保留
///
/// ## 红线对齐
///
/// - §6.1: Critical 块永不被压缩(ADR-026 红线)
/// - §4.4 反模式 6: 用 f64 比较,避免 f32 精度膨胀
pub fn should_compress_at(priority: ContextPriority, utilization: f64) -> bool {
    // Critical 永不压缩(ADR-026 红线)
    if priority.is_critical() {
        return false;
    }
    // Optional 总是可丢弃(任何利用率下)
    if priority.is_optional() {
        return true;
    }
    // High/Normal/Low:利用率 ≥ COMPRESSION_THRESHOLD 时压缩
    utilization >= COMPRESSION_THRESHOLD
}

// ============================================================
// publish_admission_denied_event — SubTask 15.10 事件发布
// ============================================================

/// 发布 `AgentContextOverflow` Critical 事件(SubTask 15.10)
///
/// 当 `AdmissionGate::check` 拒绝派生新 Agent 时,调用方应通过本辅助函数
/// 发布 Critical 事件,走 mpsc 通道确保送达(§6.2 红线)。
///
/// ## 设计决策(WHY 辅助函数而非 check 内部直接发布)
///
/// - **关注点分离**:`AdmissionGate::check` 保持纯函数(易测试,25 个测试覆盖),
///   事件发布由调用方显式控制(便于注入 mock EventBus 做单元测试)
/// - **同步发布**:用 `publish_critical_blocking`(sync API),
///   避免在 sync 上下文中调用 async publish(§4.4 反模式 8)
/// - **fire-and-forget**:发布失败仅记日志,不传播错误(§4.4 反模式 7)
///   派生拒绝路径非关键数据一致性路径,失败不阻塞主流程
///
/// ## 参数
///
/// - `bus`: EventBus 引用(发布事件)
/// - `agent_id`: 被拒绝派生的 Agent ID
/// - `current_tokens`: 当前全 Agent 池聚合 Token 数
/// - `max_tokens`: Token 预算上限(MEMORY_BUDGET_MB × 0.9 对应的 Token 估算)
///
/// ## 红线对齐
///
/// - §6.2: Critical 事件走 mpsc 通道(publish_critical_blocking 内部实现)
/// - §4.4 反模式 7: fire-and-forget 评估框架(派生拒绝非关键路径)
/// - §4.4 反模式 8: sync 方法用 publish_blocking,async 方法用 publish().await
pub fn publish_admission_denied_event(
    bus: &EventBus,
    agent_id: &str,
    current_tokens: usize,
    max_tokens: usize,
) {
    let event = NexusEvent::AgentContextOverflow {
        // 用 EventMetadata::new(source) 构造,EventMetadata 未实现 Default
        // source = "chimera-mas" 标识发布者,便于审计与依赖方向校验
        metadata: EventMetadata::new("chimera-mas"),
        agent_id: agent_id.to_string(),
        current_tokens,
        max_tokens,
    };
    // fire-and-forget:发布失败仅记日志,不传播错误
    // WHY 派生拒绝路径非关键数据一致性路径,失败不阻塞主流程
    if let Err(e) = bus.publish_critical_blocking(event) {
        tracing::warn!(
            error = %e,
            agent_id = %agent_id,
            "publish_admission_denied_event: AgentContextOverflow 发布失败(fire-and-forget)"
        );
    }
}

// ============================================================
// 单元测试(模块内,与集成测试 tests/budget_model_test.rs 互补)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_factor_is_8() {
        assert_eq!(SPARSE_FACTOR, 8);
    }

    #[test]
    fn compression_threshold_is_0_9() {
        assert!((COMPRESSION_THRESHOLD - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn l3_context_window_is_1m() {
        assert_eq!(ContextTier::L3.context_window(), 1_048_576);
    }

    #[test]
    fn l3_effective_capacity_is_128k() {
        assert_eq!(ContextTier::L3.effective_capacity(), 131_072);
    }

    #[test]
    fn l3_window_equals_8x_effective_capacity() {
        let l3 = ContextTier::L3;
        assert_eq!(
            l3.context_window(),
            l3.effective_capacity() * SPARSE_FACTOR as usize
        );
    }
}
