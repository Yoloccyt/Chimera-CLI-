//! `chimera quest execute` 端到端契约（Phase 5 T5.4/T5.5，ADR-231）
//!
//! 两类测试：
//! - **库层**（进程内）：DAG 就绪队列、级联、取消、租约生命周期 —— 并发一律用
//!   `Barrier`/峰值计数器等**结构证据**判定，不测墙钟（共享宿主上墙钟会假红）。
//! - **CLI 层**（子进程）：灰度门默认 off、`--json` 计数、`--strict`。子进程各自
//!   带 env ⇒ 不与库层争抢进程级环境变量。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use event_bus::EventBus;
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolDescriptor, ToolError, ToolInvocation, ToolProvider, ToolRegistry,
    ToolRuntime, ToolRuntimeConfig,
};
use mas_sched::{PeerScheduler, SimplePeerScheduler};
use nexus_contracts::tool_plan::SideEffectDecl;
use nexus_core::UserIntent;
use quest_engine::{DagMode, QuestConfig, QuestEngine};
use serde_json::Value;
use tokio::sync::Barrier;
use tokio_util::sync::CancellationToken;

use chimera_cli::quest_exec::{
    JsonPlanPlanner, QuestExecConfig, QuestExecMode, QuestExecutor, TaskPlanner,
};

// ============================================================
// 通用 helper
// ============================================================

fn plan_fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/quest_plans.json")
        .to_string_lossy()
        .into_owned()
}

/// 子进程跑真实二进制（CLI 层测试）
fn cli(envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_chimera"));
    command.args(args).env("RUST_LOG", "error");
    // 基线确定性：本 target 的语义门必须显式设定，不受宿主残留影响
    command.env_remove("CHIMERA_QUEST_EXEC");
    command.env_remove("CHIMERA_QUEST_DAG");
    command.env_remove("CHIMERA_TOOL_PLANE");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run chimera binary")
}

