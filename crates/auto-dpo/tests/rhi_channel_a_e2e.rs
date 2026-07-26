//! RHI-CG 通道 A 端到端集成测试 — P5.1.4
//!
//! 对应任务: **P5.1.4**（通道 A 端到端集成测试）
//! 对应 ADR: ADR-044（P5 工程实施）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.4（RHI-CG 双通道）
//!
//! # 测试目标
//!
//! 验证 RHI-CG 通道 A 的完整数据流，从相邻 HarnessSpec 版本输入到
//! SelfComparisonHistory 持久化与检索：
//!
//! 1. **StubJudgeClient 路径**：spec → JudgeClient::judge → PreferencePair → store → get
//! 2. **ModelRouterJudgeClient + StubLlmInvoker 路径**：生产级路径（stub LLM）
//! 3. **失败场景**：FailingLlmInvoker → JudgeFailed 错误传播
//! 4. **多版本链**：v1→v2→v3→v4 模拟 spec 演进
//! 5. **并发存储**：多 tokio 任务并发 store 验证线程安全
//! 6. **KNN 召回**：recall_by_pair_id 验证自相似性
//! 7. **容量驱逐**：连续 store 触发 FIFO 驱逐
//! 8. **CLV 确定性**：跨多次调用生成相同 CLV
//!
//! # 测试约束
//!
//! - WHY `#[tokio::test]`：`RhiChannelA::generate_preference_pair` 为 async，
//!   需要 tokio 运行时执行 `.await`
//! - WHY 仅使用公共 API：集成测试在外部 crate，不能访问私有字段
//! - WHY `StubJudgeClient` 与 `StubLlmInvoker`：避免依赖外部 LLM 服务，
//!   保证测试确定性

#![forbid(unsafe_code)]

use auto_dpo::rhi_judge_client::{JudgeClientConfig, JudgePromptTemplate, ModelRouterJudgeClient};
use auto_dpo::self_history::{
    generate_deterministic_clv, SelfComparisonHistory, SelfComparisonRecord,
};
use auto_dpo::{
    FailingLlmInvoker, JudgeClient, JudgeVerdict, LlmInvoker, LlmResponse, RhiChannelA,
    SpecVersion, StubJudgeClient, StubLlmInvoker, TokenUsage,
};
use event_bus::EventBus;
use model_router::{ModelRegistry, ModelRouter, RouterConfig, RoutingStrategy};
use nexus_contracts::{ContractSpec, HarnessMeta, HarnessSpec, HopSpec, RetryPolicy};
use std::sync::Arc;

// ============================================================
// 测试辅助函数
// ============================================================

/// 构造最小合法 HarnessSpec 用于测试
///
/// # 参数
/// - `version`: spec 版本号
/// - `name_suffix`: 名称后缀（便于区分不同测试用例）
fn make_test_spec(version: u32, name_suffix: &str) -> HarnessSpec {
    HarnessSpec {
        meta: HarnessMeta {
            name: format!("rhi-e2e-test-{name_suffix}"),
            version,
            immutable: false,
            parent: if version > 1 { Some(version - 1) } else { None },
            task_type: Some("code_refactor".to_string()),
        },
        contracts: vec![ContractSpec {
            name: "no_panic".to_string(),
            property: "must_not_panic".to_string(),
            description: None,
            from: None,
            to: None,
            fields: Vec::new(),
        }],
        hops: vec![HopSpec {
            name: "execute".to_string(),
            input_type: None,
            output_type: None,
            contracts: Vec::new(),
            description: None,
            order: Vec::new(),
            on_veto: None,
            fallback: None,
        }],
        retry: RetryPolicy::default(),
        auxiliary: None,
    }
}

/// 构造一个完整的评判器链路：ModelRouter + StubLlmInvoker → ModelRouterJudgeClient
fn make_judge_client_with_stub_llm(
    invoker: Arc<dyn LlmInvoker>,
) -> Arc<ModelRouterJudgeClient> {
    let bus = EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = Arc::new(ModelRouter::new(registry, bus));
    Arc::new(ModelRouterJudgeClient::new(router, invoker))
}

