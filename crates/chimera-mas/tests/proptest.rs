//! chimera-mas 不变量属性测试 — Task 15
//!
//! 对应 SubTask 15.1 - 15.4,验证 MAS 子系统核心不变量:
//! - SubTask 15.1: AgentMeta 序列化/反序列化往返(JSON + MessagePack)
//! - SubTask 15.2: TaskComplexity → ThinkingMode 映射不变量
//! - SubTask 15.3: ContextIsolationGuard 永远拒绝跨 Agent 访问 + safe_summary 不泄露
//! - SubTask 15.4: 委托深度永不超过 MAX_AGENT_DEPTH=5
//!
//! ## 语法约束(§4.1 规范)
//! proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`
//! 禁止 closure 形式(某些 pattern 解析失败)
//!
//! ## 序列化协议(ADR-004)
//! JSON(serde_json)用于配置,MessagePack(rmp-serde)用于跨层通信与持久化
//!
//! ## 设计原则
//! 属性测试验证"对任意输入,不变量恒成立",而非重复单元测试的固定 case。
//! 每个属性测试默认 256 cases,覆盖边界值与随机组合。

#![forbid(unsafe_code)]

use chimera_mas::prelude::*;
use chimera_mas::scheduler::priority_rank;
use chimera_mas::MAX_AGENT_DEPTH;
use event_bus::{EventBus, TaskPriority};
use nexus_core::{Task, TaskStatus, ThinkingMode};
use proptest::prelude::*;

// ============================================================
// 辅助策略(Strategy)— 为复杂类型生成随机测试数据
// ============================================================

/// 生成任意 AgentType 策略 — 覆盖全部 5 个变体
///
/// WHY 用 prop_oneof:AgentType 是枚举,变体含 String 字段,
/// prop_oneof + prop_map 组合可覆盖所有变体的随机字段值。
fn arb_agent_type() -> impl Strategy<Value = AgentType> {
    prop_oneof![
        Just(AgentType::RootOrchestrator),
        "[a-z]{1,10}".prop_map(|domain| AgentType::MainAgent { domain }),
        ("[a-z]{1,10}", "[a-z]{1,10}").prop_map(|(parent_id, task_scope)| AgentType::SubAgent {
            parent_id,
            task_scope
        }),
        ("[a-z]{1,10}", "[a-z]{1,10}").prop_map(|(parent_id, task_scope)| AgentType::GrandAgent {
            parent_id,
            task_scope
        }),
        prop::collection::vec("[a-z]{1,10}", 0..3)
            .prop_map(|specialty| AgentType::ExpertAgent { specialty }),
    ]
}

/// 生成任意 AgentStatus 策略 — 覆盖全部 6 个生命周期状态
fn arb_agent_status() -> impl Strategy<Value = AgentStatus> {
    prop_oneof![
        Just(AgentStatus::Idle),
        Just(AgentStatus::Running),
        Just(AgentStatus::Paused),
        Just(AgentStatus::Completed),
        Just(AgentStatus::Failed),
        Just(AgentStatus::Crashed),
    ]
}

/// 生成任意 ThinkingMode 策略 — 覆盖全部 3 个思考模式
fn arb_thinking_mode() -> impl Strategy<Value = ThinkingMode> {
    prop_oneof![
        Just(ThinkingMode::Fast),
        Just(ThinkingMode::Standard),
        Just(ThinkingMode::Deep),
    ]
}

/// 生成任意 ModelConfig 策略 — 覆盖 provider/model/temperature/max_tokens/thinking_mode
///
/// WHY 不用 ModelConfig::default():default() 固定值无法覆盖字段组合空间,
/// 属性测试需要随机组合验证序列化往返的健壮性。
fn arb_model_config() -> impl Strategy<Value = ModelConfig> {
    (
        prop_oneof![Just("openai"), Just("anthropic"), Just("local")],
        prop_oneof![Just("gpt-4"), Just("claude-3-opus"), Just("llama-3")],
        // temperature 范围 [0.0, 2.0](§4.1 规范,OpenAI 推荐)
        0.0f32..2.0,
        // max_tokens 范围 [1, 32768](非零,覆盖小到大)
        1usize..32768,
        arb_thinking_mode(),
    )
        .prop_map(
            |(provider, model, temperature, max_tokens, thinking_mode)| ModelConfig {
                provider: provider.to_string(),
                model: model.to_string(),
                temperature,
                max_tokens,
                thinking_mode,
            },
        )
}