fn parse(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// `task_id -> 单节点计划` 的 JSON 文本（同时验证 JsonPlanPlanner 的解析面）
fn plans_json(entries: &[(&str, &str)]) -> String {
    let parts: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(index, (task_id, tool))| {
            format!(
                r#""{task_id}": {{"id":"p{index}","nodes":[{{"id":"n0","op":"tool_call","tool_name":"{tool}","args_json":"{{}}"}}],"edges":[]}}"#
            )
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// 只注册 `probe.probe` 的探针 provider —— 并发证明靠它
#[derive(Clone)]
struct ProbeProvider {
    /// 到达即阻塞，直到 N 个任务同时在飞（串行则永不到齐）
    barrier: Option<Arc<Barrier>>,
    live: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl ProbeProvider {
    fn new(parties: Option<usize>) -> Self {
        Self {
            barrier: parties.map(|n| Arc::new(Barrier::new(n))),
            live: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 历史峰值在飞数
    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ToolProvider for ProbeProvider {
    fn provider_id(&self) -> &str {
        "probe"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor {
            name: "probe.probe".to_string(),
            provider: "probe".to_string(),
            description: "In-flight probe (test only)".to_string(),
            side_effect: SideEffectDecl::ReadOnly,
            timeout_ms: 5_000,
            max_concurrency: 16,
            input_schema: None,
        }]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        _cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        match &self.barrier {
            // 同一个 Arc<Barrier> 被所有在飞任务共享 ⇒ 到齐才放行
            Some(barrier) => {
                barrier.wait().await;
            }
            None => tokio::task::yield_now().await,
        }
        self.live.fetch_sub(1, Ordering::SeqCst);
        Ok(inv.args_json.clone())
    }
}

/// 装配一个只做工具执行的 runtime（L7 面全开，避免掩盖 L10 的任务窗口）
fn runtime_with(provider: Arc<dyn ToolProvider>) -> Arc<ToolRuntime> {
    let mut registry = ToolRegistry::new();
    registry.register(provider).expect("register provider");
    let scheduler = Arc::new(BoundedToolScheduler::new(16, 16, 256, 16));
    Arc::new(ToolRuntime::new(
        Arc::new(registry),
        None,
        scheduler,
        ToolRuntimeConfig {
            interactive_inflight: 16,
            background_inflight: 16,
            max_waiters: 256,
            max_provider_inflight: 16,
            default_timeout_ms: 5_000,
            max_payload_bytes: 4_096,
            max_batch: 64,
        },
    ))
}

/// 用 echo 工具面（`echo.echo`）—— 需要真实注册面而非 mock 的用例用
fn echo_runtime() -> Arc<ToolRuntime> {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(chimera_cli::tool_plane::EchoToolProvider::new()))
        .expect("register echo");
    let scheduler = Arc::new(BoundedToolScheduler::new(16, 16, 256, 16));
    Arc::new(ToolRuntime::new(
        Arc::new(registry),
        None,
        scheduler,
        ToolRuntimeConfig {
            interactive_inflight: 16,
            background_inflight: 16,
            max_waiters: 256,
            max_provider_inflight: 16,
            default_timeout_ms: 10_000,
            max_payload_bytes: 4_096,
            max_batch: 64,
        },
    ))
}

/// 建 Quest（dag_mode 决定 decompose 是否生成伪链）
async fn make_quest(dag: DagMode, prompt: &str) -> (Arc<QuestEngine>, String) {
    let engine = Arc::new(QuestEngine::with_config(
        EventBus::new(),
        QuestConfig {
            dag_mode: dag,
            ..QuestConfig::default()
        },
    ));
    let intent = UserIntent {
        intent_id: "intent-e2e".to_string(),
        raw_text: prompt.to_string(),
        multimodal_inputs: vec![],
        risk_level: 30,
    };
    let quest = engine.create_quest(intent).await.expect("create quest");
    let id = quest.quest_id.clone();
    (engine, id)
}

fn exec_cfg(dag: DagMode, tasks: usize) -> QuestExecConfig {
    QuestExecConfig {
        mode: QuestExecMode::Apply,
        max_concurrent_tasks: tasks,
        max_concurrent_tools: 8,
        task_timeout_ms: 3_000,
        dag_mode: dag,
    }
}

fn planner(text: &str) -> Arc<dyn TaskPlanner> {
    Arc::new(JsonPlanPlanner::from_json_str(text).expect("plans json"))
}

fn status_of(engine: &Arc<QuestEngine>, quest_id: &str, task_id: &str) -> String {
    let status = engine
        .get_quest(quest_id)
        .expect("quest present")
        .tasks
        .iter()
        .find(|t| t.task_id == task_id)
        .unwrap_or_else(|| panic!("task {task_id} missing"))
        .status;
    // TaskStatus 无 Display（serde 按名匹配），断言取 Debug 的变体名
    format!("{status:?}")
}

// ============================================================
// T5.4 —— DAG / 级联 / 取消 / 租约
// ============================================================

/// 多根并行：3 个无依赖任务必须在 barrier 上到齐（串行永不到齐 ⇒ 超时）
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quest_execute_fans_out_multi_root() {
    let probe = ProbeProvider::new(Some(3));
    let runtime = runtime_with(Arc::new(probe.clone()));
    let (engine, quest_id) = make_quest(DagMode::MultiRoot, "拉取。转换。落盘。").await;
    let quest = engine.get_quest(&quest_id).expect("quest");
    assert_eq!(quest.tasks.len(), 3, "三句 prompt 应分解为 3 个任务");
    assert!(
        quest.tasks.iter().all(|t| t.dependencies.is_empty()),
        "MultiRoot 不得生成合成依赖边"
    );
    let ids: Vec<&str> = quest.tasks.iter().map(|t| t.task_id.as_str()).collect();
    let executor = QuestExecutor::new(
        Arc::clone(&engine),
        runtime,
        planner(&plans_json(
            &ids.iter()
                .map(|id| (*id, "probe.probe"))
                .collect::<Vec<_>>(),
        )),
        Arc::new(SimplePeerScheduler::new()),
        exec_cfg(DagMode::MultiRoot, 3),
    );

    let report = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        executor.run(&quest_id, CancellationToken::new()),
    )
    .await
    .expect("串行会卡死在 Barrier(3) 上 ⇒ 必须超时前返回")
    .expect("run ok");

    assert_eq!(report.completed, 3, "report={report:?}");
    assert_eq!(probe.peak(), 3, "峰值在飞必须等于窗口 3（并行证据）");
}

/// 前置失败必须级联：链上 3 个任务全部记 Failed，且状态写入引擎
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quest_execute_marks_dependents_failed() {
    let (engine, quest_id) = make_quest(DagMode::Linear, "一。二。三。").await;
    // task-0 的计划指向未注册工具 ⇒ NotFound ⇒ fail_fast ⇒ 该任务失败
    let executor = QuestExecutor::new(
        Arc::clone(&engine),
        echo_runtime(),
        planner(&plans_json(&[
            ("task-0", "q.missing"),
            ("task-1", "echo.echo"),
            ("task-2", "echo.echo"),
        ])),
        Arc::new(SimplePeerScheduler::new()),
        exec_cfg(DagMode::Linear, 4),
    );

    let report = executor
        .run(&quest_id, CancellationToken::new())
        .await
        .expect("run ok");

    assert_eq!(report.failed, 3, "report={report:?}");
    assert_eq!(report.completed, 0);
    assert_eq!(
        report.total,
        report.completed + report.failed + report.skipped + report.cancelled
    );
    for id in ["task-0", "task-1", "task-2"] {
        assert_eq!(
            status_of(&engine, &quest_id, id),
            "Failed",
            "{id} 应级联失败"
        );
    }
}

/// 取消：在飞任务收敛为 Cancelled，未启动任务收尾同标
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quest_execute_cancel_marks_cancelled() {
    let (engine, quest_id) = make_quest(DagMode::Linear, "一。二。三。").await;
    // task-0 睡 10s（远超取消触发点），PlanRunner 在节点边界丢弃 ⇒ 任务收敛为 Cancelled
    let slow = r#"{"task-0": {"id":"p0","nodes":[{"id":"n0","op":"tool_call","tool_name":"echo.echo","args_json":"{\"sleep_ms\":10000}"}],"edges":[]},"task-1": {"id":"p1","nodes":[{"id":"n0","op":"tool_call","tool_name":"echo.echo","args_json":"{}"}],"edges":[]},"task-2": {"id":"p2","nodes":[{"id":"n0","op":"tool_call","tool_name":"echo.echo","args_json":"{}"}],"edges":[]}}"#;
    let executor = QuestExecutor::new(
        Arc::clone(&engine),
        echo_runtime(),
        planner(slow),
        Arc::new(SimplePeerScheduler::new()),
        exec_cfg(DagMode::Linear, 4),
    );

    let cancel = CancellationToken::new();
    let firer = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        firer.cancel();
    });

    let report = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        executor.run(&quest_id, cancel),
    )
    .await
    .expect("取消必须在节点边界生效,不得等满 10s")
    .expect("run ok");

    assert_eq!(report.cancelled, 3, "report={report:?}");
    assert_eq!(report.completed, 0);
    for id in ["task-0", "task-1", "task-2"] {
        assert_eq!(
            status_of(&engine, &quest_id, id),
            "Cancelled",
            "{id} 应标 Cancelled"
        );
    }
}

/// 无计划 ⇒ 跳过且不写状态（补计划后可重跑），后继一并跳过
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quest_execute_unplanned_is_skipped() {
    let (engine, quest_id) = make_quest(DagMode::Linear, "一。二。三。").await;
    let executor = QuestExecutor::new(
        Arc::clone(&engine),
        echo_runtime(),
        planner(&plans_json(&[
            ("task-0", "echo.echo"),
            ("task-2", "echo.echo"),
        ])),
        Arc::new(SimplePeerScheduler::new()),
        exec_cfg(DagMode::Linear, 4),
    );

    let report = executor
        .run(&quest_id, CancellationToken::new())
        .await
        .expect("run ok");

    assert_eq!(report.completed, 1, "report={report:?}");
    assert_eq!(report.skipped, 2, "无计划的 task-1 与其后继都算跳过");
    assert_eq!(report.failed, 0, "跳过不是失败");
    assert_eq!(status_of(&engine, &quest_id, "task-0"), "Completed");
    assert_eq!(
        status_of(&engine, &quest_id, "task-1"),
        "Pending",
        "跳过必须保持 Pending,否则补计划也重跑不了"
    );
    assert_eq!(status_of(&engine, &quest_id, "task-2"), "Pending");
}

/// 租约生命周期：完成即 handoff(HANDOFF) ⇒ 不残留租约、不占 peer 并发额度
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quest_execute_lease_handoff_on_completion() {
    let (engine, quest_id) = make_quest(DagMode::MultiRoot, "一。二。三。").await;
    let sched = Arc::new(SimplePeerScheduler::new());
    let executor = QuestExecutor::new(
        Arc::clone(&engine),
        echo_runtime(),
        planner(&plans_json(&[
            ("task-0", "echo.echo"),
            ("task-1", "echo.echo"),
            ("task-2", "echo.echo"),
        ])),
        Arc::clone(&sched),
        exec_cfg(DagMode::MultiRoot, 3),
    );

    let report = executor
        .run(&quest_id, CancellationToken::new())
        .await
        .expect("run ok");
    assert_eq!(report.completed, 3, "report={report:?}");
    assert_eq!(sched.lease_count(), 0, "全部任务完成必须释放租约");
    assert!(
        sched.reap_expired().is_empty(),
        "无过期租约可回收（回收面未被误当作释放路径）"
    );
}

/// 并发状态更新不丢：两任务同时在飞,引擎两条状态都要落住
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quest_execute_concurrent_status_updates() {
    let probe = ProbeProvider::new(Some(2));
    let runtime = runtime_with(Arc::new(probe.clone()));
    let (engine, quest_id) = make_quest(DagMode::MultiRoot, "一。二。").await;
    let executor = QuestExecutor::new(
        Arc::clone(&engine),
        runtime,
        planner(&plans_json(&[
            ("task-0", "probe.probe"),
            ("task-1", "probe.probe"),
        ])),
        Arc::new(SimplePeerScheduler::new()),
        exec_cfg(DagMode::MultiRoot, 2),
    );

    let report = executor
        .run(&quest_id, CancellationToken::new())
        .await
        .expect("run ok");

    assert_eq!(probe.peak(), 2, "两任务必须同时在飞");
    assert_eq!(report.completed, 2, "report={report:?}");
    assert_eq!(status_of(&engine, &quest_id, "task-0"), "Completed");
    assert_eq!(status_of(&engine, &quest_id, "task-1"), "Completed");
}

/// planner 侧的集合语义自检（`--strict` 的门就靠 `contains` 判定）
#[test]
fn json_planner_reports_membership() {
    let p = JsonPlanPlanner::from_json_str(&plans_json(&[("task-0", "echo.echo")])).expect("parse");
    assert_eq!(p.len(), 1);
    assert!(!p.is_empty());
    assert!(p.contains("task-0"));
    assert!(!p.contains("task-1"));
}

// ============================================================
// T5.5 —— CLI 面（子进程,各自 env）
// ============================================================

/// 默认关闭 ⇒ ConfigError(退出码 1) 且提示启用方式
#[test]
fn quest_execute_off_errors_with_hint() {
    let output = cli(
        &[],
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "--yes",
            "quest",
            "execute",
            "--plans",
            &plan_fixture(),
            "--prompt",
            "alpha.",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "默认必须拒绝执行");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CHIMERA_QUEST_EXEC"),
        "提示必须点名 env 开关, stderr={stderr}"
    );
}

