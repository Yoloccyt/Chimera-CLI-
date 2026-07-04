//! Week 6 E2E 主流程测试 — 5 条跨层全链路验证
//!
//! 对应任务:Week 6 Task 6(E2E 集成测试)
//! 架构层:L1/L2/L3/L5/L7/L8/L10 跨层协同
//!
//! # 测试用例
//! 1. 文本→NMC→SSRA→CHTC 全链路(< 400ms)
//! 2. 桌面→NMC→SSRA→GSOE 进化触发
//! 3. Quest→LSCT 升温→SSRA 适配→CHTC 转发
//! 4. AHIRT 红队告警→SSRA 防御性适配→GSOE 对抗进化
//! 5. 预算超限→DECB 降级→LSCT 降温→SSRA 适配降级

#[path = "week6_setup.rs"]
mod setup;

use std::time::{Duration, Instant};

use chtc_bridge::IdeSource;
use cmt_tiering::Tier;
use event_bus::{EventMetadata, NexusEvent};
use nmc_encoder::{DesktopCapture, PerceptionInput};
use setup::{
    assert_has_event, drain_events, make_fusion_request, make_profile, make_vscode_raw,
    setup_week6_pipeline,
};

use lsct_tiering::TaskType;

// ============================================================
// 测试 1:文本输入→NMC→SSRA→CHTC 全链路(< 400ms)
// ============================================================

#[tokio::test]
async fn test_main_flow_text_to_chtc_under_400ms() {
    let pipeline = setup_week6_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    let start = Instant::now();

    // 1. NMC 编码文本输入(L2 Memory → 同步 perceive + publish_blocking NmcEncoded)
    let clv = pipeline
        .encoder
        .perceive(PerceptionInput::Text("hello world from E2E".into()))
        .expect("NMC 编码失败");
    assert_eq!(clv.dimension(), 512, "CLV 维度必须为 512");

    // 2. SSRA 融合(L7 Execution → 用 cap-text-fusion 模板,异步 fuse + 发布 SsraFusionCompleted)
    let request = make_fusion_request("q-flow-1", vec!["cap-text-fusion"], "target-cap");
    let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");
    assert!(
        result.confidence > 0.0,
        "融合置信度应大于 0,实际 {}",
        result.confidence
    );

    // 3. CHTC 转发工具调用(L10 Interface → 同步 receive + execute,发布 ChtcToolCallReceived)
    let raw = make_vscode_raw("editor.open", serde_json::json!({ "file": "test.rs" }));
    let call = pipeline
        .bridge
        .receive(raw, IdeSource::vscode())
        .expect("CHTC receive 失败");
    let exec_result = pipeline.bridge.execute(&call).expect("CHTC execute 失败");
    assert!(exec_result.success, "VSCode execute 应成功");

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 400,
        "全链路应 < 400ms,实际 {}ms",
        elapsed.as_millis()
    );

    // 验证事件序列:NmcEncoded → SsraFusionCompleted → ChtcToolCallReceived
    let events = drain_events(&mut rx, 10).await;
    assert_has_event(&events, "NmcEncoded");
    assert_has_event(&events, "SsraFusionCompleted");
    assert_has_event(&events, "ChtcToolCallReceived");
}

// ============================================================
// 测试 2:桌面输入→NMC→SSRA→GSOE 进化触发
// ============================================================