/// 生成任意 AgentMeta 策略 — 覆盖全部 12 个字段
///
/// WHY 不用 new_root_orchestrator/new_main_agent/new_sub_agent 构造函数:
/// 这些构造函数固定了 agent_type/depth/parent_id 的组合,无法覆盖所有变体。
/// 直接构造 AgentMeta(所有字段 pub)可覆盖全部分支,验证序列化往返的健壮性。
fn arb_agent_meta() -> impl Strategy<Value = AgentMeta> {
    (
        // agent_id:小写字母开头 + 字母数字下划线(模拟 UUIDv7 的人类可读形式)
        "[a-z][a-z0-9_]{0,30}",
        arb_agent_type(),
        arb_model_config(),
        // context_window:1 到 1M Token 等效上限
        1usize..1_048_576,
        // parent_id:Option<String>(RootOrchestrator 为 None)
        proptest::option::of("[a-z]{1,20}"),
        // children_ids:0..5 个子 Agent ID
        proptest::collection::vec("[a-z]{1,20}", 0..5),
        arb_agent_status(),
        // depth:0..10(覆盖合法与超限区间)
        0usize..10,
    )
        .prop_map(
            |(
                agent_id,
                agent_type,
                model_config,
                context_window,
                parent_id,
                children_ids,
                status,
                depth,
            )| {
                AgentMeta {
                    agent_id,
                    name: format!("agent-{depth}"),
                    description: "属性测试 Agent".to_string(),
                    agent_type,
                    model_config,
                    context_window,
                    parent_id,
                    children_ids,
                    created_at: chrono::Utc::now(),
                    status,
                    depth,
                }
            },
        )
}

// ============================================================
// SubTask 15.1: AgentMeta 序列化/反序列化往返测试
// ============================================================

proptest! {
    /// 不变量:任意 AgentMeta 经 JSON 序列化再反序列化后,所有字段保持相等
    ///
    /// WHY 逐字段比较而非 prop_assert_eq!(meta, restored):
    /// AgentMeta/ModelConfig 未派生 PartialEq(含 chrono::DateTime + f32 temperature),
    /// 需手动比较所有字段。f32 temperature 用近似比较避免 JSON 精度损失导致 flaky
    /// (§4.4 反模式 6 教训:f32 经 f64 中转可能有精度漂移)。
    #[test]
    fn agent_meta_json_roundtrip_preserves_all_fields(meta in arb_agent_meta()) {
        let json = serde_json::to_string(&meta).expect("AgentMeta JSON 序列化失败");
        let restored: AgentMeta =
            serde_json::from_str(&json).expect("AgentMeta JSON 反序列化失败");

        // 标量字段直接比较(派生 PartialEq 的类型)
        prop_assert_eq!(meta.agent_id, restored.agent_id, "agent_id 往返不一致");
        prop_assert_eq!(meta.agent_type, restored.agent_type, "agent_type 往返不一致");
        prop_assert_eq!(meta.name, restored.name, "name 往返不一致");
        prop_assert_eq!(meta.description, restored.description, "description 往返不一致");
        prop_assert_eq!(meta.context_window, restored.context_window, "context_window 往返不一致");
        prop_assert_eq!(meta.parent_id, restored.parent_id, "parent_id 往返不一致");
        prop_assert_eq!(meta.children_ids, restored.children_ids, "children_ids 往返不一致");
        prop_assert_eq!(meta.status, restored.status, "status 往返不一致");
        prop_assert_eq!(meta.depth, restored.depth, "depth 往返不一致");
        // chrono::DateTime<Utc> 派生 PartialEq,JSON RFC3339 字符串往返无损
        prop_assert_eq!(meta.created_at, restored.created_at, "created_at 往返不一致");

        // ModelConfig 字段逐项比较(未派生 PartialEq)
        prop_assert_eq!(meta.model_config.provider, restored.model_config.provider, "provider 往返不一致");
        prop_assert_eq!(meta.model_config.model, restored.model_config.model, "model 往返不一致");
        prop_assert_eq!(meta.model_config.max_tokens, restored.model_config.max_tokens, "max_tokens 往返不一致");
        prop_assert_eq!(meta.model_config.thinking_mode, restored.model_config.thinking_mode, "thinking_mode 往返不一致");
        // f32 经 JSON f64 中转可能有微小精度漂移,用近似比较
        // WHY 1e-5 容差:serde_json 用 ryu 算法,f32→f64→f32 理论无损,
        // 但保守用 1e-5 避免 flaky(§4.4 反模式 6 教训)
        prop_assert!(
            (meta.model_config.temperature - restored.model_config.temperature).abs() < 1e-5,
            "temperature JSON 往返精度损失过大: {} vs {}",
            meta.model_config.temperature,
            restored.model_config.temperature
        );
    }
}

