//! 命名空间配额 — chimera-mas 资源配额上提
//!
//! 对应架构层: L0 Contracts（新建）
//! 对应 ADR: ADR-033
//! 对应不变量: INV-7（上下文预算界）/ INV-8（归档单调性）/ INV-9（委托图无环）
//!
//! # 设计决策(WHY)
//!
//! - **从 chimera-mas 上提**: 命名空间配额需被 L9 `chimera-mas` 与 L8 `parliament` /
//!   L8 `acb-governor` 共同消费，定义在 L0 避免跨层依赖
//!
//! - **INV-7 上下文预算界**: `memory_budget_mb` 限制命名空间内存上限，
//!   任意时刻 `m_total ≤ MEMORY_BUDGET_MB × MEMORY_BUDGET_UTILIZATION`
//!
//! - **INV-7/8 级联约束**: `max_agent_depth` 限制委托深度(默认 5)，
//!   深度计算 = 1(根) + 子任务级数，5 级时叶子必须 leaf(无 further delegation 能力)
//!
//! - **配额为静态上限**: 配额值在命名空间创建时确定，运行时不可动态调整。
//!   动态调整需经 Parliament 审议 + ASA 前置审计(走 EscalationHandler trait)
//!
//! # 完整实现时机
//!
//! 当前文件仅定义**类型骨架**（P2-W5.1），完整配额执行逻辑在 chimera-mas Stage B 落地:
//! - Stage B: chimera-mas 命名空间配额运行时检查 + INV-7/8/9 守护

use serde::{Deserialize, Serialize};

/// 命名空间配额 — 多 Agent 系统的资源上限
///
/// WHY: MAS-Q 四象限稳定分工要求每个命名空间有明确的资源边界，
/// 防止单个 Agent 任务耗尽全局资源(内存/委托深度/任务数)
///
/// # INV-7 上下文预算界
///
/// `memory_budget_mb` 与 `memory_budget_utilization` 共同约束:
/// 任意时刻 `m_total ≤ memory_budget_mb × memory_budget_utilization`
///
/// # INV-8 归档单调性
///
/// `archive_retention_days` 约束归档保留时长，
/// 归档沿 Hot→Warm→Cold→Ice 单向降级，禁止逆向升级
///
/// # INV-9 委托图无环(P3-W11.3)
///
/// `max_agent_depth` 限制委托深度(硬性递归限制，默认 5):
/// - 深度计算 = 1(根) + 子任务级数
/// - 5 级时叶子必须 leaf(无 further delegation 能力)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamespaceQuota {
    /// 命名空间标识(唯一)
    pub namespace_id: String,
    /// 内存预算上限(MB)
    ///
    /// WHY: INV-7 — 任意时刻命名空间内存总量不超过此值 × utilization
    pub memory_budget_mb: u64,
    /// 内存预算利用率 [0.0, 1.0](默认 0.9)
    ///
    /// WHY: 预留 10% 余量应对碎片化与 GC 延迟，避免 OOM
    pub memory_budget_utilization: f32,
    /// 最大委托深度(默认 5)
    ///
    /// WHY: INV-9 — 硬性递归限制，防止无限委托导致栈溢出
    pub max_agent_depth: u32,
    /// 最大并发任务数
    ///
    /// WHY: 防止单个命名空间独占调度器资源
    pub max_concurrent_tasks: u32,
    /// 最大任务队列长度
    ///
    /// WHY: 防止任务积压导致内存膨胀，超过时触发降级(LRU 淘汰 Warm/Cold)
    pub max_task_queue_length: u32,
    /// 归档保留天数
    ///
    /// WHY: INV-8 — 超过保留期的归档数据可被清理，但降级路径不可逆
    pub archive_retention_days: u32,
}

/// 默认配额限制常量 — 与 chimera-mas INV-7/8/9 规格对齐
///
/// WHY: 集中定义默认值，避免 magic number 散落各处。
/// `NamespaceQuota::default()` 与 `QuotaLimits` 常量保持一致
pub mod defaults {
    /// 默认内存预算(MB)— INV-7: 130MB
    pub const MEMORY_BUDGET_MB: u64 = 130;

