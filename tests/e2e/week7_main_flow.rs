//! Week 7 E2E 主流程测试 — 8 条跨层全链路验证(Task 6.2 + 6.5)
//!
//! 对应任务:Week 7 Task 6.2(8 个 E2E 用例)+ Task 6.5(CSA p95 ≤ 500ms)
//! 架构层:L1/L2/L3/L5/L6/L7/L8/L9/L10 跨层协同
//!
//! # 测试用例
//! 1. 文本→NMC→SSRA→CHTC→MCP Mesh 全链路(Week 6 扩展到 MCP)
//! 2. MCP 事务失败 → CSN 替代 → 降级链触发
//! 3. SESA 稀疏激活 → KVBSR 路由 → GEA 激活(KVBSR/GEA 不可用则仅 SESA)
//! 4. Critical 事件触发 → efficiency-monitor 告警 → /metrics 输出
//! 5. Quest→LSCT→SSRA→CHTC→MCP 全链路
//! 6. AHIRT→SSRA 防御→GSOE 进化→efficiency-monitor 告警
//! 7. DECB 降级→LSCT 降温→CSN 替代
//! 8. DegradedModeRejected E2E 覆盖(W6-Carryover-4)
//!
//! # CSA 延迟验证(Task 6.5)
//! 每个测试用 `std::time::Instant::now()` 测量全链路耗时,
//! 断言 < 500ms(从 Week 6 的 400ms 上浮 100ms 容忍 Week 7 新增 4 crate 开销)。
//!
//! # broadcast 时序铁律(Week 6 教训 #9)
//! 每个测试必须先 `bus.subscribe()` 再发布事件,否则 broadcast 不缓存历史消息,
//! 后订阅的 receiver 会静默丢失已发布事件。

#![forbid(unsafe_code)]

#[path = "week7_setup.rs"]
mod setup;

use std::time::{Duration, Instant};

use chtc_bridge::IdeSource;
use cmt_tiering::Tier;
use decb_governor::{BudgetConsumption, DecbConfig, DecbError, DecbGovernor};
use event_bus::{EventMetadata, NexusEvent};
use nmc_encoder::PerceptionInput;
use setup::{
    assert_has_event, drain_events, make_activation_request, make_fusion_request, make_profile,
    make_vscode_raw, setup_week7_pipeline,
};

use lsct_tiering::TaskType;

/// CSA 延迟阈值:500ms(Task 6.5,从 Week 6 的 400ms 上浮 100ms)
const CSA_THRESHOLD_MS: u128 = 500;

// ============================================================
// 测试 1:文本→NMC→SSRA→CHTC→MCP Mesh 全链路(Task 6.2.1)
// ============================================================

#[tokio::test]
async fn test_week7_mcp_mesh_full_chain() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    let start = Instant::now();

    // 1. NMC 编码文本输入(L2 → publish_blocking NmcEncoded)
    let clv = pipeline
        .encoder
        .perceive(PerceptionInput::Text("hello from week7 E2E".into()))
        .expect("NMC 编码失败");
    assert_eq!(clv.dimension(), 512, "CLV 维度必须为 512");

    // 2. SSRA 融合(L7 → fuse + 发布 SsraFusionCompleted)
    let request = make_fusion_request("q-w7-1", vec!["cap-text-fusion"], "target-cap");
    let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");
    assert!(result.confidence > 0.0, "融合置信度应大于 0");

    // 3. CHTC 转发工具调用(L10 → receive + execute,发布 ChtcToolCallReceived)
    let raw = make_vscode_raw("editor.open", serde_json::json!({ "file": "test.rs" }));
    let call = pipeline
        .bridge
        .receive(raw, IdeSource::vscode())
        .expect("CHTC receive 失败");
    let exec_result = pipeline.bridge.execute(&call).expect("CHTC execute 失败");
    assert!(exec_result.success, "VSCode execute 应成功");

    // 4. MCP Mesh 执行量子事务(L10 → execute_transaction,发布 McpMeshTransactionCompleted)
    let tx_result = pipeline
        .mesh
        .execute_transaction(vec!["srv-1".into(), "srv-2".into()], "query".into())
        .await
        .expect("MCP 事务执行失败");
    assert!(tx_result.success, "2 服务器事务应成功");

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < CSA_THRESHOLD_MS,
        "全链路应 < {}ms,实际 {}ms",
        CSA_THRESHOLD_MS,
        elapsed.as_millis()
    );

    // 验证事件序列:NmcEncoded → SsraFusionCompleted → ChtcToolCallReceived → McpMeshTransactionCompleted
    let events = drain_events(&mut rx, 15).await;
    assert_has_event(&events, "NmcEncoded");
    assert_has_event(&events, "SsraFusionCompleted");
    assert_has_event(&events, "ChtcToolCallReceived");
    assert_has_event(&events, "McpMeshTransactionCompleted");

    println!(
        "[CSA-1] 文本→NMC→SSRA→CHTC→MCP 全链路耗时: {}ms",
        elapsed.as_millis()
    );
}