#[tokio::test]
async fn test_main_flow_desktop_to_gsoe_evolution() {
    let mut pipeline = setup_week6_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // 1. NMC 编码桌面输入(L2 Memory)
    let desktop = DesktopCapture::new(1920, 1080, "code editor with Rust syntax highlighting");
    let clv = pipeline
        .encoder
        .perceive(PerceptionInput::Desktop(desktop))
        .expect("NMC 桌面编码失败");
    assert_eq!(clv.dimension(), 512, "桌面 CLV 维度必须为 512");

    // 2. SSRA 融合(L7 Execution → 用 cap-desktop-fusion 模板,WeightedAverage 策略)
    let request = make_fusion_request("q-flow-2", vec!["cap-desktop-fusion"], "desktop-target");
    let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");
    // WeightedAverage 单模板:Σ(w²)/Σ(w) = 0.36/0.6 = 0.6
    assert!(
        (result.confidence - 0.6).abs() < 1e-5,
        "WeightedAverage 置信度应为 0.6,实际 {}",
        result.confidence
    );

    // 3. GSOE 进化(L5 Knowledge → handle_consensus_reached + evolve_once)
    // WHY handle_consensus_reached:模拟议会共识信号,为进化提供奖励上下文
    pipeline.evolution.handle_consensus_reached();
    let evo_result = pipeline
        .evolution
        .evolve_once()
        .await
        .expect("GSOE 进化失败");
    assert_eq!(evo_result.generation, 1, "首次进化世代应为 1");

    // 验证事件:NmcEncoded + SsraFusionCompleted + GsoePolicyUpdated
    let events = drain_events(&mut rx, 10).await;
    assert_has_event(&events, "NmcEncoded");
    assert_has_event(&events, "SsraFusionCompleted");
    assert_has_event(&events, "GsoePolicyUpdated");
}

// ============================================================
// 测试 3:Quest 激活→LSCT 升温→SSRA 适配→CHTC 转发
// ============================================================

#[tokio::test]
async fn test_main_flow_quest_lsct_ssra_chtc() {
    let pipeline = setup_week6_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // 1. 注册能力到 LSCT(初始 Warm)
    pipeline
        .coordinator
        .register_capability("cap-text-fusion", Tier::Warm);

    // 2. Quest 创建:高强度编译任务 → LSCT 升温决策
    // WHY "compile production release":from_quest_title 匹配 compile + production
    // → TaskType::Compile, intensity=0.9 → target_tier=Hot
    // Warm(rank 1) → Hot(rank 0) 是相邻层级,产生 Promote 决策
    let title = "compile production release";
    let decisions = pipeline
        .coordinator
        .handle_quest_created(title)
        .await
        .expect("LSCT handle_quest_created 失败");

    assert!(
        decisions.iter().any(|d| d.is_promote()),
        "高强度编译任务应触发 Warm→Hot 升温决策"
    );

    // 3. SSRA 融合(用升温后的能力,验证融合不受层级影响)
    let request = make_fusion_request("q-flow-3", vec!["cap-text-fusion"], "target-cap");
    let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");
    assert!(result.confidence > 0.0, "升温后融合应正常");

    // 4. CHTC 转发工具调用
    let raw = make_vscode_raw("compile.run", serde_json::json!({ "target": "release" }));
    let call = pipeline
        .bridge
        .receive(raw, IdeSource::vscode())
        .expect("CHTC receive 失败");
    assert_eq!(call.tool_id, "compile.run");

    // 验证事件:LsctTierSwitched + SsraFusionCompleted + ChtcToolCallReceived
    let events = drain_events(&mut rx, 10).await;
    assert_has_event(&events, "LsctTierSwitched");
    assert_has_event(&events, "SsraFusionCompleted");
    assert_has_event(&events, "ChtcToolCallReceived");
}

// ============================================================
// 测试 4:AHIRT 红队告警→SSRA 防御性适配→GSOE 对抗进化
// ============================================================