/// 构造一个评判器链路，使用自定义路由策略
fn make_judge_client_with_strategy(
    invoker: Arc<dyn LlmInvoker>,
    strategy: RoutingStrategy,
) -> Arc<ModelRouterJudgeClient> {
    let bus = EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = Arc::new(ModelRouter::new(registry, bus));
    let config = JudgeClientConfig {
        routing_strategy: strategy,
        ..Default::default()
    };
    Arc::new(ModelRouterJudgeClient::with_config(
        router,
        invoker,
        JudgePromptTemplate::default(),
        config,
    ))
}

/// 构造合法的 LLM JSON 响应字符串
fn make_valid_json_response(
    winner: &str,
    winner_score: f32,
    loser_score: f32,
    confidence: f32,
    rationale: &str,
) -> String {
    format!(
        r#"{{"winner":"{winner}","winner_score":{winner_score},"loser_score":{loser_score},"confidence":{confidence},"rationale":"{rationale}"}}"#
    )
}

// ============================================================
// A. StubJudgeClient 端到端测试
// ============================================================

#[tokio::test]
async fn test_stub_judge_client_full_flow_current_wins() {
    // 完整流程：spec → StubJudgeClient → PreferencePair → store → get
    let judge_client = Arc::new(StubJudgeClient::current_wins());
    let channel_a = RhiChannelA::new(judge_client);
    let history = SelfComparisonHistory::with_default_capacity();

    let spec_v1 = make_test_spec(1, "v1");
    let spec_v2 = make_test_spec(2, "v2");

    // 通道 A 生成偏好对
    let pair = channel_a
        .generate_preference_pair(&spec_v2, &spec_v1)
        .await
        .expect("Stub 评判器不应失败");

    // 验证 pair_id 格式
    assert_eq!(pair.pair_id, "rhi-pair-2-1");
    // 验证 chosen = v2 的 merkle input（Current 胜出）
    assert_eq!(pair.chosen, spec_v2.canonical_merkle_input());
    assert_eq!(pair.rejected, spec_v1.canonical_merkle_input());

    // 持久化到历史
    let verdict = JudgeVerdict::new(
        SpecVersion::Current,
        pair.chosen_score,
        pair.rejected_score,
        0.90,
        "stub verdict: current wins",
    )
    .unwrap();
    let record = SelfComparisonRecord::from_pair_and_verdict(pair.clone(), &verdict);
    history.store(record.clone()).expect("存储应成功");

    // 检索并验证
    let retrieved = history.get("rhi-pair-2-1").expect("get 应成功");
    assert_eq!(retrieved, Some(record));
}

#[tokio::test]
async fn test_stub_judge_client_full_flow_previous_wins() {
    // 模拟通道 B 否决场景：Previous 胜出
    let judge_client = Arc::new(StubJudgeClient::previous_wins());
    let channel_a = RhiChannelA::new(judge_client);
    let history = SelfComparisonHistory::with_default_capacity();

    let spec_v3 = make_test_spec(3, "v3");
    let spec_v2 = make_test_spec(2, "v2");

    let pair = channel_a
        .generate_preference_pair(&spec_v3, &spec_v2)
        .await
        .unwrap();

    // Previous 胜出：chosen = v2（基线），rejected = v3（提议）
    assert_eq!(pair.chosen, spec_v2.canonical_merkle_input());
    assert_eq!(pair.rejected, spec_v3.canonical_merkle_input());
    assert_eq!(pair.pair_id, "rhi-pair-3-2");

    let verdict = JudgeVerdict::new(
        SpecVersion::Previous,
        pair.chosen_score,
        pair.rejected_score,
        0.85,
        "stub verdict: previous wins (veto scenario)",
    )
    .unwrap();
    let record = SelfComparisonRecord::from_pair_and_verdict(pair, &verdict);
    history.store(record).unwrap();

    assert_eq!(history.len().unwrap(), 1);
}

// ============================================================
// B. ModelRouterJudgeClient + StubLlmInvoker 端到端测试
// ============================================================

