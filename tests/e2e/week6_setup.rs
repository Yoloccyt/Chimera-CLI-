//! Week 6 E2E 测试基础设施 — 共享 EventBus + 5 个 crate 实例的统一装配
//!
//! 对应任务:Week 6 Task 6(E2E 集成测试)
//! 架构层:L1 Core(EventBus)+ L2/L3/L5/L7/L10 五个被测 crate
//!
//! # 设计要点
//! - **共享 EventBus**:5 个 crate 通过 `with_event_bus(config, bus.clone())`
//!   注入同一个 `EventBus`(Arc-based,Clone 廉价),实现跨层事件广播
//! - **week6_setup.rs 既是 [[test]] target,也被其他测试文件通过
//!   `#[path = "week6_setup.rs"] mod setup;` 复用**,避免代码重复
//! - **GSOE 的 `evolve_once` 需要 `&mut self`**,因此 `evolution` 字段
//!   直接持有所有权(非 Arc<Mutex>),测试中通过 `&mut pipeline.evolution` 访问

use std::time::Duration;

use chtc_bridge::{ChtcBridge, ChtcConfig, IdeSource};
use cmt_tiering::Tier;
use event_bus::{EventBus, EventReceiver, NexusEvent};
use gsoe_evolution::{GsoeConfig, GsoeEvolutionEngine};
use lsct_tiering::{LsctConfig, LsctCoordinator, TaskLoadProfile, TaskType};
use nmc_encoder::{NmcConfig, NmcEncoder, PerceptionInput};
use ssra_fusion::{
    precompile, FusionRequest, FusionStrategy, SlimeFusionEngine, SsraConfig, TemplateSpec,
};

// ============================================================
// Week6Pipeline — 持有共享 EventBus + 5 个 crate 实例
// ============================================================

/// Week 6 E2E 测试管线 — 持有共享 EventBus 与 5 个被测 crate 的实例
///
/// WHY 直接持有所有权:5 个 crate 在测试中不需要跨线程共享,
/// 直接持有比 Arc<Mutex<>> 更简单且无锁开销。GSOE 的 `evolve_once`
/// 与 `handle_*` 方法需要 `&mut self`,直接持有所有权可满足此约束。
pub struct Week6Pipeline {
    /// 共享事件总线(所有 crate 持有 clone,共享同一广播通道)
    pub bus: EventBus,
    /// L2 Memory:NMC 多模态编码器
    pub encoder: NmcEncoder,
    /// L7 Execution:SSRA 黏液式融合引擎
    pub fusion: SlimeFusionEngine,
    /// L10 Interface:CHTC 跨平台工具桥
    pub bridge: ChtcBridge,
    /// L5 Knowledge:GSOE 在线进化引擎(需要 &mut self 调用 evolve_once)
    pub evolution: GsoeEvolutionEngine,
    /// L3 Storage:LSCT 任务感知能力分层协调器
    pub coordinator: LsctCoordinator,
}

/// 装配 Week 6 E2E 测试管线 — 创建共享 EventBus 并注入 5 个 crate
///
/// # 流程
/// 1. 创建单一 EventBus 实例(所有 crate 共享其 clone)
/// 2. 用 `with_event_bus` 构造 5 个 crate 实例,注入 bus clone
/// 3. 注册 4 个默认 SSRA 模板(cap-text/desktop/defensive/default)
/// 4. 返回 Week6Pipeline,供测试用例驱动
///
/// # 错误
/// NmcEncoder 构造可能因配置校验失败返回 NmcError,此处用 anyhow 传播。
pub fn setup_week6_pipeline() -> anyhow::Result<Week6Pipeline> {
    let bus = EventBus::new();

    let encoder = NmcEncoder::with_event_bus(NmcConfig::default(), bus.clone())?;
    let fusion = SlimeFusionEngine::with_event_bus(SsraConfig::default(), bus.clone());
    let bridge = ChtcBridge::with_event_bus(ChtcConfig::default(), bus.clone());
    let evolution = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus.clone());
    let coordinator = LsctCoordinator::with_event_bus(LsctConfig::default(), bus.clone());

    let pipeline = Week6Pipeline {
        bus,
        encoder,
        fusion,
        bridge,
        evolution,
        coordinator,
    };

    register_default_templates(&pipeline.fusion);
    Ok(pipeline)
}

