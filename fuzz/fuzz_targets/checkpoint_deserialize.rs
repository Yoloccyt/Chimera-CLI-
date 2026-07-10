//! Fuzz target: Checkpoint MessagePack 反序列化模糊测试
//!
//! 对应架构层:L9 Quest(quest-engine / LHQP 检查点持久化)
//!
//! # 模糊目标
//! 验证 `Checkpoint` 的 serde 反序列化在任意输入下:
//! 1. 不 panic(内存安全)— 即使输入包含畸形 MessagePack / 截断数据
//! 2. 反序列化成功后,重新序列化应成功(往返不变量)
//! 3. 重新反序列化应得到相等对象(往返一致性)
//! 4. 各字段可安全访问不触发未定义行为
//!
//! # WHY 选择此 target
//! Checkpoint 持久化涉及从磁盘读取 MessagePack 数据并反序列化,
//! 磁盘文件可能因位翻转、磁盘故障或人为篡改而损坏。
//! fuzz 确保反序列化路径不 panic,崩溃恢复场景下安全。
//!
//! # 运行方式(需 nightly)
//! ```bash
//! cargo +nightly fuzz run checkpoint_deserialize
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响 34 crate 的 forbid 覆盖率。

use libfuzzer_sys::fuzz_target;
use nexus_core::Checkpoint;

fuzz_target!(|data: &[u8]| {
    // === 目标1:Checkpoint MessagePack 反序列化不 panic ===
    // ADR-004:消息序列化协议为 MessagePack,checkpoint 持久化使用此格式
    if let Ok(checkpoint) = rmp_serde::from_slice::<Checkpoint>(data) {
        // 反序列化成功后,安全访问各字段(验证不触发 UB)
        let _ = &checkpoint.quest_id;
        let _ = &checkpoint.checkpoint_id;
        let _ = &checkpoint.memory_snapshot_hash;
        let _ = &checkpoint.serialized_state;
        let _ = checkpoint.created_at;

        // 往返不变量:重新序列化应成功
        let reserialized = rmp_serde::to_vec(&checkpoint);
        assert!(
            reserialized.is_ok(),
            "Checkpoint MessagePack 重新序列化应成功,但失败: {:?}",
            reserialized.err()
        );

        // 重新反序列化应得到相等对象(Checkpoint impl PartialEq)
        if let Ok(bytes) = reserialized {
            if let Ok(checkpoint2) = rmp_serde::from_slice::<Checkpoint>(&bytes) {
                assert_eq!(
                    checkpoint, checkpoint2,
                    "Checkpoint MessagePack 往返序列化后应相等(serde 不变量)"
                );
            }
        }
    }

    // === 目标2:Checkpoint JSON 反序列化不 panic ===
    // 虽然持久化用 MessagePack,JSON 路径也需安全(调试/导出场景)
    if let Ok(checkpoint) = serde_json::from_slice::<Checkpoint>(data) {
        let reserialized = serde_json::to_vec(&checkpoint);
        assert!(
            reserialized.is_ok(),
            "Checkpoint JSON 重新序列化应成功"
        );
    }
});