proptest! {
    /// 不变量:任意 AgentMeta 经 MessagePack 序列化再反序列化后,所有字段保持相等
    ///
    /// ADR-004: MessagePack(rmp-serde)为跨层通信与持久化协议。
    /// WHY 额外验证 MessagePack:Agent 状态持久化与 Event Bus 通信使用 MessagePack,
    /// 需确保二进制序列化往返无损(不同于 JSON 的文本序列化)。
    /// MessagePack float32 对 f32 直接存储,理论无损,但仍保守用近似比较。
    #[test]
    fn agent_meta_msgpack_roundtrip_preserves_all_fields(meta in arb_agent_meta()) {
        let bytes = rmp_serde::to_vec(&meta).expect("AgentMeta MessagePack 序列化失败");
        let restored: AgentMeta =
            rmp_serde::from_slice(&bytes).expect("AgentMeta MessagePack 反序列化失败");

        // 标量字段
        prop_assert_eq!(meta.agent_id, restored.agent_id, "agent_id 往返不一致");
        prop_assert_eq!(meta.agent_type, restored.agent_type, "agent_type 往返不一致");
        prop_assert_eq!(meta.name, restored.name, "name 往返不一致");
        prop_assert_eq!(meta.description, restored.description, "description 往返不一致");
        prop_assert_eq!(meta.context_window, restored.context_window, "context_window 往返不一致");
        prop_assert_eq!(meta.parent_id, restored.parent_id, "parent_id 往返不一致");
        prop_assert_eq!(meta.children_ids, restored.children_ids, "children_ids 往返不一致");
        prop_assert_eq!(meta.status, restored.status, "status 往返不一致");
        prop_assert_eq!(meta.depth, restored.depth, "depth 往返不一致");
        prop_assert_eq!(meta.created_at, restored.created_at, "created_at 往返不一致");

        // ModelConfig 字段
        prop_assert_eq!(meta.model_config.provider, restored.model_config.provider, "provider 往返不一致");
        prop_assert_eq!(meta.model_config.model, restored.model_config.model, "model 往返不一致");
        prop_assert_eq!(meta.model_config.max_tokens, restored.model_config.max_tokens, "max_tokens 往返不一致");
        prop_assert_eq!(meta.model_config.thinking_mode, restored.model_config.thinking_mode, "thinking_mode 往返不一致");
        // f32 MessagePack float32 往返理论无损,用 1e-6 更严格容差验证
        prop_assert!(
            (meta.model_config.temperature - restored.model_config.temperature).abs() < 1e-6,
            "temperature MessagePack 往返精度损失过大: {} vs {}",
            meta.model_config.temperature,
            restored.model_config.temperature
        );
    }
}

// ============================================================
// SubTask 15.2: TaskComplexity → ThinkingMode 映射不变量
// ============================================================

proptest! {
    /// 不变量:任意 TaskComplexity 经 From<TaskComplexity> for ThinkingMode 映射后,
    /// 结果必须与 ADR-026 决策 6 的固定映射表一致
    ///
    /// 映射规则(ADR-026 决策 6):
    /// - Simple      → Fast      (快速响应:查询、格式化)
    /// - Medium      → Standard  (标准深度:单文件修改、单元测试)
    /// - Complex     → Deep      (深度推理:多文件重构、架构设计)
    /// - VeryComplex → Deep      (超深度推理:跨系统迁移、性能调优)
    ///
    /// WHY 属性测试而非单元测试:单元测试只验证 4 个固定 case(agent_meta_test.rs 已覆盖),
    /// 属性测试验证"对任意输入,映射结果恒等于期望值",更严格地约束 From impl 不会
    /// 因未来修改引入条件分支(如根据某全局状态改变映射)。
    #[test]
    fn task_complexity_to_thinking_mode_mapping_invariant(complexity_idx in 0u8..=3u8) {
        // 用索引生成 4 个变体,覆盖 TaskComplexity 全空间
        let complexity = match complexity_idx {
            0 => TaskComplexity::Simple,
            1 => TaskComplexity::Medium,
            2 => TaskComplexity::Complex,
            _ => TaskComplexity::VeryComplex,
        };
        // 期望映射(与 ADR-026 决策 6 + delegation.rs impl 一致)
        let expected = match complexity {
            TaskComplexity::Simple => ThinkingMode::Fast,
            TaskComplexity::Medium => ThinkingMode::Standard,
            TaskComplexity::Complex | TaskComplexity::VeryComplex => ThinkingMode::Deep,
        };
        // 实际映射(From<TaskComplexity> for ThinkingMode)
        let actual: ThinkingMode = complexity.into();
        prop_assert_eq!(
            actual,
            expected,
            "TaskComplexity {:?} → ThinkingMode 映射不一致: 期望 {:?}, 实际 {:?}",
            complexity,
            expected,
            actual
        );
    }
}

// ============================================================
// SubTask 15.3: ContextIsolationGuard 永远拒绝跨 Agent 访问
// ============================================================

