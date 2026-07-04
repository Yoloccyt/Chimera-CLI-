//! HCW 分层上下文窗口错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 20.5:补充错误路径测试
//!
//! # 测试覆盖
//! 1. 无效配置:HcwWindow::new 在 l0_capacity=0 时返回 InvalidConfig
//! 2. 无效配置:非递增容量(l0 >= l1)返回 InvalidConfig
//! 3. 无效配置:compression_threshold=0.0 返回 InvalidConfig
//! 4. 无效配置:compression_threshold>1.0 返回 InvalidConfig
//! 5. 错误转换:EventBusError → HcwError::EventBusError
//!
//! # 实现说明
//! HcwWindow 未实现 Debug,不能用 `unwrap_err()`(要求 T: Debug)。
//! 改用 `match` 模式提取错误,避免 Debug 约束。

#![forbid(unsafe_code)]

use event_bus::EventBus;
use hcw_window::{HcwConfig, HcwError, HcwWindow};

/// 无效配置:HcwWindow::new 在 l0_capacity=0 时返回 InvalidConfig
///
/// WHY:l0_capacity=0 意味着 L0 窗口无法容纳任何条目,属于配置失误。
/// HcwWindow::new 调用 config.validate() 在系统边界拦截此错误。
#[test]
fn test_window_new_zero_l0_capacity() {
    let config = HcwConfig::default().with_l0_capacity(0);
    let bus = EventBus::new();
    let err = match HcwWindow::new(config, bus) {
        Ok(_) => panic!("l0_capacity=0 应返回错误"),
        Err(e) => e,
    };
    assert!(matches!(err, HcwError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("l0_capacity"),
        "错误信息应包含字段名"
    );
}

/// 无效配置:非递增容量(l0 >= l1)返回 InvalidConfig
///
/// WHY:HCW 四级窗口要求 l0 < l1 < l2 < l3(容量递增),
/// 违反此约束会导致层级升级逻辑异常。
#[test]
fn test_window_new_non_monotonic_capacities() {
    // l0=4096 >= l1=4096(默认 l1=32768,这里把 l1 设为与 l0 相同)
    let config = HcwConfig::default().with_l1_capacity(4096);
    let bus = EventBus::new();
    let err = match HcwWindow::new(config, bus) {
        Ok(_) => panic!("l0 >= l1 应返回错误"),
        Err(e) => e,
    };
    assert!(matches!(err, HcwError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("l0_capacity"),
        "错误信息应包含 l0_capacity"
    );
}

/// 无效配置:compression_threshold=0.0 返回 InvalidConfig
///
/// WHY:compression_threshold 必须在 (0.0, 1.0] 范围内。
/// 0.0 意味着永不压缩,窗口溢出时无法降级,属于配置失误。
#[test]
fn test_window_new_zero_compression_threshold() {
    let config = HcwConfig::default().with_compression_threshold(0.0);
    let bus = EventBus::new();
    let err = match HcwWindow::new(config, bus) {
        Ok(_) => panic!("compression_threshold=0.0 应返回错误"),
        Err(e) => e,
    };
    assert!(matches!(err, HcwError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("compression_threshold"),
        "错误信息应包含字段名"
    );
}

/// 无效配置:compression_threshold>1.0 返回 InvalidConfig
///
/// WHY:compression_threshold > 1.0 无意义(压缩比不可能超过 100%)。
#[test]
fn test_window_new_excessive_compression_threshold() {
    let config = HcwConfig::default().with_compression_threshold(1.5);
    let bus = EventBus::new();
    let err = match HcwWindow::new(config, bus) {
        Ok(_) => panic!("compression_threshold=1.5 应返回错误"),
        Err(e) => e,
    };
    assert!(matches!(err, HcwError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("compression_threshold"),
        "错误信息应包含字段名"
    );
}

/// 错误转换:EventBusError → HcwError::EventBusError
///
/// WHY:HCW 通过 EventBus 发布 ContextWindowSwitched/ContextCompressed 事件,
/// EventBus 失败时需正确转换为 HcwError,调方可按 HcwError 统一处理。
#[test]
fn test_error_conversion_from_event_bus() {
    // 构造一个 EventBusError(ChannelClosed 变体)
    let bus_err = event_bus::EventBusError::ChannelClosed;
    let hcw_err: HcwError = bus_err.into();
    assert!(
        matches!(hcw_err, HcwError::EventBusError(_)),
        "EventBusError 应转换为 HcwError::EventBusError"
    );
}
