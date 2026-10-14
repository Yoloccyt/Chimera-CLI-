//! L0 wire 类型双格式往返 proptest — JSON + MessagePack 恒等不变量（WI-G2）
//!
//! 对应方案: `Chimera架构重构与优化方案_v3.3-L0L3融合终版.md` §2.1 WI-G2
//! （承 v3.2 WI-G2 / v3.0 L0-3「wire 类型往返 proptest 扩全」）
//!
//! # 覆盖范围（9 组 wire 类型，2026-10-07 现取）
//!
//! | 组 | 类型 |
//! |---|---|
//! | app | ThreadId/TurnId/ItemId/ReqId/UserInput/Thread/Item/ItemStatus/AppTokenUsage/ApprovalRequest/ApprovalDecision/PermissionMode/ThreadStartParams/AppOp(5 变体)/AppEvent(2 变体) |
//! | errors | ErrorLayer（7 plane × 10 kind 全覆盖）/EventFailure/ToolFailure/NexusError(7 变体) |
//! | event_v2 | EventNamespace/EventTypeId/ImportanceScore/Compressibility/EventPattern/EventMetadataV2/NamespaceQuotaV2 |
//! | experience_card | AtomicOperator/ThreeFactorScore/NormalizedThreeFactor/ErrorSignature/ExecutionStatus/EnvironmentInfo/CardMetadata/ExperienceCard |
//! | token_evidence | ToolCallRecord/TokenLedgerEntry/SegmentCreationReason/SegmentMetadata |
//! | memory_pyramid | MemoryPyramidLevel/AtomicCardType/AtomicMemoryCard/SceneBlock/PersonaSummary |
//! | skill_lifecycle | SkillLifecycleState/SkillLifecycleContract |
//! | omni_message | ModelConfig/TokenUsage/OmniMessage（6 变体全覆盖） |
//! | graph_identity | GraphIdentity |
//!
//! # 豁免登记（非 wire 面，故意不覆盖）
//!
//! - `mcsm::{SinkhornParams, ProjectedMatrix}`：纯计算面，**无 serde 派生**（2026-10-07
//!   实测 `ProjectedMatrix` 仅 Debug/Clone/PartialEq）——不属 wire 契约，不经序列化跨界。
//! - `app::AppEvent` 其余变体与 `AppOp::ThreadFork`：协议面全量往返由
//!   `nexus-app-server` 侧 JSON-RPC E2E 承担（协议面主检），本文件覆盖已确认形状的代表集。
//! - `event_v2::EventPattern::And` 递归深度 = 1（单层）；更深嵌套的编码路径与单层同构。
//!
//! # 协议纪律（ADR-004 / ADR-184 / ADR-194 D4）
//!
//! - MessagePack 统一 `to_vec_named`（命名 map 编码）：本仓含 `skip_serializing_if`
//!   可选字段的类型在 array 位置编码下会破坏反序列化长度（event_metadata.rs:64 注释
//!   同源教训），命名 map 下缺失键 → `None`/默认，双向兼容。
//! - `f32/f64` 策略一律取**有界有限区间**（NaN != NaN 会制造伪失败）。
//! - **已知上游边界（2026-10-07 实测）**：serde_json 十进制→f64 解析对紧邻双精度
//!   存在 1 ulp 非正确舍入（复现见 `serde_json_f64_parser_ulp_boundary_documented`）——
//!   JSON 路径以 f64 ≤ 4 ulp 容差比较（结构仍严格），MessagePack 路径保持**位精确**断言。
//! - 新增 L0 wire 类型必须加入本文件覆盖表（与 G-91 契约消费者门同精神）。
//!
//! # TDD 守恒
//! 本文件为既有类型的补测扩全，不改类型定义；预期首跑即绿（若红 = 抓到真实 wire 缺陷）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench/example code idiom; E-5 targets production code
                                                    // proptest 配置下移至每个 `proptest!` 块内：文件级 inner attr 无法解析
                                                    // `proptest_config`（inner attr 必须先于一切 item，而属性宏需 `use` 先导入——
                                                    // Rust 顺序限制；块内配置是 proptest 官方推荐形态）。

