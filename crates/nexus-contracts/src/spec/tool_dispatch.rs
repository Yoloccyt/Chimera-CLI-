//! 工具派发契约 — 工具运行时执行面的最小类型集（T1-1，ADR-033）
//!
//! 权威定义已上提至 L0：本模块是 `DispatchRequest` / `DispatchOutcome` /
//! `DispatchError` / `DispatchFuture` / `ToolDispatcher` 的唯一事实源，
//! L7 `gqep-executor` 的 `tool_runtime::dispatch` 仅作 re-export（旧路径不变）。
//! 上提动机：执行面契约需被 L8 及以上层（parliament 审议、quest 编排）按依赖
//! 铁律 §2.2 以 `L(N) → L(0)` 方式消费，而不必向上依赖 L7。
//!
//! 纯度约束（G-99）：仅依赖 `std`（`fmt` / `future::Future` / `pin::Pin`），
//! **不引入 `futures`**——L0 外部依赖白名单只有 serde / chrono / uuid / thiserror。
//!
//! 与 `streaming_dispatch` 的区别：流式派发面向增量解析与副作用上报
//! （`SideEffect`），本层只表达「一次工具调用」的请求/结果，且必须能作为
//! `Arc<dyn ToolDispatcher>` 的对象安全接口被装饰器链持有。

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// 一次工具派发请求
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRequest {
    /// 工具名（逻辑名，不要求与 shell 程序名一致）
    pub tool_name: String,
    /// 工具参数（JSON 字符串）
    pub args_json: String,
}

/// 借用化派发请求视图 — 热路径零分配载体
///
/// WHY 与拥有型 [`DispatchRequest`] 并存而非替换：
///
/// 拥有型是 L0 的**边界类型** —— 它能跨 `.await` 持有、能跨层传递、能存进
/// 集合，是公开契约的一部分（`DispatchRequest` 的签名不可变，见 ADR-241 D6）。
/// 但**链内派发是热路径**：每次构造拥有型请求需 2 次堆分配（`tool_name` 与
/// `args_json` 各一次），批量派发 N 条即 2N 次。
///
/// 本视图让链内派发走借用，消除这 2 次分配，同时**不改动** L0 既有类型的
/// 签名（既有调用方零改动）。
///
/// # 生命周期约束
///
/// 本视图**不得**跨 `.await` 持有 —— 借用期由 `'a` 参数在编译期约束；
/// 若调用方需要跨 `.await`，必须回退 [`DispatchRequest`]（拥有型）。
///
/// # 大小
///
/// `size_of::<DispatchRequestRef>() == 2 * size_of::<&str>()` —— 无隐藏分配、
/// 无填充。这是"零分配"的**可判定**判据（由 `nexus-contracts` 单测锁定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchRequestRef<'a> {
    /// 工具名（逻辑名）
    pub tool_name: &'a str,
    /// 工具参数（JSON 片段）
    pub args_json: &'a str,
}

impl<'a> From<&'a DispatchRequest> for DispatchRequestRef<'a> {
    /// 拥有型 → 借用视图的零成本转换
    fn from(r: &'a DispatchRequest) -> Self {
        Self {
            tool_name: &r.tool_name,
            args_json: &r.args_json,
        }
    }
}

impl<'a> DispatchRequestRef<'a> {
    /// 借用视图 → 拥有型（需跨 `.await` 持有时的显式升级）
    ///
    /// WHY 提供显式升级而非隐式 AsRef：借用/拥有的转换点在**热路径**上
    /// 必须可见 —— 让调用方明确知道此处发生了一次拷贝。
    #[must_use]
    pub fn to_owned_request(self) -> DispatchRequest {
        DispatchRequest {
            tool_name: self.tool_name.to_string(),
            args_json: self.args_json.to_string(),
        }
    }
}

/// 一次工具派发结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchOutcome {
    /// 工具输出（JSON 字符串）
    pub content_json: String,
}

/// 派发错误 — 按装饰器层归因
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// 策略层拒绝（deny）或要求审批（ask）
    Policy(String),
    /// 沙箱层拒绝
    Sandbox(String),
    /// 路由层未命中
    Route(String),
    /// 传输层/实际执行失败
    Transport(String),
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(msg) => write!(f, "policy:{msg}"),
            Self::Sandbox(msg) => write!(f, "sandbox:{msg}"),
            Self::Route(msg) => write!(f, "route:{msg}"),
            Self::Transport(msg) => write!(f, "transport:{msg}"),
        }
    }
}

impl std::error::Error for DispatchError {}

/// 派发 future — 对象安全 trait 方法返回类型
pub type DispatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>>;

/// 工具派发器 — 装饰器链的每个节点都实现此 trait
pub trait ToolDispatcher: Send + Sync {
    /// 派发一次请求
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display 四变体文案逐字锁定（`层名:消息`，供上层按前缀路由）
    #[test]
    fn display_prefixes_match_layer_names() {
        let cases = [
            (DispatchError::Policy("denied".into()), "policy:denied"),
            (DispatchError::Sandbox("blocked".into()), "sandbox:blocked"),
            (DispatchError::Route("miss".into()), "route:miss"),
            (DispatchError::Transport("boom".into()), "transport:boom"),
        ];
        for (err, want) in cases {
            assert_eq!(err.to_string(), want, "Display 文案不得漂移");
        }
    }

    /// `std::error::Error` 可用（上层可 `?` 转换 / `Box<dyn Error>`）
    #[test]
    fn implements_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(DispatchError::Policy("x".into()));
        assert_eq!(err.to_string(), "policy:x");
    }
}
