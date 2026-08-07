//! Token 效率优化集成测试 — 五项优化协同验证（ADR-069）
//!
//! 覆盖 6 个集成测试场景:
//! 1. 缓存亲和 + 语义缓存协同（CacheHitTracker + SemanticResponseCache）
//! 2. 上下文裁剪 + 输出预算协同（trim_to_budget + EarlyStopController）
//! 3. 成本熔断全链路（CostGuard check + BudgetExceeded 事件）
//! 4. S9 回滚（Cooldown 态 bypass 缓存 → Fail-open）
//! 5. 前缀稳定性校验（validate_prefix_stability 时间戳/UUID 检测）
//! 6. 命名空间隔离（跨 namespace 隐私隔离红线）
//!
//! 运行: cargo test -p mca-gateway token_efficiency_integration

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;

use event_bus::{EventBus, NexusEvent};
use mca_gateway::cost_guard::{CostGuard, BUDGET_TYPE};
use mca_gateway::early_stop::EarlyStopController;
use mca_gateway::prompt_norm::{validate_prefix_stability, PrefixInstability};
use mca_gateway::sse::StreamEvent;
use mca_gateway::{build_token_cache_key, semantic_fingerprint, AdapterOptions, VendorAdapter};
use nexus_contracts::affinity::{
    AffinityMessage, AffinityOverrides, AffinityRequest, ContentBlock, MessageRole,
    ModelAffinitySpec, OutputFormat, ProtocolDialect, ProviderId, SamplingParams,
    ThinkingPreference,
};
use nexus_contracts::{CapabilityToken, CapabilityTokenStatus, SeamId};
use scc_cache::{CacheHitTracker, SemanticResponseCache};
use sha2::{Digest, Sha256};
use std::time::Duration;

// ============================================================
// 共享 mock 基础设施
// ============================================================

