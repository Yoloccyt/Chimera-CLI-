//! P5.5.1 — 5 任务集定义（baseline v1 + 候选 v2/v3/v4）
//!
//! 对应任务: P5.5.1（5 任务集定义，5 类任务类型覆盖）
//! 架构层: 测试夹具（tests/e2e/fixtures/，非生产代码）
//!
//! # 设计要点
//!
//! - **5 类任务覆盖**: code_refactor / bug_fix / feature_add / test_write / docs_gen
//! - **4 版本递进**: 每个任务定义 v1(baseline) → v2 → v3 → v4(最终) 共 4 个 spec
//! - **确定性改进**: v(i+1) 在 contracts.len() / retry.max_attempts / hops.len()
//!   等可量化字段上严格优于 v(i)，使确定性评判器能稳定裁决 Current 胜出
//! - **validate() 合规**: 所有 spec 通过 HarnessSpec::validate()（含 acceptance_gates）
//!
//! # 评判评分函数（与 DeterministicJudgeClient 对齐）
//!
//! ```text
//! score = contracts.len() * 10 + retry.max_attempts * 2 + hops.len() * 5
//! ```
//!
//! 权重设计:
//! - contracts.len() × 10: 契约覆盖度最重要（每个契约 +10 分）
//! - hops.len() × 5: 执行步骤完整度次之（每个 hop +5 分）
//! - retry.max_attempts × 2: 重试鲁棒性第三（每次重试 +2 分）
//!
//! # 任务集版本演进矩阵
//!
//! | 任务 ID | 任务类型 | v1→v2 改进 | v2→v3 改进 | v3→v4 改进 |
//! |---------|---------|-----------|-----------|-----------|
//! | T1 | code_refactor | contracts +1 | contracts +1, hops +1 | contracts +1 |
//! | T2 | bug_fix | retry.max_attempts +2 | contracts +1, retry +2 | retry +2 |
//! | T3 | feature_add | contracts +1 | contracts +1, hops +1 | contracts +1, hops +1 |
//! | T4 | test_write | contracts +1 | contracts +1 | contracts +2 |
//! | T5 | docs_gen | contracts +1 | hops +1 | contracts +1 |

use nexus_contracts::{ContractSpec, HarnessMeta, HarnessSpec, HopSpec, RetryPolicy};

// ============================================================
// 公共辅助函数
// ============================================================

/// 强制 acceptance_gates 字符串（设计文档 §7.2 auxiliary.acceptance_gates）
///
/// WHY 常量复用: 所有 spec 的 auxiliary 都必须包含 4 个强制门
/// (tests_pass / bench_no_regression / invariants_clean / redline_scan_clean),
/// 否则 validate() 返回 MissingAcceptanceGates 错误
const ACCEPTANCE_GATES_AUX: &str = r#"acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]"#;

/// 构造基础 RetryPolicy（指定 max_attempts）
fn retry_policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        backoff_ms: 1000,
        exponential: true,
    }
}

/// 构造基础 ContractSpec（指定 name + property）
fn contract(name: &str, property: &str) -> ContractSpec {
    ContractSpec {
        name: name.to_string(),
        property: property.to_string(),
        description: None,
        from: None,
        to: None,
        fields: Vec::new(),
    }
}

/// 构造基础 HopSpec（指定 name + order，引用给定 contracts）
fn hop(name: &str, order: &[&str], contracts: &[&str]) -> HopSpec {
    HopSpec {
        name: name.to_string(),
        input_type: None,
        output_type: None,
        contracts: contracts.iter().map(|s| s.to_string()).collect(),
        description: None,
        order: order.iter().map(|s| s.to_string()).collect(),
        on_veto: None,
        fallback: None,
    }
}

// ============================================================
// QuestTask — 任务定义（ID + 类型 + 4 版本 spec）
// ============================================================