    /// 默认内存预算利用率 — INV-7: 0.9(预留 10% 余量)
    pub const MEMORY_BUDGET_UTILIZATION: f32 = 0.9;

    /// 默认最大委托深度 — INV-9: 5(MAX_AGENT_DEPTH)
    pub const MAX_AGENT_DEPTH: u32 = 5;

    /// 默认最大并发任务数
    pub const MAX_CONCURRENT_TASKS: u32 = 16;

    /// 默认最大任务队列长度
    pub const MAX_TASK_QUEUE_LENGTH: u32 = 256;

    /// 默认归档保留天数 — INV-8: 30 天
    pub const ARCHIVE_RETENTION_DAYS: u32 = 30;
}

/// 配额限制常量集合 — 便于批量引用
///
/// WHY: 当需要引用全部默认值时(如配置校验、文档生成)，
/// 使用 `QuotaLimits` 比逐个引用 `defaults::*` 更清晰
#[derive(Debug, Clone, Copy)]
pub struct QuotaLimits;

impl QuotaLimits {
    /// 默认内存预算(MB)
    pub const MEMORY_BUDGET_MB: u64 = defaults::MEMORY_BUDGET_MB;
    /// 默认内存预算利用率
    pub const MEMORY_BUDGET_UTILIZATION: f32 = defaults::MEMORY_BUDGET_UTILIZATION;
    /// 默认最大委托深度
    pub const MAX_AGENT_DEPTH: u32 = defaults::MAX_AGENT_DEPTH;
    /// 默认最大并发任务数
    pub const MAX_CONCURRENT_TASKS: u32 = defaults::MAX_CONCURRENT_TASKS;
    /// 默认最大任务队列长度
    pub const MAX_TASK_QUEUE_LENGTH: u32 = defaults::MAX_TASK_QUEUE_LENGTH;
    /// 默认归档保留天数
    pub const ARCHIVE_RETENTION_DAYS: u32 = defaults::ARCHIVE_RETENTION_DAYS;
}

impl Default for NamespaceQuota {
    fn default() -> Self {
        Self {
            namespace_id: String::new(),
            memory_budget_mb: defaults::MEMORY_BUDGET_MB,
            memory_budget_utilization: defaults::MEMORY_BUDGET_UTILIZATION,
            max_agent_depth: defaults::MAX_AGENT_DEPTH,
            max_concurrent_tasks: defaults::MAX_CONCURRENT_TASKS,
            max_task_queue_length: defaults::MAX_TASK_QUEUE_LENGTH,
            archive_retention_days: defaults::ARCHIVE_RETENTION_DAYS,
        }
    }
}

impl NamespaceQuota {
    /// 创建指定命名空间的配额(使用默认限制)
    ///
    /// # 参数
    ///
    /// - `namespace_id`: 命名空间唯一标识
    pub fn new(namespace_id: impl Into<String>) -> Self {
        Self {
            namespace_id: namespace_id.into(),
            ..Default::default()
        }
    }

    /// 计算实际内存预算上限(字节)
    ///
    /// WHY: INV-7 — `memory_budget_mb × memory_budget_utilization × 1024 × 1024`
    ///
    /// # 实现说明
    ///
    /// 使用 `.round()` 四舍五入避免 f32 → f64 转换的精度损失
    /// (f32 的 0.9 在 f64 中为 0.8999999...,截断会导致偏差)
    pub fn effective_memory_limit_bytes(&self) -> u64 {
        let mb = self.memory_budget_mb as f64;
        let utilization = self.memory_budget_utilization as f64;
        (mb * utilization * 1024.0 * 1024.0).round() as u64
    }