use chrono::{DateTime, Utc};
use nexus_contracts::app::{
    AppEvent, AppOp, AppTokenUsage, ApprovalDecision, ApprovalRequest, Item, ItemId, ItemStatus,
    PermissionMode, ReqId, Thread, ThreadId, ThreadStartParams, TurnId, UserInput,
};
use nexus_contracts::errors::{ErrorLayer, EventFailure, NexusError, ToolFailure};
use nexus_contracts::event_metadata::EventMetadata;
use nexus_contracts::event_v2::{
    Compressibility, EventMetadataV2, EventNamespace, EventPattern, EventTypeId, ImportanceScore,
    NamespaceQuotaV2,
};
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, EnvironmentInfo, ErrorSignature, ExecutionStatus, ExperienceCard,
    NormalizedThreeFactor, ThreeFactorScore,
};
use nexus_contracts::graph_identity::GraphIdentity;
use nexus_contracts::memory_pyramid::{
    AtomicCardType, AtomicMemoryCard, MemoryPyramidLevel, PersonaSummary, SceneBlock,
};
use nexus_contracts::omni_message::{ModelConfig, OmniMessage, TokenUsage};
use nexus_contracts::skill_lifecycle::{SkillLifecycleContract, SkillLifecycleState};
use nexus_contracts::token_evidence::{
    SegmentCreationReason, SegmentMetadata, TokenLedgerEntry, ToolCallRecord,
};
use proptest::prelude::*;
use serde::{de::DeserializeOwned, Serialize};

// ============================================================
// 双格式往返断言 — 全部属性共用（JSON + MessagePack 命名 map）
// ============================================================

/// JSON 值树容差等价 — f64 数值 ≤ 4 ulp 容差，其余结构严格。
///
/// WHY 非严格：实测（2026-10-07，本工作树工具链）serde_json 的十进制→f64
/// 解析对紧邻双精度存在 1 ulp 非正确舍入（最短表示 `0.22119553043508522`
/// 解析回 `…0852`，位 3fcc…ba68 → 3fcc…ba67；Python json 与 rmp-serde 均位精确）。
/// 该差异为上游解析器边界、非本仓类型缺陷；回归价值（结构/字段/编码变化）
/// 由结构严格 + MsgPack 位精确双覆盖保留。
fn json_values_tolerant_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Number(x), serde_json::Value::Number(y)) => {
            match (x.as_f64(), y.as_f64()) {
                (Some(xf), Some(yf)) => {
                    xf == yf
                        || (xf - yf).abs() <= 4.0 * f64::EPSILON * xf.abs().max(yf.abs()).max(1.0)
                }
                _ => x == y,
            }
        }
        (serde_json::Value::Array(xs), serde_json::Value::Array(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys)
                    .all(|(x, y)| json_values_tolerant_eq(x, y))
        }
        (serde_json::Value::Object(xo), serde_json::Value::Object(yo)) => {
            xo.len() == yo.len()
                && xo
                    .iter()
                    .all(|(k, xv)| yo.get(k).is_some_and(|yv| json_values_tolerant_eq(xv, yv)))
        }
        _ => a == b,
    }
}

fn assert_roundtrip<T>(value: &T, what: &str)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("JSON 序列化失败");
    let back: T = serde_json::from_str(&json).expect("JSON 反序列化失败");
    let v_orig = serde_json::to_value(value).expect("JSON to_value 失败");
    let v_back = serde_json::to_value(&back).expect("JSON to_value(back) 失败");
    assert!(
        json_values_tolerant_eq(&v_orig, &v_back),
        "{what}: JSON roundtrip 超出容差（f64 ≤ 4 ulp；serde_json 解析器边界）"
    );

    let mp = rmp_serde::to_vec_named(value).expect("MessagePack 序列化失败");
    let back2: T = rmp_serde::from_slice(&mp).expect("MessagePack 反序列化失败");
    assert_eq!(
        *value, back2,
        "{what}: MessagePack roundtrip 失配（位精确路径）"
    );
}

// ============================================================
// 共享基础策略（有界有限，规避 NaN / 极端值伪失败）
// ============================================================

/// 可打印 ASCII 字符串（0-32 字符）——覆盖空串与常规载荷
fn any_text() -> impl Strategy<Value = String> {
    "[ -~]{0,32}"
}

/// 非空标识串（GraphIdentity 等不变量要求非空）
fn any_nonempty_id() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_\\-]{1,10}"
}

fn any_box() -> impl Strategy<Value = Box<str>> {
    any_text().prop_map(Box::from)
}

fn any_nonempty_box() -> impl Strategy<Value = Box<str>> {
    any_nonempty_id().prop_map(Box::from)
}

fn any_box_list() -> impl Strategy<Value = Vec<Box<str>>> {
    proptest::collection::vec(any_box(), 0..4)
}

/// 有界有限 f32（[-1e5, 1e5]）
fn any_f32r() -> impl Strategy<Value = f32> {
    -1.0e5f32..1.0e5f32
}

/// 单位区间 f32（[0,1]，评分类）
fn any_unit() -> impl Strategy<Value = f32> {
    0.0f32..=1.0
}

fn any_opt_unit() -> impl Strategy<Value = Option<f32>> {
    proptest::option::of(any_unit())
}