/// 5 任务集中的一个任务定义
///
/// # 字段
/// - `task_id`: 任务标识（T1-T5）
/// - `task_type`: 任务类型（code_refactor / bug_fix / feature_add / test_write / docs_gen）
/// - `task_content`: 任务内容描述
/// - `versions`: 4 个 spec 版本（v1 baseline + v2/v3/v4 候选）
#[derive(Debug, Clone)]
pub struct QuestTask {
    /// 任务标识（T1-T5）
    pub task_id: &'static str,
    /// 任务类型
    pub task_type: &'static str,
    /// 任务内容描述
    pub task_content: &'static str,
    /// 4 个 spec 版本（索引 0=v1, 1=v2, 2=v3, 3=v4）
    pub versions: [HarnessSpec; 4],
}

impl QuestTask {
    /// 返回任务 ID
    pub fn task_id(&self) -> &'static str {
        self.task_id
    }

    /// 返回任务类型
    pub fn task_type(&self) -> &'static str {
        self.task_type
    }

    /// 返回任务内容
    pub fn task_content(&self) -> &'static str {
        self.task_content
    }

    /// 返回指定版本的 spec（version 从 1 开始，1-4）
    ///
    /// # Panics
    /// version 不在 1..=4 时 panic（编程错误）
    pub fn spec(&self, version: usize) -> &HarnessSpec {
        assert!(
            (1..=4).contains(&version),
            "version 必须在 1..=4 范围内，得到 {}",
            version
        );
        &self.versions[version - 1]
    }

    /// 返回所有 4 个版本 spec 的切片
    pub fn all_versions(&self) -> &[HarnessSpec] {
        &self.versions
    }
}

// ============================================================
// T1: code_refactor — 重构 Quest::new()
// ============================================================

/// 构造 T1 的 4 个版本 spec
///
/// 演进策略: 逐步增加 contracts（严谨度↑）+ hops（验证步骤↑）
/// - v1: 1 contract, 1 hop, retry=1
/// - v2: 2 contracts, 1 hop, retry=2
/// - v3: 3 contracts, 2 hops, retry=3
/// - v4: 4 contracts, 2 hops, retry=5
fn make_t1_versions() -> [HarnessSpec; 4] {
    let base_name = "quest-new-refactor";
    let base_order = ["Architect.propose", "Skeptic.review"];

    // v1: 1 contract, 1 hop, retry=1
    let v1 = HarnessSpec {
        meta: HarnessMeta {
            name: base_name.to_string(),
            version: 1,
            immutable: false,
            parent: None,
            task_type: Some("code_refactor".to_string()),
        },
        contracts: vec![contract("no_panic", "must_not_panic")],
        hops: vec![hop("execute", &base_order, &["no_panic"])],
        retry: retry_policy(1),
        auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
    };

    // v2: 2 contracts, 1 hop, retry=2
    let v2 = HarnessSpec {
        meta: HarnessMeta {
            name: base_name.to_string(),
            version: 2,
            immutable: false,
            parent: Some(1),
            task_type: Some("code_refactor".to_string()),
        },
        contracts: vec![
            contract("no_panic", "must_not_panic"),
            contract("no_unwrap", "must_not_call_unwrap"),
        ],
        hops: vec![hop("execute", &base_order, &["no_panic", "no_unwrap"])],
        retry: retry_policy(2),
        auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
    };

    // v3: 3 contracts, 2 hops, retry=3
    let v3 = HarnessSpec {
        meta: HarnessMeta {
            name: base_name.to_string(),
            version: 3,
            immutable: false,
            parent: Some(2),
            task_type: Some("code_refactor".to_string()),
        },
        contracts: vec![
            contract("no_panic", "must_not_panic"),
            contract("no_unwrap", "must_not_call_unwrap"),
            contract("no_blocking", "must_not_block_runtime"),
        ],
        hops: vec![
            hop("execute", &base_order, &["no_panic", "no_unwrap"]),
            hop("verify_blocking", &base_order, &["no_blocking"]),
        ],
        retry: retry_policy(3),
        auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
    };

    // v4: 4 contracts, 2 hops, retry=5
    let v4 = HarnessSpec {
        meta: HarnessMeta {
            name: base_name.to_string(),
            version: 4,
            immutable: false,
            parent: Some(3),
            task_type: Some("code_refactor".to_string()),
        },
        contracts: vec![
            contract("no_panic", "must_not_panic"),
            contract("no_unwrap", "must_not_call_unwrap"),
            contract("no_blocking", "must_not_block_runtime"),
            contract("send_sync", "must_be_send_sync"),
        ],
        hops: vec![
            hop("execute", &base_order, &["no_panic", "no_unwrap"]),
            hop("verify_async", &base_order, &["no_blocking", "send_sync"]),
        ],
        retry: retry_policy(5),
        auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
    };

    [v1, v2, v3, v4]
}

