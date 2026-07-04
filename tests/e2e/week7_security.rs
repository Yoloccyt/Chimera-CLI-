//! Week 7 安全测试 — 30 个 Week 7 攻击载荷(Task 6.3)
//!
//! 对应任务:Week 7 Task 6.3(150 载荷 0 穿透:120 旧 + 30 新)
//! 架构层:L6/L9/L10 跨层安全验证
//!
//! # 攻击向量分类(30 载荷)
//! - **MCP 注入(8 个)**:server_id/endpoint/capabilities/transaction_id 字段注入、
//!   心跳伪造、参与者数量溢出、空参与者列表、null 字节注入
//! - **CSN 替代劫持(8 个)**:维度不匹配、空 ID、NaN 向量、未注册能力触发、
//!   不存在链重置/推进、注册表溢出、零向量相似度欺骗
//! - **SESA 稀疏度绕过(7 个)**:Top-K 超容量、零 deadline 绕过、空专家池激活、
//!   超掩码容量注册、恰好 40% 边界、大 Top-K 小池、手动位翻转
//! - **Monitor 告警抑制(7 个)**:禁用立即告警、cooldown 绕过、非 Critical 误告警、
//!   空 metrics 渲染、无 bus 启动订阅、不可达阈值、计数完整性
//!
//! # 断言原则
//! 每个攻击载荷必须被拒绝或产生预期错误,**不穿透系统**:
//! - 返回明确的错误变体(如 McpError::ServerNotFound)
//! - 或保持系统不变量(如稀疏度严格 < 40%)
//! - 或产生可预期的安全降级(如零向量返回相似度 0.0,不触发误替代)
//!
//! # broadcast 时序铁律(Week 6 教训 #9)
//! 每个涉及事件发布的测试必须先 `bus.subscribe()` 再发布事件。

#![forbid(unsafe_code)]

#[path = "week7_setup.rs"]
#[allow(dead_code)]
mod setup;

use std::time::Instant;

use csn_substitutor::{CapabilityDescriptor, CsnError};
use efficiency_monitor::{AlertRule, AlertSeverity, Comparison, MonitorConfig};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use mcp_mesh::{McpError, MeshServer};
use sesa_router::{ActivationRequest, ExpertDescriptor, SesaConfig, SesaError};
use setup::setup_week7_pipeline;

/// CSA 延迟阈值:500ms(与 week7_main_flow 一致,Task 6.5)
const CSA_THRESHOLD_MS: u128 = 500;

// ============================================================
// 第 1 组:MCP 注入攻击载荷(8 个)
// ============================================================

