//! Week 8 Task 6 SubTask 6.2 — 37 模块全量集成测试
//!
//! 对应任务:Week 8 Task 6.2(34 crates + event-bus + nexus-core + 根测试 事件链路)
//! 架构层:L1-L10 跨层全栈协同
//!
//! # 测试用例
//! 1. test_event_bus_full_chain:EventBus 广播 + 订阅(L1 Core)
//! 2. test_layer_dependencies:L1→L10 跨层事件传递(复用 Week7 9-crate 管线)
//! 3. test_osa_sparse_routing:OSA 协调器五维度稀疏掩码计算(L6 Router)
//! 4. test_parliament_consensus:议会共识 + PVL 生产验证(L8 + L7)
//! 5. test_security_sandbox:SecCore 沙箱 + QEEP 协议(L4 Security)
//!
//! # 架构红线对齐
//! - `#![forbid(unsafe_code)]` 红线:测试代码不引入 unsafe
//! - 单运行时:用 `tokio::runtime::Runtime::new()` 而非 `#[tokio::test]`
//! - 跨层通信仅通过 EventBus(§2.2 依赖铁律)

#![forbid(unsafe_code)]

#[path = "week7_setup.rs"]
#[allow(dead_code)]
mod setup;

use std::time::Duration;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use nmc_encoder::PerceptionInput;
use osa_coordinator::{OmniSparseCoordinator, RiskLevel as OsaRiskLevel, TaskProfile};
use parliament::{Parliament, ParliamentConfig, Proposal};
use pvl_layer::{FeedbackChannel, Producer, PvlConfig, Verifier};
use qeep_protocol::QeepProtocol;
use seccore::{validate_command, Command, CommandPolicy};
use setup::{assert_has_event, drain_events, setup_week7_pipeline};

// ============================================================
// 测试 1:EventBus 广播 + 订阅全链路(L1 Core)
// 验证:EventBus 的 publish/subscribe 机制在多订阅者场景下正确广播
// ============================================================

#[test]
fn test_event_bus_full_chain() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let bus = EventBus::new();
        // 两个订阅者,验证广播语义(一发布多接收)
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let event = NexusEvent::CacheHit {
            metadata: EventMetadata::new("e2e-test"),
            cache_key: "integration-key".into(),
        };
        bus.publish(event).await.expect("publish 失败");

        // 两个订阅者都应收到同一事件
        let e1 = rx1
            .recv_timeout(Duration::from_millis(100))
            .await
            .expect("订阅者 1 未收到事件");
        let e2 = rx2
            .recv_timeout(Duration::from_millis(100))
            .await
            .expect("订阅者 2 未收到事件");

        // 验证事件类型一致(均为 CacheHit)
        assert_eq!(e1.type_name(), "CacheHit", "订阅者 1 应收到 CacheHit");
        assert_eq!(e2.type_name(), "CacheHit", "订阅者 2 应收到 CacheHit");
    });
}

// ============================================================
// 测试 2:L1→L10 跨层事件传递
// 验证:Week7 9-crate 管线(覆盖 L1/L2/L3/L5/L6/L7/L8/L9/L10)
//   通过共享 EventBus 实现跨层事件广播
// ============================================================

#[test]
fn test_layer_dependencies() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let pipeline = setup_week7_pipeline().expect("Week7 管线装配失败");
        let mut rx = pipeline.bus.subscribe();

        // L2 Memory:NMC 编码文本输入,发布 NmcEncoded 事件
        let clv = pipeline
            .encoder
            .perceive(PerceptionInput::Text("cross-layer event flow".into()))
            .expect("NMC 编码失败");
        assert_eq!(clv.dimension(), 512, "CLV 维度必须为 512");

        // L7 Execution:SSRA 融合,发布 SsraFusionCompleted 事件
        let request = setup::make_fusion_request("q-layer", vec!["cap-text-fusion"], "target");
        let result = pipeline.fusion.fuse(request).await.expect("SSRA 融合失败");
        assert!(result.confidence > 0.0, "融合置信度应大于 0");

        // L10 Interface:CHTC 转发工具调用,发布 ChtcToolCallReceived 事件
        let raw = setup::make_vscode_raw("editor.open", serde_json::json!({ "file": "test.rs" }));
        let call = pipeline
            .bridge
            .receive(raw, chtc_bridge::IdeSource::vscode())
            .expect("CHTC receive 失败");
        let exec_result = pipeline.bridge.execute(&call).expect("CHTC execute 失败");
        assert!(exec_result.success, "VSCode execute 应成功");

        // L10 Interface:MCP Mesh 量子事务,发布 McpMeshTransactionCompleted 事件
        let tx_result = pipeline
            .mesh
            .execute_transaction(vec!["srv-1".into(), "srv-2".into()], "query".into())
            .await
            .expect("MCP 事务失败");
        assert!(tx_result.success, "2 服务器事务应成功");

        // 验证:跨层事件链路完整(L2→L7→L10→L10)
        let events = drain_events(&mut rx, 15).await;
        assert_has_event(&events, "NmcEncoded");
        assert_has_event(&events, "SsraFusionCompleted");
        assert_has_event(&events, "ChtcToolCallReceived");
        assert_has_event(&events, "McpMeshTransactionCompleted");
    });
}