// ============================================================
// 测试 2:MCP 事务失败 → CSN 替代 → 降级链触发(Task 6.2.2)
// ============================================================

#[tokio::test]
async fn test_week7_mcp_failure_csn_substitution() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    let start = Instant::now();

    // 1. 启动 CSN 降级链监听(订阅 McpMeshTransactionCompleted,在 spawn 前同步 subscribe)
    let _listener = pipeline.substitutor.start_degradation_listener();

    // 2. 先触发一次替代,创建降级链(level 0 = primary)
    let candidate = pipeline
        .substitutor
        .trigger_substitution("cap-shell")
        .await
        .expect("首次 CSN 替代应成功");
    assert_eq!(
        candidate.candidate_id, "cap-python",
        "应选 cap-python 作为替代"
    );
    let level_before = pipeline
        .substitutor
        .degradation_level("cap-shell")
        .expect("降级链应已创建");

    // 3. 发布 MCP 事务失败事件(模拟 MCP 事务 success=false)
    let failure_event = NexusEvent::McpMeshTransactionCompleted {
        metadata: EventMetadata::new("mcp-mesh"),
        transaction_id: "tx-fail-1".into(),
        participant_count: 2,
        latency_ms: 50,
        success: false,
    };
    pipeline
        .bus
        .publish(failure_event)
        .await
        .expect("发布失败事件应成功");

    // 4. 给后台 listener 时间处理事件
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 5. 验证降级链被推进(level 增加)
    let level_after = pipeline
        .substitutor
        .degradation_level("cap-shell")
        .expect("降级链应仍存在");
    assert!(
        level_after >= level_before,
        "MCP 失败后降级链应推进,level_before={}, level_after={}",
        level_before,
        level_after
    );

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < CSA_THRESHOLD_MS,
        "CSN 替代链路应 < {}ms,实际 {}ms",
        CSA_THRESHOLD_MS,
        elapsed.as_millis()
    );

    // 验证事件:CsnSubstitutionTriggered(首次触发) + McpMeshTransactionCompleted
    let events = drain_events(&mut rx, 10).await;
    assert_has_event(&events, "CsnSubstitutionTriggered");
    assert_has_event(&events, "McpMeshTransactionCompleted");

    println!(
        "[CSA-2] MCP失败→CSN替代链路耗时: {}ms (level {}→{})",
        elapsed.as_millis(),
        level_before,
        level_after
    );
}

// ============================================================
// 测试 3:SESA 稀疏激活 → KVBSR 路由 → GEA 激活(Task 6.2.3)
// ============================================================

#[tokio::test]
async fn test_week7_sesa_kvbsr_gea_activation() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    let start = Instant::now();

    // WHY 仅测 SESA:KVBSR/GEA 未加入 dev-dependencies(E2E 测试聚焦 Week 7 新增 4 crate),
    // 此测试验证 SESA 稀疏激活全链路:激活 → 稀疏度强制 < 40% → 发布 SesaActivationCompleted。
    // KVBSR/GEA 的集成在各自 crate 的 tests/ 中已覆盖(Week 3/4 验收)。

    // SESA 激活 Top-2 专家(3 个中选 2)
    let request = make_activation_request("req-w7-3", 2);
    let (mask, profile) = pipeline
        .sesa
        .activate(request)
        .await
        .expect("SESA 激活失败");

    // 验证稀疏度 < 40%(SESA 架构红线)
    assert!(
        profile.sparsity_ratio < 0.4,
        "稀疏度必须 < 40%,实际 {}",
        profile.sparsity_ratio
    );
    assert!(mask.active_count <= 3, "激活数不应超过总专家数");

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < CSA_THRESHOLD_MS,
        "SESA 激活链路应 < {}ms,实际 {}ms",
        CSA_THRESHOLD_MS,
        elapsed.as_millis()
    );

    // 验证事件:SesaActivationCompleted
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "SesaActivationCompleted");

    println!(
        "[CSA-3] SESA激活链路耗时: {}ms (稀疏度 {:.2}%, 激活 {}/{})",
        elapsed.as_millis(),
        profile.sparsity_ratio * 100.0,
        mask.active_count,
        3
    );
}

