//! Week 8 Task 6 SubTask 6.4 — Week 8 最终验收测试
//!
//! 对应任务:Week 8 Task 6.4(8 周验收项核对)
//! 架构层:L1-L10 全栈可用性验证
//!
//! # 测试用例(8 周验收项)
//! 1. test_week1_infrastructure:EventBus / SecCore / Decay / QEEP 可用(L1/L4)
//! 2. test_week2_quest_repo_router:Quest Engine / Repo Wiki / Model Router 可用(L9/L5/L1)
//! 3. test_week3_memory_storage_router:MLC / HCW / CMT / OSA / KVBSR 可用(L2/L3/L6)
//! 4. test_week4_execution_router:GEA / GQEP / PVL / MTPE / SCC 可用(L6/L7/L3)
//! 5. test_week5_parliament_security:Parliament / ASA / AHIRT / TTG / DECB 可用(L8/L4)
//! 6. test_week6_multimodal_evolution:SSRA / LSCT / GSOE / NMC / CHTC 可用(L7/L3/L5/L2/L10)
//! 7. test_week7_mesh_monitoring:MCP Mesh / CSN / SESA / Efficiency Monitor 可用(L10/L6/L9)
//! 8. test_week8_production:scc-cache WAL / OWASP / Dockerfile / CI 配置存在(L3/L4/L10)
//!
//! # 架构红线对齐
//! - `#![forbid(unsafe_code)]` 红线
//! - 单运行时:用 `tokio::runtime::Runtime::new()`
//! - 验收语义:每个测试验证对应 Week 的关键 crate "可用"(实例化 + 最小操作)

#![forbid(unsafe_code)]

#[path = "week7_setup.rs"]
#[allow(dead_code)]
mod setup;

use std::path::Path;

use event_bus::EventBus;
use scc_cache::WalTrait;
use tempfile::TempDir;

// ============================================================
// Week 1 验收:EventBus / SecCore / Decay / QEEP 可用(L1/L4)
// ============================================================

#[test]
fn test_week1_infrastructure() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        // L1 Core:EventBus 广播+订阅
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let event = event_bus::NexusEvent::CacheHit {
            metadata: event_bus::EventMetadata::new("week1-test"),
            cache_key: "w1-key".into(),
        };
        bus.publish(event).await.expect("EventBus publish 失败");
        let received = rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .await
            .expect("EventBus subscribe 失败");
        assert_eq!(received.type_name(), "CacheHit", "EventBus 应广播 CacheHit");

        // L4 Security:SecCore 静态分析(命令策略)
        let policy = seccore::CommandPolicy::default_secure();
        let safe_cmd = seccore::Command::new("echo").arg("hello");
        assert!(
            seccore::validate_command(&safe_cmd, &policy).is_ok(),
            "SecCore 应放行安全命令"
        );

        // L4 Security:DecayEngine 能力衰减
        let decay = decay_engine::DecayEngine::new(decay_engine::DecayConfig::default());
        // DecayEngine 实例化成功即证明 L4 Security 衰减模型可用
        let _ = decay;

        // L4 Security:QEEP 零孤儿协议
        let qeep = qeep_protocol::QeepProtocol::new(qeep_protocol::DEFAULT_TIMEOUT);
        let result = qeep
            .entangle(async { Ok::<_, qeep_protocol::QeepError>(1_u32) })
            .await
            .expect("QEEP entangle 失败");
        assert_eq!(result, 1, "QEEP 应正常返回 future 结果");
        assert_eq!(qeep.orphan_count(), 0, "QEEP 不应产生孤儿调用");
    });
}

// ============================================================
// Week 2 验收:Quest Engine / Repo Wiki / Model Router 可用(L9/L5/L1)
// ============================================================