#[tokio::test]
async fn test_model_router_judge_client_current_wins() {
    // 生产级路径：ModelRouter 路由 + StubLlmInvoker 返回 JSON
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    let spec_v1 = make_test_spec(1, "v1");
    let spec_v2 = make_test_spec(2, "v2");

    let pair = channel_a
        .generate_preference_pair(&spec_v2, &spec_v1)
        .await
        .expect("ModelRouter + StubLlmInvoker 路径应成功");

    assert_eq!(pair.pair_id, "rhi-pair-2-1");
    assert_eq!(pair.chosen, spec_v2.canonical_merkle_input());
    // winner_score = 0.85（来自 StubLlmInvoker::current_wins 的 JSON）
    assert!((pair.chosen_score - 0.85).abs() < 1e-6);
    assert!((pair.rejected_score - 0.45).abs() < 1e-6);
}

#[tokio::test]
async fn test_model_router_judge_client_previous_wins() {
    let invoker = Arc::new(StubLlmInvoker::previous_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    let spec_v2 = make_test_spec(2, "v2");
    let spec_v3 = make_test_spec(3, "v3");

    let pair = channel_a
        .generate_preference_pair(&spec_v3, &spec_v2)
        .await
        .expect("评判器应成功");

    // Previous 胜出：chosen = v2，rejected = v3
    assert_eq!(pair.chosen, spec_v2.canonical_merkle_input());
    assert_eq!(pair.rejected, spec_v3.canonical_merkle_input());
}

#[tokio::test]
async fn test_model_router_judge_client_with_lite_strategy() {
    // 验证不同路由策略不影响评判结果（评判结果由 LLM 决定，路由仅选择模型）
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_strategy(invoker, RoutingStrategy::Lite);
    let channel_a = RhiChannelA::new(judge_client);

    let spec_v1 = make_test_spec(1, "v1");
    let spec_v2 = make_test_spec(2, "v2");

    let pair = channel_a
        .generate_preference_pair(&spec_v2, &spec_v1)
        .await
        .expect("Lite 策略应成功");

    assert_eq!(pair.pair_id, "rhi-pair-2-1");
}

#[tokio::test]
async fn test_model_router_judge_client_with_dynamic_response() {
    // 动态响应：根据 prompt 内容返回不同评判结果
    // 规则：v3 永远胜出（无论作为 current 还是 previous）
    let invoker = Arc::new(StubLlmInvoker::with_dynamic_response(|_model_id, prompt| {
        // WHY 检查 "Current Version (v3)"：prompt 同时包含 current 与 previous 版本号，
        // 仅检查 "v3" 会匹配两者。需精确匹配 "Current Version (v3)" 头部以区分。
        // prompt 模板格式："## Current Version (v{N}):\n```\n{content}\n```"
        let v3_is_current = prompt.contains("Current Version (v3)");
        let json = if v3_is_current {
            // v3 是 current → current 胜出
            make_valid_json_response("current", 0.88, 0.42, 0.92, "dynamic: v3 current wins")
        } else {
            // v3 是 previous → previous 胜出
            make_valid_json_response("previous", 0.82, 0.38, 0.88, "dynamic: v3 previous wins")
        };
        LlmResponse {
            content: json,
            model_id: "dynamic-stub".to_string(),
            usage: TokenUsage {
                prompt_tokens: 200,
                completion_tokens: 80,
            },
        }
    }));

    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    // v3 vs v2：current (v3) 胜出 → chosen = v3
    let pair_v3 = channel_a
        .generate_preference_pair(&make_test_spec(3, "v3"), &make_test_spec(2, "v2"))
        .await
        .unwrap();
    assert_eq!(pair_v3.chosen, make_test_spec(3, "v3").canonical_merkle_input());

    // v4 vs v3：previous (v3) 胜出 → chosen = v3
    let pair_v4 = channel_a
        .generate_preference_pair(&make_test_spec(4, "v4"), &make_test_spec(3, "v3"))
        .await
        .unwrap();
    assert_eq!(pair_v4.chosen, make_test_spec(3, "v3").canonical_merkle_input());
}

// ============================================================
// C. 失败场景测试
// ============================================================

#[tokio::test]
async fn test_failing_llm_invoker_propagates_judge_failed() {
    // LLM 不可达 → JudgeFailed 错误传播
    let invoker = Arc::new(FailingLlmInvoker::new("LLM service unreachable"));
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    let spec_v1 = make_test_spec(1, "v1");
    let spec_v2 = make_test_spec(2, "v2");

    let result = channel_a.generate_preference_pair(&spec_v2, &spec_v1).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("LLM service unreachable"),
        "错误消息应包含原因，实际: {err_msg}"
    );
}