/// 可捕获请求体的 mock 端点
async fn spawn_chat_mock_with_body<F, B>(handler: F) -> String
where
    F: Fn(axum::body::Bytes) -> B + Send + Sync + Clone + 'static,
    B: axum::response::IntoResponse + Send + 'static,
{
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move |body: axum::body::Bytes| {
            let h = handler.clone();
            async move { h(body) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// OpenAI Chat 方言响应
fn chat_response(cache_hit: u64) -> axum::response::Json<serde_json::Value> {
    axum::response::Json(serde_json::json!({
        "id": "chatcmpl-mock-001",
        "object": "chat.completion",
        "model": "deepseek-v4-flash",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 24,
            "completion_tokens": 10,
            "total_tokens": 34,
            "prompt_cache_hit_tokens": cache_hit
        }
    }))
}

/// DeepSeek mock 通道 spec
fn mock_spec(base_url: &str) -> ModelAffinitySpec {
    let mut spec = ModelAffinitySpec::minimal(
        ProviderId::DeepSeek,
        "deepseek-v4-flash",
        ProtocolDialect::OpenAiChat,
    );
    spec.endpoint.base_url = base_url.into();
    spec.endpoint.timeout_ms = 5_000;
    spec.endpoint.connect_timeout_ms = 1_000;
    spec
}

fn mock_request() -> AffinityRequest {
    AffinityRequest {
        intent_id: "intent-ti".into(),
        messages: vec![AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        tools: Vec::new(),
        thinking_pref: ThinkingPreference::Fast,
        budget_hint_micro: None,
        overrides: AffinityOverrides::default(),
        sampling: SamplingParams::default(),
        output_format: OutputFormat::default(),
    }
}

/// 与 adapters.rs context_hash 同算法
fn context_hash(messages: &[AffinityMessage]) -> [u8; 32] {
    let json = serde_json::to_string(messages).unwrap_or_else(|_| "[]".into());
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    hasher.finalize().into()
}

/// 提取响应中的全部 Text 块文本
fn text_of(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

// ============================================================
// 测试 1: 缓存亲和 + 语义缓存协同
// ============================================================

/// 装配 VendorAdapter 同时挂接 cache_tracker + semantic_cache,
/// 插入语义缓存条目后 invoke() 应命中语义缓存（免厂商调用）,
/// miss 时走厂商调用，解码后回填语义缓存 + 累计 CacheHitTracker。
#[tokio::test]
async fn cache_affinity_semantic_cache_coordination() {
    // --- miss 场景: 首次调用走厂商 ---
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
        c.fetch_add(1, AtomicOrdering::SeqCst);
        chat_response(0)
    })
    .await;

    let cache = Arc::new(SemanticResponseCache::default());
    let tracker = Arc::new(CacheHitTracker::new());
    let spec = Arc::new(mock_spec(&base));
    let req = mock_request();

    let adapter = VendorAdapter::assemble_with_options(
        Arc::clone(&spec),
        None,
        AdapterOptions {
            cache_tracker: Some(tracker.clone()),
            semantic_cache: Some(cache.clone()),
            ..AdapterOptions::default()
        },
    )
    .unwrap();

    // miss: 走厂商（计数 1）
    let resp1 = adapter.invoke(&req).await.unwrap();
    assert_eq!(
        calls.load(AtomicOrdering::SeqCst),
        1,
        "首次 miss 必须走厂商"
    );
    assert_eq!(text_of(&resp1.blocks), "ok");
    // miss 后回填语义缓存
    assert_eq!(
        cache.namespace_len(&req.intent_id),
        1,
        "miss 后必须回填语义缓存"
    );
    // CacheHitTracker 记录厂商调用(零命中)
    assert_eq!(tracker.tracked_providers(), 1);

    // --- hit 场景: 第二次调用命中语义缓存，免厂商调用 ---
    let resp2 = adapter.invoke(&req).await.unwrap();
    assert_eq!(
        calls.load(AtomicOrdering::SeqCst),
        1,
        "命中语义缓存后不得再发厂商请求"
    );
    assert_eq!(text_of(&resp2.blocks), "ok", "命中响应必须与缓存内容一致");
    assert_eq!(resp2.cost.total_micro, 0, "命中响应零成本（未发厂商调用）");
    assert_eq!(resp2.usage.input_tokens, 0, "命中响应零计量");
}

// ============================================================
// 测试 2: 上下文裁剪 + 输出预算协同
// ============================================================

/// 构造超长会话触发 trim_to_budget（输入面），
/// 验证 EarlyStopController 在流式解码中生效（输出面），
/// 两者不重叠 —— trim 管输入 token，early_stop 管输出 token。
#[tokio::test]
async fn context_trim_and_output_budget_synergy() {
    // --- 输入面: 超长会话触发裁剪 ---
    let sent_msg_count = Arc::new(AtomicUsize::new(0));
    let c = sent_msg_count.clone();
    let base = spawn_chat_mock_with_body(move |body: axum::body::Bytes| {
        let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&body))
            .unwrap_or_else(|_| serde_json::json!({"messages": []}));
        c.store(
            parsed["messages"].as_array().map(|a| a.len()).unwrap_or(0),
            AtomicOrdering::SeqCst,
        );
        chat_response(0)
    })
    .await;

    let adapter = VendorAdapter::assemble(Arc::new(mock_spec(&base)), None).unwrap();
    let mut req = mock_request();
    // 10 条长消息 → 约 1250 tokens → 超预算(Simple 档 4096×0.25×0.6≈614)
    req.messages = (0..10)
        .map(|i| AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: format!("history message {i} {}", "x".repeat(480)).into(),
            }],
        })
        .collect();
    adapter.invoke(&req).await.unwrap();

    let sent = sent_msg_count.load(AtomicOrdering::SeqCst);
    assert!(
        sent < 10,
        "超预算会话必须裁剪后发送, sent={sent}（输入面: trim_to_budget）"
    );

    // --- 输出面: EarlyStopController 独立验证 ---
    // 输出预算 100 tokens → 字符/4 估算 ≈ 400 字节
    let mut controller = EarlyStopController::new(100);
    // 模拟流式消费: 每次 delta 50 字节 ≈ 12 tokens
    let decisions: Vec<_> = (0..12)
        .map(|_| controller.on_event(&StreamEvent::TextDelta("a".repeat(50))))
        .collect();
    // 前 8 次 delta 累计 8×12=96 tokens < 100 → Continue
    let continues = decisions
        .iter()
        .filter(|d| matches!(d, mca_gateway::early_stop::StopDecision::Continue))
        .count();
    assert!(continues >= 8, "前 8 次 delta 必须在预算内继续");
    // 至少有一次 BudgetExceeded 停止
    let stops = decisions
        .iter()
        .filter(|d| {
            matches!(
                d,
                mca_gateway::early_stop::StopDecision::Stop {
                    reason: mca_gateway::early_stop::StopReason::BudgetExceeded,
                    ..
                }
            )
        })
        .count();
    assert!(
        stops >= 1,
        "超预算时 early_stop 必须触发 BudgetExceeded 停止（输出面）"
    );
}

