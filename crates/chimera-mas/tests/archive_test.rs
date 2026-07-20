#![forbid(unsafe_code)]

//! Task 17(RED):Agent 记忆三级归档(1mo / 3mo / 6mo)+ INV-8 失败测试
//!
//! 对应设计文档 §17(第 1014-1052 行)+ §17.5 INV-8。
//! 覆盖 5 类场景(共 18+ 个测试):
//! 1. 1 月级归档(§17.2 行 1)— cron `0 2 1 * *` 或 条目 > 10000 触发,
//!    HCW 摘要 ≤500 tok(权重 0.4/0.3/0.3),存 CMT Warm(4096),priority < 0.1 降级
//! 2. 3 月级归档(§17.2 行 2)— cron `0 3 1 1,4,7,10 *` 触发,
//!    关系抽取 → mlc L2 语义(512-dim CLV),存 CMT Cold(65536),衰减 τ=24h
//! 3. 6 月级归档(§17.2 行 3)— cron `0 4 1 1,7 *` 触发,
//!    深度压缩 + 模式抽取,存 CMT Ice(∞,只读),KeepForever
//! 4. 降级判定纯函数(§17.2 + §17.5)— should_demote_metadata + compute_priority
//! 5. INV-8 单调性集成(§17.5)— ArchiveScheduler::trigger 校验
//!
//! TDD RED 阶段:本文件引用尚未实现的 API(`ArchiveScheduleLevel` /
//! `ArchiveScheduler` / `ArchiveCompressor` / `compute_priority` /
//! `should_demote_metadata` 等),编译失败为预期。
//! GREEN 阶段实现 `src/archive/` 子模块后全部通过。
//!
//! ## 红线对齐
//!
//! - §4.1: 库层 thiserror,无 unwrap/expect
//! - §4.4 反模式 6: f32 禁止隐式转 f64,全程 f64(降级判定)
//! - §6.1: 单函数 ≤ 200 行
//! - §17.2: 三级归档映射(cron + 压缩策略 + 存储层 + 降级阈值)
//! - §17.5: INV-8 单调性 + 6mo 级 KeepForever
//! - `#![forbid(unsafe_code)]`: 纯计算,无 unsafe 需求

use chimera_mas::archive::{
    compute_priority, should_demote_metadata, ArchiveCompressor, ArchiveScheduleLevel,
    ArchiveScheduler, ArchiveTriggerCondition, CompressionStrategy, DEMOTION_THRESHOLD_F64,
    MAX_HCW_SUMMARY_TOKENS, TAU_1MO_SECONDS, TAU_3MO_SECONDS, TAU_6MO_SECONDS,
};
use chimera_mas::invariants::ArchiveTier;
use chimera_mas::MasError;

// ============================================================
// 1. 1 月级归档(§17.2 行 1)— 5 个测试
// ============================================================

/// 1mo 级 cron 表达式应为 `0 2 1 * *`(每月 1 日 02:00 触发)
///
/// 语义(§17.2):"0 2 1 * *" — 秒 0 / 分 2 / 时 1 / 日 * / 月 *
#[test]
fn month1_cron_expression_is_0_2_1_star_star() {
    assert_eq!(ArchiveScheduleLevel::Month1.cron_expression(), "0 2 1 * *");
}

/// 1mo 级目标存储层应为 CMT Warm(对应 ArchiveTier::Warm)
#[test]
fn month1_target_tier_is_warm() {
    assert_eq!(
        ArchiveScheduleLevel::Month1.target_tier(),
        ArchiveTier::Warm
    );
}

/// 1mo 级源归档层应为 Hot(Inv-8 单调性:Hot→Warm)
#[test]
fn month1_source_tier_is_hot() {
    assert_eq!(ArchiveScheduleLevel::Month1.source_tier(), ArchiveTier::Hot);
}

/// 1mo 级压缩策略应为 HCW 摘要(≤500 tok,权重 0.4/0.3/0.3)
#[test]
fn month1_compression_strategy_is_hcw_summary() {
    let strategy = ArchiveScheduleLevel::Month1.compression_strategy();
    match strategy {
        CompressionStrategy::HcwSummary {
            max_tokens,
            weights,
        } => {
            assert_eq!(max_tokens, 500, "1mo HCW 摘要 ≤500 tok");
            assert_eq!(weights, [0.4, 0.3, 0.3], "1mo HCW 权重 0.4/0.3/0.3");
        }
        other => panic!("1mo 压缩策略应为 HcwSummary,实际: {other:?}"),
    }
}