/// 注册 4 个默认 SSRA 适配器模板,供 E2E 主流程测试使用
///
/// 模板清单:
/// - `cap-text-fusion`:TopK 策略,权重 0.8(文本融合主模板)
/// - `cap-desktop-fusion`:WeightedAverage 策略,权重 0.6(桌面融合模板)
/// - `cap-defensive`:TopK 策略,权重 0.5(防御性适配基础模板)
/// - `cap-default`:MeanField 策略,权重 0.7(默认回退模板)
///
/// WHY 预注册:SSRA fuse 要求源适配器在 registry 中已注册,
/// 否则返回 TemplateNotFound。预注册保证主流程测试可直接调用 fuse。
pub fn register_default_templates(fusion: &SlimeFusionEngine) {
    let registry = fusion.registry();

    let templates = vec![
        (
            "cap-text-fusion",
            vec!["text"],
            FusionStrategy::TopK,
            0.8_f32,
        ),
        (
            "cap-desktop-fusion",
            vec!["desktop"],
            FusionStrategy::WeightedAverage,
            0.6_f32,
        ),
        ("cap-defensive", vec![], FusionStrategy::TopK, 0.5_f32),
        (
            "cap-default",
            vec!["text", "desktop"],
            FusionStrategy::MeanField,
            0.7_f32,
        ),
    ];

    for (cap_id, shape, strategy, weight) in templates {
        let shape_owned: Vec<String> = shape.into_iter().map(|s| s.to_string()).collect();
        let spec = TemplateSpec::new(cap_id, shape_owned, strategy);
        let template = precompile(spec).with_weight(weight);
        // 容量充足(默认 1024),注册不会失败;失败仅记录不中断
        let _ = registry.register(template);
    }
}

// ============================================================
// 辅助构造函数 — 生成测试用输入数据
// ============================================================

/// 构造 SSRA 融合请求(deadline=20ms,top_k=8,对齐 SsraConfig 默认值)
pub fn make_fusion_request(
    quest_id: &str,
    source_adapters: Vec<&str>,
    target: &str,
) -> FusionRequest {
    FusionRequest::new(
        quest_id,
        source_adapters.into_iter().map(String::from).collect(),
        target,
        20,
        8,
    )
}

/// 构造 VSCode 原生格式工具调用 JSON(`{ "command": ..., "args": ... }`)
pub fn make_vscode_raw(command: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "command": command, "args": args })
}

/// 构造 LSCT 任务负载画像
pub fn make_profile(task_type: TaskType, intensity: f32) -> TaskLoadProfile {
    TaskLoadProfile::new(task_type, intensity, 1)
}

// ============================================================
// 事件辅助函数 — 排空、计数、断言
// ============================================================

/// 排空事件接收器,收集最多 `max_count` 个事件(每个事件 50ms 超时)
///
/// WHY 50ms 单事件超时:平衡测试速度与可靠性。
/// 50ms 足以让 tokio runtime 调度后台任务投递事件,
/// 同时避免无事件时空等过久(50ms × N 最多等 N×50ms)。
pub async fn drain_events(rx: &mut EventReceiver, max_count: usize) -> Vec<NexusEvent> {
    let mut events = Vec::with_capacity(max_count);
    for _ in 0..max_count {
        match rx.recv_timeout(Duration::from_millis(50)).await {
            Ok(event) => events.push(event),
            Err(_) => break,
        }
    }
    events
}

/// 统计事件列表中指定类型的事件数量(按 `type_name()` 匹配)
pub fn count_event_by_type(events: &[NexusEvent], type_name: &str) -> usize {
    events.iter().filter(|e| e.type_name() == type_name).count()
}

/// 断言事件列表中包含至少一个指定类型的事件
pub fn assert_has_event(events: &[NexusEvent], type_name: &str) {
    let count = count_event_by_type(events, type_name);
    assert!(
        count > 0,
        "未找到期望的事件类型: {type_name}(实际收集到 {} 个事件)",
        events.len()
    );
}

/// 断言事件列表中包含至少 N 个指定类型的事件
#[allow(dead_code)]
pub fn assert_has_event_count(events: &[NexusEvent], type_name: &str, min_count: usize) {
    let count = count_event_by_type(events, type_name);
    assert!(
        count >= min_count,
        "事件类型 {type_name} 期望至少 {min_count} 个,实际 {count} 个",
    );
}