proptest! {
    /// 不变量:任意两个不同的 agent_id,ContextIsolationGuard::new(owner).verify_access(other)
    /// 永远返回 Err(MasError::ContextIsolationViolation)
    ///
    /// 这是 MAS 子系统的核心安全红线(§6.2):Agent 无法直接读取其他 Agent 的上下文,
    /// 跨 Agent 信息交换必须通过 EventBus 或 create_safe_summary()。
    ///
    /// 额外验证:错误变体的字段值正确(agent_id=请求方, context_id=守卫所有者),
    /// 且自身访问永远允许(对照实验,验证守卫不是无差别拒绝)。
    #[test]
    fn context_isolation_guard_always_rejects_cross_agent_access(
        owner_id in "[a-z][a-z0-9_]{0,30}",
        other_id in "[a-z][a-z0-9_]{0,30}"
    ) {
        // 跳过 owner_id == other_id 的 case(自身访问应允许,不在本测试范围)
        prop_assume!(owner_id != other_id, "owner_id 与 other_id 必须不同");

        let guard = ContextIsolationGuard::new(&owner_id);
        let result = guard.verify_access(&other_id);

        match result {
            Err(MasError::ContextIsolationViolation { agent_id, context_id }) => {
                // 错误字段:agent_id = 请求方(违规者),context_id = 守卫保护的上下文所有者
                // WHY 用 & 借用而非 move:owner_id/other_id 在对照实验(line 321)还需借用,
                // String 不是 Copy,直接传给 prop_assert_eq! 会 move 导致后续借用失败
                prop_assert_eq!(
                    &agent_id, &other_id,
                    "ContextIsolationViolation.agent_id 应为请求方(other_id)"
                );
                prop_assert_eq!(
                    &context_id, &owner_id,
                    "ContextIsolationViolation.context_id 应为上下文所有者(owner_id)"
                );
            }
            Ok(()) => {
                // 跨 Agent 访问被错误允许 — 安全红线违反
                prop_assert!(
                    false,
                    "跨 Agent 访问应被拒绝: owner={owner_id}, other={other_id}"
                );
            }
            Err(other_err) => {
                // 错误变体不符 — 期望 ContextIsolationViolation
                prop_assert!(
                    false,
                    "错误变体不符: 期望 ContextIsolationViolation, 实际 {other_err:?}"
                );
            }
        }

        // 对照实验:自身访问(owner 访问自己的上下文)永远允许
        prop_assert!(
            guard.verify_access(&owner_id).is_ok(),
            "自身访问应被允许(owner 访问自己的上下文)"
        );
    }
}