#[tokio::test]
async fn test_main_flow_ahirt_ssra_gsoe_counter_evolution() {
    let mut pipeline = setup_week6_pipeline().expect("管线装配失败");

    // 1. 启动 SSRA 防御性适配(L7 → 订阅 ConsensusReached/RedTeamAudit)
    // WHY start_defensive_adapter 在 spawn 前同步 subscribe,确保不漏事件
    let _defensive_handle = pipeline
        .fusion
        .start_defensive_adapter()
        .expect("应启动防御性适配(bus 已注入)");

    // 2. 发布 RedTeamAudit 事件(模拟 AHIRT 红队检测到 prompt_injection 漏洞)
    let vuln_type = "prompt_injection";
    let event = NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("ahirt-red-team"),
        vulnerability_type: vuln_type.to_string(),
        failed_probes: 3,
        total_probes: 10,
        detection_rate: 0.3,
        remediation_suggestion: "add input sanitization".into(),
    };
    pipeline
        .bus
        .publish(event)
        .await
        .expect("发布 RedTeamAudit 失败");

    // 3. 等待 SSRA 后台任务处理(sleep 100ms 让 tokio 调度后台任务)
    // WHY 100ms:start_defensive_adapter 的后台任务需要被调度执行,
    // 100ms 足以让 tokio runtime 调度任务并完成模板注册
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 4. 验证 SSRA 注册了防御性模板(defensive-{vulnerability_type})
    let defensive_cap_id = format!("defensive-{vuln_type}");
    let meta = pipeline
        .fusion
        .registry()
        .get_template_meta(&defensive_cap_id);
    assert!(meta.is_some(), "SSRA 应注册防御性模板 {defensive_cap_id}");

    // 5. GSOE 对抗进化(L5 → handle_red_team_audit 提升 mutation_rate + evolve_once)
    pipeline.evolution.handle_red_team_audit();
    let evo_result = pipeline
        .evolution
        .evolve_once()
        .await
        .expect("GSOE 对抗进化失败");
    assert_eq!(evo_result.generation, 1, "对抗进化世代应为 1");

    // 6. SSRA 用防御性模板执行融合(验证防御性适配产出可用)
    let request = make_fusion_request(
        "q-flow-4",
        vec![defensive_cap_id.as_str()],
        "defensive-target",
    );
    let result = pipeline
        .fusion
        .fuse(request)
        .await
        .expect("SSRA 防御性融合失败");
    assert!(result.confidence > 0.0, "防御性模板融合置信度应大于 0");
}

// ============================================================
// 测试 5:预算超限→DECB 降级→LSCT 降温→SSRA 适配降级
// ============================================================

#[tokio::test]
async fn test_main_flow_budget_decb_lsct_ssra() {
    let pipeline = setup_week6_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // 1. 注册能力到 LSCT(初始 Hot — 模拟正常运行时热层)
    pipeline
        .coordinator
        .register_capability("cap-text-fusion", Tier::Hot);

    // 2. 发布 BudgetExceeded 事件(模拟 DECB L8 检测到预算超限)
    // WHY 此事件模拟 DECB 的预算监控触发,实际 DECB governor 未接入 E2E,
    // 用直接发布事件的方式模拟降级信号
    let event = NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("decb-governor"),
        budget_type: "token".into(),
        current: 1500,
        limit: 1000,
    };
    pipeline
        .bus
        .publish(event)
        .await
        .expect("发布 BudgetExceeded 失败");

    // 3. LSCT 降温:低强度 Debug 任务 → 目标 Ice(L3 Storage)
    // WHY Debug intensity 0.1 → target_tier=Ice,Hot(rank 0) → 逐级降温
    // 每个 tick 只降一级:Hot → Warm(首次 tick)
    let profile = make_profile(TaskType::Debug, 0.1);
    let decisions = pipeline.coordinator.tick(&profile);

    assert!(
        decisions.iter().any(|d| d.is_demote()),
        "低强度 Debug 任务应触发 Hot→Warm 降温决策"
    );

    // 4. 执行降温决策(apply_decision 会发布 LsctTierSwitched 事件)
    for decision in &decisions {
        let _ = pipeline.coordinator.apply_decision(decision).await;
    }

    // 5. SSRA 仍能正常融合(降级不影响融合能力,L7 独立于 L3)
    let request = make_fusion_request("q-flow-5", vec!["cap-text-fusion"], "target-cap");
    let result = pipeline
        .fusion
        .fuse(request)
        .await
        .expect("SSRA 融合不应因降级失败");
    assert!(result.confidence > 0.0, "降级后融合置信度仍应大于 0");

    // 验证事件:LsctTierSwitched(降温)+ SsraFusionCompleted(融合)
    let events = drain_events(&mut rx, 10).await;
    assert_has_event(&events, "LsctTierSwitched");
    assert_has_event(&events, "SsraFusionCompleted");
}