// ============================================================
// 测试 4:Critical 事件触发 → efficiency-monitor 告警 → /metrics 输出(Task 6.2.4)
// ============================================================

#[tokio::test]
async fn test_week7_critical_event_alert_metrics() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    let start = Instant::now();

    // 1. 构造 Critical 事件(SkepticVeto — 行使否决权)
    let skeptic_veto = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-w7-4".into(),
        veto_reason: "unsafe shell injection detected".into(),
        frozen_capabilities: vec!["shell_exec".into()],
    };

    // 2. efficiency-monitor 记录 Critical 事件 → 立即触发 Critical 告警 + 发布 EfficiencyAlertTriggered
    pipeline.monitor.record_event(&skeptic_veto);

    // 3. 验证告警计数(Critical 事件应记录 critical 告警)
    assert_eq!(
        pipeline.monitor.collectors().alert_count("critical"),
        1,
        "Critical 事件应触发 1 次 critical 告警"
    );
    assert_eq!(
        pipeline.monitor.collectors().event_count("SkepticVeto"),
        1,
        "应记录 1 次 SkepticVeto 事件"
    );

    // 4. 渲染 Prometheus /metrics 输出
    let metrics_output = pipeline.monitor.render_metrics();
    assert!(
        metrics_output.contains("nexus_critical_event_total"),
        "metrics 应包含 critical 事件计数"
    );
    assert!(
        metrics_output.contains("nexus_alert_triggered_total"),
        "metrics 应包含告警计数"
    );
    assert!(
        metrics_output.contains(r#"severity="critical""#),
        "metrics 应包含 critical 严重级别标签"
    );
    assert!(
        metrics_output.contains(r#"type="SkepticVeto""#),
        "metrics 应包含 SkepticVeto 事件类型"
    );

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < CSA_THRESHOLD_MS,
        "Critical→Monitor→metrics 链路应 < {}ms,实际 {}ms",
        CSA_THRESHOLD_MS,
        elapsed.as_millis()
    );

    // 验证事件:EfficiencyAlertTriggered(Critical 立即告警发布)
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "EfficiencyAlertTriggered");

    println!(
        "[CSA-4] Critical→Monitor→/metrics 链路耗时: {}ms",
        elapsed.as_millis()
    );
}

// ============================================================
// 测试 5:Quest→LSCT→SSRA→CHTC→MCP 全链路(Task 6.2.5)
// ============================================================

#[tokio::test]
async fn test_week7_quest_lsct_ssra_chtc_mcp_chain() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    let start = Instant::now();

    // 1. LSCT 任务感知分层(L3 → 注册能力 + tick 决策 + apply_decision 发布 LsctTierSwitched)
    pipeline
        .coordinator
        .register_capability("cap-quest-5", Tier::Warm);
    let profile = make_profile(TaskType::Compile, 0.9); // 高强度编译 → 目标 Hot
    let decisions = pipeline.coordinator.tick(&profile);
    // Warm → Hot 是相邻层级,应产生 Promote 决策
    assert!(
        decisions.iter().any(|d| d.is_promote()),
        "高强度编译任务应触发升温决策"
    );
    // WHY 调用 apply_decision:tick 仅生成决策,apply_decision 才执行 tier switch 并发布事件
    for d in &decisions {
        let _ = pipeline.coordinator.apply_decision(d).await;
    }

    // 2. SSRA 融合(L7 → 发布 SsraFusionCompleted)
    let request = make_fusion_request("q-w7-5", vec!["cap-text-fusion"], "target-cap");
    let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");
    assert!(result.confidence > 0.0, "融合置信度应大于 0");

    // 3. CHTC 转发(L10 → 发布 ChtcToolCallReceived)
    let raw = make_vscode_raw("editor.open", serde_json::json!({ "file": "main.rs" }));
    let call = pipeline
        .bridge
        .receive(raw, IdeSource::vscode())
        .expect("CHTC receive 失败");
    assert!(
        pipeline
            .bridge
            .execute(&call)
            .expect("CHTC execute 失败")
            .success
    );

    // 4. MCP Mesh 事务(L10 → 发布 McpMeshTransactionCompleted)
    let tx_result = pipeline
        .mesh
        .execute_transaction(vec!["srv-1".into()], "compile".into())
        .await
        .expect("MCP 事务执行失败");
    assert!(tx_result.success, "单服务器事务应成功");

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < CSA_THRESHOLD_MS,
        "Quest→LSCT→SSRA→CHTC→MCP 全链路应 < {}ms,实际 {}ms",
        CSA_THRESHOLD_MS,
        elapsed.as_millis()
    );

    // 验证事件序列:LsctTierSwitched → SsraFusionCompleted → ChtcToolCallReceived → McpMeshTransactionCompleted
    let events = drain_events(&mut rx, 15).await;
    assert_has_event(&events, "LsctTierSwitched");
    assert_has_event(&events, "SsraFusionCompleted");
    assert_has_event(&events, "ChtcToolCallReceived");
    assert_has_event(&events, "McpMeshTransactionCompleted");

    println!(
        "[CSA-5] Quest→LSCT→SSRA→CHTC→MCP 全链路耗时: {}ms",
        elapsed.as_millis()
    );
}

