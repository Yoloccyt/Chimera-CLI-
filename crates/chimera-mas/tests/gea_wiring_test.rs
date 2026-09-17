//! MAS → GEA 接线集成测试(M12 / ADR-185 D1)——非空转验收
//!
//! 对应 mission 验收断言:
//! ① mas 委托路径产生 ExpertActivated 事件(共享总线真实断言)
//! ② record_expert_outcome 后 confidence 影响后续激活(完整升降循环)
//!
//! ## 装配形态
//! 与生产组合根同构:`gea_bridge::build_mas_activator`(config 校验 + E01-E08
//! 注册)+ `RootOrchestrator::with_gea` + `DelegationExecutor::with_gea`,
//! 三者共享同一 `Arc<GeaActivator>` 实例(激活 → 执行 → 反馈闭环要求单一注册表)。

#![forbid(unsafe_code)]

use chimera_mas::gea_bridge::{build_mas_activator, mas_gea_config};
use chimera_mas::{AgentTask, DelegationExecutor, QualityLevel, RootOrchestrator, TaskComplexity};
use event_bus::{EventBus, NexusEvent};
use nexus_core::{Task, TaskStatus};
use std::sync::Arc;
use std::time::Duration;

/// 构造测试 AgentTask(指定 task_id 与复杂度)
fn make_task(task_id: &str, complexity: TaskComplexity) -> AgentTask {
    AgentTask::new(
        Task {
            task_id: task_id.into(),
            description: "GEA 接线集成测试".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        },
        complexity,
        1000,
        Duration::from_secs(60),
        QualityLevel::Standard,
    )
}

/// 接线同一激活器的 orchestrator + executor 组合(生产组合根同构)
fn wired_stack(
    bus: &EventBus,
) -> (
    RootOrchestrator,
    DelegationExecutor,
    Arc<gea_activator::GeaActivator>,
) {
    let gea = build_mas_activator(mas_gea_config(), bus.clone()).expect("装配应成功");
    let orchestrator = RootOrchestrator::new(bus.clone()).with_gea(Arc::clone(&gea));
    let executor =
        DelegationExecutor::new(bus.clone(), Duration::from_secs(60)).with_gea(Arc::clone(&gea));
    (orchestrator, executor, gea)
}

/// 集成断言 ①:mas 委托路径产生 ExpertActivated(共享总线真实事件)
#[tokio::test]
async fn delegate_path_publishes_expert_activated() {
    let bus = EventBus::new();
    // §4.4 反模式 3:subscribe 必须在委托调用之前同步调用
    let mut rx = bus.subscribe();
    let (orchestrator, _executor, gea) = wired_stack(&bus);
    assert_eq!(gea.expert_count(), 8, "E01-E08 已注册");

    let handles = orchestrator
        .delegate(make_task("t-integ-1", TaskComplexity::VeryComplex))
        .await
        .expect("委托应成功");
    assert_eq!(handles.len(), 5, "VeryComplex 扇出不变(行为零变化)");

    // 委托路径经 gea.activate 发布 ExpertActivated(Normal 级,broadcast)
    let event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("500ms 内应收到事件")
        .expect("recv 失败");
    match event {
        NexusEvent::ExpertActivated {
            activated_experts,
            top_gate_value,
            ..
        } => {
            assert!(
                !activated_experts.is_empty(),
                "VeryComplex 委托应激活至少一个专家(Ω-Sparse 下非空)"
            );
            assert!(
                (0.0..=1.0).contains(&top_gate_value),
                "top_gate_value 应在 [0,1]"
            );
        }
        other => panic!("期望 ExpertActivated,实际 {}", other.type_name()),
    }
}