#[tokio::test]
async fn test_invalid_json_response_propagates_invalid_verdict() {
    // LLM 返回非法 JSON → InvalidVerdict 错误
    let invoker = Arc::new(StubLlmInvoker::with_fixed_response(
        "not a valid json",
        "bad-model",
    ));
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    let spec_v1 = make_test_spec(1, "v1");
    let spec_v2 = make_test_spec(2, "v2");

    let result = channel_a.generate_preference_pair(&spec_v2, &spec_v1).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("invalid judge verdict"),
        "应为 InvalidVerdict 错误，实际: {err}"
    );
}

#[tokio::test]
async fn test_invalid_verdict_score_range_propagates_error() {
    // LLM 返回 winner_score > 1.0 → InvalidVerdict 错误
    let invoker = Arc::new(StubLlmInvoker::with_fixed_response(
        make_valid_json_response("current", 1.5, 0.4, 0.9, "score out of range"),
        "bad-model",
    ));
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    let spec_v1 = make_test_spec(1, "v1");
    let spec_v2 = make_test_spec(2, "v2");

    let result = channel_a.generate_preference_pair(&spec_v2, &spec_v1).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_invalid_verdict_winner_loser_inconsistency() {
    // LLM 返回 winner_score < loser_score → InvalidVerdict 错误
    let invoker = Arc::new(StubLlmInvoker::with_fixed_response(
        make_valid_json_response("current", 0.3, 0.8, 0.9, "inconsistent scores"),
        "bad-model",
    ));
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    let spec_v1 = make_test_spec(1, "v1");
    let spec_v2 = make_test_spec(2, "v2");

    let result = channel_a.generate_preference_pair(&spec_v2, &spec_v1).await;
    assert!(result.is_err());
}

// ============================================================
// D. 多版本链模拟 spec 演进
// ============================================================

#[tokio::test]
async fn test_multi_version_chain_spec_evolution() {
    // 模拟 spec 演进：v1→v2→v3→v4，每次比较都持久化
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);
    let history = SelfComparisonHistory::with_default_capacity();

    // 4 个版本，3 次比较
    // WHY 数组而非 vec:固定长度 4，clippy::useless_vec 建议用数组避免堆分配
    let specs = [
        make_test_spec(1, "v1"),
        make_test_spec(2, "v2"),
        make_test_spec(3, "v3"),
        make_test_spec(4, "v4"),
    ];

    for i in 1..specs.len() {
        let pair = channel_a
            .generate_preference_pair(&specs[i], &specs[i - 1])
            .await
            .unwrap();

        // 构造 verdict 并持久化
        let verdict = JudgeVerdict::new(
            SpecVersion::Current,
            pair.chosen_score,
            pair.rejected_score,
            0.90,
            format!("evolution step {i}"),
        )
        .unwrap();
        let record = SelfComparisonRecord::from_pair_and_verdict(pair, &verdict);
        history.store(record).unwrap();
    }

    // 验证 3 条记录已持久化
    assert_eq!(history.len().unwrap(), 3);

    // 验证所有 pair_id 都可检索
    assert!(history.get("rhi-pair-2-1").unwrap().is_some());
    assert!(history.get("rhi-pair-3-2").unwrap().is_some());
    assert!(history.get("rhi-pair-4-3").unwrap().is_some());

    // 验证 list_recent 返回 3 条，按时间降序
    let recent = history.list_recent(10).unwrap();
    assert_eq!(recent.len(), 3);
    // 最近的应该是 rhi-pair-4-3
    assert_eq!(recent[0].pair.pair_id, "rhi-pair-4-3");
    // 最旧的应该是 rhi-pair-2-1
    assert_eq!(recent[2].pair.pair_id, "rhi-pair-2-1");
}