fn any_opt_box() -> impl Strategy<Value = Option<Box<str>>> {
    proptest::option::of(any_box())
}

fn any_opt_text() -> impl Strategy<Value = Option<String>> {
    proptest::option::of(any_text())
}

fn any_user_input() -> impl Strategy<Value = UserInput> {
    (any_box(), any_opt_box()).prop_map(|(text, extras)| UserInput { text, extras })
}

/// 合理毫秒时间戳 -> DateTime<Utc>（1970..2100）
fn any_dt() -> impl Strategy<Value = DateTime<Utc>> {
    (0i64..=4_102_444_800_000i64).prop_map(|ms| DateTime::<Utc>::from_timestamp_millis(ms).unwrap())
}

fn any_env_info() -> impl Strategy<Value = EnvironmentInfo> {
    (any_box(), any_box(), any_box(), any_box()).prop_map(
        |(rust_version, os, cpu_arch, chimera_version)| EnvironmentInfo {
            rust_version,
            os,
            cpu_arch,
            chimera_version,
        },
    )
}

fn any_token_usage_omni() -> impl Strategy<Value = TokenUsage> {
    (any::<u64>(), any::<u64>()).prop_map(|(prompt_tokens, completion_tokens)| TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens.wrapping_add(completion_tokens),
    })
}

fn any_card_metadata() -> impl Strategy<Value = CardMetadata> {
    (
        any::<u64>(),
        any_token_usage_omni(),
        any::<i32>(),
        any_box_list(),
        any_env_info(),
    )
        .prop_map(
            |(execution_time_ms, token_usage, lines_changed, skills_used, environment)| {
                CardMetadata {
                    execution_time_ms,
                    token_usage,
                    lines_changed,
                    skills_used,
                    environment,
                }
            },
        )
}

fn any_event_metadata() -> impl Strategy<Value = EventMetadata> {
    any_nonempty_id().prop_map(|source| EventMetadata::new(&source))
}

fn any_graph_identity() -> impl Strategy<Value = GraphIdentity> {
    (any_nonempty_id(), any_nonempty_id(), any_nonempty_id())
        .prop_map(|(g, r, n)| GraphIdentity::new(&g, &r, &n))
}

fn any_three_factor() -> impl Strategy<Value = ThreeFactorScore> {
    (any_f32r(), any_f32r(), any_unit()).prop_map(|(quality, progress, novelty)| ThreeFactorScore {
        quality,
        progress,
        novelty,
    })
}

fn any_error_signature() -> impl Strategy<Value = ErrorSignature> {
    (any_box(), any_box(), any_box(), any_box()).prop_map(
        |(error_type, error_location, error_summary, error_hash)| ErrorSignature {
            error_type,
            error_location,
            error_summary,
            error_hash,
        },
    )
}

fn any_tool_call_record() -> impl Strategy<Value = ToolCallRecord> {
    (any_box(), any_box(), any_box(), any::<u32>()).prop_map(
        |(tool_name, arguments, result, latency_ms)| ToolCallRecord {
            tool_name,
            arguments,
            result,
            latency_ms,
        },
    )
}

fn any_event_pattern() -> impl Strategy<Value = EventPattern> {
    let leaf = prop_oneof![
        any_box()
            .prop_map(EventTypeId)
            .prop_map(EventPattern::Exact),
        any_box().prop_map(EventPattern::Namespace),
        (proptest::collection::vec(-1.0f32..1.0f32, 0..8), any_unit()).prop_map(
            |(embedding, threshold)| EventPattern::Semantic {
                embedding,
                threshold,
            }
        ),
    ];
    prop_oneof![
        leaf.clone(),
        // And 递归深度 = 1（本轮收窄；更深嵌套与单层编码路径同构，见文件头豁免登记）
        proptest::collection::vec(leaf, 0..3).prop_map(|v| EventPattern::And(v.into_boxed_slice())),
    ]
}

// ============================================================
// 1. graph_identity（WI-04 GIP 三元组）
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn graph_identity_roundtrip(gi in any_graph_identity()) {
        assert_roundtrip(&gi, "GraphIdentity");
    }
}