// ============================================================
// 测试 2.5: 完成度感知早停(ADR-072 决策 ⑥)—— 流中断 + 输出 token 下降
// ============================================================

/// 流中断接线验证:结构化输出(Json)语义完成即停,阻止模型继续生成
/// 尾随文本(流中断 = 消费者收到 Stop 决策后停止消费,未消费 token
/// 不再产生)。对比全量消费基线,输出 token 下降必须 ≥ 10%
/// (SMART 输出治理目标,Phase 4 验收基准)。
#[test]
fn completion_detector_stream_interrupt_reduces_output_tokens() {
    use mca_gateway::early_stop::{StopDecision, StopReason};
    use mca_gateway::estimate_text;
    use nexus_contracts::affinity::OutputFormat;

    // 流:完整 JSON(2 段) + 尾随冗长文本(模型多输出的部分,全量消费会
    // 计入成本;语义完成即停后这部分不再产生)
    let chunks = [
        r#"{"result":"success","#,
        r#""metrics":[1,2,3]}"#,
        "\n\n总结:执行成功,所有指标已计算完毕,无任何异常。",
        "全部步骤完成,报告生成结束,谢谢配合。",
    ];
    // 全量消费基线(对照组):所有 chunk 均被消费
    let full: u64 = chunks.iter().map(|c| u64::from(estimate_text(c))).sum();
    assert!(full > 0);

    // 实验组:启用完成度检测(Json),流中断语义
    let mut controller = EarlyStopController::with_completion(100_000, OutputFormat::Json);
    let mut consumed_total: u64 = 0;
    let mut stopped = false;
    for chunk in chunks {
        let decision = controller.on_event(&StreamEvent::TextDelta(chunk.into()));
        match decision {
            StopDecision::Continue => {
                consumed_total += u64::from(estimate_text(chunk));
            }
            StopDecision::Stop { reason, consumed } => {
                // 触发 chunk 本身已生成(计费),后续 token 不再消费
                assert_eq!(
                    reason,
                    StopReason::SemanticComplete,
                    "完整 JSON 必须触发 SemanticComplete"
                );
                consumed_total = consumed;
                stopped = true;
                break; // 流中断:停止消费后续 chunk
            }
        }
    }
    assert!(stopped, "完整 JSON 后必须语义完成停止(流中断)");

    let reduction = (full - consumed_total) as f64 / full as f64;
    assert!(
        reduction >= 0.10,
        "语义完成即停必须省 ≥10% 输出 token, got {reduction:.2} (full={full}, consumed={consumed_total})"
    );
}