/// 1mo 级 cron 触发应生成 ArchiveOperation,target_tier=Warm
#[test]
fn month1_cron_trigger_produces_warm_operation() {
    let op = ArchiveScheduler::trigger(ArchiveScheduleLevel::Month1, ArchiveTriggerCondition::Cron)
        .expect("1mo cron 触发应成功");
    assert_eq!(op.level, ArchiveScheduleLevel::Month1);
    assert_eq!(op.target_tier, ArchiveTier::Warm);
    assert_eq!(op.source_tier, ArchiveTier::Hot);
}

/// 1mo 级容量触发:条目 > 10000 时应触发归档
#[test]
fn month1_capacity_trigger_when_entries_exceed_10000() {
    let op = ArchiveScheduler::trigger(
        ArchiveScheduleLevel::Month1,
        ArchiveTriggerCondition::CapacityExceeded {
            entry_count: 10001,
            threshold: 10000,
        },
    )
    .expect("1mo 容量超限应触发归档");
    assert_eq!(op.target_tier, ArchiveTier::Warm);
    // 容量触发应在 operation 中记录条件
    match op.condition {
        ArchiveTriggerCondition::CapacityExceeded {
            entry_count,
            threshold,
        } => {
            assert_eq!(entry_count, 10001);
            assert_eq!(threshold, 10000);
        }
        other => panic!("容量触发条件应保留,实际: {other:?}"),
    }
}

/// 1mo 级常量 TAU_1MO_SECONDS 应为 86400.0(24h,与 CMT 默认一致)
#[test]
fn month1_tau_constant_is_86400_seconds() {
    assert_eq!(TAU_1MO_SECONDS, 86400.0, "1mo τ 应为 86400 秒(24h)");
}

/// 1mo 级常量 MAX_HCW_SUMMARY_TOKENS 应为 500(§17.2)
#[test]
fn month1_max_hcw_summary_tokens_is_500() {
    assert_eq!(MAX_HCW_SUMMARY_TOKENS, 500, "HCW 摘要 ≤500 tok");
}

// ============================================================
// 2. 3 月级归档(§17.2 行 2)— 4 个测试
// ============================================================

/// 3mo 级 cron 表达式应为 `0 3 1 1,4,7,10 *`(1/4/7/10 月 1 日 03:00 触发)
#[test]
fn month3_cron_expression_is_0_3_1_1_4_7_10_star() {
    assert_eq!(
        ArchiveScheduleLevel::Month3.cron_expression(),
        "0 3 1 1,4,7,10 *"
    );
}

/// 3mo 级目标存储层应为 CMT Cold(对应 ArchiveTier::Cold)
#[test]
fn month3_target_tier_is_cold() {
    assert_eq!(
        ArchiveScheduleLevel::Month3.target_tier(),
        ArchiveTier::Cold
    );
}

/// 3mo 级源归档层应为 Warm(Inv-8 单调性:Warm→Cold)
#[test]
fn month3_source_tier_is_warm() {
    assert_eq!(
        ArchiveScheduleLevel::Month3.source_tier(),
        ArchiveTier::Warm
    );
}

/// 3mo 级压缩策略应为关系抽取(RelationExtraction → mlc L2 语义 512-dim CLV)
#[test]
fn month3_compression_strategy_is_relation_extraction() {
    let strategy = ArchiveScheduleLevel::Month3.compression_strategy();
    assert!(
        matches!(strategy, CompressionStrategy::RelationExtraction),
        "3mo 压缩策略应为 RelationExtraction,实际: {strategy:?}"
    );
}

/// 3mo 级 cron 触发应生成 ArchiveOperation,target_tier=Cold
#[test]
fn month3_cron_trigger_produces_cold_operation() {
    let op = ArchiveScheduler::trigger(ArchiveScheduleLevel::Month3, ArchiveTriggerCondition::Cron)
        .expect("3mo cron 触发应成功");
    assert_eq!(op.target_tier, ArchiveTier::Cold);
    assert_eq!(op.source_tier, ArchiveTier::Warm);
}

/// 3mo 级常量 TAU_3MO_SECONDS 应为 86400.0(§17.2 "衰减 τ=24h")
#[test]
fn month3_tau_constant_is_86400_seconds() {
    assert_eq!(TAU_3MO_SECONDS, 86400.0, "3mo τ 应为 86400 秒(24h,§17.2)");
}