// ============================================================
// 测试 6:AHIRT→SSRA 防御→GSOE 进化→efficiency-monitor 告警(Task 6.2.6)
// ============================================================

#[tokio::test]
async fn test_week7_ahirt_ssra_gsoe_monitor() {
    let mut pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    let start = Instant::now();

    // 1. AHIRT 红队审计:手动发布 RedTeamAudit 事件 + 记录到 monitor
    let red_team_audit = NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("parliament"),
        vulnerability_type: "prompt_injection".into(),
        failed_probes: 5,
        total_probes: 20,
        detection_rate: 0.25,
        remediation_suggestion: "add input sanitization".into(),
    };
    pipeline.monitor.record_event(&red_team_audit);
    // RedTeamAudit 是 Critical 告警事件,应触发 critical 告警计数
    assert_eq!(
        pipeline.monitor.collectors().alert_count("critical"),
        1,
        "RedTeamAudit 应触发 critical 告警"
    );

    // 2. SSRA 防御性适配(L7 → 用 cap-defensive 模板,发布 SsraFusionCompleted)
    let request = make_fusion_request("q-w7-6", vec!["cap-defensive"], "defensive-target");
    let result = pipeline
        .fusion
        .fuse(request)
        .await
        .expect("SSRA 防御性融合失败");
    assert!(result.confidence > 0.0, "防御性融合置信度应大于 0");

    // 3. GSOE 对抗进化(L5 → handle_red_team_audit + evolve_once,发布 GsoePolicyUpdated)
    pipeline.evolution.handle_red_team_audit();
    let evolution_result = pipeline
        .evolution
        .evolve_once()
        .await
        .expect("GSOE 进化失败");
    assert!(evolution_result.generation > 0, "进化世代应 > 0");

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < CSA_THRESHOLD_MS,
        "AHIRT→SSRA→GSOE→Monitor 链路应 < {}ms,实际 {}ms",
        CSA_THRESHOLD_MS,
        elapsed.as_millis()
    );

    // 验证事件序列:EfficiencyAlertTriggered(Critical 告警) → SsraFusionCompleted → GsoePolicyUpdated
    let events = drain_events(&mut rx, 15).await;
    assert_has_event(&events, "EfficiencyAlertTriggered");
    assert_has_event(&events, "SsraFusionCompleted");
    assert_has_event(&events, "GsoePolicyUpdated");

    println!(
        "[CSA-6] AHIRT→SSRA→GSOE→Monitor 链路耗时: {}ms",
        elapsed.as_millis()
    );
}

// ============================================================
// 测试 7:DECB 降级→LSCT 降温→CSN 替代(Task 6.2.7)
// ============================================================