/// 误判保护:未闭合 JSON 不触发流中断(正确性优先,截断即事故)
#[test]
fn completion_detector_no_interrupt_on_unclosed_json() {
    use mca_gateway::early_stop::StopDecision;
    use nexus_contracts::affinity::OutputFormat;

    let mut controller = EarlyStopController::with_completion(100_000, OutputFormat::Json);
    // 流在未闭合 JSON 处结束(厂商流中断/截断输出场景)
    let chunks = [r#"{"result":"pending","#, r#""partial":"#];
    for chunk in chunks {
        let decision = controller.on_event(&StreamEvent::TextDelta(chunk.into()));
        assert!(
            matches!(decision, StopDecision::Continue),
            "未闭合 JSON 不得触发任何停止: {decision:?}"
        );
    }
    assert!(!controller.should_stop());
}

// ============================================================
// 测试 3: 成本熔断全链路
// ============================================================

/// 创建 CostGuard 并设置 budget_limit_micro，
/// 累计成本超过上限后 check() 返回 Err(CircuitOpen)，
/// 验证 BudgetExceeded(Critical) 事件发布，
/// 验证后续请求在熔断期内被拒绝。
#[tokio::test]
async fn cost_guard_full_circuit_lifecycle() {
    // --- 阶段 1: 未超限放行 ---
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
        c.fetch_add(1, AtomicOrdering::SeqCst);
        chat_response(0)
    })
    .await;

    let guard = Arc::new(CostGuard::new(Some(1_000_000))); // 1M 微元上限
    let adapter = VendorAdapter::assemble_with_options(
        Arc::new(mock_spec(&base)),
        None,
        AdapterOptions {
            cost_guard: Some(guard.clone()),
            ..AdapterOptions::default()
        },
    )
    .unwrap();

    // invoke #1: 未超限 → 放行
    adapter.invoke(&mock_request()).await.unwrap();
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "未超限必须放行");

    // --- 阶段 2: 跨线熔断 + BudgetExceeded 事件 ---
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let guard2 = Arc::new(CostGuard::with_bus(Some(1_000_000), Some(bus)));
    let adapter2 = VendorAdapter::assemble_with_options(
        Arc::new(mock_spec(&base)),
        None,
        AdapterOptions {
            cost_guard: Some(guard2.clone()),
            ..AdapterOptions::default()
        },
    )
    .unwrap();

    // 模拟累计成本跨线
    guard2.record(1_000_000);

    // invoke: 跨线 → 熔断拒绝
    let err = adapter2.invoke(&mock_request()).await.unwrap_err();
    assert!(
        matches!(err, mca_gateway::AffinityError::Quota { .. }),
        "熔断拒绝必须映射为 Quota 错误"
    );

    // 验证 BudgetExceeded 事件
    let mut seen = false;
    while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)).await {
        if let NexusEvent::BudgetExceeded {
            budget_type,
            current,
            limit,
            ..
        } = ev
        {
            seen = true;
            assert_eq!(budget_type, BUDGET_TYPE);
            assert_eq!(current, guard2.spent_micro());
            assert_eq!(limit, 1_000_000);
        }
    }
    assert!(seen, "跨线必须发布 BudgetExceeded(Critical) 事件");

    // --- 阶段 3: 熔断期内后续请求被拒绝 ---
    assert!(
        adapter2.invoke(&mock_request()).await.is_err(),
        "熔断期内后续请求必须被拒绝"
    );

    // --- 阶段 4: 零上限立即熔断（传输前拒绝，厂商零调用）---
    let calls3 = Arc::new(AtomicUsize::new(0));
    let c3 = calls3.clone();
    let base3 = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
        c3.fetch_add(1, AtomicOrdering::SeqCst);
        chat_response(0)
    })
    .await;
    let adapter3 = VendorAdapter::assemble_with_options(
        Arc::new(mock_spec(&base3)),
        None,
        AdapterOptions {
            cost_guard: Some(Arc::new(CostGuard::new(Some(0)))),
            ..AdapterOptions::default()
        },
    )
    .unwrap();

    let err3 = adapter3.invoke(&mock_request()).await.unwrap_err();
    assert!(
        matches!(err3, mca_gateway::AffinityError::Quota { .. }),
        "limit=0 首次 check 即熔断"
    );
    assert_eq!(
        calls3.load(AtomicOrdering::SeqCst),
        0,
        "熔断必须发生在传输前（厂商零调用）"
    );
}

// ============================================================
// 测试 4: S9 回滚
// ============================================================