#[test]
fn test_week2_quest_repo_router() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let bus = EventBus::new();

        // L9 Quest:Quest Engine 任务分解
        let engine = quest_engine::QuestEngine::new(bus.clone());
        let intent = nexus_core::UserIntent {
            intent_id: "i-w2".into(),
            raw_text: "分析需求。设计方案。".into(),
            multimodal_inputs: vec![nexus_core::MultimodalInput::Text("test".into())],
            risk_level: 20,
        };
        let quest = engine.create_quest(intent).await.expect("Quest 创建失败");
        assert_eq!(quest.tasks.len(), 2, "Quest Engine 应分解为 2 个 Task");

        // L5 Knowledge:Repo Wiki 持久化
        let tmp = TempDir::new().expect("TempDir 创建失败");
        let store =
            repo_wiki::WikiStore::open(&tmp.path().join("w2.db")).expect("WikiStore 打开失败");
        assert_eq!(
            store.count().await.expect("WikiStore count 失败"),
            0,
            "WikiStore 初始应为空"
        );

        // L1 Core:Model Router 路由
        let registry =
            model_router::ModelRegistry::from_config(&model_router::RouterConfig::default());
        let router = model_router::ModelRouter::new(registry, bus);
        let req = model_router::RoutingRequest {
            quest_id: "q-w2".into(),
            intent: nexus_core::UserIntent {
                intent_id: "i-w2-r".into(),
                raw_text: "test".into(),
                multimodal_inputs: vec![],
                risk_level: 10,
            },
            estimated_tokens: 100,
            strategy: model_router::RoutingStrategy::Lite,
        };
        let decision = router.route(req).await.expect("Model Router 路由失败");
        assert!(!decision.model_id.is_empty(), "Model Router 应选中非空模型");
    });
}

// ============================================================
// Week 3 验收:MLC / HCW / CMT / OSA / KVBSR 可用(L2/L3/L6)
// ============================================================

#[test]
fn test_week3_memory_storage_router() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let bus = EventBus::new();

        // L2 Memory:MLC 四级记忆引擎(用 in_memory 避免 SQLite 文件锁)
        // WHY _mlc:仅证明实例化成功,后续不使用该绑定,前缀下划线避免 unused 警告
        let _mlc = mlc_engine::MlcEngine::new_in_memory(bus.clone()).expect("MLC Engine 创建失败");
        // MLC 实例化成功即证明 L2 可用

        // L2 Memory:HCW 分层上下文窗口
        let hcw = hcw_window::HcwWindow::with_default_config(bus.clone()).expect("HCW 创建失败");
        let tier = hcw
            .select_window(0.6)
            .await
            .expect("HCW select_window 失败");
        // 0.6 复杂度应选中 L2(Complex 档位)
        assert_eq!(tier, hcw_window::WindowTier::L2, "HCW 应选中 L2 窗口");

        // L3 Storage:CMT 能力分层(Tier 类型复用)
        let _tier = cmt_tiering::Tier::Hot;
        // CMT 类型可用即证明 L3 Storage 可用

        // L6 Router:OSA 五维度稀疏协调器
        let coord = osa_coordinator::OmniSparseCoordinator::new(bus.clone());
        let profile =
            osa_coordinator::TaskProfile::new("w3-task", 0.5, osa_coordinator::RiskLevel::Medium);
        let masks = coord
            .compute_all_masks(&profile)
            .await
            .expect("OSA 掩码计算失败");
        assert!(!masks.mask_hash().is_empty(), "OSA 应计算非空 mask_hash");

        // L6 Router:KVBSR 两级块语义路由器
        let _router = kvbsr_router::KVBlockSemanticRouter::new(bus);
        // KVBSR 实例化成功即证明 L6 Router 可用
    });
}

// ============================================================
// Week 4 验收:GEA / GQEP / PVL / MTPE / SCC 可用(L6/L7/L3)
// ============================================================

#[test]
fn test_week4_execution_router() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let bus = EventBus::new();

        // L6 Router:GEA 门控专家激活器
        let gea =
            gea_activator::GeaActivator::new(gea_activator::GeaConfig::default(), bus.clone())
                .expect("GEA 创建失败");
        // GEA 实例化成功即证明 L6 Router 可用

        // L6 Router:GQEP 聚集执行器
        let gqep =
            gqep_executor::GqepExecutor::new(gqep_executor::GqepConfig::default(), bus.clone());
        // GQEP 实例化成功即证明 L6 Router 聚集执行可用

        // L7 Execution:PVL 生产验证闭环
        let pvl_config = pvl_layer::PvlConfig::default();
        let _producer = pvl_layer::Producer::new(pvl_config.clone(), bus.clone());
        let _verifier = pvl_layer::Verifier::new(pvl_config.clone(), bus.clone());
        let _feedback = pvl_layer::FeedbackChannel::new(pvl_config, bus.clone());
        // PVL 三组件实例化成功即证明 L7 Execution 可用

        // L7 Execution:MTPE 多步预测执行器
        let mtpe =
            mtpe_executor::MtpeExecutor::new(mtpe_executor::MtpeConfig::default(), bus.clone());
        // MTPE 实例化成功即证明 L7 Execution 多步预测可用

        // L3 Storage:SCC 推测上下文缓存
        let scc = scc_cache::SccCache::new(scc_cache::SccConfig::default(), bus);
        let entry = scc_cache::ContextEntry::new("ctx-w4", "content-w4");
        scc.insert(entry);
        // SCC 实例化 + 插入成功即证明 L3 Storage 可用

        // 引用变量避免未使用警告
        let _ = (&gea, &gqep, &mtpe);
    });
}