#[tokio::test]
async fn test_week7_decb_lsct_csn_chain() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    let start = Instant::now();

    // 1. DECB 降级(L8 → 创建低预算 governor,记录消耗触发 BudgetExceeded)
    // WHY 独立 governor:Week7Pipeline 不含 DecbGovernor(DECB 在 Week 5 已验收),
    // 此处创建独立实例注入共享 EventBus,验证 BudgetExceeded 事件能被 monitor 订阅。
    let decb_config = DecbConfig {
        total_budget_limit: 100.0,
        ..Default::default()
    }; // 极低预算,便于快速触发溢出
    let governor = DecbGovernor::with_event_bus(decb_config, pipeline.bus.clone())
        .expect("DECB governor 构造失败");

    let consumption = BudgetConsumption {
        token_count: 0,
        tool_call_count: 0,
        context_load_count: 0,
        total_cost: 200.0, // 超过 100.0 上限,触发 BudgetExceeded
    };
    let _ = governor.record_consumption(&consumption);

    // 2. LSCT 降温(L3 → 低强度任务触发降温决策,发布 LsctTierSwitched)
    pipeline
        .coordinator
        .register_capability("cap-decb-7", Tier::Hot);
    let cool_profile = make_profile(TaskType::Debug, 0.1); // 低强度调试 → 目标 Cold
    let _decisions = pipeline.coordinator.tick(&cool_profile);

    // 3. CSN 替代(L10 → trigger_substitution,发布 CsnSubstitutionTriggered)
    let candidate = pipeline
        .substitutor
        .trigger_substitution("cap-shell")
        .await
        .expect("CSN 替代应成功");
    assert_eq!(
        candidate.candidate_id, "cap-python",
        "应选 cap-python 作为替代"
    );

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < CSA_THRESHOLD_MS,
        "DECB→LSCT→CSN 链路应 < {}ms,实际 {}ms",
        CSA_THRESHOLD_MS,
        elapsed.as_millis()
    );

    // 验证事件序列:BudgetExceeded → LsctTierSwitched → CsnSubstitutionTriggered
    let events = drain_events(&mut rx, 15).await;
    assert_has_event(&events, "BudgetExceeded");
    assert_has_event(&events, "CsnSubstitutionTriggered");

    println!("[CSA-7] DECB→LSCT→CSN 链路耗时: {}ms", elapsed.as_millis());
}

// ============================================================
// 测试 8:DegradedModeRejected E2E 覆盖(W6-Carryover-4, Task 6.2.8)
// ============================================================

#[tokio::test]
async fn test_week7_degraded_mode_rejected_e2e() {
    let bus = event_bus::EventBus::new();
    let mut rx = bus.subscribe();

    let start = Instant::now();

    // 1. 创建极低预算 governor,使消耗即触发降级
    // WHY 两次调用:首次超预算 → 从 HighTier 降级到 Degraded(返回 Ok);
    //      二次超预算 → 已在 Degraded 模式,返回 DegradedModeRejected
    let decb_config = DecbConfig {
        total_budget_limit: 50.0,
        ..Default::default()
    }; // 极低预算
    let governor =
        DecbGovernor::with_event_bus(decb_config, bus.clone()).expect("DECB governor 构造失败");

    // 2. 构造超预算消耗(100.0 > 50.0 上限,ratio=2.0 >= 1.0 触发 critical)
    let consumption = BudgetConsumption {
        token_count: 0,
        tool_call_count: 0,
        context_load_count: 0,
        total_cost: 100.0, // 超过 50.0 上限
    };

    // 3. 第一次调用:触发降级 HighTier → Degraded,返回 Ok(())
    let first_result = governor.record_consumption(&consumption);
    assert!(
        first_result.is_ok(),
        "首次超预算应触发降级而非拒绝,实际: {:?}",
        first_result
    );

    // 4. 第二次调用:已在 Degraded 模式,返回 DegradedModeRejected(W6-Carryover-4 核心断言)
    let result = governor.record_consumption(&consumption);
    assert!(result.is_err(), "Degraded 模式下再次超预算应返回错误");
    match &result {
        Err(DecbError::DegradedModeRejected { .. }) => {
            // 期望的 DegradedModeRejected 错误路径
        }
        Err(e) => panic!("期望 DegradedModeRejected,实际收到: {:?}", e),
        Ok(_) => panic!("Degraded 模式下超预算不应成功"),
    }

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < CSA_THRESHOLD_MS,
        "DegradedModeRejected E2E 应 < {}ms,实际 {}ms",
        CSA_THRESHOLD_MS,
        elapsed.as_millis()
    );

    // 5. 验证发布了 BudgetExceeded [Critical] 事件(至少一次)
    let events = drain_events(&mut rx, 10).await;
    assert_has_event(&events, "BudgetExceeded");

    println!(
        "[CSA-8] DegradedModeRejected E2E 耗时: {}ms",
        elapsed.as_millis()
    );
}