/// CapabilityToken 进入 Cooldown 态 →
/// allows_learned_policy() = false → bypass 全部缓存逻辑（Fail-open）。
/// 请求应正常完成（不阻塞），仅 token 消耗上升。
#[tokio::test]
async fn s9_cooldown_rollback_fail_open() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
        c.fetch_add(1, AtomicOrdering::SeqCst);
        chat_response(0)
    })
    .await;

    let spec = mock_spec(&base);
    let cache = Arc::new(SemanticResponseCache::default());
    let req = mock_request();

    // 预填缓存: 即使存在可命中条目，Cooldown 态也不得查询
    let key = build_token_cache_key(&Arc::new(spec.clone()), &req);
    let clv = semantic_fingerprint(&req.messages, &req.tools);
    let hash = context_hash(&req.messages);
    cache.insert(req.intent_id.as_ref(), key, clv, "prefilled", hash, 1);

    // Cooldown 态 token
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);
    // 先推升到 Authorized 再触发 ASA → Cooldown
    for _ in 0..40 {
        token.record_outcome(true);
        token.maybe_promote();
    }
    token.trigger_asa_intervention(now);
    assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);
    assert!(!token.allows_learned_policy(now + 1), "冷却期内必须未授权");

    let adapter = VendorAdapter::assemble_with_options(
        Arc::new(spec),
        None,
        AdapterOptions {
            semantic_cache: Some(cache.clone()),
            capability_token: Some(Arc::new(token)),
            ..AdapterOptions::default()
        },
    )
    .unwrap();

    let resp = adapter.invoke(&req).await.unwrap();
    // Cooldown bypass: 必须走厂商（不查缓存）
    assert_eq!(
        calls.load(AtomicOrdering::SeqCst),
        1,
        "Cooldown bypass: 必须走厂商（不查缓存，Fail-open 不阻塞）"
    );
    assert_eq!(text_of(&resp.blocks), "ok", "响应必须来自厂商而非缓存");
    // 请求正常完成（不阻塞），仅 token 消耗上升（未命中缓存）
    assert!(
        resp.usage.input_tokens > 0,
        "Cooldown 态走厂商，产生真实 token 消耗"
    );
}

// ============================================================
// 测试 5: 前缀稳定性校验
// ============================================================

/// 构造包含时间戳的 Prompt → validate_prefix_stability 返回 Err
/// 构造包含 UUID 的 Prompt → validate_prefix_stability 返回 Err
/// 干净 Prompt → 返回 Ok
#[test]
fn prefix_stability_validation() {
    // 时间戳检测
    let timestamp_content = "System prompt generated at 2026-08-02T12:30:00Z for user";
    let result = validate_prefix_stability(timestamp_content, "L2");
    assert!(result.is_err(), "时间戳必须被检测为不稳定");
    assert!(
        matches!(
            result.unwrap_err(),
            PrefixInstability::TimestampDetected { .. }
        ),
        "错误类型必须是 TimestampDetected"
    );

    // UUID 检测
    let uuid_content = "Session: 550e8400-e29b-41d4-a716-446655440000 active";
    let result = validate_prefix_stability(uuid_content, "L1");
    assert!(result.is_err(), "UUID 必须被检测为不稳定");
    assert!(
        matches!(
            result.unwrap_err(),
            PrefixInstability::RandomIdDetected { .. }
        ),
        "错误类型必须是 RandomIdDetected"
    );

    // 干净 Prompt — 通过
    let clean = "You are Chimera, a terminal-first AI coding agent.";
    assert!(
        validate_prefix_stability(clean, "L1").is_ok(),
        "干净 Prompt 必须通过稳定性校验"
    );

    // 工具声明（无时间戳/UUID）— 通过
    let tools = r#"[{"name":"read_file","description":"Read a file"}]"#;
    assert!(
        validate_prefix_stability(tools, "L2").is_ok(),
        "干净工具声明必须通过稳定性校验"
    );

    // 边界: 只有日期无时间部分 → 不触发时间戳检测（模式要求 T 分隔符）
    let date_only = "Generated on 2026-08-02";
    assert!(
        validate_prefix_stability(date_only, "L1").is_ok(),
        "仅日期无时间部分不触发时间戳检测"
    );
}

