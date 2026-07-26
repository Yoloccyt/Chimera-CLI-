//! Fuzz target: HarnessSpec TOML 解析与校验模糊测试（P4-W15.3.1）
//!
//! 对应架构层:L5 Knowledge（gsoe-evolution / SpecLoader）
//! 对应 ADR: **ADR-031**（Harness-as-Spec + omega-learner 边界）
//!
//! # 模糊目标
//! 验证 `SpecLoader::load_from_str` 在任意输入下:
//! 1. 不 panic（内存安全）— 即使输入包含畸形 TOML / 不可进化面攻击 / 嵌套结构
//! 2. 反序列化成功后，`HarnessSpec::validate()` 不 panic
//! 3. 反序列化成功后，`canonical_merkle_input()` 不 panic（用于 Merkle 哈希）
//! 4. 不可进化面攻击必须被拒绝（`ImmutableSurfaceViolation` 错误，不可绕过）
//! 5. 字节切片加载 `load_from_bytes` 在非 UTF-8 输入下不 panic
//!
//! # WHY 选择此 target
//! HarnessSpec 是 RHI-CG 双通道评估器的核心 DSL（设计文档 §7.2），
//! 恶意/畸形 spec 可能试图绕过不可进化面守护（如修改 AsaIntervention 行为）。
//! fuzz 确保任何输入都不导致 panic 或绕过安全校验。
//!
//! # 运行方式（需 nightly）
//! ```bash
//! cargo +nightly fuzz run harness_spec_parse
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响 35 crate 的 forbid 覆盖率。

// Windows-GNU 下使用 stub 宏(chimera_fuzz),非 Windows 使用 libfuzzer_sys
#[cfg(not(windows))]
use libfuzzer_sys::fuzz_target;
#[cfg(windows)]
use chimera_fuzz::fuzz_target;
use gsoe_evolution::SpecLoader;
use nexus_contracts::{HarnessSpecError, ImmutableSurface};

fuzz_target!(|data: &[u8]| {
    // === 目标 1: load_from_bytes 在任意字节切片下不 panic ===
    // 字节切片可能为非 UTF-8，load_from_bytes 应返回 Err 而非 panic
    let _ = SpecLoader::load_from_bytes(data);

    // === 目标 2: load_from_str 在合法 UTF-8 输入下不 panic ===
    // 仅当字节切片为合法 UTF-8 时，转换为字符串并测试 TOML 解析
    if let Ok(toml_str) = std::str::from_utf8(data) {
        let result = SpecLoader::load_from_str(toml_str);

        if let Ok(spec) = result {
            // === 目标 3: 加载成功后 validate() 不 panic ===
            // （SpecLoader 已调用 validate，但显式再调一次验证幂等性）
            let validate_result = spec.validate();
            assert!(
                validate_result.is_ok(),
                "SpecLoader 加载成功的 spec 再次 validate 应通过，但失败: {:?}",
                validate_result.err()
            );

            // === 目标 4: canonical_merkle_input() 不 panic ===
            // 用于 Merkle 哈希计算的规范化字符串，必须稳定无副作用
            let merkle_input_1 = spec.canonical_merkle_input();
            let merkle_input_2 = spec.canonical_merkle_input();
            assert_eq!(
                merkle_input_1, merkle_input_2,
                "canonical_merkle_input 应为纯函数，多次调用返回相同结果"
            );

            // === 目标 5: 不可进化面清单可访问 ===
            // immutable_surfaces() 应返回固定 20 个变体
            let surfaces = SpecLoader::immutable_surfaces();
            assert_eq!(
                surfaces.len(),
                20,
                "不可进化面清单应固定为 20 个变体"
            );
        }
        // 若 result 为 Err，这是预期的（畸形/恶意输入应被拒绝），不需断言
    }

    // === 目标 6: 不可进化面攻击必须被拒绝（安全不变量）===
    // 即使输入包含不可进化面标识（如 "critical-asa-intervention"），
    // SpecLoader 必须拒绝，不允许绕过守护。
    // 此检查使用预定义的攻击 payload，而非 fuzz 输入（确保覆盖关键攻击向量）
    let attack_payloads = [
        // contracts[].from 引用不可进化面
        r#"
[meta]
name = "attack-1"
version = 1

[[contracts]]
name = "c1"
property = "p1"
from = "critical-asa-intervention"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#,
        // hops[].order 引用不可进化面
        r#"
[meta]
name = "attack-2"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
order = ["critical-budget-exceeded"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#,
        // contracts[].fields 引用不可进化面
        r#"
[meta]
name = "attack-3"
version = 1

[[contracts]]
name = "c1"
property = "p1"
fields = ["inv-7-memory-budget"]

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#,
        // hops[].on_veto 引用不可进化面
        r#"
[meta]
name = "attack-4"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]
on_veto = "critical-asa-intervention"

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#,
    ];

    for payload in &attack_payloads {
        let result = SpecLoader::load_from_str(payload);
        match result {
            Err(gsoe_evolution::SpecLoaderError::ValidationFailed(
                HarnessSpecError::ImmutableSurfaceViolation { surface, .. },
            )) => {
                // 验证 surface 是预期的不可进化面变体
                assert!(
                    matches!(
                        surface,
                        ImmutableSurface::CriticalAsaIntervention
                            | ImmutableSurface::CriticalBudgetExceeded
                            | ImmutableSurface::Invariant7MemoryBudget
                    ),
                    "不可进化面攻击应被正确的 surface 变体拒绝，实际: {:?}",
                    surface
                );
            }
            Err(other) => {
                // 其他错误也可接受（如 MissingAcceptanceGates），只要不是 Ok
                // 但 ImmutableSurfaceViolation 必须是首选错误
                let _ = other; // 接受其他错误类型
            }
            Ok(_) => {
                panic!("不可进化面攻击 payload 不应加载成功: {}", payload);
            }
        }
    }

    // === 目标 7: 合法 spec 的往返不变量 ===
    // 加载成功的 spec 应能再次 validate + canonical_merkle_input 而不 panic
    // （此目标已在目标 3-4 覆盖，此处仅作显式注释）
});