/// 集成断言 ②:record_expert_outcome 后 confidence 影响后续激活
///
/// 两段式证据:
/// 1) 真实路径段——经 DelegationExecutor(接线 gea)执行 10 失败 + 10 成功任务,
///    断言 gea 侧成功率随执行结果变化(0.0 → 0.5),证明回填闭环真实生效;
/// 2) 门控因果段——专用低负载激活器(仅 E03、w4 加重、缓存预热稳态阈值),
///    fresh(conf=0.5)→ 10 失败(conf=0.0,E03 被抑制)→ 10 成功(conf 回升,E03
///    重新激活),以"激活成员资格 + 门控值序"双重断言锁定因果,排除动态阈值噪声。
#[tokio::test]
async fn recorded_outcomes_influence_subsequent_activation() {
    let bus = EventBus::new();
    let (orchestrator, executor, gea) = wired_stack(&bus);

    // 前置:经委托路径走一次激活(与断言①同路径,确认接线在端到端链路上)
    orchestrator
        .delegate(make_task("t-cycle-base", TaskComplexity::VeryComplex))
        .await
        .expect("基线委托应成功");

    // 失败注入:10 个不同 task_id 任务,激活清单仅 E03,runner 全部失败
    let failing_runner: chimera_mas::TaskRunner = Arc::new(|_task: AgentTask| {
        Box::pin(async { Err("集成测试注入失败".to_string()) })
    });
    let fail_executor =
        DelegationExecutor::with_runner(bus.clone(), Duration::from_secs(60), failing_runner)
            .with_gea(Arc::clone(&gea));
    for i in 0..10 {
        let stamped = make_task(&format!("t-cycle-fail-{i}"), TaskComplexity::Simple)
            .with_activated_experts(vec!["E03".to_string()]);
        let results = fail_executor
            .execute_delegation("p-fail", vec![stamped])
            .await
            .expect("失败注入执行框架应成功");
        assert!(!results[0].success);
    }
    // confidence 小样本收缩:10 次全失败 → confidence = 0.0
    let conf_after_fail = gea.expert_success_rate(&gea_activator::ExpertId::new("E03"));
    assert_eq!(conf_after_fail, Some(0.0), "10 次失败 → 成功率 0.0");

    // 成功注入:10 个不同 task_id 任务,激活清单仅 E03,默认 runner 成功
    for i in 0..10 {
        let stamped = make_task(&format!("t-cycle-ok-{i}"), TaskComplexity::Simple)
            .with_activated_experts(vec!["E03".to_string()]);
        let results = executor
            .execute_delegation("p-ok", vec![stamped])
            .await
            .expect("成功注入应成功");
        assert!(results[0].success);
    }
    let conf_after_ok = gea.expert_success_rate(&gea_activator::ExpertId::new("E03"));
    assert!(
        (conf_after_ok.expect("应存在") - (10.0_f32 / 20.0)).abs() < 1e-6,
        "10 失败 + 10 成功 → 成功率 0.5"
    );

    // 门控影响断言:专用低负载激活器(仅注册 E03)+ w4 加重测试配置。
    // WHY 双偏离 mas_gea_config:① 单专家注册把规模因子压到 0.03,避免 8 专家
    // 负载把动态阈值抬过桥接词表门控上限(Ω-Sparse 稀疏语义本身没错,但会让
    // 因果断言淹没在阈值噪声里);② w4=0.4 放大 confidence 通道(±0.2 logit),
    // 锁定"confidence → 门控"因果链。缓存预热 3 次命中压低未命中率(阈值稳态),
    // 各阶段换优先级破缓存(风险维仅影响 ~1e-3 范数项,方向比较有效)。
    let test_config = gea_activator::GeaConfig {
        w1: 0.3,
        w2: 0.2,
        w3: 0.1,
        w4_confidence: 0.4,
        bias: 0.35,
        ..Default::default()
    };
    let fresh = gea_activator::GeaActivator::new(test_config, EventBus::new())
        .expect("测试配置权重和=1.0,应通过校验");
    let e03_mas = chimera_mas::ExpertRegistry::new()
        .get("E03")
        .expect("E03 在静态编制中");
    fresh.register_expert(chimera_mas::gea_bridge::build_gea_expert_profile(e03_mas));
    let e03_id = gea_activator::ExpertId::new("E03");

    let profile_at = |priority: event_bus::TaskPriority, tag: &str| {
        let task = make_task(tag, TaskComplexity::Complex).with_priority(priority);
        chimera_mas::gea_bridge::task_profile_from_agent_task(&task)
    };

    // fresh 相:confidence=0.5(无反馈)→ E03 应激活,记录基线门控值
    let p_fresh = profile_at(event_bus::TaskPriority::Low, "t-gate-fresh");
    let r_fresh = fresh.activate(&p_fresh).await.expect("fresh 激活应成功");
    let gate_fresh = r_fresh.top_gate_value;
    assert!(
        r_fresh.activated.contains(&e03_id),
        "fresh 相 E03(confidence=0.5)应被激活"
    );
    // 预热 3 次缓存命中:hit_rate↑ → 未命中率↓ → 动态阈值稳态化
    for _ in 0..3 {
        let _ = fresh.activate(&p_fresh).await;
    }

    // failed 相:10 次失败 → confidence=0.0 → E03 门控值跌破阈值,不得激活
    for _ in 0..10 {
        fresh.record_expert_outcome(&e03_id, false, 10.0);
    }
    assert_eq!(
        fresh.expert_success_rate(&e03_id),
        Some(0.0),
        "10 次失败 → 成功率 0.0"
    );
    let r_failed = fresh
        .activate(&profile_at(
            event_bus::TaskPriority::Medium,
            "t-gate-failed",
        ))
        .await
        .expect("failed 相激活调用应成功(空结果也是 Ok)");
    assert!(
        !r_failed.activated.contains(&e03_id),
        "confidence 归零后 E03 不得激活(Ω-Evolve 负反馈门控)"
    );
    assert!(
        r_failed.top_gate_value < gate_fresh,
        "confidence 归零后门控值必须下降:fresh={gate_fresh}, failed={}",
        r_failed.top_gate_value
    );

    // recovered 相:10 次成功 → confidence 回升(20 次中 10 成 → 0.5)→ E03 重新激活
    for _ in 0..10 {
        fresh.record_expert_outcome(&e03_id, true, 10.0);
    }
    let r_recovered = fresh
        .activate(&profile_at(
            event_bus::TaskPriority::High,
            "t-gate-recovered",
        ))
        .await
        .expect("recovered 相激活应成功");
    assert!(
        r_recovered.activated.contains(&e03_id),
        "confidence 回升后 E03 应重新激活(完整升降循环)"
    );
    assert!(
        r_recovered.top_gate_value > r_failed.top_gate_value,
        "confidence 回升后门控值必须上升:failed={}, recovered={}",
        r_failed.top_gate_value,
        r_recovered.top_gate_value
    );
}
