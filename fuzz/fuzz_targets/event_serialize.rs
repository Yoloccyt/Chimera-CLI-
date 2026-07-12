//! Fuzz target:Event 序列化/反序列化模糊测试
//!
//! 对应任务:Week 8 Task 3 SubTask 3.2
//! 架构层:L1 Core(event-bus)
//!
//! # 模糊目标
//! 验证 `NexusEvent` 的 serde 序列化/反序列化在任意输入下:
//! 1. 不 panic(内存安全)
//! 2. 反序列化成功后,重新序列化结果一致(往返不变量)
//! 3. JSON 与 MessagePack 两种编码格式均稳定
//! 4. 畸形事件载荷不导致未定义行为
//!
//! # 运行方式(需 nightly)
//! ```bash
//! cargo +nightly fuzz run event_serialize
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响 34 crate 的 forbid 覆盖率。

// WHY 条件 use:Windows-GNU 无法编译 libfuzzer-sys(C++ MSVC pragma 不兼容),
// 改用 crate 内部 stub 宏(chimera_fuzz::fuzz_target)验证 fuzz 逻辑语法。
// 非 Windows-GNU 环境(Linux CI / Windows-MSVC)使用真正的 libfuzzer-sys。
#[cfg(not(all(target_os = "windows", target_env = "gnu")))]
use libfuzzer_sys::fuzz_target;

#[cfg(all(target_os = "windows", target_env = "gnu"))]
use chimera_fuzz::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // === 目标1:NexusEvent JSON 反序列化不 panic ===
    if let Ok(event) = serde_json::from_slice::<event_bus::NexusEvent>(data) {
        // 往返不变量:反序列化成功后,重新序列化应成功
        let reserialized = serde_json::to_vec(&event);
        assert!(
            reserialized.is_ok(),
            "NexusEvent JSON 重新序列化应成功"
        );

        // 重新反序列化应得到相等事件(往返一致性)
        let reserialized = reserialized.unwrap();
        if let Ok(event2) = serde_json::from_slice::<event_bus::NexusEvent>(&reserialized) {
            assert_eq!(
                event, event2,
                "NexusEvent JSON 往返序列化后应相等(serde 不变量)"
            );
        }
    }

    // === 目标2:NexusEvent MessagePack 反序列化不 panic ===
    // ADR-004:消息序列化协议为 MessagePack,跨进程通信使用此格式
    if let Ok(event) = rmp_serde::from_slice::<event_bus::NexusEvent>(data) {
        let reserialized = rmp_serde::to_vec(&event);
        assert!(
            reserialized.is_ok(),
            "NexusEvent MessagePack 重新序列化应成功"
        );

        // 往返一致性
        let reserialized = reserialized.unwrap();
        if let Ok(event2) = rmp_serde::from_slice::<event_bus::NexusEvent>(&reserialized) {
            assert_eq!(
                event, event2,
                "NexusEvent MessagePack 往返序列化后应相等"
            );
        }
    }

    // === 目标3:EventMetadata JSON 反序列化不 panic ===
    if let Ok(meta) = serde_json::from_slice::<event_bus::EventMetadata>(data) {
        // WHY 用 assert! 而非 expect():项目 §4.1 禁止 expect(),
        // assert! 在失败时同样提供诊断信息,且符合 invariant 检查范式
        let reserialized = serde_json::to_vec(&meta);
        assert!(
            reserialized.is_ok(),
            "EventMetadata 重新序列化应成功,但失败: {:?}",
            reserialized.err()
        );
    }

    // WHY 不在此处构造 256KB 固定 JSON 字符串(原 W4 反模式):
    // 1. fuzz harness 必须只依赖 `data` 输入,固定大输入应由 corpus seed 覆盖
    // 2. 每次迭代分配 256KB 严重拖慢 fuzz 吞吐量,且不增加覆盖率
    // 3. libFuzzer 会自动基于 `data` 探索超长输入,无需手动构造
});
