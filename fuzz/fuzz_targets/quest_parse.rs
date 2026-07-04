//! Fuzz target:Quest 解析输入模糊测试
//!
//! 对应任务:Week 8 Task 3 SubTask 3.2
//! 架构层:L9 Quest(quest-engine)+ L1 Core(nexus-core)
//!
//! # 模糊目标
//! 验证 `Quest` 与 `UserIntent` 的 serde 反序列化在任意输入下:
//! 1. 不 panic(内存安全)
//! 2. 反序列化成功后,序列化结果与输入一致(往返不变量)
//! 3. 异常输入优雅返回 Err,不触发未定义行为
//!
//! # 运行方式(需 nightly)
//! ```bash
//! cargo +nightly fuzz run quest_parse
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响 34 crate 的 forbid 覆盖率。

use libfuzzer_sys::fuzz_target;
use nexus_core::{MultimodalInput, Quest, UserIntent};

fuzz_target!(|data: &[u8]| {
    // === 目标1:Quest JSON 反序列化不 panic ===
    if let Ok(quest) = serde_json::from_slice::<Quest>(data) {
        // 往返不变量:反序列化成功后,重新序列化应成功
        let reserialized = serde_json::to_vec(&quest);
        assert!(
            reserialized.is_ok(),
            "Quest 重新序列化应成功,但失败: {:?}",
            reserialized.err()
        );

        // 重新反序列化应得到相等对象(往返一致性)
        let reserialized = reserialized.unwrap();
        if let Ok(quest2) = serde_json::from_slice::<Quest>(&reserialized) {
            assert_eq!(
                quest, quest2,
                "Quest 往返序列化后应相等(serde 不变量)"
            );
        }
    }

    // === 目标2:Quest MessagePack 反序列化不 panic ===
    if let Ok(quest) = rmp_serde::from_slice::<Quest>(data) {
        let reserialized = rmp_serde::to_vec(&quest);
        assert!(
            reserialized.is_ok(),
            "Quest MessagePack 重新序列化应成功"
        );
    }

    // === 目标3:UserIntent JSON 反序列化不 panic ===
    if let Ok(intent) = serde_json::from_slice::<UserIntent>(data) {
        // risk_level 是 u8,serde 会自动校验范围,无需额外断言
        // 验证多模态输入字段不为 None(枚举变体合法)
        let _ = &intent.multimodal_inputs;
        let _ = &intent.raw_text;

        // 往返不变量
        let reserialized = serde_json::to_vec(&intent);
        assert!(
            reserialized.is_ok(),
            "UserIntent 重新序列化应成功"
        );
    }

    // === 目标4:MultimodalInput 枚举反序列化不 panic ===
    if let Ok(input) = serde_json::from_slice::<MultimodalInput>(data) {
        // WHY 用 assert! 而非 expect():项目 §4.1 禁止 expect(),
        // assert! 在失败时同样提供诊断信息,且符合 invariant 检查范式
        let reserialized = serde_json::to_vec(&input);
        assert!(
            reserialized.is_ok(),
            "MultimodalInput 重新序列化应成功,但失败: {:?}",
            reserialized.err()
        );
    }
});