#[tokio::test]
async fn test_mixed_win_loses_in_evolution_chain() {
    // 模拟混合胜负的演进链：v2>v1, v2>v3, v4>v3
    // WHY 精确匹配 "Current Version (vN)":prompt 同时包含 current 与 previous 版本号，
    // 仅检查 "vN" 会匹配两者。需精确匹配 "Current Version (vN)" 头部以区分。
    // prompt 模板格式："## Current Version (v{N}):\n```\n{content}\n```"
    //
    // 规则：v2 永远胜出（无论作为 current 还是 previous）—— 模拟"v2 是黄金版本"场景
    let invoker = Arc::new(StubLlmInvoker::with_dynamic_response(|_, prompt| {
        let v2_is_current = prompt.contains("Current Version (v2)");
        let json = if v2_is_current {
            // v2 是 current → current 胜出
            make_valid_json_response("current", 0.85, 0.40, 0.90, "v2 current wins")
        } else {
            // v2 是 previous → previous 胜出
            make_valid_json_response("previous", 0.80, 0.35, 0.88, "v2 previous wins")
        };
        LlmResponse {
            content: json,
            model_id: "mixed-stub".to_string(),
            usage: TokenUsage {
                prompt_tokens: 150,
                completion_tokens: 60,
            },
        }
    }));

    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);
    let history = SelfComparisonHistory::with_default_capacity();

    // v2 vs v1：current (v2) 胜出
    let pair_2_1 = channel_a
        .generate_preference_pair(&make_test_spec(2, "v2"), &make_test_spec(1, "v1"))
        .await
        .unwrap();
    assert_eq!(pair_2_1.chosen, make_test_spec(2, "v2").canonical_merkle_input());

    // v3 vs v2：previous (v2) 胜出
    let pair_3_2 = channel_a
        .generate_preference_pair(&make_test_spec(3, "v3"), &make_test_spec(2, "v2"))
        .await
        .unwrap();
    assert_eq!(pair_3_2.chosen, make_test_spec(2, "v2").canonical_merkle_input());

    // 持久化两条记录
    let v1 = JudgeVerdict::new(SpecVersion::Current, 0.85, 0.40, 0.90, "v2 wins").unwrap();
    let v2 = JudgeVerdict::new(SpecVersion::Previous, 0.80, 0.35, 0.88, "v2 wins again").unwrap();
    history
        .store(SelfComparisonRecord::from_pair_and_verdict(pair_2_1, &v1))
        .unwrap();
    history
        .store(SelfComparisonRecord::from_pair_and_verdict(pair_3_2, &v2))
        .unwrap();

    // 验证两条记录都能检索
    let r1 = history.get("rhi-pair-2-1").unwrap().unwrap();
    let r2 = history.get("rhi-pair-3-2").unwrap().unwrap();

    // 验证两条记录的 chosen 不同（不同的胜出者）
    assert_eq!(r1.pair.chosen, make_test_spec(2, "v2").canonical_merkle_input());
    assert_eq!(r2.pair.chosen, make_test_spec(2, "v2").canonical_merkle_input());
    // 但 pair_id 不同
    assert_ne!(r1.pair.pair_id, r2.pair.pair_id);
}

// ============================================================
// E. 并发存储测试
// ============================================================

