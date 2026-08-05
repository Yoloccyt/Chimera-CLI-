//! S9 Token Efficiency 四态能力场验证（ADR-069 Task 6.3）
//!
//! 验证 `CapabilityToken`（nexus-contracts L0）在 S9TokenEfficiency 接缝的
//! 四态生命周期语义,以及与 mca-gateway 语义缓存的 bypass 联动:
//!
//! # 覆盖矩阵
//! | 场景 | 断言 |
//! |------|------|
//! | 状态机不变量(proptest 随机动作序列) | Frozen 恒禁;冷却期内恒禁;Authorized 恒达标 |
//! | 渐进授权可达性(proptest) | 充足成功样本 + promote 必达 Authorized |
//! | 冷却期边界(确定性) | until-1 仍禁 / until 恢复(常量驱动,非硬编码 30) |
//! | 连续 ASA → Frozen → unfreeze | 第 3 次 ASA 冻结;Frozen 恒禁;解冻回 Provisional |
//! | 冷却结束降级 | level 低于阈值 → 恢复 Provisional |
//! | Cooldown 态 bypass 语义缓存 | 预填缓存必命中仍走厂商(不查缓存) |
//! | Frozen 态 bypass 语义缓存 | 同上 |

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;

use mca_gateway::{AdapterOptions, VendorAdapter};
use nexus_contracts::affinity::{
    AffinityMessage, AffinityOverrides, AffinityRequest, ContentBlock, MessageRole,
    ModelAffinitySpec, ProtocolDialect, ProviderId, ThinkingPreference,
};
use nexus_contracts::{CapabilityToken, CapabilityTokenStatus, SeamId};
use proptest::prelude::*;
use scc_cache::SemanticResponseCache;
use sha2::{Digest, Sha256};

// ============================================================
// S9 四态状态机(proptest)
// ============================================================

/// 状态机动作 — 模拟编排器/学习者对 token 的随机操作序列
#[derive(Debug, Clone, Copy)]
enum S9Action {
    /// 记录一次成功 outcome(EWMA 上升)
    RecordSuccess,
    /// 记录一次失败 outcome(EWMA 下降)
    RecordFailure,
    /// 尝试渐进授权提升
    Promote,
    /// 触发 AsaIntervention(→ Cooldown,连续 3 次 → Frozen)
    TriggerAsa,
    /// 冷却期检查恢复
    Recover,
    /// 时间推进(秒)
    AdvanceTime(i64),
}

/// 动作生成策略 — 权重偏向成功累积,穿插少量 ASA/时间推进
fn s9_action() -> impl Strategy<Value = S9Action> {
    prop_oneof![
        3 => Just(S9Action::RecordSuccess),
        1 => Just(S9Action::RecordFailure),
        3 => Just(S9Action::Promote),
        1 => Just(S9Action::TriggerAsa),
        1 => Just(S9Action::Recover),
        2 => (1..300i64).prop_map(S9Action::AdvanceTime),
    ]
}

proptest! {
    /// 状态机不变量(任意动作序列下守恒):
    /// I1: Frozen 态 allows_learned_policy 恒 false
    /// I2: 冷却期内 allows_learned_policy 恒 false
    /// I3: allows_learned_policy = true ⇒ level 达标 ∧ 非 Frozen ∧ 非冷却期
    /// I4: Authorized 态 level 必达标
    /// I5: level 恒 ∈ [0.0, 1.0]
    #[test]
    fn s9_state_machine_invariants_hold(
        actions in proptest::collection::vec(s9_action(), 1..120),
    ) {
        let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);
        let mut now: i64 = 1_000_000;
        for action in actions {
            now = now.saturating_add(1);
            match action {
                S9Action::RecordSuccess => token.record_outcome(true),
                S9Action::RecordFailure => token.record_outcome(false),
                S9Action::Promote => {
                    token.maybe_promote();
                }
                S9Action::TriggerAsa => {
                    token.trigger_asa_intervention(now);
                }
                S9Action::Recover => {
                    token.maybe_recover_from_cooldown(now);
                }
                S9Action::AdvanceTime(secs) => now = now.saturating_add(secs),
            }
            // I1: Frozen 恒不允许 Learned(熔断入口不可阻塞 fallback_to_static)
            if token.status() == CapabilityTokenStatus::Frozen {
                prop_assert!(
                    !token.allows_learned_policy(now),
                    "Frozen 态必须恒不允许 Learned"
                );
            }
            // I2: 冷却期内不允许 Learned
            if token.is_in_cooldown(now) {
                prop_assert!(
                    !token.allows_learned_policy(now),
                    "冷却期内必须不允许 Learned"
                );
            }
            // I3: 允许 Learned 的必要条件(状态 + level 双重检查)
            if token.allows_learned_policy(now) {
                prop_assert!(
                    token.authorized_level() >= CapabilityToken::ACTIVATION_THRESHOLD,
                    "允许 Learned 必须 level 达标, level = {}",
                    token.authorized_level()
                );
                prop_assert_ne!(
                    token.status(),
                    CapabilityTokenStatus::Frozen,
                    "Frozen 态不允许 Learned"
                );
                prop_assert!(
                    !token.is_in_cooldown(now),
                    "冷却期不允许 Learned"
                );
            }
            // I4: Authorized 态 level 必达标(状态与 level 强一致)
            if token.status() == CapabilityTokenStatus::Authorized {
                prop_assert!(
                    token.authorized_level() >= CapabilityToken::ACTIVATION_THRESHOLD,
                    "Authorized 态 level 必须达标, level = {}",
                    token.authorized_level()
                );
            }
            // I5: level 恒 ∈ [0.0, 1.0]
            prop_assert!((0.0..=1.0).contains(&token.authorized_level()));
        }
    }

    /// 渐进授权可达性:充足的成功样本 + 提升动作后必达 Authorized
    /// (EWMA α=0.1 单调逼近 1.0,自适应步长几何收敛,30 轮绰绰有余)
    #[test]
    fn s9_sufficient_successes_reach_authorized(extra in 0..100u32) {
        let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);
        for _ in 0..(30 + extra) {
            token.record_outcome(true);
            token.maybe_promote();
        }
        prop_assert!(token.allows_learned_policy(0));
        prop_assert_eq!(token.status(), CapabilityTokenStatus::Authorized);
    }
}