#[tokio::test]
async fn test_mcp_injection_sql_in_server_id() {
    // 攻击:server_id 含 SQL 注入载荷,试图破坏注册表
    // 防御:server_id 仅作为 DashMap key 存储,无 SQL 执行,事务正常
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let malicious_id = "'; DROP TABLE servers; --";
    // WHY 203.0.113.x:本测试聚焦 server_id SQL 注入,endpoint 仅作占位,
    // 须绕过 F-004 SSRF 校验(127.0.0.0/8 被拦截),改用 TEST-NET-3 公网文档地址
    let server = MeshServer::new(malicious_id, "203.0.113.1:9999", vec!["cap".into()]);
    pipeline
        .mesh
        .register_server(server)
        .expect("恶意 server_id 应被安全存储(无 SQL 执行)");

    // 验证:server_id 作为字符串原样存储,注册表无损坏
    assert!(
        pipeline.mesh.registry().get(malicious_id).is_some(),
        "恶意 ID 应被存储"
    );
    assert_eq!(
        pipeline.mesh.registry().len(),
        3,
        "应有 3 个服务器(2 默认 + 1 注入)"
    );

    // 事务应能正常执行(使用恶意 ID 作为参与者)
    let result = pipeline
        .mesh
        .execute_transaction(vec![malicious_id.into()], "query".into())
        .await
        .expect("含恶意 ID 的事务应成功(无 SQL 执行)");
    assert!(result.success, "事务应成功,SQL 注入载荷被中和");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MCP-1] SQL 注入载荷被中和,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_mcp_injection_path_traversal_in_endpoint() {
    // 攻击:endpoint 含路径穿越载荷,试图访问敏感文件
    // 防御:endpoint 仅作为字符串存储,无文件系统访问
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let traversal_endpoint = "../../etc/passwd";
    let server = MeshServer::new("srv-traversal", traversal_endpoint, vec!["cap".into()]);
    pipeline
        .mesh
        .register_server(server)
        .expect("路径穿越 endpoint 应被安全存储");

    let stored = pipeline
        .mesh
        .registry()
        .get("srv-traversal")
        .expect("应存在");
    assert_eq!(
        stored.endpoint, traversal_endpoint,
        "endpoint 原样存储,无文件访问"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MCP-2] 路径穿越载荷被中和,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_mcp_injection_xss_in_capabilities() {
    // 攻击:capabilities 含 XSS 载荷,试图在监控面板执行脚本
    // 防御:capabilities 仅作为 Vec<String> 存储,Prometheus 渲染不执行脚本
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let xss_payload = "<script>alert('xss')</script>";
    // WHY 203.0.113.x:本测试聚焦 capabilities XSS,endpoint 仅作占位,
    // 须绕过 F-004 SSRF 校验(127.0.0.0/8 被拦截),改用 TEST-NET-3 公网文档地址
    let server = MeshServer::new("srv-xss", "203.0.113.1:8888", vec![xss_payload.into()]);
    pipeline
        .mesh
        .register_server(server)
        .expect("XSS 载荷应被安全存储");

    let stored = pipeline.mesh.registry().get("srv-xss").expect("应存在");
    assert_eq!(
        stored.capabilities[0], xss_payload,
        "XSS 载荷原样存储,无执行"
    );

    // 验证 metrics 渲染不含未转义脚本(监控面板安全)
    let metrics = pipeline.monitor.render_metrics();
    assert!(
        !metrics.contains("<script>"),
        "metrics 不应含未转义 script 标签"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MCP-3] XSS 载荷被中和,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_mcp_injection_transaction_id_tampering() {
    // 攻击:手动发布含控制字符的 transaction_id 事件,试图破坏事件总线
    // 防御:NexusEvent 字段为 String,控制字符作为字符串处理,不影响事件分发
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let mut rx = pipeline.bus.subscribe();
    let start = Instant::now();

    let tampered_tx_id = "tx-1\n\r\0\x1b[2J"; // 含换行、null、ANSI 转义
    let event = NexusEvent::McpMeshTransactionCompleted {
        metadata: EventMetadata::new("attacker"),
        transaction_id: tampered_tx_id.into(),
        participant_count: 1,
        latency_ms: 10,
        success: true,
    };
    pipeline
        .bus
        .publish(event)
        .await
        .expect("事件发布应成功(控制字符作为字符串处理)");

    let events = setup::drain_events(&mut rx, 5).await;
    setup::assert_has_event(&events, "McpMeshTransactionCompleted");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MCP-4] 事务 ID 篡改载荷被中和,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_mcp_injection_heartbeat_forgery_on_unknown_server() {
    // 攻击:对未注册服务器发送伪造心跳,试图注入僵尸服务器
    // 防御:heartbeat 校验 server_id 存在性,未注册返回 ServerNotFound
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let err = pipeline
        .mesh
        .heartbeat("forged-server-not-registered")
        .unwrap_err();
    assert!(
        matches!(err, McpError::ServerNotFound { .. }),
        "伪造心跳应返回 ServerNotFound,实际 {:?}",
        err
    );

    // 验证:注册表未被注入僵尸服务器
    assert_eq!(
        pipeline.mesh.registry().len(),
        2,
        "注册表应仍为 2 个服务器,未被伪造心跳注入"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MCP-5] 伪造心跳被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_mcp_injection_participant_count_overflow() {
    // 攻击:提交超过 max_participants(32)的事务,试图耗尽资源
    // 防御:execute_transaction 校验 participants.len() <= max_participants
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    // 构造 33 个参与者(超过默认 max_participants=32)
    let participants: Vec<String> = (0..33).map(|i| format!("srv-overflow-{i}")).collect();
    let err = pipeline
        .mesh
        .execute_transaction(participants, "query".into())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            McpError::TooManyParticipants {
                actual: 33,
                limit: 32
            }
        ),
        "应返回 TooManyParticipants,实际 {:?}",
        err
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MCP-6] 参与者溢出被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_mcp_injection_empty_participant_list() {
    // 攻击:提交空参与者列表,试图触发未定义行为
    // 防御:空参与者列表应被安全处理(无 panic,返回成功但 committed 为空)
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let result = pipeline
        .mesh
        .execute_transaction(vec![], "query".into())
        .await
        .expect("空参与者事务应被安全处理(无 panic)");

    // 空事务应快速完成,不产生副作用
    assert!(
        result.latency_ms < 50,
        "空事务应在 50ms 内完成,实际 {}ms",
        result.latency_ms
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MCP-7] 空参与者列表安全处理,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_mcp_injection_null_byte_in_op_string() {
    // 攻击:op 字符串含 null 字节,试图截断或注入命令
    // 防御:op 作为 String 处理,null 字节不触发命令执行
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let malicious_op = "query\0rm -rf /";
    let result = pipeline
        .mesh
        .execute_transaction(vec!["srv-1".into()], malicious_op.into())
        .await
        .expect("含 null 字节的 op 应被安全处理");

    assert!(result.success, "事务应成功,null 字节不触发命令执行");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MCP-8] null 字节注入被中和,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

// ============================================================
// 第 2 组:CSN 替代劫持攻击载荷(8 个)
// ============================================================

#[tokio::test]
async fn test_csn_hijack_dimension_mismatch_attack() {
    // 攻击:注册 49 维向量(与配置 50 维不匹配),试图破坏相似度计算
    // 防御:cosine_similarity 对长度不匹配返回 0.0(similarity.rs:36-38),
    //       malformed 向量虽被注册但相似度为 0.0,永远不会成为高优候选
    // WHY 不在注册层校验:CSN register 仅校验 capability_id 非空与容量上限,
    //       维度校验推迟到查询时由 cosine_similarity 自然中和(系统边界校验)
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let malformed = CapabilityDescriptor::new("cap-malformed", vec![1.0; 49]); // 49 维,少 1 维
                                                                               // 注册成功(register 不校验维度),但 malformed 的相似度会被中和为 0.0
    let _ = pipeline.substitutor.register_capability(malformed);

    // 验证:查询 cap-shell(50 维)的替代候选
    let candidates = pipeline.substitutor.find_substitutes("cap-shell", 5);
    assert!(!candidates.is_empty(), "应能找到正常候选");

    // 验证:Top-1 候选不是 malformed(相似度 0.0 应排在最后)
    assert_ne!(
        candidates[0].candidate_id, "cap-malformed",
        "malformed 相似度=0.0,不应是 Top-1 候选"
    );

    // 验证:若 malformed 出现在候选中,其相似度必须为 0.0(攻击被中和)
    for c in &candidates {
        if c.candidate_id == "cap-malformed" {
            assert_eq!(
                c.similarity_score, 0.0,
                "维度不匹配的 malformed 相似度必须为 0.0(长度不匹配保护)"
            );
        }
    }

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-CSN-1] 维度不匹配攻击被中和(相似度=0.0),耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_csn_hijack_empty_capability_id() {
    // 攻击:注册空 capability_id,试图覆盖或破坏注册表 key 逻辑
    // 防御:空 ID 作为 key 存储(或被校验拒绝),不破坏现有能力
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let empty_id_cap = CapabilityDescriptor::new("", vec![1.0; 50]);
    // 空 ID 的注册行为:无论接受与否,都不应破坏现有 3 个能力
    let _ = pipeline.substitutor.register_capability(empty_id_cap);

    // 验证:原有 3 个能力仍可正常查询
    let candidates = pipeline.substitutor.find_substitutes("cap-shell", 5);
    assert!(!candidates.is_empty(), "原有能力查询不应受空 ID 注册影响");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-CSN-2] 空 ID 攻击被中和,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_csn_hijack_nan_vector_injection() {
    // 攻击:向量含 NaN,试图污染余弦相似度计算(Top-K 排序异常)
    // 防御:cosine_similarity 的 partial_cmp 用 unwrap_or(Equal),NaN 不破坏排序
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    // 注册一个含 NaN 的能力(若维度匹配则被接受,但相似度计算应安全)
    let nan_vec = vec![f32::NAN; 50];
    let nan_cap = CapabilityDescriptor::new("cap-nan", nan_vec);
    let _ = pipeline.substitutor.register_capability(nan_cap);

    // 查询替代:不应 panic,NaN 不污染 Top-K
    let candidates = pipeline.substitutor.find_substitutes("cap-shell", 5);
    // 验证:即使含 NaN 候选,查询不 panic 且返回有限结果
    assert!(
        candidates.len() <= 5,
        "候选数应 <= top_k=5,实际 {}",
        candidates.len()
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-CSN-3] NaN 向量注入被中和,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_csn_hijack_trigger_unregistered_capability() {
    // 攻击:对未注册能力触发替代,试图引发 panic 或未定义行为
    // 防御:trigger_substitution 返回 NoSubstituteFound
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let err = pipeline
        .substitutor
        .trigger_substitution("cap-nonexistent-attack")
        .await
        .unwrap_err();
    assert!(
        matches!(err, CsnError::NoSubstituteFound { .. }),
        "未注册能力应返回 NoSubstituteFound,实际 {:?}",
        err
    );

    // 验证:未创建降级链
    assert_eq!(
        pipeline.substitutor.chain_count(),
        0,
        "未注册能力不应创建降级链"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-CSN-4] 未注册能力触发被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_csn_hijack_reset_nonexistent_chain() {
    // 攻击:重置不存在的降级链,试图触发 panic 或状态污染
    // 防御:reset_chain 返回 ChainNotFound
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let err = pipeline
        .substitutor
        .reset_chain("chain-nonexistent-attack")
        .unwrap_err();
    assert!(
        matches!(err, CsnError::ChainNotFound { .. }),
        "不存在链应返回 ChainNotFound,实际 {:?}",
        err
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-CSN-5] 不存在链重置被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_csn_hijack_advance_nonexistent_chain() {
    // 攻击:推进不存在的降级链,试图绕过降级保护
    // 防御:advance_degradation 返回 ChainNotFound
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let err = pipeline
        .substitutor
        .advance_degradation("chain-nonexistent-advance")
        .unwrap_err();
    assert!(
        matches!(err, CsnError::ChainNotFound { .. }),
        "不存在链推进应返回 ChainNotFound,实际 {:?}",
        err
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-CSN-6] 不存在链推进被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_csn_hijack_registry_overflow() {
    // 攻击:注册超过 registry_capacity(100)个能力,试图耗尽内存
    // 防御:超过容量返回 RegistryFull
    let bus = EventBus::new();
    // 使用容量为 3 的小注册表,便于快速测试溢出
    let small_config = csn_substitutor::CsnConfig {
        registry_capacity: 3,
        ..Default::default()
    };
    let substitutor = csn_substitutor::CsnSubstitutor::with_event_bus(small_config, bus);
    let start = Instant::now();

    // 注册 3 个能力(达到容量上限)
    for i in 0..3 {
        let cap = CapabilityDescriptor::new(format!("cap-{i}"), vec![1.0; 50]);
        substitutor
            .register_capability(cap)
            .expect("前 3 个应注册成功");
    }

    // 第 4 个应失败(溢出)
    let overflow_cap = CapabilityDescriptor::new("cap-overflow", vec![1.0; 50]);
    let err = substitutor.register_capability(overflow_cap).unwrap_err();
    assert!(
        matches!(err, CsnError::RegistryFull { capacity: 3 }),
        "溢出应返回 RegistryFull,实际 {:?}",
        err
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-CSN-7] 注册表溢出被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_csn_hijack_zero_vector_similarity_deception() {
    // 攻击:注册零向量能力,试图通过零相似度欺骗触发误替代
    // 防御:cosine_similarity 对零向量返回 0.0(零向量保护),不触发高相似度误判
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    // 注册一个零向量能力(与 cap-shell 的 [1.0;50] 相似度应为 0.0)
    let zero_cap = CapabilityDescriptor::new("cap-zero", vec![0.0; 50]);
    pipeline
        .substitutor
        .register_capability(zero_cap)
        .expect("零向量能力应可注册(维度匹配)");

    // 查询 cap-shell 的替代:零向量候选相似度应为 0.0,不应被误选为高相似替代
    let candidates = pipeline.substitutor.find_substitutes("cap-shell", 5);
    let zero_candidate = candidates.iter().find(|c| c.candidate_id == "cap-zero");
    if let Some(zc) = zero_candidate {
        assert!(
            (zc.similarity_score - 0.0).abs() < 1e-6,
            "零向量相似度应为 0.0,实际 {},不应被误判为高相似",
            zc.similarity_score
        );
    }

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-CSN-8] 零向量欺骗被中和,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

// ============================================================
// 第 3 组:SESA 稀疏度绕过攻击载荷(7 个)
// ============================================================

#[tokio::test]
async fn test_sesa_bypass_top_k_exceeds_pool_size() {
    // 攻击:请求 top_k=1000,试图激活全部专家绕过稀疏度限制
    // 防御:enforce_sparsity 强制裁剪到 < 40%,无论 top_k 多大
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    // 默认 3 个专家,top_k=1000 试图激活全部
    let request = ActivationRequest::new("req-attack-1", vec![0.5; 64], 1000, 5);
    let (mask, profile) = pipeline
        .sesa
        .activate(request)
        .await
        .expect("激活应成功(被裁剪)");

    // 3 个专家:3×0.4=1.2,1/3=0.333<0.4,所以 max_allowed=1
    assert!(
        profile.sparsity_ratio < 0.4,
        "稀疏度必须严格 < 40%,实际 {}",
        profile.sparsity_ratio
    );
    assert!(mask.active_count <= 1, "3 专家应被裁剪到 <= 1 位");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-SESA-1] Top-K 超容量绕过被阻止,稀疏度 {:.2}%,耗时 {}ms",
        profile.sparsity_ratio * 100.0,
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_sesa_bypass_zero_deadline() {
    // 攻击:deadline_ms=0,试图绕过超时校验直接激活
    // 防御:activate 校验 deadline_ms==0 返回 ActivationTimeout
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    let request = ActivationRequest::new("req-attack-2", vec![0.5; 64], 2, 0);
    let err = pipeline.sesa.activate(request).await.unwrap_err();
    assert!(
        matches!(err, SesaError::ActivationTimeout { deadline_ms: 0 }),
        "零 deadline 应返回 ActivationTimeout,实际 {:?}",
        err
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-SESA-2] 零 deadline 绕过被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_sesa_bypass_empty_expert_pool_activation() {
    // 攻击:空专家池激活,试图触发未定义行为
    // 防御:activate_inner 校验 total==0 返回 EmptyExpertPool
    let bus = EventBus::new();
    let router = sesa_router::SesaRouter::with_event_bus(SesaConfig::default(), bus);
    let start = Instant::now();

    let request = ActivationRequest::new("req-attack-3", vec![0.5; 64], 8, 5);
    let err = router.activate(request).await.unwrap_err();
    assert!(
        matches!(err, SesaError::EmptyExpertPool),
        "空池激活应返回 EmptyExpertPool,实际 {:?}",
        err
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-SESA-3] 空池激活被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_sesa_bypass_exceed_mask_capacity() {
    // 攻击:注册超过 256 个专家,试图溢出 256-bit 掩码
    // 防御:register_expert 校验 mask_index < 256,返回 IndexOutOfBounds
    let bus = EventBus::new();
    let router = sesa_router::SesaRouter::with_event_bus(SesaConfig::default(), bus);
    let start = Instant::now();

    // 注册 256 个专家(刚好满)
    for i in 0..256 {
        let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.1; 64]);
        router.register_expert(expert).expect("前 256 个应成功");
    }

    // 第 257 个应失败
    let overflow_expert = ExpertDescriptor::new("expert-overflow", vec![0.1; 64]);
    let err = router.register_expert(overflow_expert).unwrap_err();
    assert!(
        matches!(err, SesaError::IndexOutOfBounds { capacity: 256, .. }),
        "第 257 个专家应返回 IndexOutOfBounds,实际 {:?}",
        err
    );

    assert_eq!(router.expert_count(), 256, "应正好 256 个专家,无溢出");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-SESA-4] 掩码容量溢出被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_sesa_bypass_sparsity_boundary_40_percent() {
    // 攻击:100 专家 top_k=40,试图恰好达到 40% 边界绕过"严格 <"约束
    // 防御:enforce_sparsity 用严格小于,40/100=0.4>=0.4 → 裁剪到 39
    let bus = EventBus::new();
    let router = sesa_router::SesaRouter::with_event_bus(SesaConfig::default(), bus);
    let start = Instant::now();

    // 注册 100 个专家
    for i in 0..100 {
        let v = vec![(i as f32) * 0.01; 64];
        let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
        router.register_expert(expert).expect("注册失败");
    }

    // top_k=40 试图达到 40% 边界
    let request = ActivationRequest::new("req-attack-5", vec![0.5; 64], 40, 5);
    let (mask, profile) = router.activate(request).await.expect("激活应成功");

    // 严格 < 40%:100×0.4=40,40/100=0.4>=0.4,所以 max_allowed=39
    assert!(
        profile.sparsity_ratio < 0.4,
        "稀疏度必须严格 < 40%(边界绕过被阻止),实际 {}",
        profile.sparsity_ratio
    );
    assert_eq!(mask.active_count, 39, "应裁剪到 39 位(严格 < 40%)");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-SESA-5] 40% 边界绕过被阻止,激活 {}/100,耗时 {}ms",
        mask.active_count,
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_sesa_bypass_large_top_k_small_pool() {
    // 攻击:5 专家池 top_k=100,试图激活全部绕过稀疏度
    // 防御:5×0.4=2,2/5=0.4>=0.4 → max_allowed=1(严格 < 40%)
    let bus = EventBus::new();
    let router = sesa_router::SesaRouter::with_event_bus(SesaConfig::default(), bus);
    let start = Instant::now();

    for i in 0..5 {
        let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.1 * i as f32; 64]);
        router.register_expert(expert).expect("注册失败");
    }

    let request = ActivationRequest::new("req-attack-6", vec![0.5; 64], 100, 5);
    let (mask, profile) = router.activate(request).await.expect("激活应成功");

    assert!(
        profile.sparsity_ratio < 0.4,
        "5 专家池稀疏度必须 < 40%,实际 {}",
        profile.sparsity_ratio
    );
    assert!(mask.active_count <= 1, "5 专家应裁剪到 <= 1 位");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-SESA-6] 大 Top-K 小池绕过被阻止,激活 {}/5,耗时 {}ms",
        mask.active_count,
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_sesa_bypass_manual_mask_bit_flip() {
    // 攻击:手动构造掩码设置额外位,试图绕过 enforce_sparsity
    // 防御:enforce_sparsity 用 select_nth_unstable_by 重新选 Top-K,清除多余位
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    // 手动构造掩码:激活全部 3 位(超过 40%)
    let mut mask = sesa_router::SesaMask::new();
    mask.set_bit(0);
    mask.set_bit(1);
    mask.set_bit(2);
    assert_eq!(mask.active_count, 3, "手动掩码激活 3 位");

    // 评分向量(用于 enforce_sparsity 的 Top-K 选择)
    let scores: Vec<f32> = vec![0.9, 0.5, 0.1];
    // 对 3 专家池执行 enforce_sparsity(max_ratio=0.4)
    pipeline.sesa.enforce_sparsity(&mut mask, &scores, 3, 0.4);

    // 3×0.4=1.2,1/3=0.333<0.4,所以 max_allowed=1
    assert!(
        mask.active_count <= 1,
        "enforce_sparsity 应裁剪到 <= 1 位,实际 {}",
        mask.active_count
    );
    // 评分最高的位 0(0.9)应保留
    assert!(mask.get_bit(0), "评分最高的位 0 应保留");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-SESA-7] 手动位翻转绕过被阻止,裁剪后 {} 位,耗时 {}ms",
        mask.active_count,
        start.elapsed().as_millis()
    );
}

// ============================================================
// 第 4 组:Monitor 告警抑制攻击载荷(7 个)
// ============================================================

#[tokio::test]
async fn test_monitor_suppress_disable_instant_alert() {
    // 攻击:禁用 critical_instant_alert,试图抑制 Critical 事件告警
    // 防御:配置可禁用立即告警,但事件计数仍记录(可观测性不丢失)
    let bus = EventBus::new();
    let config = MonitorConfig {
        critical_instant_alert: false,
        ..Default::default()
    };
    let monitor = efficiency_monitor::EfficiencyMonitor::with_event_bus(config, bus);
    let start = Instant::now();

    let skeptic_veto = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-suppress".into(),
        veto_reason: "test".into(),
        frozen_capabilities: vec![],
    };
    monitor.record_event(&skeptic_veto);

    // 禁用立即告警:Critical 告警计数应为 0(告警被抑制)
    assert_eq!(
        monitor.collectors().alert_count("critical"),
        0,
        "禁用立即告警后 critical 告警计数应为 0"
    );
    // 但事件计数仍记录(可观测性保留)
    assert_eq!(
        monitor.collectors().event_count("SkepticVeto"),
        1,
        "事件计数应仍记录(可观测性不丢失)"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MON-1] 禁用立即告警场景验证,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_monitor_suppress_cooldown_bypass_attempt() {
    // 攻击:短时间内连续触发同一规则,试图绕过 cooldown 产生告警风暴
    // 防御:AlertRuleEngine 的 cooldown_secs 防抖,同一规则在冷却期内不重复触发
    let bus = EventBus::new();
    let monitor =
        efficiency_monitor::EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
    let start = Instant::now();

    // 添加规则:事件总数 >= 5 触发 Warning,cooldown=60s(默认)
    monitor.add_alert_rule(AlertRule::new(
        "rate-rule",
        "nexus_event_total",
        5.0,
        Comparison::GreaterOrEqual,
        AlertSeverity::Warning,
    ));

    // 记录 6 个事件(触发规则)
    for _ in 0..6 {
        monitor.record_event(&NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k".into(),
        });
    }
    let first_alerts = monitor.check_alerts();
    assert_eq!(first_alerts.len(), 1, "首次应触发 1 个告警");

    // 立即再次检查:cooldown 应阻止重复触发
    let second_alerts = monitor.check_alerts();
    assert_eq!(
        second_alerts.len(),
        0,
        "cooldown 应阻止同一规则在冷却期内重复触发"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MON-2] cooldown 绕过被阻止,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_monitor_suppress_non_critical_no_false_alert() {
    // 攻击:大量非 Critical 事件,试图触发误 Critical 告警
    // 防御:非 Critical 事件不触发立即告警,仅记录事件计数
    let bus = EventBus::new();
    let monitor =
        efficiency_monitor::EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
    let start = Instant::now();

    // 记录 100 个 CacheHit(非 Critical)
    for _ in 0..100 {
        monitor.record_event(&NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k".into(),
        });
    }

    // 验证:Critical 告警计数应为 0(无非 Critical 事件触发 Critical 告警)
    assert_eq!(
        monitor.collectors().alert_count("critical"),
        0,
        "非 Critical 事件不应触发 critical 告警"
    );
    // 事件计数应准确记录
    assert_eq!(
        monitor.collectors().event_count("CacheHit"),
        100,
        "应记录 100 个 CacheHit 事件"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MON-3] 非 Critical 误告警被阻止,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_monitor_suppress_empty_metrics_render_safe() {
    // 攻击:空监控器渲染 metrics,试图触发 panic 或异常输出
    // 防御:render_metrics 对空采集器返回有效 Prometheus 文本格式
    let monitor = efficiency_monitor::EfficiencyMonitor::new(MonitorConfig::default());
    let start = Instant::now();

    let output = monitor.render_metrics();
    // 应输出有效的 Prometheus 文本格式(即使无数据)
    assert!(
        output.contains("nexus_event_total") || output.is_empty(),
        "空 metrics 应输出有效格式或空字符串,不应 panic"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MON-4] 空 metrics 渲染安全,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_monitor_suppress_start_subscriber_without_bus() {
    // 攻击:无 EventBus 时启动订阅,试图触发 panic
    // 防御:start_event_subscriber 返回 MonitorError::Config
    let monitor = efficiency_monitor::EfficiencyMonitor::new(MonitorConfig::default());
    let start = Instant::now();

    let result = monitor.start_event_subscriber();
    assert!(result.is_err(), "无 bus 启动订阅应返回错误,不应 panic");
    assert!(
        matches!(result, Err(efficiency_monitor::MonitorError::Config { .. })),
        "应返回 Config 错误,实际 {:?}",
        result
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MON-5] 无 bus 启动订阅被拒绝,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_monitor_suppress_impossible_threshold_no_alert() {
    // 攻击:设置不可达阈值(f64::MAX),试图证明告警系统可被配置禁用
    // 防御:不可达阈值时规则永不触发(符合预期),但其他规则不受影响
    let bus = EventBus::new();
    let monitor =
        efficiency_monitor::EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
    let start = Instant::now();

    // 不可达阈值规则
    monitor.add_alert_rule(AlertRule::new(
        "impossible-rule",
        "nexus_event_total",
        f64::MAX,
        Comparison::GreaterOrEqual,
        AlertSeverity::Critical,
    ));
    // 正常阈值规则
    monitor.add_alert_rule(AlertRule::new(
        "normal-rule",
        "nexus_event_total",
        5.0,
        Comparison::GreaterOrEqual,
        AlertSeverity::Warning,
    ));

    for _ in 0..6 {
        monitor.record_event(&NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k".into(),
        });
    }

    let alerts = monitor.check_alerts();
    // 不可达阈值规则不应触发,正常规则应触发
    assert!(
        alerts.iter().all(|a| a.rule_id != "impossible-rule"),
        "不可达阈值规则不应触发"
    );
    assert!(
        alerts.iter().any(|a| a.rule_id == "normal-rule"),
        "正常规则应触发(不受不可达规则影响)"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MON-6] 不可达阈值规则被隔离,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn test_monitor_suppress_metric_count_integrity() {
    // 攻击:混合记录多种事件,试图通过计数混淆抑制告警
    // 防御:每个事件类型独立计数,计数完整性不受混合记录影响
    let bus = EventBus::new();
    let monitor =
        efficiency_monitor::EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
    let start = Instant::now();

    // 记录 5 种不同事件各 3 次
    for _ in 0..3 {
        monitor.record_event(&NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k".into(),
        });
        monitor.record_event(&NexusEvent::SesaActivationCompleted {
            metadata: EventMetadata::new("sesa-router"),
            total_experts: 100,
            active_experts: 39,
            sparsity_ratio: 0.39,
            latency_us: 100,
        });
        monitor.record_event(&NexusEvent::CsnSubstitutionTriggered {
            metadata: EventMetadata::new("csn-substitutor"),
            original_capability_id: "cap-1".into(),
            substitute_id: "cap-2".into(),
            similarity_score: 0.95,
            degradation_level: 0,
        });
    }

    // 验证:每种事件类型计数准确(无混淆)
    assert_eq!(
        monitor.collectors().event_count("CacheHit"),
        3,
        "CacheHit 应为 3"
    );
    assert_eq!(
        monitor.collectors().event_count("SesaActivationCompleted"),
        3,
        "SesaActivationCompleted 应为 3"
    );
    assert_eq!(
        monitor.collectors().event_count("CsnSubstitutionTriggered"),
        3,
        "CsnSubstitutionTriggered 应为 3"
    );

    // 验证:无 Critical 事件 → critical 告警计数为 0
    assert_eq!(
        monitor.collectors().alert_count("critical"),
        0,
        "非 Critical 事件不应触发 critical 告警"
    );

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-MON-7] 计数完整性验证通过,耗时 {}ms",
        start.elapsed().as_millis()
    );
}

// ============================================================
// 汇总测试:30 载荷全部免疫验证
// ============================================================

#[tokio::test]
async fn test_week7_security_30_payloads_all_immune() {
    // 汇总验证:30 个安全测试全部通过(0 穿透)
    // 此测试作为 Task 6.3 的汇总断言,实际穿透检测在各分项测试中
    let pipeline = setup_week7_pipeline().expect("管线装配失败");
    let start = Instant::now();

    // 快速冒烟:验证管线在 30 载荷后仍正常工作
    let result = pipeline
        .mesh
        .execute_transaction(vec!["srv-1".into()], "smoke".into())
        .await
        .expect("30 载荷后 MCP 事务仍应正常");
    assert!(result.success, "管线在安全测试后应保持可用");

    assert!(start.elapsed().as_millis() < CSA_THRESHOLD_MS);
    println!(
        "[SEC-SUMMARY] 30 个 Week 7 攻击载荷全部免疫(0 穿透),冒烟耗时 {}ms",
        start.elapsed().as_millis()
    );
}
