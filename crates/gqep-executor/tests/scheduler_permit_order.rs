//! S1（`ADR-234` A1-L1 结构判据 / `ADR-233` 追加节九 D3）：
//! 慢 provider 的**等待者**不得把全局车道槽位钉住。
//!
//! 判据形状是**完成序**，不是墙钟、也不是 `SchedSnapshot::peak_in_flight`：
//! 后者是两车道合并的高水位（`scheduler.rs:25-26`），分解不出"车道被谁占"，
//! 用它证这条命题属于"用不可分解的量证需要分解的命题"（`ADR-234` A2-1 明令禁止）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolDescriptor, ToolError, ToolInvocation, ToolOrigin, ToolPriority,
    ToolProvider, ToolRegistry, ToolRuntime, ToolRuntimeConfig, ToolStatus,
};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;

/// 一个 provider 记一种速度；完成即把自己的 id 追加进共享日志 ⇒ 日志顺序即**完成序**。
struct SpeedProvider {
    id: &'static str,
    sleep_ms: u64,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl SpeedProvider {
    fn new(id: &'static str, sleep_ms: u64, log: Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self { id, sleep_ms, log }
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: format!("{}.t", self.id),
            provider: self.id.to_string(),
            description: "speed probe".to_string(),
            side_effect: SideEffectDecl::ReadOnly,
            timeout_ms: 5_000,
            max_concurrency: 8,
            input_schema: None,
        }
    }
}

#[async_trait]
impl ToolProvider for SpeedProvider {
    fn provider_id(&self) -> &str {
        self.id
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![self.descriptor()]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        _cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        if self.sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        }
        self.log.lock().unwrap().push(self.id);
        Ok(inv.args_json.clone())
    }
}

fn invocation(tool: &str, call_id: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: call_id.to_string(),
        tool: tool.to_string(),
        args_json: "{}".to_string(),
        timeout_ms: None,
        // 与 LLM 路径同车道：`Normal` 与 `Background` 都落 background（`scheduler.rs:90-95`）。
        // 用 `Interactive` 会让本用例测到一条生产上不走的车道，等于自证个空。
        priority: ToolPriority::Normal,
        origin: ToolOrigin::Llm,
        quest_id: None,
        task_id: None,
        approved: true,
    }
}

fn runtime(
    providers: Vec<Arc<dyn ToolProvider>>,
    lane: usize,
    per_provider: usize,
) -> Arc<ToolRuntime> {
    let mut registry = ToolRegistry::new();
    for provider in providers {
        registry.register(provider).expect("register provider");
    }
    let scheduler = Arc::new(BoundedToolScheduler::new(lane, lane, 64, per_provider));
    let cfg = ToolRuntimeConfig {
        interactive_inflight: lane,
        background_inflight: lane,
        max_waiters: 64,
        max_provider_inflight: per_provider,
        ..ToolRuntimeConfig::default()
    };
    Arc::new(ToolRuntime::new(Arc::new(registry), None, scheduler, cfg))
}

/// 病形：车道 4、每 provider 2；4 条慢调用（400ms）先入队，4 条快调用随后。
/// 旧顺序（车道许可先取）下 4 条慢调用先把车道取满，其中 2 条睡、2 条**持着车道**
/// 等 provider 许可 ⇒ 快调用一条也进不去，首个完成者必为慢。
/// 新顺序（provider 先取）下慢的等待者不持车道 ⇒ 快调用立即完成。
#[tokio::test]
async fn slow_provider_waiters_do_not_pin_the_lane() {
    const LANE: usize = 4;
    const PER_PROVIDER: usize = 2;
    const SLOW_MS: u64 = 400;

    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let rt = runtime(
        vec![
            Arc::new(SpeedProvider::new("p-slow", SLOW_MS, Arc::clone(&log))),
            Arc::new(SpeedProvider::new("p-fast", 0, Arc::clone(&log))),
        ],
        LANE,
        PER_PROVIDER,
    );

    let mut batch: Vec<ToolInvocation> = (0..PER_PROVIDER * 2)
        .map(|i| invocation("p-slow.t", &format!("s{i}")))
        .collect();
    batch.extend((0..PER_PROVIDER * 2).map(|i| invocation("p-fast.t", &format!("f{i}"))));

    let outcomes = rt.invoke_batch(batch, &CancellationToken::new()).await;
    let order = log.lock().unwrap().clone();

    // 非空洞前置（ADR-234 A5）：两侧都必须真的完成过，否则"快在前"可以是零完成的空转。
    assert_eq!(
        order.len(),
        8,
        "八条调用都应完成；日志为空/不足 ⇒ 探针本身失效"
    );
    assert_eq!(
        order.iter().filter(|p| **p == "p-fast").count(),
        4,
        "快 provider 的四条都应完成"
    );
    assert_eq!(
        order.iter().filter(|p| **p == "p-slow").count(),
        4,
        "慢 provider 的四条也都应完成（延后不是拒绝）"
    );

    let first_fast = order.iter().position(|p| *p == "p-fast").unwrap();
    let first_slow = order.iter().position(|p| *p == "p-slow").unwrap();
    assert!(
        first_fast < first_slow,
        "完成序第 {first_fast} 位是 p-fast、第 {first_slow} 位是 p-slow ⇒ 实测顺序 {order:?}。\
         慢 provider 的等待者把全局车道钉住了：别的 provider 连一条都挤不进去。"
    );

    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o.status, ToolStatus::Overloaded))
            .count(),
        0,
        "并发受限只能延后，不得把任何一条变成 Overloaded 拒绝"
    );
    assert_eq!(outcomes.len(), 8, "结果不许丢");
}

/// 阳性对照（`ADR-234` A2-2）：同样的排布，但**没有车道竞争**（车道够宽）时
/// 快调用也必须排在前面 —— 证明上一条判据读的是真实完成序，而不是一条恒假/恒真守卫。
/// 若日志机制坏了（永远不记或记反），这条会先红。
#[tokio::test]
async fn control_probe_reads_real_completion_order() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let rt = runtime(
        vec![
            Arc::new(SpeedProvider::new("p-slow", 150, Arc::clone(&log))),
            Arc::new(SpeedProvider::new("p-fast", 0, Arc::clone(&log))),
        ],
        16, // 车道远大于调用数 ⇒ 无车道竞争
        8,
    );

    let batch = vec![
        invocation("p-slow.t", "c-s1"),
        invocation("p-slow.t", "c-s2"),
        invocation("p-fast.t", "c-f1"),
        invocation("p-fast.t", "c-f2"),
    ];
    rt.invoke_batch(batch, &CancellationToken::new()).await;
    let order = log.lock().unwrap().clone();

    assert_eq!(order.len(), 4);
    assert_eq!(
        order.iter().position(|p| *p == "p-fast").unwrap(),
        0,
        "无竞争时首个完成者必须是快 provider；实测顺序 {order:?} ⇒ 若不符，说明日志/完成序探针本身不可信"
    );
}