proptest! {
    /// 不变量:create_safe_summary() 永远不泄露完整对话(输出长度有上限 + 脱敏)
    ///
    /// 验证三点:
    /// 1. summary 中不包含 raw_conversation 块的内容(模式匹配脱敏)
    /// 2. summary 长度有明确上界(每个 section content 截断至 200 字符)
    /// 3. summary 中每个 section 的 content 部分字符数 <= 200(截断保护)
    ///
    /// WHY 这是属性测试:任意 content 长度 + 任意块名称组合,
    /// create_safe_summary 的输出长度永远有明确上界,不受输入膨胀影响。
    /// 这是 Ω-Sparse 在上下文隔离层的体现:跨 Agent 通信只传递稀疏摘要。
    #[test]
    fn safe_summary_never_leaks_full_conversation(
        status_content in ".{0,500}",
        raw_content in ".{0,500}",
        extra_block_count in 0u8..=5u8
    ) {
        let agent_id = "agent-owner";
        // 构造 AgentContext,EventBus::new() 在测试中无副作用(仅用于 build_prompt)
        let mut ctx = AgentContext::new(agent_id, 1_048_576, EventBus::new())
            .expect("AgentContext 构造不应失败");

        // 添加 status 块(name 含 "status",会被 create_safe_summary 提取)
        ctx.add_block(ContextBlock::new(
            "task_status",
            status_content.clone(),
            status_content.len(),
            ContextPriority::Normal,
        ))
        .expect("add_block status 不应失败");

        // 添加 raw_conversation 块(name 不含 status/decision/conclusion,应被排除)
        // WHY 用独特 marker:验证 summary 不包含 raw 块内容(marker 位于开头,若被误提取必出现)
        let raw_secret_marker = "RAW_SECRET_DO_NOT_LEAK";
        let raw_full_content = format!("{raw_secret_marker}_{raw_content}");
        ctx.add_block(ContextBlock::new(
            "raw_conversation",
            raw_full_content.clone(),
            raw_full_content.len(),
            ContextPriority::Normal,
        ))
        .expect("add_block raw 不应失败");

        // 添加额外 decision 块(随机数量,验证多块场景)
        for i in 0..extra_block_count {
            ctx.add_block(ContextBlock::new(
                format!("decision-{i}"),
                format!("decision content {i}"),
                20,
                ContextPriority::Normal,
            ))
            .expect("add_block extra 不应失败");
        }

        let guard = ContextIsolationGuard::new(agent_id);
        let summary = guard
            .create_safe_summary(&ctx)
            .expect("create_safe_summary 不应失败(守卫 owner 与 context 一致)");

        // 不变量 1:summary 中不包含 raw_conversation 块的秘密标记(脱敏)
        // WHY 只检查 marker 而非完整 content:content 可能被截断,
        // 但 marker 位于开头,若 raw 块被误提取则 marker 必出现
        prop_assert!(
            !summary.contains(raw_secret_marker),
            "safe_summary 泄露了 raw_conversation 内容(包含秘密标记 {raw_secret_marker})"
        );

        // 不变量 2:summary 字符长度有明确上限(每个 section <= 200 content + 标题开销)
        // 上界 = (status 块 + extra decision 块) × 250 + 兜底消息余量 100
        // WHY 250:200 content 字符 + "## Task Status\n"(16 字符) + 余量;
        //       100:兜底消息 "Agent ... context summary (no extractable content)" 长度
        let max_expected_chars = (1 + extra_block_count as usize) * 250 + 100;
        let summary_chars = summary.chars().count();
        prop_assert!(
            summary_chars <= max_expected_chars,
            "summary 字符数 {} 超过上界 {}(输入膨胀未受截断保护)",
            summary_chars,
            max_expected_chars
        );

        // 不变量 3:每个 "## " section 的 content 部分字符数 <= 200(截断保护)
        // 解析 Markdown section,验证截断保护
        // WHY 用 chars().count():content 可能含多字节 UTF-8 字符,
        // create_safe_summary 用 chars().take(200) 截断,字符数 <= 200
        for section in summary.split("\n\n") {
            let content = section
                .strip_prefix("## Task Status\n")
                .or_else(|| section.strip_prefix("## Key Decision\n"))
                .or_else(|| section.strip_prefix("## Conclusion\n"));
            if let Some(content_text) = content {
                let content_chars = content_text.chars().count();
                prop_assert!(
                    content_chars <= 200,
                    "section content 字符数 {} 超过 200 上限(截断保护失效)",
                    content_chars
                );
            }
        }
    }
}

// ============================================================
// SubTask 15.4: 委托深度永不超过 MAX_AGENT_DEPTH=5
// ============================================================

proptest! {
    /// 不变量:任意 delegation_depth,RootOrchestrator::check_depth(depth) 当
    /// depth >= MAX_AGENT_DEPTH(5) 时返回 Err(MaxDepthExceeded),
    /// 当 depth < MAX_AGENT_DEPTH 时返回 Ok(())
    ///
    /// 这是 ADR-026 决策 1 的核心约束:防止递归委托爆炸。
    /// 边界值:depth=5 必须被拒绝(>= 5),depth=4 必须通过(< 5)。
    ///
    /// WHY 属性测试:验证 0..=10 全区间(含边界 4/5),确保 check_depth 的
    /// 比较运算符是 >= 而非 >,防止 off-by-one 错误。
    #[test]
    fn delegation_depth_respects_max_agent_depth(depth in 0usize..=10) {
        // WHY 每次 new:RootOrchestrator::new 是轻量构造(仅存储 EventBus + 创建 AgentFactory),
        // 无 spawn/订阅副作用,适合在属性测试中反复构造
        let orchestrator = RootOrchestrator::new(EventBus::new());
        let result = orchestrator.check_depth(depth);

        if depth >= MAX_AGENT_DEPTH {
            // 深度超限:必须返回 MaxDepthExceeded,且字段值正确
            match result {
                Err(MasError::MaxDepthExceeded { current_depth, max_depth }) => {
                    prop_assert_eq!(
                        current_depth, depth,
                        "MaxDepthExceeded.current_depth 应等于输入 depth"
                    );
                    prop_assert_eq!(
                        max_depth, MAX_AGENT_DEPTH,
                        "MaxDepthExceeded.max_depth 应等于 MAX_AGENT_DEPTH({})",
                        MAX_AGENT_DEPTH
                    );
                }
                Ok(()) => {
                    // 深度超限却返回 Ok — 安全红线违反
                    prop_assert!(
                        false,
                        "depth={depth} >= MAX_AGENT_DEPTH({MAX_AGENT_DEPTH}) 应返回 Err, 实际返回 Ok"
                    );
                }
                Err(other) => {
                    // 错误变体不符 — 期望 MaxDepthExceeded
                    prop_assert!(
                        false,
                        "depth={depth} 应返回 MaxDepthExceeded, 实际返回 {other:?}"
                    );
                }
            }
        } else {
            // 深度合法:必须返回 Ok
            // WHY result 在 else 分支可用:if/else 互斥,if 分支的 match result 未执行
            prop_assert!(
                result.is_ok(),
                "depth={depth} < MAX_AGENT_DEPTH({MAX_AGENT_DEPTH}) 应返回 Ok, 实际: {:?}",
                result
            );
        }
    }
}