// ============================================================
// 测试 3:OSA 协调器 + 五维度稀疏掩码计算(L6 Router)
// 验证:OSA 基于 TaskProfile 计算 routing/context/memory/audit/budget 五维度掩码,
//   并发布 OmniSparseMasksComputed 事件
// ============================================================

#[test]
fn test_osa_sparse_routing() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let coord = OmniSparseCoordinator::new(bus);

        // 构造中等复杂度任务特征(0.6 → ComplexityBand::Complex)
        let mut profile = TaskProfile::new("task-osa-1", 0.6, OsaRiskLevel::Medium);
        // 填充五维度候选集,验证 Top-K 选取
        profile.available_tools = vec!["tool-a".into(), "tool-b".into(), "tool-c".into()];
        profile.available_files = vec!["file-1".into(), "file-2".into()];
        profile.available_memories = vec!["mem-1".into()];
        profile.recent_operations = vec!["op-1".into(), "op-2".into()];
        profile.active_tasks = vec!["task-x".into()];

        let masks = coord
            .compute_all_masks(&profile)
            .await
            .expect("OSA 掩码计算失败");

        // 验证:五维度掩码均已计算(非空,因为提供了候选集)
        assert!(
            masks.routing.active_count() > 0,
            "routing 维度应至少激活 1 个工具"
        );
        assert!(
            masks.context.active_count() > 0,
            "context 维度应至少激活 1 个文件"
        );
        assert!(
            masks.memory.active_count() > 0,
            "memory 维度应至少激活 1 个记忆"
        );
        // mask_hash 应为 64 字符的 SHA-256 hex
        assert_eq!(
            masks.mask_hash().len(),
            64,
            "mask_hash 应为 64 字符 SHA-256 hex"
        );

        // 验证:OmniSparseMasksComputed 事件已发布
        let events = drain_events(&mut rx, 5).await;
        assert_has_event(&events, "OmniSparseMasksComputed");
    });
}

// ============================================================
// 测试 4:议会共识 + PVL 生产验证(L8 Parliament + L7 Execution)
// 验证:Parliament deliberat¬e 达成共识 + PVL Producer/Verifier 流式生产验证
// ============================================================