// ============================================================
// 3. 6 月级归档(§17.2 行 3)— 4 个测试
// ============================================================

/// 6mo 级 cron 表达式应为 `0 4 1 1,7 *`(1/7 月 1 日 04:00 触发)
#[test]
fn month6_cron_expression_is_0_4_1_1_7_star() {
    assert_eq!(
        ArchiveScheduleLevel::Month6.cron_expression(),
        "0 4 1 1,7 *"
    );
}

/// 6mo 级目标存储层应为 CMT Ice(对应 ArchiveTier::Ice,只读)
#[test]
fn month6_target_tier_is_ice() {
    assert_eq!(ArchiveScheduleLevel::Month6.target_tier(), ArchiveTier::Ice);
}

/// 6mo 级源归档层应为 Cold(Inv-8 单调性:Cold→Ice)
#[test]
fn month6_source_tier_is_cold() {
    assert_eq!(
        ArchiveScheduleLevel::Month6.source_tier(),
        ArchiveTier::Cold
    );
}

/// 6mo 级压缩策略应为深度压缩 + 模式抽取(DeepCompression)
#[test]
fn month6_compression_strategy_is_deep_compression() {
    let strategy = ArchiveScheduleLevel::Month6.compression_strategy();
    assert!(
        matches!(strategy, CompressionStrategy::DeepCompression),
        "6mo 压缩策略应为 DeepCompression,实际: {strategy:?}"
    );
}

/// 6mo 级 cron 触发应生成 ArchiveOperation,target_tier=Ice
#[test]
fn month6_cron_trigger_produces_ice_operation() {
    let op = ArchiveScheduler::trigger(ArchiveScheduleLevel::Month6, ArchiveTriggerCondition::Cron)
        .expect("6mo cron 触发应成功");
    assert_eq!(op.target_tier, ArchiveTier::Ice);
    assert_eq!(op.source_tier, ArchiveTier::Cold);
}

/// 6mo 级常量 TAU_6MO_SECONDS 应为 f64::INFINITY(KeepForever,无衰减)
///
/// 语义(§17.5):"6 月级 KeepForever 且关键决策不压缩"
#[test]
fn month6_tau_constant_is_infinity() {
    assert!(
        TAU_6MO_SECONDS.is_infinite() && TAU_6MO_SECONDS > 0.0,
        "6mo τ 应为 +∞(KeepForever,无衰减),实际: {TAU_6MO_SECONDS}"
    );
}

// ============================================================
// 4. 降级判定纯函数(§17.2 + §17.5)— 6 个测试
// ============================================================

/// compute_priority 公式验证:priority = access_count × exp(-Δt / τ)
///
/// 边界 1:access_count = 0 时 priority = 0.0(从未访问)
#[test]
fn compute_priority_zero_access_count_returns_zero() {
    let priority = compute_priority(0, 3600.0, 86400.0);
    assert_eq!(priority, 0.0, "access_count=0 时 priority 应为 0");
}

/// compute_priority 公式验证:Δt = 0 时 priority = access_count(刚访问)
#[test]
fn compute_priority_zero_delta_t_returns_access_count() {
    let priority = compute_priority(10, 0.0, 86400.0);
    assert!(
        (priority - 10.0).abs() < 1e-9,
        "Δt=0 时 priority = access_count,期望 10.0,实际 {priority}"
    );
}

/// compute_priority 公式验证:τ=24h, Δt=24h 时 priority = access_count × exp(-1)
///
/// exp(-1) ≈ 0.36787944,access_count=1 时 priority ≈ 0.36787944
#[test]
fn compute_priority_tau_24h_delta_24h_returns_exp_neg1() {
    let priority = compute_priority(1, 86400.0, 86400.0);
    assert!(
        (priority - 0.36787944117144233).abs() < 1e-9,
        "τ=24h Δt=24h: priority = exp(-1) ≈ 0.3679,实际 {priority}"
    );
}

