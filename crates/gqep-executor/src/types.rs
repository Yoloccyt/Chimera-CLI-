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
///
/// # 四段守恒不变量(ADR-202 I5,红线级)
/// ```text
/// succeeded + failed + abandoned + not_started == total
/// ```
/// 四段各自的语义(不得相互混用,否则重试判定会错):
/// - `succeeded` / `failed`:已入流、已被 poll、已有结果(含单操作超时)
/// - `abandoned`:已入流且已被 poll,但因**全局 deadline** 放弃而未完成——
///   它的 `entangle` 已执行,故 `OrphanGuard` 会触发并经
///   `NexusEvent::OrphanCallDetected`(event-bus)上报(可观测)
/// - `not_started`:**从未入流、因此从未被 poll**——`entangle` 的注册与
///   `OrphanGuard` 创建都发生在 async 体首次执行
///   (`qeep-protocol/src/protocol.rs:283-284` 源码自述),故这一类**没有任何
///   孤儿上报**,只能靠本字段显式记账。这是并发上界(分批入流)引入的新段。
///
/// WHY 必须拆成两段而不是合并为一个「未完成」计数:副作用型/计费型调用方
/// 需要区分「根本没执行(可安全重试)」与「执行了但被放弃(重试可能重复副作用)」,
/// 即 `not_started` 兼有**重试幂等判据**语义(ADR-202 Annex C-6 契约变更登记)。
///
/// WHY `#[non_exhaustive]`:本结构字段集刚发生结构性扩展(加两个计数),
/// 标注后外部无法用字面量整体构造,未来再加字段不属破坏性变更。
/// 仓内全部字面量构造点均在本 crate 内(gatherer.rs / batch.rs),故零影响。
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GatherResult {
    /// 批次中总操作数(传入的 futures 数量)
    pub total: u32,
    /// 成功操作数
    pub succeeded: u32,
    /// 失败操作数(含单操作超时、执行错误;不含全局超时放弃的 future)
    pub failed: u32,
    /// 已入流被 poll、但因全局 deadline 放弃而未完成的操作数
    ///
    /// 这一类会触发 `OrphanGuard` 并另有 `OrphanCallDetected` 事件上报。
    pub abandoned: u32,
    /// 从未入流、因此从未被 poll 的操作数(并发上界下留在 backlog 的尾部)
    ///
    /// WHY 必须有这一字段:没有它,限流下的未执行操作就是**静默消失**,
    /// 直接破坏 UNLEARNABLE #3「零孤儿调用保证(QEEP)不可绕过」。
    /// 它无 `OrphanGuard` 故无孤儿事件,并另经 `GatherTimedOut` 事件字段上报。
    pub not_started: u32,
    /// 聚集延迟(毫秒),从 gather 开始到所有 future 完成的总耗时
    pub latency_ms: f32,
    /// 失败操作的错误列表。
    ///
    /// 常规场景长度 == `failed`(每个失败操作一条错误)。全局 gather 超时时,
    /// 额外追加一条 `GqepError::GlobalTimedOut` 汇总错误(此时长度 == failed + 1),
    /// 用于让调用者区分"整批被全局超时放弃"与"单个操作失败"。
    /// 注意:`not_started` 的操作**不产生**逐条错误(它们没有错误,只是没跑),
    /// 计数只落在本结构字段与 `GatherTimedOut` 事件上。
    pub errors: Vec<GqepError>,
}

impl GatherResult {
    /// 创建全成功的聚集结果
    ///
    /// 便捷构造器,用于测试或已知全部成功的场景。
    /// 全成功即 `abandoned == not_started == 0`,四段守恒自然成立。
    pub fn all_success(total: u32, latency_ms: f32) -> Self {
        Self {
            total,
            succeeded: total,
            failed: 0,
            abandoned: 0,
            not_started: 0,
            latency_ms,
            errors: Vec::new(),
        }
    }

    /// 判断是否全部成功(无失败操作)
    ///
    /// 注意语义边界：本方法只看 `failed`,不等价于「全部跑完」——
    /// `abandoned`/`not_started` 非零时也没有逐条错误,但操作确实未完成。
    /// 调用方若需区分「全部完成」与「有未完成但无错误」,应直接读后两个字段。
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
            abandoned: 0,
            not_started: 0,
            latency_ms: 0.0,
            errors: Vec::new(),
        }
    }
}

/// 带值聚集结果 —— `GqepExecutor::gather_collected` 的返回类型(M12 / ADR-185 D3)
///
/// 与 stats-only 的 [`GatherResult`] 相比额外保留成功操作的返回值,
/// 供调用方(如 chimera-cli doctor 探针并行化)按自身 tag 还原顺序。
/// `stats` 字段语义与 `gather` 的返回值完全一致(同一泛型内核产出)。
#[derive(Debug, Clone)]
pub struct GatherCollected<T> {
    /// 聚集执行统计(与 `gather` 返回的 GatherResult 同语义同字段)
    pub stats: GatherResult,
    /// 成功操作的返回值集合
    ///
    /// 顺序为流式完成序(**不保证输入序**);失败/超时的操作不计入
    /// (其错误已入 `stats.errors`)。需要输入序的调用方应自行在
    /// `T` 内携带序号 tag 并在聚集后排序还原。
    pub values: Vec<T>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
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