// ============================================================
// Week 5 验收:Parliament / ASA / AHIRT / TTG / DECB 可用(L8/L4)
// ============================================================

#[test]
fn test_week5_parliament_security() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let bus = EventBus::new();

        // L8 Parliament:议会实例化(含 5 角色注册表)
        let parliament =
            parliament::Parliament::new(parliament::ParliamentConfig::default(), bus.clone());
        // Parliament 实例化成功即证明 L8 可用

        // L8 Parliament:ASA 审计(SecCore 内的 AsaAuditor,with_event_bus 注入 bus)
        let asa = seccore::AsaAuditor::with_event_bus(seccore::AsaConfig::default(), bus.clone());
        // ASA 实例化成功即证明 L4 Security 审计可用
        let _ = asa;

        // L8 Parliament:AHIRT 红队(parliament 内的 AhirtRedTeam)
        // WHY ProbePayloadLibrary::new():AHIRT 需要探测载荷库,默认库含 100 个载荷
        let library = parliament::ProbePayloadLibrary::new();
        let ahirt = parliament::AhirtRedTeam::with_event_bus(library, bus.clone());
        // AHIRT 实例化成功即证明反黑客红队可用
        let _ = ahirt;

        // L9 Quest:TTG 思考切换治理(quest_engine 内的 TtgGovernor)
        let ttg = quest_engine::TtgGovernor::new(quest_engine::TtgConfig::default());
        // TTG 实例化成功即证明思考切换治理可用
        let _ = ttg;

        // L8 Parliament:DECB 预算治理器
        let decb = decb_governor::DecbGovernor::new(decb_governor::DecbConfig::default());
        // DECB 实例化成功即证明预算治理可用
        let _ = (parliament, decb);
    });
}

// ============================================================
// Week 6 验收:SSRA / LSCT / GSOE / NMC / CHTC 可用(L7/L3/L5/L2/L10)
// ============================================================

#[test]
fn test_week6_multimodal_evolution() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        // 复用 Week7 管线(含 NMC/SSRA/GSOE/CHTC/LSCT 5 个 crate)
        // WHY mut:GSOE 的 handle_consensus_reached/evolve_once 需要可变借用
        let mut pipeline = setup::setup_week7_pipeline().expect("Week7 管线装配失败");

        // L2 Memory:NMC 多模态编码
        let clv = pipeline
            .encoder
            .perceive(nmc_encoder::PerceptionInput::Text(
                "week6 acceptance".into(),
            ))
            .expect("NMC 编码失败");
        assert_eq!(clv.dimension(), 512, "NMC CLV 维度必须为 512");

        // L7 Execution:SSRA 黏液式融合
        let request = setup::make_fusion_request("q-w6", vec!["cap-text-fusion"], "target");
        let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");
        assert!(result.confidence > 0.0, "SSRA 融合置信度应大于 0");

        // L5 Knowledge:GSOE 在线进化
        pipeline.evolution.handle_consensus_reached();
        let evo = pipeline
            .evolution
            .evolve_once()
            .await
            .expect("GSOE 进化失败");
        assert_eq!(evo.generation, 1, "GSOE 首次进化世代应为 1");

        // L10 Interface:CHTC 跨平台工具桥
        let raw = setup::make_vscode_raw("editor.open", serde_json::json!({ "file": "w6.rs" }));
        let call = pipeline
            .bridge
            .receive(raw, chtc_bridge::IdeSource::vscode())
            .expect("CHTC receive 失败");
        let exec = pipeline.bridge.execute(&call).expect("CHTC execute 失败");
        assert!(exec.success, "CHTC execute 应成功");

        // L3 Storage:LSCT 任务感知能力分层(注册能力验证)
        pipeline
            .coordinator
            .register_capability("cap-text-fusion", cmt_tiering::Tier::Warm);
        // LSCT 注册成功即证明 L3 Storage 可用
    });
}