// ============================================================
// 2. app（L10 协议面契约：ID newtype / 条目 / 操作 / 事件）
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn app_id_newtypes_roundtrip(
        t in any_nonempty_box(), u in any_nonempty_box(),
        i in any_nonempty_box(), r in any_nonempty_box()
    ) {
        assert_roundtrip(&ThreadId(t), "ThreadId");
        assert_roundtrip(&TurnId(u), "TurnId");
        assert_roundtrip(&ItemId(i), "ItemId");
        assert_roundtrip(&ReqId(r), "ReqId");
    }

    #[test]
    fn app_payload_and_enums_roundtrip(
        text in any_box(), extras in any_opt_box(),
        input_tokens in any::<u64>(), output_tokens in any::<u64>(),
        cache_read_tokens in any::<u64>(), cache_creation_tokens in any::<u64>(),
        status in prop_oneof![
            Just(ItemStatus::Started), Just(ItemStatus::InProgress),
            Just(ItemStatus::Completed), Just(ItemStatus::Failed),
        ],
        decision in prop_oneof![
            Just(ApprovalDecision::Allow), Just(ApprovalDecision::Deny),
            Just(ApprovalDecision::AllowOnce),
        ],
        mode in prop_oneof![
            Just(PermissionMode::Plan), Just(PermissionMode::AcceptEdits),
            Just(PermissionMode::Default), Just(PermissionMode::DontAsk),
            Just(PermissionMode::Auto), Just(PermissionMode::BypassPermissions),
        ],
    ) {
        assert_roundtrip(&UserInput { text, extras }, "UserInput");
        assert_roundtrip(&AppTokenUsage { input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens }, "AppTokenUsage");
        assert_roundtrip(&status, "ItemStatus");
        assert_roundtrip(&decision, "ApprovalDecision");
        assert_roundtrip(&mode, "PermissionMode");
    }

    #[test]
    fn app_structs_roundtrip(
        thread_id in any_nonempty_id(), goal_id in any_box(), run_id in any_box(),
        created_at_ms in any::<u64>(),
        item_id in any_nonempty_id(), turn_id in any_nonempty_id(),
        kind in any_box(), payload in any_box(), extras in any_opt_box(),
        req_id in any_nonempty_id(), description in any_box(), category in any_box(),
        has_item in any::<bool>(),
        p_goal in any_box(), p_run in any_box(),
        p_input in proptest::option::of(any_user_input()), p_extras in any_opt_box(),
    ) {
        let thread = Thread {
            thread_id: ThreadId::new(&thread_id),
            goal_id, run_id, created_at_ms,
        };
        assert_roundtrip(&thread, "Thread");

        let item = Item {
            item_id: ItemId::new(&item_id),
            thread_id: ThreadId::new(&thread_id),
            turn_id: TurnId::new(&turn_id),
            kind, status: ItemStatus::Completed, payload, extras,
        };
        assert_roundtrip(&item, "Item");

        let req = ApprovalRequest {
            request_id: ReqId::new(&req_id),
            description, category,
            item_id: if has_item { Some(ItemId::new(&item_id)) } else { None },
        };
        assert_roundtrip(&req, "ApprovalRequest");

        let params = ThreadStartParams {
            goal_id: p_goal, run_id: p_run,
            initial_input: p_input,
            extras: p_extras,
        };
        assert_roundtrip(&params, "ThreadStartParams");
    }
}

// ============================================================
// 2b. app 操作/事件（协议动线代表集）
// ============================================================

fn any_thread_start_params() -> impl Strategy<Value = ThreadStartParams> {
    (
        any_box(),
        any_box(),
        proptest::option::of(any_user_input()),
        any_opt_box(),
    )
        .prop_map(
            |(goal_id, run_id, initial_input, extras)| ThreadStartParams {
                goal_id,
                run_id,
                initial_input,
                extras,
            },
        )
}

fn any_app_op() -> impl Strategy<Value = AppOp> {
    prop_oneof![
        any_thread_start_params().prop_map(AppOp::ThreadStart),
        (any_nonempty_id(), any_user_input()).prop_map(|(thread_id, input)| AppOp::TurnSubmit {
            thread_id: ThreadId::new(&thread_id),
            input,
        }),
        any_nonempty_id().prop_map(|turn_id| AppOp::TurnInterrupt {
            turn_id: TurnId::new(&turn_id),
        }),
        (any_nonempty_id(), any::<bool>()).prop_map(|(request_id, allow)| AppOp::ApprovalRespond {
            request_id: ReqId::new(&request_id),
            decision: if allow {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Deny
            },
        }),
        prop_oneof![
            Just(PermissionMode::Plan),
            Just(PermissionMode::Default),
            Just(PermissionMode::DontAsk),
        ]
        .prop_map(|mode| AppOp::ModeSet { mode }),
    ]
}