/// apply + `--json` ⇒ 计数账本（total = completed+failed+skipped+cancelled）
#[test]
fn quest_execute_json_reports_counts() {
    let output = cli(
        &[("CHIMERA_QUEST_EXEC", "apply")],
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "--yes",
            "quest",
            "execute",
            "--plans",
            &plan_fixture(),
            "--prompt",
            "先取数。",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse(&output);
    assert_eq!(json["status"], "ok");
    let data = &json["data"];
    assert_eq!(data["total"], 1, "单句 prompt = 1 个任务");
    assert_eq!(data["completed"], 1, "data={data}");
    assert_eq!(data["failed"], 0);
    assert_eq!(data["skipped"], 0);
    assert_eq!(data["cancelled"], 0);
}

/// `--strict`：任务表里有任务无计划 ⇒ 红，且不执行
#[test]
fn quest_execute_strict_fails_on_unplanned() {
    let output = cli(
        &[("CHIMERA_QUEST_EXEC", "apply")],
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "--yes",
            "quest",
            "execute",
            "--plans",
            &plan_fixture(),
            "--prompt",
            "一。二。三。四。",
            "--strict",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "四句 prompt 只有三个计划");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("无计划") || stderr.contains("task-3"),
        "必须点名缺计划的任务, stderr={stderr}"
    );
}

/// 缺 `--prompt` 与 `--id` ⇒ 直接拒绝（不猜默认值）
#[test]
fn quest_execute_requires_prompt_or_id() {
    let output = cli(
        &[("CHIMERA_QUEST_EXEC", "apply")],
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "--yes",
            "quest",
            "execute",
            "--plans",
            &plan_fixture(),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--prompt") || stderr.contains("--id"),
        "stderr={stderr}"
    );
}