// ============================================================
// ADR-027: 四象限 INV-3/INV-4 + 优先级排序不变量
// ============================================================

/// 构造指定优先级的 AgentTask(用于调度器属性测试)。
fn make_prio_task(id: &str, priority: TaskPriority) -> AgentTask {
    let task = Task {
        task_id: id.into(),
        description: format!("task {id}"),
        status: TaskStatus::Pending,
        dependencies: vec![],
    };
    AgentTask::new(
        task,
        TaskComplexity::Medium,
        1000,
        std::time::Duration::from_secs(60),
        QualityLevel::Standard,
    )
    .with_priority(priority)
}

proptest! {
    /// 不变量(INV-3 + INV-4): 任意复杂度, 激活象限数 ≤ 4 且无重复
    ///
    /// WHY 属性测试: 单元测试仅验证 4 个固定复杂度, 本测试确保对 TaskComplexity
    /// 全空间, `activated_quadrants` 返回的象限集恒满足孙层扇出上界与唯一性。
    #[test]
    fn quadrant_activation_respects_inv3_and_inv4(complexity_idx in 0u8..=3u8) {
        let complexity = match complexity_idx {
            0 => TaskComplexity::Simple,
            1 => TaskComplexity::Medium,
            2 => TaskComplexity::Complex,
            _ => TaskComplexity::VeryComplex,
        };
        let quadrants = activated_quadrants(complexity);
        // INV-3: 扇出 ≤ 4
        prop_assert!(
            quadrants.len() <= MAX_QUADRANT_FANOUT,
            "INV-3: 激活象限数 {} 应 ≤ {}",
            quadrants.len(),
            MAX_QUADRANT_FANOUT
        );
        // INV-4: 无重复
        let mut seen = std::collections::HashSet::new();
        for q in &quadrants {
            prop_assert!(seen.insert(*q), "INV-4: 激活象限不应重复");
        }
        // QuadrantPlan 与 activated_quadrants 一致
        let plan = QuadrantPlan::from_complexity("base", complexity);
        prop_assert_eq!(plan.fanout(), quadrants.len(), "QuadrantPlan 扇出应与激活集一致");
    }
}

proptest! {
    /// 不变量: 任意 base 字符串 + 任意象限, encode_scope 后 from_task_scope 可无损还原
    ///
    /// base 不含 '#'(正则排除), 保证尾缀 `#Qn` 唯一可解。
    #[test]
    fn quadrant_scope_encode_decode_roundtrip(
        base in "[a-zA-Z0-9_-]{0,40}",
        q_idx in 0usize..4
    ) {
        let quadrant = Quadrant::ALL[q_idx];
        let scope = quadrant.encode_scope(&base);
        prop_assert_eq!(
            Quadrant::from_task_scope(&scope),
            Some(quadrant),
            "encode/decode 往返应还原同一象限"
        );
    }
}

proptest! {
    /// INV-4 强制: 任意象限的重复显式构造必被拒绝
    #[test]
    fn quadrant_plan_rejects_duplicate_quadrant(q_idx in 0usize..4) {
        let quadrant = Quadrant::ALL[q_idx];
        let result = QuadrantPlan::from_quadrants("t", vec![quadrant, quadrant]);
        prop_assert!(
            matches!(result, Err(MasError::QuadrantConflict { .. })),
            "重复象限应返回 QuadrantConflict"
        );
    }
}

proptest! {
    /// 不变量: priority_rank 严格保序 — 任意成对优先级, 高优先级秩严格更大
    #[test]
    fn priority_rank_is_total_order(a in 0u8..=3u8, b in 0u8..=3u8) {
        let to_priority = |i: u8| match i {
            0 => TaskPriority::Low,
            1 => TaskPriority::Medium,
            2 => TaskPriority::High,
            _ => TaskPriority::Critical,
        };
        let pa = to_priority(a);
        let pb = to_priority(b);
        if a > b {
            prop_assert!(priority_rank(pa) > priority_rank(pb), "高档优先级秩应更大");
        } else if a == b {
            prop_assert_eq!(priority_rank(pa), priority_rank(pb), "同档优先级秩应相等");
        }
    }
}