fn any_app_event() -> impl Strategy<Value = AppEvent> {
    prop_oneof![
        (any_nonempty_id(), any_box(), any_box(), any::<u64>()).prop_map(
            |(thread_id, goal_id, run_id, created_at_ms)| AppEvent::ThreadStarted {
                thread: Thread {
                    thread_id: ThreadId::new(&thread_id),
                    goal_id,
                    run_id,
                    created_at_ms,
                },
            }
        ),
        (any_nonempty_id(), any_nonempty_id(), any_box(), any_box()).prop_map(
            |(item_id, thread_id, kind, payload)| AppEvent::ItemChanged {
                item: Item {
                    item_id: ItemId::new(&item_id),
                    thread_id: ThreadId::new(&thread_id),
                    turn_id: TurnId::new("turn-1"),
                    kind,
                    status: ItemStatus::InProgress,
                    payload,
                    extras: None,
                },
            }
        ),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn app_ops_events_roundtrip(op in any_app_op(), ev in any_app_event()) {
        assert_roundtrip(&op, "AppOp(代表集)");
        assert_roundtrip(&ev, "AppEvent(代表集)");
    }
}

// ============================================================
// 3. errors（ErrorLayer 全矩阵 + NexusError 代表集）
// ============================================================

fn any_event_failure() -> impl Strategy<Value = EventFailure> {
    prop_oneof![
        any_text().prop_map(EventFailure::SerializationError),
        any_text().prop_map(EventFailure::InvalidEventType),
        (any_text(), any::<usize>(), any::<usize>()).prop_map(|(namespace, current, max)| {
            EventFailure::QuotaExceeded {
                namespace,
                current,
                max,
            }
        }),
    ]
}

fn any_tool_failure() -> impl Strategy<Value = ToolFailure> {
    prop_oneof![
        (any_text(), any::<u64>()).prop_map(|(tool_name, duration_ms)| ToolFailure::Timeout {
            tool_name,
            duration_ms
        }),
        any_text().prop_map(|server_name| ToolFailure::McpDisconnected { server_name }),
    ]
}

/// ErrorLayer 全矩阵（7 plane × 10 kind 全覆盖，2026-10-07 现状）
fn any_error_layer() -> impl Strategy<Value = ErrorLayer> {
    prop_oneof![
        any_event_failure().prop_map(ErrorLayer::Event),
        (any::<usize>(), any::<usize>())
            .prop_map(|(current, max)| ErrorLayer::MemoryBudgetExceeded { current, max }),
        any_tool_failure().prop_map(ErrorLayer::Tool),
        any_text().prop_map(|details| ErrorLayer::SecuritySandboxViolation { details }),
        Just(ErrorLayer::SubAgentNestingForbidden),
        any_text().prop_map(|operation| ErrorLayer::ApprovalDenied { operation }),
        (any::<u16>(), any_text())
            .prop_map(|(status, message)| ErrorLayer::ModelApiError { status, message }),
    ]
}

fn any_nexus_error() -> impl Strategy<Value = NexusError> {
    prop_oneof![
        any_text().prop_map(NexusError::SerializationError),
        any_text().prop_map(NexusError::InvalidEventType),
        (any::<usize>(), any::<usize>())
            .prop_map(|(current, max)| NexusError::ContextBudgetExceeded { current, max }),
        (any_text(), any::<u64>()).prop_map(|(tool_name, duration_ms)| NexusError::ToolTimeout {
            tool_name,
            duration_ms
        }),
        any_text().prop_map(|details| NexusError::SandboxViolation { details }),
        Just(NexusError::NestedSubAgentForbidden),
        any_text().prop_map(|server_name| NexusError::McpDisconnected { server_name }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn errors_roundtrip(layer in any_error_layer(), err in any_nexus_error()) {
        assert_roundtrip(&layer, "ErrorLayer(全矩阵)");
        assert_roundtrip(&err, "NexusError(代表集)");
    }
}

// ============================================================
// 4. event_v2（双轨元数据 + 订阅模式）
// ============================================================

fn any_event_metadata_v2() -> impl Strategy<Value = EventMetadataV2> {
    (
        any_event_metadata(),
        proptest::option::of(any_graph_identity()),
        -1.0f64..1.0f64,
        0.0f64..=1.0f64,
        prop_oneof![
            Just(Compressibility::Incompressible),
            Just(Compressibility::Low),
            Just(Compressibility::Medium),
            Just(Compressibility::High),
        ],
        any_box_list(),
        proptest::option::of(any_event_pattern()),
    )
        .prop_map(
            |(
                base,
                graph_identity,
                residual_weight,
                residual_decay,
                compressibility,
                key_symbols,
                subscriber_pattern,
            )| {
                EventMetadataV2 {
                    base,
                    graph_identity,
                    residual_weight,
                    residual_decay,
                    compressibility,
                    key_symbols,
                    subscriber_pattern,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn event_v2_roundtrip(
        meta in any_event_metadata_v2(),
        pattern in any_event_pattern(),
        ns in prop_oneof![
            Just(EventNamespace::Builtin), Just(EventNamespace::Mcp),
            Just(EventNamespace::SubAgent), Just(EventNamespace::Hook),
            Just(EventNamespace::External),
        ],
        score in any_unit(),
        quota in (any::<usize>(), any::<usize>()),
    ) {
        assert_roundtrip(&meta, "EventMetadataV2");
        assert_roundtrip(&pattern, "EventPattern");
        assert_roundtrip(&ns, "EventNamespace");
        assert_roundtrip(&ImportanceScore::new(score), "ImportanceScore");
        let (max_types_per_namespace, max_total_types) = quota;
        assert_roundtrip(&NamespaceQuotaV2 { max_types_per_namespace, max_total_types }, "NamespaceQuotaV2");
    }
}

// ============================================================
// 5. experience_card（Ω₁₀ 核心契约）
// ============================================================

fn any_experience_card() -> impl Strategy<Value = ExperienceCard> {
    (
        (
            any_box(),
            any_box(),
            any_box(),
            any_opt_box(),
            any_dt(),
            prop_oneof![
                Just(AtomicOperator::Draft),
                Just(AtomicOperator::Improve),
                Just(AtomicOperator::Debug),
                Just(AtomicOperator::Crossover),
            ],
            any_unit(),
            any_f32r(),
        ),
        (
            any_box(),
            proptest::option::of(any_error_signature()),
            any_three_factor(),
            prop_oneof![
                Just(ExecutionStatus::Success),
                Just(ExecutionStatus::Error),
                Just(ExecutionStatus::MissingCode),
                Just(ExecutionStatus::NoSubmit),
                Just(ExecutionStatus::ScoreFailed),
                Just(ExecutionStatus::Timeout),
            ],
            any_box_list(),
            any_opt_box(),
            any_card_metadata(),
        ),
    )
        .prop_map(
            |(
                (
                    card_id,
                    task_id,
                    node_id,
                    parent_id,
                    created_at,
                    operator,
                    score,
                    delta_vs_parent,
                ),
                (
                    method_family,
                    error_signature,
                    three_factor,
                    execution_status,
                    token_evidence_ids,
                    segment_id,
                    metadata,
                ),
            )| ExperienceCard {
                card_id,
                task_id,
                node_id,
                parent_id,
                created_at,
                operator,
                score,
                delta_vs_parent,
                method_family,
                error_signature,
                three_factor,
                execution_status,
                token_evidence_ids,
                segment_id,
                metadata,
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn experience_card_core_roundtrip(
        normalized in (any_unit(), any_unit(), any_unit())
            .prop_map(|(quality, progress, novelty)| NormalizedThreeFactor { quality, progress, novelty }),
        sig in any_error_signature(),
        op in prop_oneof![
            Just(AtomicOperator::Draft), Just(AtomicOperator::Improve),
            Just(AtomicOperator::Debug), Just(AtomicOperator::Crossover),
        ],
        tf in any_three_factor(),
        env in any_env_info(),
    ) {
        assert_roundtrip(&normalized, "NormalizedThreeFactor");
        assert_roundtrip(&sig, "ErrorSignature");
        assert_roundtrip(&op, "AtomicOperator");
        assert_roundtrip(&tf, "ThreeFactorScore");
        assert_roundtrip(&env, "EnvironmentInfo");
    }

    #[test]
    fn experience_card_full_roundtrip(card in any_experience_card()) {
        assert_roundtrip(&card, "ExperienceCard");
    }
}

// ============================================================
// 6. token_evidence（Dressage 训练证据契约）
// ============================================================

fn any_token_ledger_entry() -> impl Strategy<Value = TokenLedgerEntry> {
    (
        (
            any_box(),
            any::<u32>(),
            any_box(),
            any_box(),
            any_box(),
            any::<u64>(),
            proptest::option::of(any_graph_identity()),
        ),
        (
            proptest::collection::vec(any::<u32>(), 0..8),
            proptest::collection::vec(any::<u32>(), 0..8),
            proptest::collection::vec(-2.0f32..2.0f32, 0..8),
            proptest::collection::vec(any::<bool>(), 0..8),
        ),
        (
            proptest::collection::vec(any_tool_call_record(), 0..3),
            proptest::option::of(proptest::collection::vec(
                proptest::collection::vec(any::<u32>(), 0..4),
                0..3,
            )),
        ),
    )
        .prop_map(
            |(
                (
                    entry_id,
                    turn_id,
                    session_id,
                    instance_id,
                    weight_version,
                    timestamp,
                    graph_identity,
                ),
                (input_token_ids, output_token_ids, output_logprobs, loss_mask),
                (tool_calls, moe_routing),
            )| {
                // 证据完整性不变量：输出三序列等长（截断到最小长度覆盖全部合法形态；输入独立不定长）
                let out_n = output_token_ids
                    .len()
                    .min(output_logprobs.len())
                    .min(loss_mask.len());
                TokenLedgerEntry {
                    entry_id,
                    turn_id,
                    session_id,
                    instance_id,
                    input_token_ids: input_token_ids.into_boxed_slice(),
                    output_token_ids: output_token_ids[..out_n].to_vec().into_boxed_slice(),
                    output_logprobs: output_logprobs[..out_n].to_vec().into_boxed_slice(),
                    loss_mask: loss_mask[..out_n].to_vec().into_boxed_slice(),
                    weight_version,
                    tool_calls,
                    moe_routing,
                    timestamp,
                    graph_identity,
                    schema_version: 0,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn token_evidence_roundtrip(
        record in any_tool_call_record(),
        reason in prop_oneof![
            Just(SegmentCreationReason::HistoryCompaction),
            Just(SegmentCreationReason::ToolSchemaChange),
            Just(SegmentCreationReason::MessageRewrite),
            Just(SegmentCreationReason::TitoFallback),
            Just(SegmentCreationReason::NaturalBoundary),
            Just(SegmentCreationReason::MaxLengthReached),
        ],
        seg_id in any_box(), parent in any_box(), idx in any::<u32>(),
        is_anchor in any::<bool>(), entries in any_box_list(),
        snapshot in proptest::collection::vec(any::<u8>(), 0..16),
        start in any::<u32>(), end in any::<u32>(),
        ledger in any_token_ledger_entry(),
    ) {
        assert_roundtrip(&record, "ToolCallRecord");
        assert_roundtrip(&reason, "SegmentCreationReason");
        let seg = SegmentMetadata {
            segment_id: seg_id, parent_traj_id: parent, segment_index: idx,
            is_anchor, token_entries: entries,
            context_snapshot: snapshot.into_boxed_slice(),
            start_turn: start, end_turn: end, creation_reason: reason,
            schema_version: 0,
        };
        assert_roundtrip(&seg, "SegmentMetadata");
        assert_roundtrip(&ledger, "TokenLedgerEntry");
    }
}

// ============================================================
// 7. memory_pyramid（MSCE + TencentDB 金字塔契约）
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn memory_pyramid_roundtrip(
        level in prop_oneof![
            Just(MemoryPyramidLevel::L0RawLog), Just(MemoryPyramidLevel::L1AtomicMemory),
            Just(MemoryPyramidLevel::L2SceneBlock), Just(MemoryPyramidLevel::L3Persona),
        ],
        card_type in prop_oneof![
            Just(AtomicCardType::Preference), Just(AtomicCardType::Event),
            Just(AtomicCardType::Rule), Just(AtomicCardType::Trace),
            Just(AtomicCardType::Policy), Just(AtomicCardType::EnvCognition),
        ],
        card_id in any_box(), priority in any::<u8>(), scene in any_box(),
        content in any_box(), source_traj_id in any_opt_box(),
        observation in any_opt_box(), reflection in any_opt_box(),
        value in any_opt_unit(), created_at in any::<u64>(), updated_at in any::<u64>(),
    ) {
        assert_roundtrip(&level, "MemoryPyramidLevel");
        assert_roundtrip(&card_type, "AtomicCardType");
        let card = AtomicMemoryCard {
            card_id, card_type, priority, scene, content, source_traj_id,
            state_snapshot: None, action_record: None,
            observation, reflection, value, created_at, updated_at,
        };
        assert_roundtrip(&card, "AtomicMemoryCard");
    }

    #[test]
    fn memory_pyramid_block_and_persona_roundtrip(
        block_id in any_box(), scene_name in any_box(), cards in any_box_list(),
        summary in any_box(), heat_value in any::<u32>(),
        trigger_phi in any_opt_box(), procedure_pi in any_opt_box(),
        verification_kappa in any_opt_box(), boundary_beta in any_opt_box(),
        gain_gamma in any_opt_unit(),
        persona_id in any_box(), user_id in any_box(), summary_p in any_box(),
        preferences in any_box_list(), rules in any_box_list(),
        created in any::<u64>(), updated in any::<u64>(),
    ) {
        let block = SceneBlock {
            block_id, scene_name, cards, summary, heat_value,
            trigger_phi, procedure_pi, verification_kappa, boundary_beta, gain_gamma,
        };
        assert_roundtrip(&block, "SceneBlock");

        let persona = PersonaSummary {
            persona_id, user_id, summary: summary_p, preferences, rules,
            created_at: created, updated_at: updated,
        };
        assert_roundtrip(&persona, "PersonaSummary");
    }
}

// ============================================================
// 8. skill_lifecycle（MSCE 技能生命周期契约）
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn skill_lifecycle_roundtrip(
        state in prop_oneof![
            Just(SkillLifecycleState::Probationary),
            Just(SkillLifecycleState::Active),
            Just(SkillLifecycleState::Archived),
        ],
        skill_id in any_box(), probation_start in any::<u64>(),
        probation_end in proptest::option::of(any::<u64>()),
        activation_threshold in any::<u32>(), success_count in any::<u32>(),
        failure_count in any::<u32>(), archive_threshold in any::<u32>(),
        last_used in any::<u64>(),
    ) {
        assert_roundtrip(&state, "SkillLifecycleState");
        let contract = SkillLifecycleContract {
            skill_id, state, probation_start, probation_end,
            activation_threshold, success_count, failure_count,
            archive_threshold, last_used,
        };
        assert_roundtrip(&contract, "SkillLifecycleContract");
    }
}

// ============================================================
// 9. omni_message（6 变体全覆盖）
// ============================================================

fn any_model_config() -> impl Strategy<Value = ModelConfig> {
    (
        any_text(),
        any_text(),
        -0.0f32..=2.0f32,
        any_unit(),
        0usize..=65_536usize,
        any::<bool>(),
    )
        .prop_map(
            |(provider, model, temperature, top_p, max_tokens, stream)| ModelConfig {
                provider,
                model,
                temperature,
                top_p,
                max_tokens,
                stream,
            },
        )
}

fn any_omni_message() -> impl Strategy<Value = OmniMessage> {
    prop_oneof![
        (any_text(), any_text(), any_model_config(), any::<u64>()).prop_map(
            |(request_id, prompt, model_config, timestamp)| OmniMessage::ModelRequest {
                request_id,
                prompt,
                model_config,
                timestamp
            }
        ),
        (any_text(), any_text(), any_token_usage_omni(), any::<u64>()).prop_map(
            |(request_id, content, usage, timestamp)| OmniMessage::ModelResponse {
                request_id,
                content,
                usage,
                timestamp
            }
        ),
        (any_text(), any_text(), any_box(), any::<u64>()).prop_map(
            |(request_id, tool_name, parameters, timestamp)| OmniMessage::ToolRequest {
                request_id,
                tool_name,
                parameters,
                timestamp
            }
        ),
        (
            any_text(),
            any::<bool>(),
            any_text(),
            any_opt_text(),
            any::<u64>()
        )
            .prop_map(|(request_id, success, output, error, timestamp)| {
                OmniMessage::ToolResult {
                    request_id,
                    success,
                    output,
                    error,
                    timestamp,
                }
            }),
        (any_text(), any_box(), any::<u64>()).prop_map(|(key, value, timestamp)| {
            OmniMessage::StateUpdate {
                key,
                value,
                timestamp,
            }
        }),
        (
            any::<u32>(),
            any_text(),
            any_text(),
            any_f32r(),
            any::<u64>()
        )
            .prop_map(|(step, action, observation, reward, timestamp)| {
                OmniMessage::TraceRecord {
                    step,
                    action,
                    observation,
                    reward,
                    timestamp,
                }
            }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn omni_message_roundtrip(
        msg in any_omni_message(),
        cfg in any_model_config(),
        usage in any_token_usage_omni(),
    ) {
        assert_roundtrip(&msg, "OmniMessage");
        assert_roundtrip(&cfg, "ModelConfig");
        assert_roundtrip(&usage, "TokenUsage");
    }
}

// ============================================================
// 上游边界登记（serde_json f64 解析 ≤1 ulp）
// ============================================================

#[test]
fn serde_json_f64_parser_ulp_boundary_documented() {
    // 2026-10-07 实测：最短表示 "0.22119553043508522"（位 3fcc5022989eba68）
    // 经 serde_json 解析回 "0.2211955304350852"（位 …ba67，1 ulp 下邻）；
    // Python json 与 rmp-serde 对同串均位精确 ⇒ 差异归上游 serde_json 解析器。
    // 本测试锁定上界（≤ 1 ulp）与 MsgPack 位精确；若上游修复为位精确，断言仍成立。
    let a: f64 = 0.22119553043508522;
    let s = serde_json::to_string(&a).unwrap();
    let back: f64 = serde_json::from_str(&s).unwrap();
    assert!(
        (back - a).abs() <= f64::EPSILON * a.abs().max(1.0),
        "差异超出登记的 1 ulp 上界: {back:?} vs {a:?}"
    );
    let mp = rmp_serde::to_vec_named(&a).unwrap();
    let back_mp: f64 = rmp_serde::from_slice(&mp).unwrap();
    assert_eq!(back_mp.to_bits(), a.to_bits(), "MsgPack 路径必须位精确");
}