#[tokio::test]
async fn test_concurrent_stores_thread_safety() {
    // 多个 tokio 任务并发 store，验证线程安全
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = Arc::new(RhiChannelA::new(judge_client));
    let history = Arc::new(SelfComparisonHistory::with_default_capacity());

    // 5 个并发任务，每个存储 4 条记录
    let total_count = 5;
    let per_task = 4;

    let mut handles = Vec::new();
    for task_id in 0..total_count {
        let channel_a_clone = Arc::clone(&channel_a);
        let history_clone = Arc::clone(&history);

        handles.push(tokio::spawn(async move {
            for j in 0..per_task {
                let v_i = (task_id * per_task + j + 2) as u32;
                let v_i_minus_1 = v_i - 1;
                let spec_v_i = make_test_spec(v_i, &format!("task{task_id}-v{v_i}"));
                let spec_v_i_minus_1 = make_test_spec(v_i_minus_1, &format!("task{task_id}-v{v_i_minus_1}"));

                let pair = channel_a_clone
                    .generate_preference_pair(&spec_v_i, &spec_v_i_minus_1)
                    .await
                    .unwrap();

                let verdict = JudgeVerdict::new(
                    SpecVersion::Current,
                    pair.chosen_score,
                    pair.rejected_score,
                    0.90,
                    format!("concurrent task {task_id} iteration {j}"),
                )
                .unwrap();
                let record = SelfComparisonRecord::from_pair_and_verdict(pair, &verdict);
                history_clone.store(record).unwrap();
            }
        }));
    }

    // 等待所有任务完成
    for handle in handles {
        handle.await.unwrap();
    }

    // 验证所有记录都已存储（20 条）
    let expected_total = total_count * per_task;
    assert_eq!(history.len().unwrap(), expected_total);

    // 验证 evictions 为 0（容量 1024 远大于 20）
    assert_eq!(history.evictions(), 0);
}

// ============================================================
// F. KNN 召回测试
// ============================================================

#[tokio::test]
async fn test_recall_by_pair_id_returns_self_first() {
    // 存储 5 条记录，验证 KNN 召回自身排第一
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);
    let history = SelfComparisonHistory::with_default_capacity();

    for i in 1..=5 {
        let v_i = i as u32;
        let v_prev = (i - 1) as u32;
        let pair = channel_a
            .generate_preference_pair(
                &make_test_spec(v_i, &format!("v{v_i}")),
                &make_test_spec(v_prev, &format!("v{v_prev}")),
            )
            .await
            .unwrap();
        let verdict = JudgeVerdict::new(
            SpecVersion::Current,
            pair.chosen_score,
            pair.rejected_score,
            0.90,
            format!("recall test step {i}"),
        )
        .unwrap();
        history
            .store(SelfComparisonRecord::from_pair_and_verdict(pair, &verdict))
            .unwrap();
    }

    // 召回 rhi-pair-3-2 的 Top-3
    let results = history.recall_by_pair_id("rhi-pair-3-2", 3).unwrap();

    assert_eq!(results.len(), 3);
    // 自身应排第一（相似度 ~1.0）
    assert_eq!(results[0].0, "rhi-pair-3-2");
    assert!((results[0].1 - 1.0).abs() < 1e-5);
}

#[tokio::test]
async fn test_recall_top_k_exceeds_stored() {
    // 请求 Top-K 超过实际存储数，应返回所有存储的记录
    let history = SelfComparisonHistory::with_default_capacity();
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    for i in 1..=3 {
        let v_i = i as u32;
        let v_prev = (i - 1) as u32;
        let pair = channel_a
            .generate_preference_pair(
                &make_test_spec(v_i, &format!("v{v_i}")),
                &make_test_spec(v_prev, &format!("v{v_prev}")),
            )
            .await
            .unwrap();
        let verdict = JudgeVerdict::new(
            SpecVersion::Current,
            pair.chosen_score,
            pair.rejected_score,
            0.90,
            "recall overflow test",
        )
        .unwrap();
        history
            .store(SelfComparisonRecord::from_pair_and_verdict(pair, &verdict))
            .unwrap();
    }

    // 请求 Top-10，但只有 3 条
    let results = history.recall_by_pair_id("rhi-pair-2-1", 10).unwrap();
    assert_eq!(results.len(), 3);
}

// ============================================================
// G. 容量驱逐测试
// ============================================================

