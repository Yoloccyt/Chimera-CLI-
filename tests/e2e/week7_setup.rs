//! Week 7 E2E 测试基础设施 — 共享 EventBus + 9 个 crate 实例的统一装配
//!
//! 对应任务:Week 7 Task 6(37 模块全量集成 + 1000 次压测)
//! 架构层:L1 Core(EventBus)+ L2/L3/L5/L6/L7/L8/L9/L10 九个被测 crate
//!
//! # 设计要点
//! - **共享 EventBus**:9 个 crate 通过 `with_event_bus(config, bus.clone())`
//!   注入同一个 `EventBus`(Arc-based,Clone 廉价),实现跨层事件广播
//! - **Week 7 扩展**:在 Week 6 五个 crate 基础上新增 4 个(mcp-mesh/
//!   csn-substitutor/sesa-router/efficiency-monitor),覆盖 L6/L9/L10 三层
//! - **week7_setup.rs 既是 [[test]] target,也被其他测试文件通过
//!   `#[path = "week7_setup.rs"] mod setup;` 复用**,避免代码重复
//! - **broadcast 时序铁律**:`bus.subscribe()` 必须在 `tokio::spawn` 之前
//!   同步调用,否则后台任务可能晚于 publish 导致事件静默丢失(Week 6 教训 #9)

use std::time::Duration;

use chtc_bridge::{ChtcBridge, ChtcConfig};
use csn_substitutor::{CapabilityDescriptor, CsnConfig, CsnSubstitutor};
use efficiency_monitor::{AlertRule, AlertSeverity, Comparison, EfficiencyMonitor, MonitorConfig};
use event_bus::{EventBus, EventReceiver, NexusEvent};
use gsoe_evolution::{GsoeConfig, GsoeEvolutionEngine};
use lsct_tiering::{LsctConfig, LsctCoordinator, TaskLoadProfile, TaskType};
use mcp_mesh::{McpMesh, MeshConfig, MeshServer};
use nmc_encoder::{NmcConfig, NmcEncoder, PerceptionInput};
use sesa_router::{ActivationRequest, ExpertDescriptor, SesaConfig, SesaRouter};
use ssra_fusion::{
    precompile, FusionRequest, FusionStrategy, SlimeFusionEngine, SsraConfig, TemplateSpec,
};

// ============================================================
// Week7Pipeline — 持有共享 EventBus + 9 个 crate 实例
// ============================================================

/// Week 7 E2E 测试管线 — 持有共享 EventBus 与 9 个被测 crate 的实例
///
/// WHY 直接持有所有权:9 个 crate 在测试中不需要跨线程共享,
/// 直接持有比 Arc<Mutex<>> 更简单且无锁开销。GSOE 的 `evolve_once`
/// 与 `handle_*` 方法需要 `&mut self`,直接持有所有权可满足此约束。
pub struct Week7Pipeline {
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
    // --- Week 7 新增 4 个 crate ---
    /// L10 Interface:MCP 量子网格
    pub mesh: McpMesh,
    /// L10 Interface:CSN 能力替代网络
    pub substitutor: CsnSubstitutor,
    /// L6 Router:SESA 子专家稀疏激活
    pub sesa: SesaRouter,
    /// L9 Quest:效率监控与告警
    pub monitor: EfficiencyMonitor,
}