// ============================================================
// T2: bug_fix — 修复 severity 误判
// ============================================================

/// 构造 T2 的 4 个版本 spec
///
/// 演进策略: 逐步增加 retry.max_attempts（鲁棒性↑）+ contracts（覆盖度↑）
/// - v1: 1 contract, retry=1, backoff=100
/// - v2: 1 contract, retry=3, backoff=200
/// - v3: 2 contracts, retry=5, backoff=500
/// - v4: 2 contracts, retry=7, backoff=1000
fn make_t2_versions() -> [HarnessSpec; 4] {
    let base_name = "severity-bug-fix";
    let base_order = ["Architect.propose", "Tester.verify"];

    let make_spec =
        |version: u32, parent: Option<u32>, contracts: Vec<ContractSpec>, retry_attempts: u32| {
            HarnessSpec {
                meta: HarnessMeta {
                    name: base_name.to_string(),
                    version,
                    immutable: false,
                    parent,
                    task_type: Some("bug_fix".to_string()),
                },
                contracts,
                hops: vec![hop("verify_severity", &base_order, &["severity_correct"])],
                retry: RetryPolicy {
                    max_attempts: retry_attempts,
                    backoff_ms: 100 * (2_u64.pow(version - 1)),
                    exponential: true,
                },
                auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
            }
        };

    let v1 = make_spec(
        1,
        None,
        vec![contract("severity_correct", "severity_must_match_enum")],
        1,
    );
    let v2 = make_spec(
        2,
        Some(1),
        vec![contract("severity_correct", "severity_must_match_enum")],
        3,
    );
    let v3 = make_spec(
        3,
        Some(2),
        vec![
            contract("severity_correct", "severity_must_match_enum"),
            contract("severity_propagated", "severity_must_propagate_to_event"),
        ],
        5,
    );
    let v4 = make_spec(
        4,
        Some(3),
        vec![
            contract("severity_correct", "severity_must_match_enum"),
            contract("severity_propagated", "severity_must_propagate_to_event"),
        ],
        7,
    );

    [v1, v2, v3, v4]
}

// ============================================================
// T3: feature_add — 添加 select_arm_with_exploration()
// ============================================================

/// 构造 T3 的 4 个版本 spec
///
/// 演进策略: 逐步增加 contracts（参数约束↑）+ hops（验证步骤↑）
/// - v1: 1 contract, 1 hop
/// - v2: 2 contracts, 1 hop
/// - v3: 3 contracts, 2 hops
/// - v4: 4 contracts, 3 hops
fn make_t3_versions() -> [HarnessSpec; 4] {
    let base_name = "select-arm-exploration";
    let base_order = ["Architect.design", "Tester.validate"];

    let make_spec =
        |version: u32, parent: Option<u32>, contracts: Vec<ContractSpec>, hops: Vec<HopSpec>| {
            HarnessSpec {
                meta: HarnessMeta {
                    name: base_name.to_string(),
                    version,
                    immutable: false,
                    parent,
                    task_type: Some("feature_add".to_string()),
                },
                contracts,
                hops,
                retry: retry_policy(3),
                auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
            }
        };

    let v1 = make_spec(
        1,
        None,
        vec![contract("returns_valid_arm", "must_return_arm_in_range")],
        vec![hop("select_arm", &base_order, &["returns_valid_arm"])],
    );

    let v2 = make_spec(
        2,
        Some(1),
        vec![
            contract("returns_valid_arm", "must_return_arm_in_range"),
            contract("epsilon_in_range", "epsilon_must_be_in_zero_one"),
        ],
        vec![hop(
            "select_arm",
            &base_order,
            &["returns_valid_arm", "epsilon_in_range"],
        )],
    );

    let v3 = make_spec(
        3,
        Some(2),
        vec![
            contract("returns_valid_arm", "must_return_arm_in_range"),
            contract("epsilon_in_range", "epsilon_must_be_in_zero_one"),
            contract("explores_when_cold", "must_explore_when_uncertain"),
        ],
        vec![
            hop(
                "select_arm",
                &base_order,
                &["returns_valid_arm", "epsilon_in_range"],
            ),
            hop("verify_exploration", &base_order, &["explores_when_cold"]),
        ],
    );

    let v4 = make_spec(
        4,
        Some(3),
        vec![
            contract("returns_valid_arm", "must_return_arm_in_range"),
            contract("epsilon_in_range", "epsilon_must_be_in_zero_one"),
            contract("explores_when_cold", "must_explore_when_uncertain"),
            contract("logs_selection", "must_log_arm_selection"),
        ],
        vec![
            hop(
                "select_arm",
                &base_order,
                &["returns_valid_arm", "epsilon_in_range"],
            ),
            hop("verify_exploration", &base_order, &["explores_when_cold"]),
            hop("verify_logging", &base_order, &["logs_selection"]),
        ],
    );

    [v1, v2, v3, v4]
}