#[test]
fn test_parliament_consensus() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        // === 阶段 1:Parliament 共识 ===
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus.clone());

        // 构造低风险 Quest + 低风险 Proposal,确保共识达成(非否决)
        let quest = Quest {
            quest_id: "q-parliament-1".into(),
            title: "集成测试任务".into(),
            tasks: vec![Task {
                task_id: "t-1".into(),
                description: "执行集成测试".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        };
        let proposal = Proposal::new("p-1", "q-parliament-1", "执行计划", 0.2);
        let consensus = parliament
            .deliberate(&quest, &proposal)
            .await
            .expect("议会审议失败");

        // 验证:低风险提案应达成共识(非否决)
        assert!(
            !matches!(consensus, parliament::Consensus::Vetoed { .. }),
            "低风险提案不应被否决"
        );

        // 验证:议会事件已发布(DebateStarted 或 ConsensusReached)
        let events = drain_events(&mut rx, 10).await;
        let has_debate_or_consensus = events
            .iter()
            .any(|e| e.type_name() == "DebateStarted" || e.type_name() == "ConsensusReached");
        assert!(
            has_debate_or_consensus,
            "应发布 DebateStarted 或 ConsensusReached 事件"
        );

        // === 阶段 2:PVL 生产验证(独立 bus,避免事件交叉)===
        let pvl_bus = EventBus::new();
        let mut pvl_rx = pvl_bus.subscribe();
        let config = PvlConfig::default();
        let producer = Producer::new(config.clone(), pvl_bus.clone());
        let verifier = Verifier::new(config.clone(), pvl_bus.clone());
        let feedback = FeedbackChannel::new(config, pvl_bus);

        let (op_tx, mut op_rx) = tokio::sync::mpsc::channel(128);
        let (fb_tx, mut fb_rx) = tokio::sync::mpsc::channel(128);

        // 启动验证者后台任务
        let verifier_handle = tokio::spawn(async move { verifier.run(&mut op_rx, &fb_tx).await });

        // 生产 5 个操作
        producer
            .produce("q-pvl-1", 5, &op_tx)
            .await
            .expect("PVL 生产失败");
        drop(op_tx); // 关闭生产端,触发 verifier 退出

        // 处理反馈
        let mut feedback_count = 0;
        while let Some(fb) = fb_rx.recv().await {
            if feedback.process_feedback(fb) {
                let _ = feedback.check_and_adjust_strategy(&producer);
            }
            feedback_count += 1;
        }

        // 验证:5 个操作均产生反馈(生产→验证→反馈链路完整)
        assert_eq!(feedback_count, 5, "应收到 5 个反馈,实际 {feedback_count}");

        // 验证:验证者正常退出
        verifier_handle
            .await
            .expect("verifier task panic")
            .expect("verifier run 失败");

        // 验证:PVL 事件已发布
        let pvl_events = drain_events(&mut pvl_rx, 10).await;
        assert_has_event(&pvl_events, "OperationProduced");
    });
}

// ============================================================
// 测试 5:SecCore 沙箱 + QEEP 协议(L4 Security)
// 验证:SecCore 静态分析拦截注入命令 + QEEP 纠缠协议零孤儿调用
// ============================================================

#[test]
fn test_security_sandbox() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        // === 阶段 1:SecCore 沙箱静态分析 ===
        let policy = CommandPolicy::default_secure();

        // 1a. 安全命令应通过(echo)
        let safe_cmd = Command::new("echo").arg("hello");
        let safe_result = validate_command(&safe_cmd, &policy);
        assert!(safe_result.is_ok(), "安全命令 echo 应通过静态分析");

        // 1b. 注入命令应被拦截(包含 $( 命令替换)
        let inject_cmd = Command::new("echo").arg("$(rm -rf /)");
        let inject_result = validate_command(&inject_cmd, &policy);
        assert!(inject_result.is_err(), "注入命令 $(...) 应被静态分析拦截");

        // 1c. 权限提升命令应被拦截(sudo)
        let sudo_cmd = Command::new("sudo").arg("ls");
        let sudo_result = validate_command(&sudo_cmd, &policy);
        assert!(sudo_result.is_err(), "sudo 提权命令应被静态分析拦截");

        // === 阶段 2:QEEP 纠缠协议零孤儿调用 ===
        // WHY 用 DEFAULT_TIMEOUT:与 crate 默认值一致,30s 足够覆盖测试 future
        let qeep = QeepProtocol::new(qeep_protocol::DEFAULT_TIMEOUT);

        // 2a. 正常完成的 future 不产生孤儿
        let result = qeep
            .entangle(async {
                // 模拟一个异步操作,正常返回成功
                Ok::<_, qeep_protocol::QeepError>(42_u32)
            })
            .await
            .expect("QEEP entangle 正常 future 失败");
        assert_eq!(result, 42, "QEEP entangle 应返回 future 的结果");

        // 2b. entangle 错误传播(future 返回 Err)
        let err_result = qeep
            .entangle(async { Err::<u32, _>(qeep_protocol::QeepError::Timeout) })
            .await;
        assert!(err_result.is_err(), "QEEP entangle 应传播 future 的错误");

        // 2c. 验证 QEEP 完成计数 > 0(至少 1 个成功完成)
        // WHY completed_count:成功 + 失败 + 超时均计数,孤儿不计数
        let completed = qeep.completed_count();
        assert!(
            completed >= 1,
            "QEEP 应至少有 1 个已完成调用,实际 {completed}"
        );

        // 2d. 验证无孤儿调用(正常 await 的 future 不产生孤儿)
        let orphans = qeep.orphan_count();
        assert_eq!(
            orphans, 0,
            "正常 await 的 future 不应产生孤儿调用,实际 {orphans}"
        );
    });
}