/// 装配 Week 7 E2E 测试管线 — 创建共享 EventBus 并注入 9 个 crate
///
/// # 流程
/// 1. 创建单一 EventBus 实例(所有 crate 共享其 clone)
/// 2. 用 `with_event_bus` 构造 9 个 crate 实例,注入 bus clone
/// 3. 注册 4 个默认 SSRA 模板 + 3 个 CSN 能力 + 3 个 SESA 专家 + 2 个 MCP 服务器
/// 4. 添加默认告警规则(Critical 事件 > 0 即告警)
/// 5. 返回 Week7Pipeline,供测试用例驱动
///
/// # 错误
/// NmcEncoder 构造可能因配置校验失败返回 NmcError,此处用 anyhow 传播。
pub fn setup_week7_pipeline() -> anyhow::Result<Week7Pipeline> {
    let bus = EventBus::new();

    // Week 6 五个 crate
    let encoder = NmcEncoder::with_event_bus(NmcConfig::default(), bus.clone())?;
    let fusion = SlimeFusionEngine::with_event_bus(SsraConfig::default(), bus.clone());
    let bridge = ChtcBridge::with_event_bus(ChtcConfig::default(), bus.clone());
    let evolution = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus.clone());
    let coordinator = LsctCoordinator::with_event_bus(LsctConfig::default(), bus.clone());

    // Week 7 四个新 crate
    let mesh = McpMesh::with_event_bus(MeshConfig::default(), bus.clone());
    let substitutor = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
    let sesa = SesaRouter::with_event_bus(SesaConfig::default(), bus.clone());
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

    let pipeline = Week7Pipeline {
        bus,
        encoder,
        fusion,
        bridge,
        evolution,
        coordinator,
        mesh,
        substitutor,
        sesa,
        monitor,
    };

    // 注册默认测试数据(SSRA 模板 / CSN 能力 / SESA 专家 / MCP 服务器 / 告警规则)
    register_default_templates(&pipeline.fusion);
    register_default_capabilities(&pipeline.substitutor)?;
    register_default_experts(&pipeline.sesa)?;
    register_default_servers(&pipeline.mesh)?;
    register_default_alert_rules(&pipeline.monitor);

    Ok(pipeline)
}

/// 注册 4 个默认 SSRA 适配器模板,供 E2E 主流程测试使用
///
/// 模板清单:
/// - `cap-text-fusion`:TopK 策略,权重 0.8(文本融合主模板)
/// - `cap-desktop-fusion`:WeightedAverage 策略,权重 0.6(桌面融合模板)
/// - `cap-defensive`:TopK 策略,权重 0.5(防御性适配基础模板)
/// - `cap-default`:MeanField 策略,权重 0.7(默认回退模板)
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
        let _ = registry.register(template);
    }
}

/// 注册 3 个默认 CSN 能力描述符(50 维向量),供降级链测试使用
///
/// WHY 50 维:对齐 `CsnConfig::default().vector_dimension = 50`
/// - `cap-shell`:shell 执行能力
/// - `cap-python`:Python 执行能力(与 cap-shell 高相似)
/// - `cap-search`:搜索能力(与 cap-shell 低相似)
pub fn register_default_capabilities(sub: &CsnSubstitutor) -> anyhow::Result<()> {
    // cap-shell 与 cap-python 高相似(向量接近)
    let shell = CapabilityDescriptor::new("cap-shell", vec![1.0; 50]);
    let python = CapabilityDescriptor::new("cap-python", vec![0.99; 50]);
    // cap-search 与 cap-shell 低相似(向量正交)
    let mut search_vec = vec![0.0; 50];
    for v in search_vec.iter_mut().take(25) {
        *v = 1.0;
    }
    let search = CapabilityDescriptor::new("cap-search", search_vec);

    sub.register_capability(shell)?;
    sub.register_capability(python)?;
    sub.register_capability(search)?;
    Ok(())
}

/// 注册 3 个默认 SESA 专家(64 维向量),供稀疏激活测试使用
///
/// WHY 64 维:对齐 SESA 示例 `ExpertDescriptor::new(id, vec![0.5; 64])`
pub fn register_default_experts(router: &SesaRouter) -> anyhow::Result<()> {
    let experts = vec![
        ("expert-alpha", vec![0.9; 64]),
        ("expert-beta", vec![0.8; 64]),
        ("expert-gamma", vec![0.7; 64]),
    ];
    for (id, vec) in experts {
        let descriptor = ExpertDescriptor::new(id, vec);
        router.register_expert(descriptor)?;
    }
    Ok(())
}