/// compute_priority 公式验证:τ=24h, Δt=72h 时 priority ≈ 0.0498 < 0.1(应降级)
///
/// 任务要求:τ=24h, Δt=72h 时 priority < 0.1 触发降级
#[test]
fn compute_priority_tau_24h_delta_72h_below_threshold() {
    let priority = compute_priority(1, 259200.0, 86400.0); // 72h = 259200s
    assert!(
        (priority - 0.049787068367863944).abs() < 1e-9,
        "τ=24h Δt=72h: priority ≈ 0.0498,实际 {priority}"
    );
    assert!(priority < DEMOTION_THRESHOLD_F64, "应 < 0.1 触发降级");
}

/// should_demote_metadata 仅用 access_count + delta_t + tau(不加载 content)
///
/// 验证:access_count=1, Δt=72h, τ=24h → priority ≈ 0.0498 < 0.1 → 应降级
#[test]
fn should_demote_metadata_demotes_when_priority_below_threshold() {
    let demote = should_demote_metadata(1, 259200.0, 86400.0); // 72h, τ=24h
    assert!(
        demote,
        "access_count=1 Δt=72h τ=24h: priority ≈ 0.0498 < 0.1,应降级"
    );
}

/// should_demote_metadata:access_count=0 时永远降级(从未访问的条目)
#[test]
fn should_demote_metadata_demotes_when_access_count_zero() {
    let demote = should_demote_metadata(0, 0.0, 86400.0);
    assert!(demote, "access_count=0 时应降级(从未访问)");
}

/// should_demote_metadata:Δt=0 时不应降级(刚访问的条目)
#[test]
fn should_demote_metadata_does_not_demote_recently_accessed() {
    let demote = should_demote_metadata(10, 0.0, 86400.0);
    assert!(!demote, "access_count=10 Δt=0: priority=10 > 0.1,不应降级");
}

/// should_demote_metadata:6mo 级 KeepForever(tau=∞)永不降级
///
/// 语义(§17.5):"6 月级 KeepForever 且关键决策不压缩"
#[test]
fn should_demote_metadata_never_demotes_when_tau_infinity() {
    // 即使 access_count=1 且 Δt=1 年(31536000s),tau=∞ 时 priority = 1.0 > 0.1
    let demote = should_demote_metadata(1, 31_536_000.0, f64::INFINITY);
    assert!(
        !demote,
        "tau=∞(KeepForever): priority = access_count,不应降级"
    );
}

/// DEMOTION_THRESHOLD_F64 常量应为 0.1(§17.2)
#[test]
fn demotion_threshold_constant_is_0_1() {
    assert_eq!(DEMOTION_THRESHOLD_F64, 0.1, "降级阈值应为 0.1(§17.2)");
}

// ============================================================
// 5. INV-8 单调性集成(§17.5)— 3 个测试
// ============================================================

/// 三级归档的 source→target 均满足 INV-8 单调性(合法降级)
///
/// 1mo: Hot→Warm, 3mo: Warm→Cold, 6mo: Cold→Ice — 全链路单调
#[test]
fn archive_levels_satisfy_inv8_monotonicity() {
    use chimera_mas::invariants::InvariantChecker;
    for (from, to, label) in [
        (
            ArchiveScheduleLevel::Month1.source_tier(),
            ArchiveScheduleLevel::Month1.target_tier(),
            "1mo: Hot→Warm",
        ),
        (
            ArchiveScheduleLevel::Month3.source_tier(),
            ArchiveScheduleLevel::Month3.target_tier(),
            "3mo: Warm→Cold",
        ),
        (
            ArchiveScheduleLevel::Month6.source_tier(),
            ArchiveScheduleLevel::Month6.target_tier(),
            "6mo: Cold→Ice",
        ),
    ] {
        assert!(
            InvariantChecker::check_inv8_archive_monotonicity(from, to).is_ok(),
            "{label} 应满足 INV-8 单调性"
        );
    }
}

/// ArchiveScheduler::trigger 对三级归档均成功(因 source<target 单调)
#[test]
fn archive_scheduler_trigger_all_levels_succeed() {
    for level in [
        ArchiveScheduleLevel::Month1,
        ArchiveScheduleLevel::Month3,
        ArchiveScheduleLevel::Month6,
    ] {
        let result = ArchiveScheduler::trigger(level, ArchiveTriggerCondition::Cron);
        assert!(
            result.is_ok(),
            "{level:?} cron 触发应成功(INV-8 单调性满足)"
        );
    }
}

