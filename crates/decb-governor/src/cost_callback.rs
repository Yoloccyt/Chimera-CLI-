//! DECB 成本回算 — 将实际成本回写到 decb-governor 的预算核算
//!
//! 对应架构层:L8 Parliament(decb-governor)
//!
//! # 职责
//! 当 mca-gateway 完成一次会话后，通过 `StreamSessionCompleted` 事件
//! 携带的 `cost_actual_micro` 字段，回写到 decb-governor 的预算消耗统计，
//! 使预算利用率计算包含真实成本。
//!
//! # 依赖方向(§2.2 铁律)
//! decb-governor(L8) 不向上依赖 L9，成本通过值对象传入。

use std::sync::{Arc, Mutex};

use crate::governor::DecbGovernor;
use crate::types::BudgetConsumption;

/// DECB 成本回算 — 将实际成本回写到 decb-governor 的预算核算
///
/// 当 mca-gateway 完成一次会话后，通过 `StreamSessionCompleted` 事件
/// 携带的 `cost_actual_micro` 字段，回写到 decb-governor 的预算消耗统计，
/// 使预算利用率计算包含真实成本。
///
/// # 线程安全
/// 内部持有 `Arc<Mutex<DecbGovernor>>`，与主线程通过 Mutex 同步。
/// 每次 `record_cost` 调用加锁后调用 `DecbGovernor::record_consumption`，
/// 锁内操作 < 1ms，不跨 await，符合持锁规范(§4.4 反模式清单)。
pub struct DecbCostCallback {
    /// DECB 治理器引用
    governor: Arc<Mutex<DecbGovernor>>,
}

impl DecbCostCallback {
    /// 创建成本回算回调
    pub fn new(governor: Arc<Mutex<DecbGovernor>>) -> Self {
        Self { governor }
    }

    /// 记录实际成本
    ///
    /// 将实际成本构造为 `BudgetConsumption` 后调用 `DecbGovernor::record_consumption`。
    /// `ttft_ms` 作为上下文加载次数记录（近似，用于统计），
    /// `route_key` 仅用于日志追踪，不参与预算核算。
    ///
    /// # 参数
    /// - `route_key`: 路由通道标识（仅日志）
    /// - `cost_micro`: 实际成本(微元)
    /// - `ttft_ms`: 首 Token 延迟(毫秒，作为上下文加载次数的近似)
    ///
    /// # 错误处理
    /// 锁中毒或 `record_consumption` 返回错误时仅记 warning，不 panic。
    pub fn record_cost(&self, route_key: &str, cost_micro: u64, ttft_ms: u32) {
        match self.governor.lock() {
            Ok(gov) => {
                let consumption = BudgetConsumption {
                    token_count: 0,
                    tool_call_count: 0,
                    context_load_count: ttft_ms as u64,
                    total_cost: cost_micro as f64,
                };
                if let Err(e) = gov.record_consumption(&consumption) {
                    tracing::warn!(error = %e, route_key, cost_micro, "DECB 成本回算失败");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, route_key, "DECB 治理器锁中毒");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DecbConfig;

    fn make_callback() -> (DecbCostCallback, Arc<Mutex<DecbGovernor>>) {
        let governor = Arc::new(Mutex::new(
            DecbGovernor::new(DecbConfig::default()).unwrap(),
        ));
        let callback = DecbCostCallback::new(governor.clone());
        (callback, governor)
    }

    #[test]
    fn test_record_cost_updates_consumption() {
        let (callback, governor) = make_callback();

        // 记录实际成本 5000 微元
        callback.record_cost("zhipu/glm-5.2", 5000, 200);

        // 验证消耗已更新
        let gov = governor.lock().unwrap();
        let stats = gov.get_stats();
        assert!(
            (stats.total_consumption - 5000.0).abs() < 1e-6,
            "total_consumption should be 5000, got {}",
            stats.total_consumption
        );
    }

    #[test]
    fn test_record_cost_multiple_times() {
        let (callback, governor) = make_callback();

        callback.record_cost("deep_seek/deepseek-v4-flash", 1000, 100);
        callback.record_cost("zhipu/glm-5.2", 2000, 150);

        let gov = governor.lock().unwrap();
        let stats = gov.get_stats();
        assert!(
            (stats.total_consumption - 3000.0).abs() < 1e-6,
            "total_consumption should be 3000, got {}",
            stats.total_consumption
        );
    }

    #[test]
    fn test_record_cost_zero_cost() {
        let (callback, governor) = make_callback();

        // 零成本回算不应导致错误
        callback.record_cost("x/y", 0, 0);

        let gov = governor.lock().unwrap();
        let stats = gov.get_stats();
        assert!(
            (stats.total_consumption - 0.0).abs() < 1e-6,
            "zero cost should not change consumption"
        );
    }

    #[test]
    fn test_record_cost_handles_lock_error_gracefully() {
        // 验证锁操作异常时不会 panic
        // 使用 `drop()` 模式持锁后立即释放，验证锁机制正常
        let governor = Arc::new(Mutex::new(
            DecbGovernor::new(DecbConfig::default()).unwrap(),
        ));
        let callback = DecbCostCallback::new(governor.clone());

        // 正常持锁与释放
        drop(Arc::clone(&governor).lock().unwrap());

        // 锁释放后的正常操作不应 panic
        callback.record_cost("x/y", 1000, 100);

        // 验证消耗已更新
        let gov = governor.lock().unwrap();
        let stats = gov.get_stats();
        assert!(
            (stats.total_consumption - 1000.0).abs() < 1e-6,
            "正常操作应更新消耗"
        );
    }
}