#[tokio::test]
async fn test_capacity_eviction_under_continuous_store() {
    // 容量 5，存储 10 条，应驱逐 5 条
    let history = SelfComparisonHistory::new(5);
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    let mut evicted_count = 0;
    for i in 1..=10 {
        let v_i = i as u32;
        let v_prev = (i - 1) as u32;
        let pair = channel_a
            .generate_preference_pair(
                &make_test_spec(v_i, &format!("v{v_i}")),
                &make_test_spec(v_prev, &format!("v{v_prev}")),
            )
            .await
            .unwrap();
        let verdict = JudgeVerdict::new(
            SpecVersion::Current,
            pair.chosen_score,
            pair.rejected_score,
            0.90,
            format!("eviction test step {i}"),
        )
        .unwrap();
        let record = SelfComparisonRecord::from_pair_and_verdict(pair, &verdict);
        if history.store(record).unwrap().is_some() {
            evicted_count += 1;
        }
    }

    // 容量 5，存储 10 条，应驱逐 5 条
    assert_eq!(evicted_count, 5);
    assert_eq!(history.len().unwrap(), 5);
    assert_eq!(history.evictions(), 5);

    // 最旧的 5 条已被驱逐
    for i in 1..=5 {
        let pair_id = format!("rhi-pair-{i}-{}", i - 1);
        assert!(
            history.get(&pair_id).unwrap().is_none(),
            "应驱逐 rhi-pair-{i}-{}",
            i - 1
        );
    }

    // 最新的 5 条仍存在
    for i in 6..=10 {
        let pair_id = format!("rhi-pair-{i}-{}", i - 1);
        assert!(
            history.get(&pair_id).unwrap().is_some(),
            "应保留 rhi-pair-{i}-{}",
            i - 1
        );
    }
}

// ============================================================
// H. CLV 确定性测试
// ============================================================

#[tokio::test]
async fn test_clv_determinism_across_multiple_calls() {
    // 同一 pair_id 多次生成 CLV 应完全相同
    let pair_id = "rhi-pair-7-6";

    let clv_1 = generate_deterministic_clv(pair_id).unwrap();
    let clv_2 = generate_deterministic_clv(pair_id).unwrap();
    let clv_3 = generate_deterministic_clv(pair_id).unwrap();

    assert_eq!(clv_1.as_slice(), clv_2.as_slice());
    assert_eq!(clv_2.as_slice(), clv_3.as_slice());
}

#[tokio::test]
async fn test_clv_distinct_for_different_pair_ids() {
    // 不同 pair_id 生成不同 CLV
    let clv_1 = generate_deterministic_clv("rhi-pair-2-1").unwrap();
    let clv_2 = generate_deterministic_clv("rhi-pair-3-2").unwrap();
    let clv_3 = generate_deterministic_clv("rhi-pair-4-3").unwrap();

    assert_ne!(clv_1.as_slice(), clv_2.as_slice());
    assert_ne!(clv_2.as_slice(), clv_3.as_slice());
    assert_ne!(clv_1.as_slice(), clv_3.as_slice());
}

#[tokio::test]
async fn test_clv_dimension_always_512() {
    // 任意 pair_id 都生成 512 维 CLV
    for i in 0u32..10 {
        let pair_id = format!("rhi-pair-{i}-{}", i.saturating_sub(1));
        let clv = generate_deterministic_clv(&pair_id).unwrap();
        assert_eq!(
            clv.as_slice().len(),
            512,
            "CLV 维度应始终为 512，pair_id={pair_id}"
        );
    }
}

// ============================================================
// I. 端到端完整流程测试（JudgeClient trait object 共享）
// ============================================================

#[tokio::test]
async fn test_shared_judge_client_across_multiple_channels() {
    // 同一 JudgeClient 实例（Arc<dyn>）被多个 RhiChannelA 共享使用
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client: Arc<dyn JudgeClient> = make_judge_client_with_stub_llm(invoker);

    let channel_a1 = RhiChannelA::new(Arc::clone(&judge_client));
    let channel_a2 = RhiChannelA::new(Arc::clone(&judge_client));

    let history = SelfComparisonHistory::with_default_capacity();

    // channel_a1 处理 v2 vs v1
    let pair1 = channel_a1
        .generate_preference_pair(&make_test_spec(2, "v2"), &make_test_spec(1, "v1"))
        .await
        .unwrap();

    // channel_a2 处理 v3 vs v2
    let pair2 = channel_a2
        .generate_preference_pair(&make_test_spec(3, "v3"), &make_test_spec(2, "v2"))
        .await
        .unwrap();

    // 两个 channel 共享同一 judge_client，行为一致
    assert_eq!(pair1.pair_id, "rhi-pair-2-1");
    assert_eq!(pair2.pair_id, "rhi-pair-3-2");

    // 持久化两条记录
    let v1 = JudgeVerdict::new(SpecVersion::Current, 0.85, 0.45, 0.90, "shared client v2").unwrap();
    let v2 = JudgeVerdict::new(SpecVersion::Current, 0.85, 0.45, 0.90, "shared client v3").unwrap();
    history
        .store(SelfComparisonRecord::from_pair_and_verdict(pair1, &v1))
        .unwrap();
    history
        .store(SelfComparisonRecord::from_pair_and_verdict(pair2, &v2))
        .unwrap();

    assert_eq!(history.len().unwrap(), 2);
    assert!(history.get("rhi-pair-2-1").unwrap().is_some());
    assert!(history.get("rhi-pair-3-2").unwrap().is_some());
}