// ============================================================
// T4: test_write — 补充 immune_system 边界测试
// ============================================================

/// 构造 T4 的 4 个版本 spec
///
/// 演进策略: 逐步增加 contracts（测试断言数↑）
/// - v1: 1 contract (基础断言)
/// - v2: 2 contracts
/// - v3: 3 contracts
/// - v4: 5 contracts (完整边界覆盖)
fn make_t4_versions() -> [HarnessSpec; 4] {
    let base_name = "immune-boundary-tests";
    let base_order = ["Tester.write", "Reviewer.audit"];

    let make_spec = |version: u32, parent: Option<u32>, contracts: Vec<ContractSpec>| HarnessSpec {
        meta: HarnessMeta {
            name: base_name.to_string(),
            version,
            immutable: false,
            parent,
            task_type: Some("test_write".to_string()),
        },
        contracts,
        hops: vec![hop("run_boundary_tests", &base_order, &["test_passes"])],
        retry: retry_policy(2),
        auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
    };

    let v1 = make_spec(
        1,
        None,
        vec![contract("test_passes", "boundary_test_must_pass")],
    );

    let v2 = make_spec(
        2,
        Some(1),
        vec![
            contract("test_passes", "boundary_test_must_pass"),
            contract("empty_input_handled", "must_handle_empty_input"),
        ],
    );

    let v3 = make_spec(
        3,
        Some(2),
        vec![
            contract("test_passes", "boundary_test_must_pass"),
            contract("empty_input_handled", "must_handle_empty_input"),
            contract("max_input_handled", "must_handle_max_input"),
        ],
    );

    let v4 = make_spec(
        4,
        Some(3),
        vec![
            contract("test_passes", "boundary_test_must_pass"),
            contract("empty_input_handled", "must_handle_empty_input"),
            contract("max_input_handled", "must_handle_max_input"),
            contract("concurrent_access_safe", "must_be_thread_safe"),
            contract("panic_recovery_works", "must_recover_from_panic"),
        ],
    );

    [v1, v2, v3, v4]
}

// ============================================================
// T5: docs_gen — 生成 ADR 草案
// ============================================================