/// ArchiveOperation 字段完整性:level / condition / source_tier / target_tier / compression
#[test]
fn archive_operation_fields_are_complete() {
    let op = ArchiveScheduler::trigger(ArchiveScheduleLevel::Month1, ArchiveTriggerCondition::Cron)
        .expect("1mo 触发应成功");
    // 字段完整性验证
    assert_eq!(op.level, ArchiveScheduleLevel::Month1);
    assert!(matches!(op.condition, ArchiveTriggerCondition::Cron));
    assert_eq!(op.source_tier, ArchiveTier::Hot);
    assert_eq!(op.target_tier, ArchiveTier::Warm);
    assert!(matches!(
        op.compression,
        CompressionStrategy::HcwSummary { .. }
    ));
}

// ============================================================
// 6. ArchiveCompressor 压缩器(§17.2 压缩策略)— 4 个测试
// ============================================================

/// ArchiveCompressor HCW 摘要:输出 token 数 ≤ 500
///
/// 复用 crate API 不匹配(hcw-window ContextCompressor 面向 ContextEntry 数组),
/// 本地实现:按权重 0.4/0.3/0.3 切分内容,取前 max_tokens 字符作为摘要
#[test]
fn compressor_hcw_summary_produces_within_token_limit() {
    let content = "这是一段需要被压缩的 Agent 记忆内容。".repeat(100);
    let strategy = CompressionStrategy::HcwSummary {
        max_tokens: 500,
        weights: [0.4, 0.3, 0.3],
    };
    let result = ArchiveCompressor::compress(&strategy, &content).expect("HCW 摘要压缩应成功");
    assert!(
        result.token_count <= 500,
        "HCW 摘要 token 数应 ≤ 500,实际 {}",
        result.token_count
    );
}

/// ArchiveCompressor 关系抽取:生成 512-dim CLV 占位向量
///
/// 复用 crate API 不匹配(mlc-engine L2 SemanticMemory 需 SQLite 持久化),
/// 本地实现:生成 512-dim 零向量占位(实际语义抽取由 mlc-engine 异步完成)
#[test]
fn compressor_relation_extraction_produces_clv_placeholder() {
    let content = "Agent 1 与 Agent 2 协作完成任务 X";
    let result = ArchiveCompressor::compress(&CompressionStrategy::RelationExtraction, content)
        .expect("关系抽取压缩应成功");
    // 验证生成了 512-dim CLV 占位向量
    assert!(
        result.metadata.clv_placeholder.len() == 512,
        "关系抽取应生成 512-dim CLV 占位向量,实际维度: {}",
        result.metadata.clv_placeholder.len()
    );
}

/// ArchiveCompressor 深度压缩:6mo 级关键决策不压缩(KeepForever)
///
/// 语义(§17.5):"6 月级 KeepForever 且关键决策不压缩"
#[test]
fn compressor_deep_compression_preserves_key_decisions() {
    let content = "关键决策:选择方案 A 而非方案 B,原因是性能更优";
    let result = ArchiveCompressor::compress(&CompressionStrategy::DeepCompression, content)
        .expect("深度压缩应成功");
    // 6mo 级 KeepForever:摘要应保留原文关键内容
    assert!(
        result.summary.contains("关键决策"),
        "深度压缩应保留关键决策原文,实际摘要: {}",
        result.summary
    );
}

/// ArchiveCompressor 返回 CompressedContent 结构完整性
#[test]
fn compressor_returns_complete_compressed_content() {
    let strategy = CompressionStrategy::HcwSummary {
        max_tokens: 100,
        weights: [0.4, 0.3, 0.3],
    };
    let result = ArchiveCompressor::compress(&strategy, "test content").expect("压缩应成功");
    // 验证 CompressedContent 字段完整性
    assert!(
        !result.summary.is_empty() || result.token_count == 0,
        "摘要字段存在"
    );
    assert!(result.token_count <= 100, "token 数受 max_tokens 约束");
}

// ============================================================
// 7. ArchiveTierInvalid 错误变体(SubTask 17.11)— 1 个测试
// ============================================================

/// MasError::ArchiveTierInvalid 变体存在且 Display 正确
///
/// SubTask 17.11:新增变体,用于触发未定义归档层级时返回
#[test]
fn mas_error_archive_tier_invalid_variant_exists() {
    let err = MasError::ArchiveTierInvalid {
        tier: "Unknown".to_string(),
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("Unknown"),
        "ArchiveTierInvalid Display 应包含 tier 字段,实际: {msg}"
    );
}