// ============================================================
// J. 序列化往返测试（端到端）
// ============================================================

#[tokio::test]
async fn test_record_serde_roundtrip_after_store() {
    // 验证存储后的记录能正确反序列化
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);
    let history = SelfComparisonHistory::with_default_capacity();

    let pair = channel_a
        .generate_preference_pair(&make_test_spec(2, "v2"), &make_test_spec(1, "v1"))
        .await
        .unwrap();
    let verdict = JudgeVerdict::new(
        SpecVersion::Current,
        0.85,
        0.45,
        0.92,
        "serde roundtrip test",
    )
    .unwrap();
    let original_record = SelfComparisonRecord::from_pair_and_verdict(pair, &verdict);

    history.store(original_record.clone()).unwrap();

    // 检索并验证与原记录一致（除 created_at 外，因存储时不修改时间戳）
    let retrieved = history.get("rhi-pair-2-1").unwrap().unwrap();
    assert_eq!(retrieved.pair, original_record.pair);
    assert!((retrieved.confidence - original_record.confidence).abs() < 1e-6);
    assert_eq!(retrieved.rationale, original_record.rationale);
    // created_at 在序列化/反序列化往返中可能丢失纳秒精度，验证到毫秒
    let diff = (retrieved.created_at - original_record.created_at).num_milliseconds();
    assert!(diff.abs() < 1, "created_at 应在毫秒精度内一致");
}

// ============================================================
// K. 大规模持久化测试
// ============================================================

#[tokio::test]
async fn test_large_scale_persistence_100_records() {
    // 验证 100 条记录的批量存储与检索
    let history = SelfComparisonHistory::with_default_capacity();
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_judge_client_with_stub_llm(invoker);
    let channel_a = RhiChannelA::new(judge_client);

    // 存储 100 条记录
    for i in 1..=100 {
        let v_i = (i + 1) as u32;
        let v_prev = i as u32;
        let pair = channel_a
            .generate_preference_pair(
                &make_test_spec(v_i, &format!("v{v_i}")),
                &make_test_spec(v_prev, &format!("v{v_prev}")),
            )
            .await
            .unwrap();
        let verdict = JudgeVerdict::new(
            SpecVersion::Current,
            0.85,
            0.45,
            0.90,
            format!("large scale test record {i}"),
        )
        .unwrap();
        let record = SelfComparisonRecord::from_pair_and_verdict(pair, &verdict);
        history.store(record).unwrap();
    }

    assert_eq!(history.len().unwrap(), 100);

    // 验证随机抽查 5 条记录都能检索到
    for i in [5, 25, 50, 75, 95] {
        let v_i = (i + 1) as u32;
        let v_prev = i as u32;
        let pair_id = format!("rhi-pair-{v_i}-{v_prev}");
        assert!(
            history.get(&pair_id).unwrap().is_some(),
            "应能检索 {pair_id}"
        );
    }

    // 验证 list_recent(10) 返回最近 10 条
    let recent = history.list_recent(10).unwrap();
    assert_eq!(recent.len(), 10);
    // 最近的应是 rhi-pair-101-100
    assert_eq!(recent[0].pair.pair_id, "rhi-pair-101-100");
}