/// 构造 T5 的 4 个版本 spec
///
/// 演进策略: 逐步增加 contracts（文档完整度↑）+ hops（审阅步骤↑）
/// - v1: 1 contract, 1 hop
/// - v2: 2 contracts, 1 hop
/// - v3: 2 contracts, 2 hops
/// - v4: 3 contracts, 2 hops
fn make_t5_versions() -> [HarnessSpec; 4] {
    let base_name = "adr-draft-generation";
    let base_order = ["Architect.draft", "Reviewer.audit"];

    let make_spec =
        |version: u32, parent: Option<u32>, contracts: Vec<ContractSpec>, hops: Vec<HopSpec>| {
            HarnessSpec {
                meta: HarnessMeta {
                    name: base_name.to_string(),
                    version,
                    immutable: false,
                    parent,
                    task_type: Some("docs_gen".to_string()),
                },
                contracts,
                hops,
                retry: retry_policy(2),
                auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
            }
        };

    let v1 = make_spec(
        1,
        None,
        vec![contract("adr_has_title", "adr_must_have_title")],
        vec![hop("generate_draft", &base_order, &["adr_has_title"])],
    );

    let v2 = make_spec(
        2,
        Some(1),
        vec![
            contract("adr_has_title", "adr_must_have_title"),
            contract("adr_has_context", "adr_must_include_context"),
        ],
        vec![hop(
            "generate_draft",
            &base_order,
            &["adr_has_title", "adr_has_context"],
        )],
    );

    let v3 = make_spec(
        3,
        Some(2),
        vec![
            contract("adr_has_title", "adr_must_have_title"),
            contract("adr_has_context", "adr_must_include_context"),
        ],
        vec![
            hop(
                "generate_draft",
                &base_order,
                &["adr_has_title", "adr_has_context"],
            ),
            hop("review_draft", &base_order, &["adr_has_title"]),
        ],
    );

    let v4 = make_spec(
        4,
        Some(3),
        vec![
            contract("adr_has_title", "adr_must_have_title"),
            contract("adr_has_context", "adr_must_include_context"),
            contract("adr_has_consequences", "adr_must_document_consequences"),
        ],
        vec![
            hop(
                "generate_draft",
                &base_order,
                &["adr_has_title", "adr_has_context"],
            ),
            hop("review_draft", &base_order, &["adr_has_consequences"]),
        ],
    );

    [v1, v2, v3, v4]
}

// ============================================================
// quest_set_v1 — 5 任务集主入口
// ============================================================

/// 返回 P5.5.1 定义的 5 任务集
///
/// # 任务清单
///
/// | ID | 类型 | 内容 |
/// |----|------|------|
/// | T1 | code_refactor | 重构 Quest::new() |
/// | T2 | bug_fix | 修复 severity 误判 |
/// | T3 | feature_add | 添加 select_arm_with_exploration() |
/// | T4 | test_write | 补充 immune_system 边界测试 |
/// | T5 | docs_gen | 生成 ADR 草案 |
///
/// # 确定性保证
///
/// 每个任务的 v(i+1) 在评分函数 `score = contracts*10 + retry*2 + hops*5`
/// 下严格大于 v(i)，确保 DeterministicJudgeClient 裁决 Current 胜出。
pub fn quest_set_v1() -> Vec<QuestTask> {
    vec![
        QuestTask {
            task_id: "T1",
            task_type: "code_refactor",
            task_content: "重构 Quest::new() — 提取参数校验到独立函数",
            versions: make_t1_versions(),
        },
        QuestTask {
            task_id: "T2",
            task_type: "bug_fix",
            task_content: "修复 severity 误判 — EventSeverity 与 NexusEvent 映射错误",
            versions: make_t2_versions(),
        },
        QuestTask {
            task_id: "T3",
            task_type: "feature_add",
            task_content: "添加 select_arm_with_exploration() — epsilon-greedy 探索",
            versions: make_t3_versions(),
        },
        QuestTask {
            task_id: "T4",
            task_type: "test_write",
            task_content: "补充 immune_system 边界测试 — 空输入/最大输入/并发/panic 恢复",
            versions: make_t4_versions(),
        },
        QuestTask {
            task_id: "T5",
            task_type: "docs_gen",
            task_content: "生成 ADR 草案 — ImmuneSystem facade 决策记录",
            versions: make_t5_versions(),
        },
    ]
}

// ============================================================
// 评分函数（与 DeterministicJudgeClient 对齐）
// ============================================================

/// 计算 spec 的确定性评分
///
/// 评分公式: `contracts.len() * 10 + retry.max_attempts * 2 + hops.len() * 5`
///
/// # 权重设计
/// - contracts × 10: 契约覆盖度最重要（每个契约 +10 分）
/// - hops × 5: 执行步骤完整度次之（每个 hop +5 分）
/// - retry × 2: 重试鲁棒性第三（每次重试 +2 分）
///
/// # 用途
/// - 供 DeterministicJudgeClient 比较相邻版本优劣
/// - 供测试验证 v(i+1) 严格优于 v(i)
pub fn spec_score(spec: &HarnessSpec) -> u32 {
    spec.contracts.len() as u32 * 10 + spec.retry.max_attempts * 2 + spec.hops.len() as u32 * 5
}