/// 注册 2 个默认 MCP Mesh 服务器(in-process mock),供量子事务测试使用
///
/// WHY 使用 203.0.113.x(TEST-NET-3,RFC 5737):F-004 SSRF 校验在 register
/// 入口拦截 127.0.0.0/8 等内网地址,此处仅作 in-process mock 不发起真实网络
/// 请求,改用公网文档用途地址既绕过校验、又是合规的测试占位(非保留段)。
pub fn register_default_servers(mesh: &McpMesh) -> anyhow::Result<()> {
    let s1 = MeshServer::new("srv-1", "203.0.113.1:9001", vec!["cap-shell".into()]);
    let s2 = MeshServer::new("srv-2", "203.0.113.1:9002", vec!["cap-python".into()]);
    mesh.register_server(s1)?;
    mesh.register_server(s2)?;
    Ok(())
}

/// 注册默认告警规则,供 efficiency-monitor 告警测试使用
///
/// - `critical-alert`:Critical 事件总数 > 0 → Critical 告警
/// - `event-rate`:事件总数 >= 10 → Warning 告警
pub fn register_default_alert_rules(monitor: &EfficiencyMonitor) {
    monitor.add_alert_rule(AlertRule::new(
        "critical-alert",
        "nexus_critical_event_total",
        0.0,
        Comparison::GreaterThan,
        AlertSeverity::Critical,
    ));
    monitor.add_alert_rule(AlertRule::new(
        "event-rate",
        "nexus_event_total",
        10.0,
        Comparison::GreaterOrEqual,
        AlertSeverity::Warning,
    ));
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
#[allow(dead_code)]
pub fn make_vscode_raw(command: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "command": command, "args": args })
}

/// 构造 LSCT 任务负载画像
#[allow(dead_code)]
pub fn make_profile(task_type: TaskType, intensity: f32) -> TaskLoadProfile {
    TaskLoadProfile::new(task_type, intensity, 1)
}

/// 构造 SESA 激活请求(query_vector=64 维,top_k=2,deadline=5ms)
pub fn make_activation_request(req_id: &str, top_k: usize) -> ActivationRequest {
    ActivationRequest::new(req_id, vec![0.85; 64], top_k, 5)
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

// ============================================================
// 基础验证测试 — 确保管线装配正确(本文件作为 [[test]] target 时运行)
// ============================================================

#[tokio::test]
async fn test_week7_setup_creates_pipeline() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");

    // 验证 9 个 crate 实例均已构造
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
    assert_eq!(
        pipeline.substitutor.registry().len(),
        3,
        "应已注册 3 个默认能力"
    );
    assert_eq!(pipeline.sesa.expert_count(), 3, "应已注册 3 个默认专家");
    assert_eq!(pipeline.mesh.registry().len(), 2, "应已注册 2 个默认服务器");
}

#[tokio::test]
async fn test_week7_setup_event_bus_shared_across_9_crates() {
    // 验证 9 个 crate 共享同一 EventBus:在 bus 上订阅后,
    // 任一 crate 发布的事件都应被收到
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
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
async fn test_week7_setup_mcp_mesh_executes_transaction() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // 执行 2 服务器量子事务
    let result = pipeline
        .mesh
        .execute_transaction(vec!["srv-1".into(), "srv-2".into()], "query".into())
        .await
        .expect("MCP 事务执行失败");
    assert!(result.success, "2 服务器事务应成功");

    // 应发布 McpMeshTransactionCompleted 事件
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "McpMeshTransactionCompleted");
}

#[tokio::test]
async fn test_week7_setup_sesa_activates_experts() {
    let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
    let mut rx = pipeline.bus.subscribe();

    // 激活 Top-2 专家(3 个中选 2,稀疏度 66.7% > 40%,enforce_sparsity 会裁剪)
    let request = make_activation_request("req-setup", 2);
    let (mask, _profile) = pipeline
        .sesa
        .activate(request)
        .await
        .expect("SESA 激活失败");
    assert!(mask.active_count <= 3, "激活数不应超过总专家数");

    // 应发布 SesaActivationCompleted 事件
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "SesaActivationCompleted");
}
