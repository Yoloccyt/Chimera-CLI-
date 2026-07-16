//! Fuzz target: ChimeraConfig 配置 section 解析模糊测试
//!
//! 对应架构层:L10 Interface(chimera-cli 配置加载)
//!
//! # 模糊目标
//! 验证 `ChimeraConfig` 的 serde 反序列化在任意输入下:
//! 1. 不 panic(内存安全)— 即使输入包含畸形 JSON / 截断数据 / 嵌套错误
//! 2. 反序列化成功后,重新序列化应成功(往返不变量)
//! 3. 各 section 可安全访问不触发未定义行为
//!
//! # WHY 选择此 target
//! omega.yaml 配置文件由 figment 从多源加载,用户编辑可能引入畸形值。
//! ChimeraConfig 包含 14 个顶层 section(nexus/quest/seccore/...),
//! 每个 section 的字段都可能被用户错误编辑。fuzz 确保反序列化路径安全。
//!
//! # 注意:此 target 替代原计划的 moe_gate_compute
//! MoE 门控计算尚未实现(model-router 中无 moe 模块),
//! 改为模糊测试 ChimeraConfig 配置 section 解析。
//! 待 MoE 实现后可新增 moe_gate_compute target。
//!
//! # 运行方式(需 nightly)
//! ```bash
//! cargo +nightly fuzz run config_section_parse
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响 34 crate 的 forbid 覆盖率。

// Windows-GNU 下使用 stub 宏(chimera_fuzz),非 Windows 使用 libfuzzer_sys
#[cfg(not(windows))]
use libfuzzer_sys::fuzz_target;
#[cfg(windows)]
use chimera_fuzz::fuzz_target;
use nexus_core::ChimeraConfig;

fuzz_target!(|data: &[u8]| {
    // === 目标1:ChimeraConfig JSON 反序列化不 panic ===
    // ChimeraConfig 带 #[serde(default)],缺失字段会填默认值,
    // 但畸形类型(如 section 值为数字却期望 struct)会返回 Err
    if let Ok(config) = serde_json::from_slice::<ChimeraConfig>(data) {
        // 反序列化成功后,安全访问各 section(验证不触发 UB)
        let _ = &config.nexus;
        let _ = &config.quest;
        let _ = &config.seccore;

        // 往返不变量:重新序列化应成功
        let reserialized = serde_json::to_vec(&config);
        assert!(
            reserialized.is_ok(),
            "ChimeraConfig JSON 重新序列化应成功,但失败: {:?}",
            reserialized.err()
        );
    }

    // === 目标2:ChimeraConfig MessagePack 反序列化不 panic ===
    // 虽然 figment 不用 MessagePack 加载,但 serde 路径应通用安全
    if let Ok(config) = rmp_serde::from_slice::<ChimeraConfig>(data) {
        let _ = &config.nexus;
        let _ = &config.quest;
        let _ = &config.seccore;

        let reserialized = rmp_serde::to_vec(&config);
        assert!(
            reserialized.is_ok(),
            "ChimeraConfig MessagePack 重新序列化应成功"
        );
    }
});