// ============================================================
// Week 7 验收:MCP Mesh / CSN / SESA / Efficiency Monitor 可用(L10/L6/L9)
// ============================================================

#[test]
fn test_week7_mesh_monitoring() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let pipeline = setup::setup_week7_pipeline().expect("Week7 管线装配失败");

        // L10 Interface:MCP Mesh 量子事务
        let tx = pipeline
            .mesh
            .execute_transaction(vec!["srv-1".into(), "srv-2".into()], "w7-query".into())
            .await
            .expect("MCP 事务失败");
        assert!(tx.success, "MCP Mesh 2 服务器事务应成功");

        // L10 Interface:CSN 能力替代查询
        let candidates = pipeline.substitutor.find_substitutes("cap-shell", 3);
        // find_substitutes 返回 Vec,非 Result;cap-shell 与 cap-python 高相似
        assert!(!candidates.is_empty(), "CSN 应返回 cap-shell 的替代候选");

        // L6 Router:SESA 子专家稀疏激活
        let req = setup::make_activation_request("w7-req", 2);
        let (mask, _profile) = pipeline.sesa.activate(req).await.expect("SESA 激活失败");
        assert!(mask.active_count <= 3, "SESA 激活数不应超过总专家数");

        // L9 Quest:Efficiency Monitor 告警规则注册(验证可用)
        // WHY 不触发告警:告警需要 Prometheus 指标累积,此处仅验证 monitor 实例可用
        let _monitor = &pipeline.monitor;
        // Efficiency Monitor 实例化 + 告警规则注册成功即证明 L9 Quest 监控可用
    });
}

// ============================================================
// Week 8 验收:scc-cache WAL / OWASP / Dockerfile / CI 配置存在(L3/L4/L10)
// ============================================================

#[test]
fn test_week8_production() {
    // 1. L3 Storage:scc-cache WAL 接口可用
    let wal = scc_cache::InMemoryWal::new();
    // WHY WalEntry::new:WalEntry 不允许 struct literal 构造(timestamp 自动生成),
    // 必须用 ::new(entry_id, operation, context_id, payload) 构造
    let entry = scc_cache::WalEntry::new(
        "wal-w8-1",
        scc_cache::WalOperation::Insert,
        scc_cache::ContextId::new("ctx-wal"),
        vec![],
    );
    // WHY &entry:WalTrait::write_ahead_log 接受 &WalEntry(借用,不转移所有权)
    wal.write_ahead_log(&entry).expect("WAL 写入失败");
    // WAL 写入成功即证明 L3 Storage 持久化接口可用
    let _ = wal;

    // 2. L4 Security:OWASP Top 10 渗透测试文件存在
    let owasp_path = Path::new("tests/security/owasp_top10.rs");
    assert!(
        owasp_path.exists(),
        "OWASP Top 10 渗透测试文件应存在: tests/security/owasp_top10.rs"
    );

    // 3. L10 Interface:Dockerfile 存在(跨平台发布)
    let dockerfile_path = Path::new("Dockerfile");
    assert!(dockerfile_path.exists(), "Dockerfile 应存在(跨平台发布)");

    // 4. L10 Interface:CI/CD 配置存在
    let ci_path = Path::new(".github/workflows/release.yml");
    assert!(
        ci_path.exists(),
        "CI/CD 配置应存在: .github/workflows/release.yml"
    );

    // 5. 验证 OWASP 测试文件已注册为 [[test]] target
    // WHY 通过 Cargo.toml 内容检查:owasp_top10 test target 在根 Cargo.toml 注册
    let cargo_toml = std::fs::read_to_string("Cargo.toml").expect("读取 Cargo.toml 失败");
    assert!(
        cargo_toml.contains("name = \"owasp_top10\""),
        "Cargo.toml 应注册 owasp_top10 test target"
    );
    assert!(
        cargo_toml.contains("path = \"tests/security/owasp_top10.rs\""),
        "Cargo.toml 应指向 owasp_top10 测试路径"
    );
}
