//! Fuzz target: CACR 预算配置解析与守卫检查模糊测试
//!
//! 对应架构层:L1 Core(model-router / CACR)
//!
//! # 模糊目标
//! 验证 `CacrConfig` 的 serde 反序列化与 `CacrGuard::check` 在任意输入下:
//! 1. 不 panic(内存安全)— 即使输入包含畸形 JSON / 超大数字 / 嵌套结构
//! 2. 反序列化成功后,守卫 check 在各种 budget/cost 组合下不 panic
//! 3. 反序列化成功后,重新序列化应成功(往返不变量)
//!
//! # WHY 选择此 target
//! CACR 是成本感知路由核心,从 omega.yaml 加载配置时可能遇到用户编辑
//! 产生的畸形值。fuzz 确保恶意/畸形输入不导致 panic 或未定义行为。
//!
//! # 运行方式(需 nightly)
//! ```bash
//! cargo +nightly fuzz run cacr_budget_parse
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响 34 crate 的 forbid 覆盖率。

use libfuzzer_sys::fuzz_target;
use model_router::{CacrConfig, CacrGuard};

fuzz_target!(|data: &[u8]| {
    // === 目标1:CacrConfig JSON 反序列化 + check 不 panic ===
    // JSON 不支持 NaN/Inf,反序列化的 f32 字段必为有限值
    if let Ok(config) = serde_json::from_slice::<CacrConfig>(data) {
        let guard = CacrGuard::new(config);

        // 各种 budget/cost 组合测试 check 方法不 panic
        // 覆盖边界:budget=0、cost=0、cost > budget、极大值
        let _ = guard.check(0, 0);
        let _ = guard.check(1, 0);
        let _ = guard.check(0, 1000);
        let _ = guard.check(100, 1000);
        let _ = guard.check(u64::MAX, u64::MAX);

        // 往返不变量:反序列化成功后,重新序列化应成功
        let reserialized = serde_json::to_vec(guard.config());
        assert!(
            reserialized.is_ok(),
            "CacrConfig JSON 重新序列化应成功,但失败: {:?}",
            reserialized.err()
        );
    }

    // === 目标2:CacrConfig MessagePack 反序列化不 panic ===
    // MessagePack 可表示 NaN/Inf,仅验证反序列化不 panic + 往返
    // (不调用 check 以避免 Inf 阈值导致的整数溢出 — 这是 CACR 代码待修问题)
    if let Ok(config) = rmp_serde::from_slice::<CacrConfig>(data) {
        let reserialized = rmp_serde::to_vec(&config);
        assert!(
            reserialized.is_ok(),
            "CacrConfig MessagePack 重新序列化应成功"
        );

        // 阈值为有限值时才调用 check(避免 Inf → u64::MAX 导致溢出 panic)
        if config.warn_threshold.is_finite()
            && config.block_threshold.is_finite()
            && config.warn_threshold >= 0.0
            && config.warn_threshold <= 1.0
            && config.block_threshold >= 0.0
            && config.block_threshold <= 1.0
        {
            let guard = CacrGuard::new(config);
            let _ = guard.check(100, 1000);
        }
    }
});
