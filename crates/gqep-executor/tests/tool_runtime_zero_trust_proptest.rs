//! `ToolRuntime` 零信任不变量属性测试（T1-2）
//!
//! # 验证的不变量（64 cases）
//! 任意 tool / args 组合下，零信任策略（`ExecPolicy::new()`，无任何规则）经
//! `ToolExecutor::execute` 路径**恒**返回 `Err`，且委托执行器调用次数恒为 0。
//!
//! WHY：`ToolRuntime` 是生产接线的唯一入口；若某条输入能穿过策略前置，
//! 真实副作用已经发生。这里用任意输入把「零信任 = 全拒」锁成机器不变量，
//! 而不是只依赖手写的几个样例。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use gqep_executor::{ToolExecutor, ToolRuntime};
use proptest::prelude::*;
use seccore::execpolicy::ExecPolicy;

/// 计数型委托执行器 — 记录被调用次数（期望恒为 0）
struct CountingExecutor {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for CountingExecutor {
    async fn execute(&self, _tool_name: &str, _args_json: &str) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("{}".to_string())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 零信任策略下 `execute` 恒 Err 且零委托
    #[test]
    fn zero_trust_execute_always_errs_without_delegate(
        tool_name in any::<String>(),
        args_json in any::<String>(),
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = ToolRuntime::new(
            Arc::new(CountingExecutor { calls: Arc::clone(&calls) }),
            Arc::new(ExecPolicy::new()),
        ).map_err(|e| TestCaseError::fail(e.to_string()))?;

        let rt = tokio::runtime::Runtime::new().map_err(|e| TestCaseError::fail(e.to_string()))?;
        let result = rt.block_on(runtime.execute(&tool_name, &args_json));

        prop_assert!(
            result.is_err(),
            "零信任策略下任意输入必须 Err，实际成功: {:?}（tool={tool_name:?}）",
            result
        );
        prop_assert_eq!(
            calls.load(Ordering::SeqCst), 0,
            "策略拒绝后委托不得被调用"
        );
    }
}