// ============================================================
// 确定性生命周期边界(常量驱动,不硬编码秒数)
// ============================================================

/// 冷却期边界:until - 1 仍禁,until 恢复(隐式),显式同步回 Authorized
#[test]
fn cooldown_boundary_exact_duration_blocks_then_recovers() {
    let start = 1_000_000i64;
    let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);
    // 推升到 Authorized,且 level 高到 ASA 衰减 0.2 后仍达标
    for _ in 0..40 {
        token.record_outcome(true);
        token.maybe_promote();
    }
    assert!(token.allows_learned_policy(start));
    assert!(
        token.authorized_level() - CapabilityToken::DECAY_ON_ASA
            >= CapabilityToken::ACTIVATION_THRESHOLD,
        "ASA 衰减 0.2 后必须仍达标才能恢复 Authorized"
    );

    token.trigger_asa_intervention(start);
    assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);
    let until = token.cooldown_until.expect("ASA 后必有冷却截止时间");
    assert_eq!(
        until,
        start + CapabilityToken::COOLDOWN_DURATION_SECS,
        "冷却截止 = start + COOLDOWN_DURATION_SECS"
    );

    // 冷却期结束前 1 秒:仍拒绝
    assert!(
        !token.allows_learned_policy(until - 1),
        "冷却期结束前必须仍不允许 Learned"
    );
    // 冷却期刚结束:隐式恢复(level 仍达标 → 允许,状态字段未同步)
    assert!(
        token.allows_learned_policy(until),
        "冷却期结束后(level 达标)必须允许 Learned"
    );
    // 显式恢复:状态字段同步回 Authorized,ASA 计数清零
    assert!(token.maybe_recover_from_cooldown(until));
    assert_eq!(token.status(), CapabilityTokenStatus::Authorized);
    assert_eq!(token.consecutive_asa_count(), 0, "恢复后 ASA 计数清零");
}

/// 连续 ASA 触发 → 第 3 次自动 Frozen → 恒禁 → unfreeze 恢复 Provisional
#[test]
fn consecutive_asa_triggers_freeze_then_unfreeze_recovers() {
    let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);
    for _ in 0..40 {
        token.record_outcome(true);
        token.maybe_promote();
    }
    assert!(token.allows_learned_policy(0));

    // 连续 3 次 ASA → 自动冻结(consecutive_asa_count >= ASA_FREEZE_THRESHOLD)
    assert!(!token.trigger_asa_intervention(1000), "第 1 次仅冷却");
    assert!(!token.trigger_asa_intervention(1100), "第 2 次仅冷却");
    assert!(token.trigger_asa_intervention(1200), "第 3 次必须冻结");
    assert_eq!(token.status(), CapabilityTokenStatus::Frozen);

    // Frozen 恒不允许 Learned(任意时间点)
    assert!(!token.allows_learned_policy(999_999_999));
    // Frozen 无冷却期可言,不会自动恢复
    assert!(!token.maybe_recover_from_cooldown(1_000_000_000));

    // 手动解冻 → 回到 Provisional,需重新累积 EWMA 才能再次激活
    token.unfreeze();
    assert_eq!(token.status(), CapabilityTokenStatus::Provisional);
    assert!(!token.allows_learned_policy(1_000_000_000));
}

