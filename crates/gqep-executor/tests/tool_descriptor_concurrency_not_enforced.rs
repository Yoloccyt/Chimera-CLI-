//! ADR-234 D4 —— `ToolDescriptor::max_concurrency` **声明未强制**的阳性对照锁
//!
//! 对应架构层：L7 Execution（gqep-executor）
//! 上游决议：`docs/architecture/ADR-234-perf-parallelism-verification-limits.md` D4
//!
//! # 这条测试证明什么
//! 描述符上的 `max_concurrency` 没有任何读取方：per-provider 的并发额度实际来自
//! `ToolRuntimeConfig::max_provider_inflight`（见 `scheduler.rs` 的
//! `provider_semaphore`，它只用 provider id 作键、用 config 值作容量）。
//! 于是"文档里有这个字段"与"这个字段生效"是两件事，本仓此前只有前者。
//!
//! # 为什么是"对照"而不是"断言字段无效"
//! 两个 provider 只差声明值（1 与 8）。若字段被读取，两者峰值必然不同；
//! 实测**相同**才证明它没被读取。再加一条 `峰值 == config 额度` 的断言，
//! 把"两个峰值恰好都等于 1"这种巧合（可能来自车道限流、也可能来自
//! provider 自身没并发）一并排除 —— 缺了这条，等值断言是恒真的空转。
//!
//! # 将来真要启用这个字段时
//! 本测试会红。那是**预期的**：启用属收紧（实测声明值 proc=2 < 现额度 8，
//! 会直接压低进程面并发），必须由治理裁决显式删除/改写本文件，
//! 而不是让它静默腐烂成一个总在绿的装饰。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolDescriptor, ToolError, ToolInvocation, ToolOrigin, ToolPriority,
    ToolProvider, ToolRegistry, ToolRuntime, ToolRuntimeConfig, ToolStatus,
};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;

/// per-provider 额度（config 侧），也是本测试预期的真实峰值
const PROVIDER_BOUND: usize = 4;
/// 每个 provider 投多少条（须 > PROVIDER_BOUND，否则限流不发生，峰值等于条数）
const PER_PROVIDER_CALLS: usize = 6;
/// 车道额度取大值，确保"峰值 = PROVIDER_BOUND"只可能来自 provider 额度而非车道
const LANE_BOUND: usize = 32;

/// 记录自身在飞峰值的 provider；两个实例之间**只有声明的 `max_concurrency` 不同**
struct DeclaringProvider {
    id: &'static str,
    declared_max: usize,
    live: AtomicUsize,
    peak: AtomicUsize,
}

impl DeclaringProvider {
    fn new(id: &'static str, declared_max: usize) -> Self {
        Self {
            id,
            declared_max,
            live: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn invocations(&self) -> Vec<ToolInvocation> {
        (0..PER_PROVIDER_CALLS)
            .map(|i| ToolInvocation {
                call_id: format!("{}-{i}", self.id),
                tool: format!("{}.op", self.id),
                args_json: "{}".to_string(),
                timeout_ms: None,
                priority: ToolPriority::Normal,
                origin: ToolOrigin::Cli,
                quest_id: None,
                task_id: None,
                approved: false,
            })
            .collect()
    }
}

#[async_trait]
impl ToolProvider for DeclaringProvider {
    fn provider_id(&self) -> &str {
        self.id
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor {
            name: format!("{}.op", self.id),
            provider: self.id.to_string(),
            description: "declares a max_concurrency nobody reads".to_string(),
            side_effect: SideEffectDecl::ReadOnly,
            timeout_ms: 5_000,
            // 本测试唯一的自变量：1 与 PROVIDER_BOUND*2
            max_concurrency: self.declared_max,
            input_schema: None,
        }]
    }

    async fn invoke(
        &self,
        _inv: &ToolInvocation,
        _cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        // 足够长，让后到的调用在额度用尽前必然排队（峰值才由额度决定，而非由收尾速度决定）
        tokio::time::sleep(Duration::from_millis(40)).await;
        self.live.fetch_sub(1, Ordering::SeqCst);
        Ok("ok".to_string())
    }
}

fn runtime(providers: Vec<Arc<DeclaringProvider>>) -> Arc<ToolRuntime> {
    let mut registry = ToolRegistry::new();
    for provider in providers {
        registry.register(provider).expect("register provider");
    }
    let scheduler = Arc::new(BoundedToolScheduler::new(
        LANE_BOUND,
        LANE_BOUND,
        256,
        PROVIDER_BOUND,
    ));
    Arc::new(ToolRuntime::new(
        Arc::new(registry),
        None,
        scheduler,
        ToolRuntimeConfig {
            interactive_inflight: LANE_BOUND,
            background_inflight: LANE_BOUND,
            max_waiters: 256,
            max_provider_inflight: PROVIDER_BOUND,
            default_timeout_ms: 5_000,
            max_payload_bytes: 4_096,
            max_batch: 64,
        },
    ))
}

/// 声明 1 与声明 8 的两个 provider，在飞峰值必须**相同且等于 config 额度**。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declared_max_concurrency_is_not_read() {
    let tight = Arc::new(DeclaringProvider::new("p-tight", 1));
    let loose = Arc::new(DeclaringProvider::new("p-loose", PROVIDER_BOUND * 2));
    let runtime = runtime(vec![Arc::clone(&tight), Arc::clone(&loose)]);
    let cancel = CancellationToken::new();

    // 两批同时提交：任何一批单独跑满都可能被另一批的排队掩盖，同批跑才对照得干净
    let (a, b) = tokio::join!(
        runtime.invoke_batch(tight.invocations(), &cancel),
        runtime.invoke_batch(loose.invocations(), &cancel),
    );

    for outcomes in [&a, &b] {
        assert_eq!(outcomes.len(), PER_PROVIDER_CALLS, "结果数必须等于派发数");
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| o.status == ToolStatus::Overloaded)
                .count(),
            0,
            "限流只延后，不得把调用变成 Overloaded"
        );
    }

    let tight_peak = tight.peak();
    let loose_peak = loose.peak();

    // 判据 ①（非空转）：峰值必须真的达到过并发，否则下面的等值断言恒真
    assert!(
        tight_peak > 1 && loose_peak > 1,
        "阳性对照失效：峰值 {tight_peak}/{loose_peak} 未超过 1，说明根本没有并发，\n\
         '两者相等'就只是'两边都被按住'的巧合，本文件将永远绿而证明不了任何东西"
    );
    // 判据 ②（字段未被读取）：声明值差 16 倍，峰值必须相同
    assert_eq!(
        tight_peak,
        loose_peak,
        "声明 max_concurrency=1 与 ={} 的峰值必须相同（实测 {tight_peak} vs {loose_peak}）；\n\
         一旦不同，说明这个字段开始被消费 —— 那是收紧，要重走 ADR-234 D4 的裁决",
        PROVIDER_BOUND * 2
    );
    // 判据 ③（归因）：相等的那个值必须来自 config 额度，而不是别处的挤压
    assert_eq!(
        tight_peak, PROVIDER_BOUND,
        "峰值应恰为 ToolRuntimeConfig::max_provider_inflight={PROVIDER_BOUND}\
         （实测 {tight_peak}）；小于它说明瓶颈在别处（车道/取消），本对照的归因不成立"
    );
}