    /// 校验配额合法性
    ///
    /// WHY: 创建/修改配额时调用，确保配置值在合理范围内
    ///
    /// # 返回
    ///
    /// - `Ok(())`: 配额合法
    /// - `Err(String)`: 配额非法，含错误描述
    pub fn validate(&self) -> Result<(), String> {
        if self.namespace_id.is_empty() {
            return Err("namespace_id 不能为空".to_string());
        }
        if self.memory_budget_mb == 0 {
            return Err("memory_budget_mb 不能为 0".to_string());
        }
        if !(0.0..=1.0).contains(&self.memory_budget_utilization) {
            return Err(format!(
                "memory_budget_utilization 必须在 [0.0, 1.0] 范围内，当前: {}",
                self.memory_budget_utilization
            ));
        }
        if self.max_agent_depth == 0 {
            return Err("max_agent_depth 不能为 0(至少允许根 Agent)".to_string());
        }
        if self.max_concurrent_tasks == 0 {
            return Err("max_concurrent_tasks 不能为 0".to_string());
        }
        if self.max_task_queue_length == 0 {
            return Err("max_task_queue_length 不能为 0".to_string());
        }
        Ok(())
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_quota() {
        let quota = NamespaceQuota::default();
        assert_eq!(quota.memory_budget_mb, defaults::MEMORY_BUDGET_MB);
        assert_eq!(quota.memory_budget_mb, 130);
        assert!((quota.memory_budget_utilization - 0.9).abs() < 1e-6);
        assert_eq!(quota.max_agent_depth, 5);
        assert_eq!(quota.max_concurrent_tasks, 16);
        assert_eq!(quota.max_task_queue_length, 256);
        assert_eq!(quota.archive_retention_days, 30);
    }

    #[test]
    fn test_quota_new() {
        let quota = NamespaceQuota::new("team-alpha");
        assert_eq!(quota.namespace_id, "team-alpha");
        assert_eq!(quota.memory_budget_mb, defaults::MEMORY_BUDGET_MB);
    }

    #[test]
    fn test_effective_memory_limit_bytes() {
        let quota = NamespaceQuota::new("test");
        // 130 MB × 0.9(f32) × 1024 × 1024 ≈ 122,683,389 bytes
        // WHY: f32 的 0.9 精度低于 f64,转换后略小于 0.9,
        // 使用 .round() 后约为 122,683,389(与实现路径一致)
        let expected = (130.0_f64 * (0.9f32 as f64) * 1024.0 * 1024.0).round() as u64;
        assert_eq!(quota.effective_memory_limit_bytes(), expected);
        // 验证近似值在合理范围内(±10 bytes 容差)
        let limit = quota.effective_memory_limit_bytes();
        assert!(
            (122_683_380..=122_683_400).contains(&limit),
            "effective_memory_limit_bytes = {limit}, expected ~122,683,389"
        );
    }

    #[test]
    fn test_validate_valid_quota() {
        let quota = NamespaceQuota::new("valid-ns");
        assert!(quota.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_namespace_id() {
        let quota = NamespaceQuota {
            namespace_id: String::new(),
            ..Default::default()
        };
        assert!(quota.validate().is_err());
    }

    #[test]
    fn test_validate_zero_memory_budget() {
        let quota = NamespaceQuota {
            namespace_id: "test".to_string(),
            memory_budget_mb: 0,
            ..Default::default()
        };
        assert!(quota.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_utilization() {
        let quota = NamespaceQuota {
            namespace_id: "test".to_string(),
            memory_budget_utilization: 1.5, // 超出 [0.0, 1.0]
            ..Default::default()
        };
        assert!(quota.validate().is_err());
    }

    #[test]
    fn test_validate_zero_agent_depth() {
        let quota = NamespaceQuota {
            namespace_id: "test".to_string(),
            max_agent_depth: 0,
            ..Default::default()
        };
        assert!(quota.validate().is_err());
    }

    #[test]
    fn test_inv7_constants() {
        // INV-7: 130MB × 0.9 = 117MB 有效预算
        assert_eq!(QuotaLimits::MEMORY_BUDGET_MB, 130);
        assert!((QuotaLimits::MEMORY_BUDGET_UTILIZATION - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_inv9_max_agent_depth() {
        // INV-9: MAX_AGENT_DEPTH = 5
        assert_eq!(QuotaLimits::MAX_AGENT_DEPTH, 5);
    }

    #[test]
    fn test_serde_roundtrip() {
        let quota = NamespaceQuota::new("serde-test");
        let json = serde_json::to_string(&quota).expect("序列化失败");
        let restored: NamespaceQuota = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(quota, restored);
    }
}