/// 冷却结束但 level 低于阈值 → 降级回 Provisional(非 Authorized)
#[test]
fn cooldown_recovery_downgrades_when_level_below_threshold() {
    let start = 1_000_000i64;
    let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);
    // 初始 level 0.2,ASA 衰减 0.2 → 0.0,远低于阈值
    token.trigger_asa_intervention(start);
    assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);

    let until = token.cooldown_until.expect("冷却截止必须存在");
    assert!(token.maybe_recover_from_cooldown(until + 1));
    assert_eq!(
        token.status(),
        CapabilityTokenStatus::Provisional,
        "level 未达标必须恢复为 Provisional"
    );
    assert!(!token.allows_learned_policy(until + 1));
}

// ============================================================
// S9 bypass 联动:Cooldown/Frozen 态不查语义缓存(ADR-069 回滚接缝)
// ============================================================

/// 本地 mock OpenAI Chat 端点(响应体由 handler 决定),零外部网络依赖
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

/// OpenAI Chat 方言响应(固定 usage,成本/遥测断言用)
fn chat_response() -> axum::response::Json<serde_json::Value> {
    axum::response::Json(serde_json::json!({
        "id": "chatcmpl-s9-mock",
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
            "prompt_cache_hit_tokens": 0
        }
    }))
}

/// DeepSeek mock 通道 spec(base_url 指向本地 mock,免鉴权)
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
        intent_id: "intent-s9".into(),
        messages: vec![AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        tools: Vec::new(),
        thinking_pref: ThinkingPreference::Fast,
        budget_hint_micro: None,
        overrides: AffinityOverrides::default(),
    }
}

/// 与 adapters.rs context_hash 同算法(serde 确定性序列化 + SHA-256),
/// 保证预填条目的上下文哈希与 adapter 查询端一致(Context Ledger 校验通过)
fn context_hash(messages: &[AffinityMessage]) -> [u8; 32] {
    let json = serde_json::to_string(messages).unwrap_or_else(|_| "[]".into());
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    hasher.finalize().into()
}

/// 提取响应中的全部 Text 块文本(命中/厂商路径对比用)
fn text_of(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

/// Cooldown 态:预填缓存(若被查询必命中),invoke 仍走厂商(不查缓存不回填)
#[tokio::test]
async fn s9_cooldown_token_bypasses_semantic_cache() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
        c.fetch_add(1, AtomicOrdering::SeqCst);
        chat_response()
    })
    .await;
    let spec = mock_spec(&base);
    let cache = Arc::new(SemanticResponseCache::default());
    let req = mock_request();
    // 预填:同键/同指纹/同上下文哈希 → 一旦查询必命中
    let key = mca_gateway::build_token_cache_key(&spec, &req);
    let clv = mca_gateway::semantic_fingerprint(&req.messages, &req.tools);
    let hash = context_hash(&req.messages);
    cache.insert(req.intent_id.as_ref(), key, clv, "prefilled", hash, 1);

    // Cooldown 态 token(真实时钟触发,invoke 于冷却期内发生)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);
    token.trigger_asa_intervention(now);
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
    assert_eq!(
        calls.load(AtomicOrdering::SeqCst),
        1,
        "Cooldown bypass:必须走厂商(不查缓存)"
    );
    assert_eq!(text_of(&resp.blocks), "ok", "响应必须来自厂商而非缓存");
}

/// Frozen 态:同上,恒 bypass(Frozen 无时间依赖,更严格)
#[tokio::test]
async fn s9_frozen_token_bypasses_semantic_cache() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
        c.fetch_add(1, AtomicOrdering::SeqCst);
        chat_response()
    })
    .await;
    let spec = mock_spec(&base);
    let cache = Arc::new(SemanticResponseCache::default());
    let req = mock_request();
    let key = mca_gateway::build_token_cache_key(&spec, &req);
    let clv = mca_gateway::semantic_fingerprint(&req.messages, &req.tools);
    let hash = context_hash(&req.messages);
    cache.insert(req.intent_id.as_ref(), key, clv, "prefilled", hash, 1);

    let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);
    token.freeze();
    assert_eq!(token.status(), CapabilityTokenStatus::Frozen);
    assert!(!token.allows_learned_policy(i64::MAX), "Frozen 恒未授权");

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
    assert_eq!(
        calls.load(AtomicOrdering::SeqCst),
        1,
        "Frozen bypass:必须走厂商(不查缓存)"
    );
    assert_eq!(text_of(&resp.blocks), "ok", "响应必须来自厂商而非缓存");
}