// ============================================================
// 基础验证测试 — 确保管线装配正确
// ============================================================

#[tokio::test]
async fn test_setup_creates_pipeline() {
    let pipeline = setup_week6_pipeline().expect("管线装配失败");

    // 验证 5 个 crate 实例均已构造
    assert_eq!(
        pipeline
            .encoder
            .perceive(PerceptionInput::Text("test".into()))
            .expect("NMC 编码失败")
            .dimension(),
        512,
        "CLV 维度必须为 512"
    );
    assert_eq!(pipeline.fusion.registry().len(), 4, "应已注册 4 个默认模板");
    // CHTC bridge 可接收 VSCode 格式
    let raw = make_vscode_raw("editor.open", serde_json::json!({}));
    let call = pipeline
        .bridge
        .receive(raw, IdeSource::vscode())
        .expect("CHTC receive 失败");
    assert_eq!(call.tool_id, "editor.open");
}

#[tokio::test]
async fn test_setup_registers_default_templates() {
    let pipeline = setup_week6_pipeline().expect("管线装配失败");
    let registry = pipeline.fusion.registry();

    // 4 个默认模板均应可查找到元数据
    for cap_id in &[
        "cap-text-fusion",
        "cap-desktop-fusion",
        "cap-defensive",
        "cap-default",
    ] {
        let meta = registry.get_template_meta(cap_id);
        assert!(meta.is_some(), "默认模板 {cap_id} 应已注册");
    }
    assert_eq!(registry.len(), 4, "注册表应恰好包含 4 个模板");
}

#[tokio::test]
async fn test_setup_event_bus_shared_across_crates() {
    // 验证 5 个 crate 共享同一 EventBus:在 bus 上订阅后,
    // 任一 crate 发布的事件都应被收到
    let pipeline = setup_week6_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // NMC perceive 会发布 NmcEncoded 事件(同步 publish_blocking)
    let _ = pipeline
        .encoder
        .perceive(PerceptionInput::Text("shared bus test".into()))
        .expect("NMC 编码失败");

    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "NmcEncoded");
}

#[tokio::test]
async fn test_setup_chtc_receives_and_executes() {
    let pipeline = setup_week6_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // VSCode 格式 receive + execute 全链路
    let raw = make_vscode_raw("editor.open", serde_json::json!({ "file": "/tmp/a.rs" }));
    let call = pipeline
        .bridge
        .receive(raw, IdeSource::vscode())
        .expect("CHTC receive 失败");
    let result = pipeline.bridge.execute(&call).expect("CHTC execute 失败");
    assert!(result.success, "VSCode execute 应成功");

    // receive 应发布 ChtcToolCallReceived 事件
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "ChtcToolCallReceived");
}

#[tokio::test]
async fn test_setup_ssra_fusion_completes() {
    let pipeline = setup_week6_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // 用预注册的 cap-text-fusion 模板执行融合
    let request = make_fusion_request("q-setup-1", vec!["cap-text-fusion"], "target-cap");
    let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");

    assert_eq!(result.selected_count, 1, "应选中 1 个模板");
    // TopK 策略取最大权重 0.8
    assert!(
        (result.confidence - 0.8).abs() < 1e-5,
        "TopK 置信度应为 0.8,实际 {}",
        result.confidence
    );

    // fuse 成功应发布 SsraFusionCompleted 事件
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "SsraFusionCompleted");
}

#[tokio::test]
async fn test_setup_gsoe_and_lsct_work() {
    let mut pipeline = setup_week6_pipeline().expect("管线装配失败");

    // GSOE:evolve_once 需要 &mut self,通过 &mut pipeline.evolution 访问
    let result = pipeline
        .evolution
        .evolve_once()
        .await
        .expect("GSOE 进化失败");
    assert_eq!(result.generation, 1, "首次进化世代应为 1");

    // LSCT:注册能力 + tick 决策(同步)
    pipeline
        .coordinator
        .register_capability("cap-gsoe", Tier::Warm);
    let profile = make_profile(TaskType::Compile, 0.9); // 高强度编译 → 目标 Hot
    let decisions = pipeline.coordinator.tick(&profile);
    // Warm → Hot 是相邻层级,应产生 Promote 决策
    assert!(
        decisions.iter().any(|d| d.is_promote()),
        "高强度编译任务应触发升温决策"
    );
}
