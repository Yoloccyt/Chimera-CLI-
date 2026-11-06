//! 派发契约 — 工具运行时执行面的最小类型集（T2-3 / T1-1）
//!
//! 权威定义**已上提 L0**（`nexus_contracts::spec::tool_dispatch`，ADR-033）：
//! 本模块只做 re-export，`gqep_executor::tool_runtime::*` 旧路径逐字不变。
//!
//! WHY 上提而非本地定义：执行面契约要被 L8+ 层按依赖铁律 §2.2 以
//! `L(N) → L(0)` 消费（parliament 审议、quest 编排都需要表达「一次工具调用」），
//! 留在 L7 会迫使上层向上依赖。与 `streaming_dispatch` 的区别保持不变：
//! 流式派发面向增量解析与副作用上报（`SideEffect`），本层只表达「一次工具调用」
//! 的请求/结果，且必须能作为 `Arc<dyn ToolDispatcher>` 的对象安全接口被装饰器链持有。

pub use nexus_contracts::tool_dispatch::{
    DispatchError, DispatchFuture, DispatchOutcome, DispatchRequest, DispatchRequestRef,
    ToolDispatcher,
};

use std::future::Future;
use std::pin::Pin;

/// 借用化派发 future — 对象安全 trait 方法返回类型
pub type BorrowedDispatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>>;

/// 借用化派发器 — 链内零分配派发
///
/// WHY 与 [`ToolDispatcher`] 并存而非替换：既有 `ToolDispatcher` 是 L0 公开
/// 契约（被上层持有），签名变更属 BREAKING。本 trait 是**增量契约**，
/// 由 [`crate::tool_runtime::SandboxedDispatcher`] 同时提供两个入口。
///
/// # 契约
///
/// 对同一请求，`dispatch_borrowed` 与 `dispatch` 的结果必须**完全一致**
/// （含错误变体与文案）。这是链内借用化的硬约束 —— 若两者判决不同，
/// 借用路径就成了绕过安全校验的旁路。
///
/// # 生命周期
///
/// 借用视图不得跨 `.await` 持有；由 `'a` 参数在编译期约束。
pub trait BorrowedToolDispatcher: Send + Sync {
    /// 借用式派发一次请求
    fn dispatch_borrowed<'a>(
        &'a self,
        req: &'a DispatchRequestRef<'a>,
    ) -> BorrowedDispatchFuture<'a>;
}