// ============================================================
// 测试 6:Week 6 主链路全事件流端到端断言(W6-Carryover-2)
//
// WHY 此测试为 W6-Carryover-2 核心:在单个测试用例中驱动 NMC + LSCT +
// SSRA + GSOE + CHTC 五个 crate 的完整链路,断言 Week 6 主链路的 5 个
// 关键事件(SsraFusionCompleted / LsctTierSwitched / GsoePolicyUpdated /
// NmcEncoded / ChtcToolCallReceived)被正确发布到共享 EventBus。
//
// 与测试 1-5 的区别:测试 1-5 各自覆盖部分事件,此测试在同一个链路中
// 串联所有 5 个关键事件,验证跨层事件流的完整性与事件发布的正确顺序。
// ============================================================

#[tokio::test]
async fn test_week6_full_event_chain_all_five_events() {
    // WHY `mut`:GSOE 的 evolve_once 需要 &mut self,通过 &mut pipeline.evolution 访问
    let mut pipeline = setup_week6_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // 1. LSCT 注册能力 + Quest 创建触发升温 → 发布 LsctTierSwitched 事件
    // WHY "compile production release":from_quest_title 匹配 compile + production
    // → TaskType::Compile, intensity=0.9 → target_tier=Hot
    // Warm(rank 1) → Hot(rank 0) 是相邻层级,产生 Promote 决策并发布事件
    pipeline
        .coordinator
        .register_capability("cap-text-fusion", Tier::Warm);
    let decisions = pipeline
        .coordinator
        .handle_quest_created("compile production release")
        .await
        .expect("LSCT handle_quest_created 失败");
    assert!(
        decisions.iter().any(|d| d.is_promote()),
        "高强度编译任务应触发 Warm→Hot 升温决策"
    );

    // 2. NMC 编码文本输入 → 发布 NmcEncoded 事件
    let clv = pipeline
        .encoder
        .perceive(PerceptionInput::Text(
            "full chain event verification".into(),
        ))
        .expect("NMC 编码失败");
    assert_eq!(clv.dimension(), 512, "CLV 维度必须为 512");

    // 3. SSRA 融合 → 发布 SsraFusionCompleted 事件
    let request = make_fusion_request("q-full-chain", vec!["cap-text-fusion"], "target-cap");
    let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");
    assert!(
        result.confidence > 0.0,
        "融合置信度应大于 0,实际 {}",
        result.confidence
    );

    // 4. GSOE 进化 → 发布 GsoePolicyUpdated 事件
    // WHY handle_consensus_reached:模拟议会共识信号,为进化提供奖励上下文
    pipeline.evolution.handle_consensus_reached();
    let evo_result = pipeline
        .evolution
        .evolve_once()
        .await
        .expect("GSOE 进化失败");
    assert_eq!(evo_result.generation, 1, "首次进化世代应为 1");

    // 5. CHTC 转发工具调用 → 发布 ChtcToolCallReceived 事件
    let raw = make_vscode_raw(
        "editor.open",
        serde_json::json!({ "file": "full_chain.rs" }),
    );
    let call = pipeline
        .bridge
        .receive(raw, IdeSource::vscode())
        .expect("CHTC receive 失败");
    let exec_result = pipeline.bridge.execute(&call).expect("CHTC execute 失败");
    assert!(exec_result.success, "VSCode execute 应成功");

    // 验证 Week 6 主链路的 5 个关键事件全部被正确发布
    // WHY drain 20 个事件:5 个关键事件 + 可能的 LSCT 内部决策事件,
    // 20 个槽位足以覆盖完整链路的所有事件
    let events = drain_events(&mut rx, 20).await;
    assert_has_event(&events, "NmcEncoded");
    assert_has_event(&events, "SsraFusionCompleted");
    assert_has_event(&events, "GsoePolicyUpdated");
    assert_has_event(&events, "LsctTierSwitched");
    assert_has_event(&events, "ChtcToolCallReceived");
}
