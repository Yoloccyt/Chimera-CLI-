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
    DispatchError, DispatchFuture, DispatchOutcome, DispatchRequest, ToolDispatcher,
};