// ============================================================
// 测试 6: 命名空间隔离
// ============================================================

/// 在 namespace "quest-A" 插入语义缓存，
/// 在 namespace "quest-B" 查询同键应返回 None（隐私隔离），
/// 在 namespace "quest-A" 查询应命中。
#[test]
fn namespace_isolation_privacy_redline() {
    let cache = SemanticResponseCache::default();
    let spec = ModelAffinitySpec::minimal(
        ProviderId::DeepSeek,
        "deepseek-v4-flash",
        ProtocolDialect::OpenAiChat,
    );
    let req_a = AffinityRequest {
        intent_id: "quest-A".into(),
        messages: vec![AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "hello from A".into(),
            }],
        }],
        tools: Vec::new(),
        thinking_pref: ThinkingPreference::Fast,
        budget_hint_micro: None,
        overrides: AffinityOverrides::default(),
        sampling: SamplingParams::default(),
        output_format: OutputFormat::default(),
    };
    let req_b = AffinityRequest {
        intent_id: "quest-B".into(),
        messages: vec![AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "hello from B".into(),
            }],
        }],
        tools: Vec::new(),
        thinking_pref: ThinkingPreference::Fast,
        budget_hint_micro: None,
        overrides: AffinityOverrides::default(),
        sampling: SamplingParams::default(),
        output_format: OutputFormat::default(),
    };

    // 在 quest-A 插入语义缓存
    let key = build_token_cache_key(&spec, &req_a);
    let clv = semantic_fingerprint(&req_a.messages, &req_a.tools);
    let hash = context_hash(&req_a.messages);
    cache.insert("quest-A", key.clone(), clv.clone(), "response-A", hash, 1);

    // 同 namespace 查询 → 命中
    let hit_a = cache.lookup("quest-A", &key, &clv);
    assert!(hit_a.is_some(), "同 namespace 必须命中");
    assert_eq!(hit_a.unwrap().response.as_ref(), "response-A");

    // 跨 namespace (quest-B) 查询同键 → 不命中（隐私隔离红线）
    let miss_b = cache.lookup("quest-B", &key, &clv);
    assert!(miss_b.is_none(), "跨 namespace 禁止命中（隐私隔离红线）");

    // 验证 quest-A 缓存仍存在（未被跨 namespace 查询影响）
    assert_eq!(
        cache.namespace_len("quest-A"),
        1,
        "quest-A 缓存必须未被跨 namespace 查询影响"
    );
    assert_eq!(cache.namespace_len("quest-B"), 0, "quest-B 无缓存条目");

    // 反向验证: 在 quest-B 插入后再查 quest-A 不受影响
    let key_b = build_token_cache_key(&spec, &req_b);
    let clv_b = semantic_fingerprint(&req_b.messages, &req_b.tools);
    let hash_b = context_hash(&req_b.messages);
    cache.insert(
        "quest-B",
        key_b.clone(),
        clv_b.clone(),
        "response-B",
        hash_b,
        1,
    );

    // quest-A 仍命中
    let hit_a2 = cache.lookup("quest-A", &key, &clv);
    assert!(hit_a2.is_some(), "quest-B 插入不影响 quest-A 命中");
    assert_eq!(hit_a2.unwrap().response.as_ref(), "response-A");

    // quest-B 命中自己的条目
    let hit_b = cache.lookup("quest-B", &key_b, &clv_b);
    assert!(hit_b.is_some(), "quest-B 必须命中自己的条目");
    assert_eq!(hit_b.unwrap().response.as_ref(), "response-B");

    // 清理 quest-A 后 quest-B 不受影响
    cache.clear_namespace("quest-A");
    assert_eq!(cache.namespace_len("quest-A"), 0);
    assert_eq!(
        cache.namespace_len("quest-B"),
        1,
        "清理 quest-A 不影响 quest-B"
    );
}
