//! 进程内派发器 — `ToolExecutor` 到 `ToolDispatcher` 的适配（T1-2）
//!
//! 传输环的最小落地形态：把「一次派发请求」翻译为一次 `ToolExecutor::execute`，
//! 执行成功回填 `DispatchOutcome`，执行失败归因为
//! [`DispatchError::Transport`]（传输层/实际执行失败，策略与沙箱的拒绝在读
//! 到本层之前已由链上前置层归因完毕）。

use std::sync::Arc;

use super::dispatch::{
    DispatchError, DispatchFuture, DispatchOutcome, DispatchRequest, ToolDispatcher,
};
use crate::toolplan_runner::ToolExecutor;

/// 进程内派发器 — 持有注入的工具执行器
pub struct InProcDispatcher {
    /// 实际执行者（生产路径为 `ExecPolicyToolExecutor`，见 [`super::runtime`]）
    executor: Arc<dyn ToolExecutor>,
}

impl InProcDispatcher {
    /// 新建 — 注入工具执行器
    #[must_use]
    pub fn new(executor: Arc<dyn ToolExecutor>) -> Self {
        Self { executor }
    }
}

impl ToolDispatcher for InProcDispatcher {
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        // 先取出借用，再 move 进 future：future 借用 `req`（'a）与 `self`（'a），
        // 不克隆请求体 —— 派发是热路径，克隆 `args_json` 没有收益。
        let tool = req.tool_name.as_str();
        let args = req.args_json.as_str();
        Box::pin(async move {
            self.executor
                .execute(tool, args)
                .await
                .map(|content_json| DispatchOutcome { content_json })
                .map_err(DispatchError::Transport)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 回显型执行器（成功路径）+ 调用计数
    struct EchoExecutor {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ToolExecutor for EchoExecutor {
        async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(args_json.to_string())
        }
    }

    /// 失败型执行器（错误映射路径）
    struct FailingExecutor;

    #[async_trait::async_trait]
    impl ToolExecutor for FailingExecutor {
        async fn execute(&self, _tool_name: &str, _args_json: &str) -> Result<String, String> {
            Err("boom".to_string())
        }
    }

    fn request(tool_name: &str, args_json: &str) -> DispatchRequest {
        DispatchRequest {
            tool_name: tool_name.to_string(),
            args_json: args_json.to_string(),
        }
    }

    /// 成功路径：内容回显 + 执行器恰好被调用 1 次
    #[tokio::test]
    async fn echoes_content_and_calls_executor_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = InProcDispatcher::new(Arc::new(EchoExecutor {
            calls: Arc::clone(&calls),
        }));

        let outcome = dispatcher
            .dispatch(&request("echo", r#"{"k":"v"}"#))
            .await
            .expect("回显执行器不应失败");
        assert_eq!(outcome.content_json, r#"{"k":"v"}"#);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "执行器应被调用 1 次");
    }

    /// 失败路径：执行器 Err 归因为 `DispatchError::Transport`
    #[tokio::test]
    async fn executor_error_maps_to_transport() {
        let dispatcher = InProcDispatcher::new(Arc::new(FailingExecutor));

        match dispatcher.dispatch(&request("echo", "{}")).await {
            Err(DispatchError::Transport(msg)) => assert_eq!(msg, "boom"),
            other => panic!("期望 Transport 错误，实际: {other:?}"),
        }
    }
}