proptest! {
    /// 核心排序不变量: Critical 与 Low 同队时, Critical 永远先出队
    ///
    /// 不受入队顺序与 WSJF 影响(即使故意让 Low 的 WSJF 更高, 优先级仍主导排序)。
    #[test]
    fn scheduler_critical_dequeues_before_low(
        crit_enqueued_first in any::<bool>(),
        low_has_higher_wsjf in any::<bool>()
    ) {
        let mut scheduler = PriorityScheduler::new();
        // 故意让 Low 的 WSJF 可能更高, 验证优先级主导(非 WSJF 主导)
        let low_input = if low_has_higher_wsjf {
            WsjfInput::new(10.0, 10.0, 10.0, 10.0, 1.0)
        } else {
            WsjfInput::new(1.0, 1.0, 1.0, 1.0, 10.0)
        };
        let crit_input = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 10.0);
        let low = make_prio_task("low", TaskPriority::Low);
        let crit = make_prio_task("crit", TaskPriority::Critical);
        if crit_enqueued_first {
            scheduler.enqueue(crit, &crit_input);
            scheduler.enqueue(low, &low_input);
        } else {
            scheduler.enqueue(low, &low_input);
            scheduler.enqueue(crit, &crit_input);
        }
        let first = scheduler.dequeue().expect("非空队列应能出队");
        prop_assert_eq!(
            first.inner.task_id,
            "crit",
            "Critical 应永远先于 Low 出队"
        );
    }
}

// ============================================================
// Task 21: INV-7 / INV-8 不变量属性测试
// ============================================================
//
// 对应设计文档 §21.2 + §15.4(INV-7)+ §17.5(INV-8)。
// 属性测试验证"对任意输入,不变量恒成立",而非重复单元测试的固定 case。
// 每个属性测试默认 256 cases,覆盖边界值与随机组合。
//
// ## 语法约束(§4.1 规范)
// proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`
// 禁止 closure 形式(某些 pattern 解析失败)
//
// ## 红线对齐
// - §4.4 反模式 6: f32 禁止隐式转 f64,全程 f64
// - §15.4: INV-7 失败复用 MasError::TokenBudgetExceeded
// - §17.5: INV-8 失败返回 MasError::ArchiveMonotonicityViolated

use chimera_mas::invariants::{ArchiveTier, InvariantChecker, MEMORY_BUDGET_MB};

/// 生成任意 ArchiveTier 策略 — 覆盖全部 4 个层级
fn arb_archive_tier() -> impl Strategy<Value = ArchiveTier> {
    prop_oneof![
        Just(ArchiveTier::Hot),
        Just(ArchiveTier::Warm),
        Just(ArchiveTier::Cold),
        Just(ArchiveTier::Ice),
    ]
}

proptest! {
    /// INV-7 不变量:任意 resident > capacity 时必须返回 Err(TokenBudgetExceeded)
    ///
    /// 验证单 Agent 驻留约束(§15.4):resident ≤ effective_capacity 是硬上界,
    /// 超限必拒绝,且错误变体为 TokenBudgetExceeded(§15.4 复用既有变体)。
    #[test]
    fn inv7_resident_above_capacity_always_rejected(
        resident in 1usize..100_000,
        capacity in 0usize..100_000
    ) {
        // 跳过 resident <= capacity 的合法 case(本测试聚焦超限)
        prop_assume!(resident > capacity, "本测试仅验证 resident > capacity");
        let result = InvariantChecker::check_inv7_context_budget(
            resident,
            capacity,
            0, // m_total=0:排除全局约束干扰
            MEMORY_BUDGET_MB,
        );
        match result {
            Err(MasError::TokenBudgetExceeded { current_tokens, max_tokens, .. }) => {
                prop_assert_eq!(current_tokens, resident, "current_tokens 应为输入 resident");
                prop_assert_eq!(max_tokens, capacity, "max_tokens 应为输入 capacity");
            }
            other => prop_assert!(
                false,
                "resident={resident} > capacity={capacity} 应返回 TokenBudgetExceeded, 实际: {other:?}"
            ),
        }
    }
}

proptest! {
    /// INV-7 不变量:任意 m_total > m_budget×0.9 时必须返回 Err(TokenBudgetExceeded)
    ///
    /// 验证全局派生准入闸(§15.3):M_total ≤ 130MB×0.9 是派生硬上界。
    /// 全程 f64 计算(§4.4 反模式 6),避免 f32 精度膨胀。
    #[test]
    fn inv7_global_above_threshold_always_rejected(
        m_total in 100usize..200,
        m_budget in 100usize..200
    ) {
        // 跳过 m_total <= m_budget*0.9 的合法 case
        let threshold = (m_budget as f64 * 0.9) as usize;
        prop_assume!(m_total > threshold, "本测试仅验证 m_total > m_budget×0.9");
        // resident=0:排除单 Agent 约束干扰
        let result = InvariantChecker::check_inv7_context_budget(0, 100_000, m_total, m_budget);
        match result {
            Err(MasError::TokenBudgetExceeded { current_tokens, max_tokens, .. }) => {
                prop_assert_eq!(current_tokens, m_total, "current_tokens 应为输入 m_total");
                prop_assert_eq!(max_tokens, threshold, "max_tokens 应为阈值 m_budget×0.9");
            }
            other => prop_assert!(
                false,
                "m_total={m_total} > threshold={threshold} 应返回 TokenBudgetExceeded, 实际: {other:?}"
            ),
        }
    }
}