// ============================================================
// 单元测试 — 验证任务集定义正确性
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 5 任务集包含 5 个任务
    #[test]
    fn test_quest_set_v1_has_5_tasks() {
        let tasks = quest_set_v1();
        assert_eq!(tasks.len(), 5, "任务集应包含 5 个任务");
    }

    /// 验证 5 类任务类型全覆盖
    #[test]
    fn test_quest_set_v1_covers_5_task_types() {
        let tasks = quest_set_v1();
        let types: Vec<&str> = tasks.iter().map(|t| t.task_type()).collect();
        assert!(types.contains(&"code_refactor"), "缺少 code_refactor");
        assert!(types.contains(&"bug_fix"), "缺少 bug_fix");
        assert!(types.contains(&"feature_add"), "缺少 feature_add");
        assert!(types.contains(&"test_write"), "缺少 test_write");
        assert!(types.contains(&"docs_gen"), "缺少 docs_gen");
    }

    /// 验证每个任务有 4 个版本
    #[test]
    fn test_each_task_has_4_versions() {
        let tasks = quest_set_v1();
        for task in &tasks {
            assert_eq!(
                task.versions.len(),
                4,
                "任务 {} 应有 4 个版本，实际 {}",
                task.task_id(),
                task.versions.len()
            );
        }
    }

    /// 验证所有 spec 通过 validate()
    #[test]
    fn test_all_specs_pass_validation() {
        let tasks = quest_set_v1();
        for task in &tasks {
            for (idx, spec) in task.versions.iter().enumerate() {
                let result = spec.validate();
                assert!(
                    result.is_ok(),
                    "任务 {} 版本 v{} validate() 失败: {:?}",
                    task.task_id(),
                    idx + 1,
                    result.err()
                );
            }
        }
    }

    /// 验证 v(i+1) 评分严格大于 v(i)（确定性改进保证）
    #[test]
    fn test_each_version_is_strictly_better() {
        let tasks = quest_set_v1();
        for task in &tasks {
            for i in 0..3 {
                let score_v_i = spec_score(&task.versions[i]);
                let score_v_i_plus_1 = spec_score(&task.versions[i + 1]);
                assert!(
                    score_v_i_plus_1 > score_v_i,
                    "任务 {} v{} 评分 {} 应严格大于 v{} 评分 {}",
                    task.task_id(),
                    i + 2,
                    score_v_i_plus_1,
                    i + 1,
                    score_v_i
                );
            }
        }
    }

    /// 验证版本号单调递增且 parent 链正确
    #[test]
    fn test_version_numbers_and_parent_chain() {
        let tasks = quest_set_v1();
        for task in &tasks {
            for (idx, spec) in task.versions.iter().enumerate() {
                let expected_version = (idx + 1) as u32;
                assert_eq!(
                    spec.meta.version,
                    expected_version,
                    "任务 {} 版本[{}] 的 meta.version 应为 {}",
                    task.task_id(),
                    idx,
                    expected_version
                );
                let expected_parent = if idx == 0 { None } else { Some(idx as u32) };
                assert_eq!(
                    spec.meta.parent,
                    expected_parent,
                    "任务 {} 版本[{}] 的 meta.parent 应为 {:?}",
                    task.task_id(),
                    idx,
                    expected_parent
                );
            }
        }
    }

    /// 验证 spec_score 评分函数与设计文档对齐
    #[test]
    fn test_spec_score_weights() {
        // 构造一个 contracts=2, hops=3, retry=5 的 spec
        let spec = HarnessSpec {
            meta: HarnessMeta {
                name: "test".to_string(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: None,
            },
            contracts: vec![contract("c1", "p1"), contract("c2", "p2")],
            hops: vec![
                hop("h1", &["a.b"], &["c1"]),
                hop("h2", &["a.b"], &["c1"]),
                hop("h3", &["a.b"], &["c1"]),
            ],
            retry: retry_policy(5),
            auxiliary: Some(ACCEPTANCE_GATES_AUX.to_string()),
        };
        // 期望: 2*10 + 5*2 + 3*5 = 20 + 10 + 15 = 45
        assert_eq!(spec_score(&spec), 45);
    }
}
