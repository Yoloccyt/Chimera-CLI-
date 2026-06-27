//! GQEP 核心类型定义
//!
//! 包含聚集查询执行协议的核心领域类型:
//! - `OperationId`:操作唯一标识(基于 `nexus_core::id_newtype!` 宏)
//! - `GqepFuture<T>`:GQEP 异步操作 Future 类型别名
//! - `GatherResult`:聚集执行结果统计

use std::future::Future;
use std::pin::Pin;

use crate::error::GqepError;

// 使用 L1 Core 的 id_newtype! 宏生成 OperationId newtype
// WHY 集中宏:消除各 crate 重复实现 newtype,确保所有 ID 类型行为一致
// (Deref/AsRef/Borrow/From/Display),且 #[serde(transparent)] 保证向后兼容
nexus_core::id_newtype!(OperationId, "GQEP 操作唯一标识");

/// GQEP 异步操作 Future 类型
///
/// WHY `'static`:GQEP 需要将 future 放入 `FuturesUnordered` 长期持有,
/// 并可能跨 async fn 边界传递(如 QEEP `entangle_spawn` 要求 `'static`)。
/// `Send` 约束保证可跨线程调度(§4.1:所有 async fn 满足 Send + 'static)。
///
/// WHY `Pin<Box<dyn ...>>`:类型擦除使不同具体 future 类型可统一存储于
/// `Vec<GqepFuture<T>>`,调用者无需关心具体 future 类型。
pub type GqepFuture<T> = Pin<Box<dyn Future<Output = Result<T, GqepError>> + Send + 'static>>;

/// 聚集执行结果统计
///
/// 记录一次 `gather` 调用的整体执行情况,用于事件发布与监控指标。
/// 字段 `succeeded + failed` 可能小于 `total`(批量原子性场景下,
/// 失败后剩余操作不执行)。
#[derive(Debug, Clone)]
pub struct GatherResult {
    /// 批次中总操作数(传入的 futures 数量)
    pub total: u32,
    /// 成功操作数
    pub succeeded: u32,
    /// 失败操作数(含超时、执行错误)
    pub failed: u32,
    /// 聚集延迟(毫秒),从 gather 开始到所有 future 完成的总耗时
    pub latency_ms: f32,
    /// 失败操作的错误列表(仅含失败操作的错误,长度 == failed)
    pub errors: Vec<GqepError>,
}

impl GatherResult {
    /// 创建全成功的聚集结果
    ///
    /// 便捷构造器,用于测试或已知全部成功的场景。
    pub fn all_success(total: u32, latency_ms: f32) -> Self {
        Self {
            total,
            succeeded: total,
            failed: 0,
            latency_ms,
            errors: Vec::new(),
        }
    }

    /// 判断是否全部成功(无失败操作)
    pub fn is_all_success(&self) -> bool {
        self.failed == 0
    }
}

impl Default for GatherResult {
    fn default() -> Self {
        Self {
            total: 0,
            succeeded: 0,
            failed: 0,
            latency_ms: 0.0,
            errors: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_id_newtype() {
        let id = OperationId::new("op-001");
        assert_eq!(id.as_str(), "op-001");
        // Deref<Target=str> 允许 &id 当作 &str
        let s: &str = &id;
        assert_eq!(s, "op-001");
        // From<&str>
        let id2 = OperationId::from("op-001");
        assert_eq!(id, id2);
        // Display
        assert_eq!(id.to_string(), "op-001");
    }

    #[test]
    fn test_gather_result_all_success() {
        let result = GatherResult::all_success(10, 50.0);
        assert_eq!(result.total, 10);
        assert_eq!(result.succeeded, 10);
        assert_eq!(result.failed, 0);
        assert!(result.is_all_success());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_gather_result_default() {
        let result = GatherResult::default();
        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.latency_ms, 0.0);
    }
}