proptest! {
    /// INV-7 不变量:resident ≤ capacity 且 m_total ≤ m_budget×0.9 时必返回 Ok
    ///
    /// 这是 INV-7 的"正向"属性:两个约束均满足时,派生准入闸必须放行。
    /// 验证 AND 关系的两侧均通过时,不会因副作用误拒。
    #[test]
    fn inv7_both_constraints_satisfied_always_ok(
        capacity in 1usize..100_000,
        resident_pct in 0u8..=100,
        m_budget in 100usize..200,
        m_total_pct in 0u8..=90
    ) {
        // resident = capacity × (resident_pct / 100),保证 resident ≤ capacity
        let resident = capacity * resident_pct as usize / 100;
        // m_total = m_budget × (m_total_pct / 100),m_total_pct ≤ 90 保证 m_total ≤ m_budget×0.9
        let m_total = m_budget * m_total_pct as usize / 100;
        let result = InvariantChecker::check_inv7_context_budget(resident, capacity, m_total, m_budget);
        prop_assert!(
            result.is_ok(),
            "resident={resident} ≤ capacity={capacity}, m_total={m_total} ≤ m_budget×0.9 应通过, 实际: {result:?}"
        );
    }
}

proptest! {
    /// INV-8 不变量:from.level() < to.level() 时必返回 Ok(合法降级)
    ///
    /// 验证 Hot→Warm→Cold→Ice 单向降级恒成立(§17.5)。
    /// 跨级降级(如 Hot→Ice)也合法,因为 level 严格递增。
    #[test]
    fn inv8_monotonic_demotion_always_ok(
        from_idx in 0u8..=2,
        delta in 1u8..=3
    ) {
        // 构造合法降级:from_idx + delta ≤ 3,to_idx 严格大于 from_idx
        let to_idx = from_idx + delta;
        prop_assume!(to_idx <= 3, "to_idx 不超过 Ice(3)");
        let from_tier = tier_from_idx(from_idx);
        let to_tier = tier_from_idx(to_idx);
        let result = InvariantChecker::check_inv8_archive_monotonicity(from_tier, to_tier);
        prop_assert!(
            result.is_ok(),
            "{from_tier:?}→{to_tier:?} (level {}→{}) 应通过",
            from_tier.level(),
            to_tier.level()
        );
    }
}

proptest! {
    /// INV-8 不变量:from.level() >= to.level() 时必返回 Err(同层或反向膨胀)
    ///
    /// 验证记忆不可反向膨胀(§17.5)。覆盖:
    /// - 同层(Hot→Hot 等)
    /// - 反向(Ice→Hot 等,跨多级反向)
    /// 错误变体必为 ArchiveMonotonicityViolated,且字段正确反映输入 tier 名。
    #[test]
    fn inv8_non_monotonic_always_rejected(
        from in arb_archive_tier(),
        to in arb_archive_tier()
    ) {
        // 跳过合法降级 case
        prop_assume!(to.level() <= from.level(), "本测试仅验证非单调");
        let result = InvariantChecker::check_inv8_archive_monotonicity(from, to);
        match result {
            Err(MasError::ArchiveMonotonicityViolated { from_tier, to_tier: tt }) => {
                let expected_from = format!("{from:?}");
                let expected_to = format!("{to:?}");
                prop_assert_eq!(
                    from_tier, expected_from,
                    "from_tier 字段应等于 {:?} 的 Debug 格式",
                    from
                );
                prop_assert_eq!(
                    tt, expected_to,
                    "to_tier 字段应等于 {:?} 的 Debug 格式",
                    to
                );
            }
            other => prop_assert!(
                false,
                "{from:?}→{to:?} 应返回 ArchiveMonotonicityViolated, 实际: {other:?}"
            ),
        }
    }
}

/// 索引到 ArchiveTier 的辅助函数(0=Hot, 1=Warm, 2=Cold, 3=Ice)
fn tier_from_idx(idx: u8) -> ArchiveTier {
    match idx {
        0 => ArchiveTier::Hot,
        1 => ArchiveTier::Warm,
        2 => ArchiveTier::Cold,
        _ => ArchiveTier::Ice,
    }
}
