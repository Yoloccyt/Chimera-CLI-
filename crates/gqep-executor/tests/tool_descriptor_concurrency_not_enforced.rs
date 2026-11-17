//! ADR-234 D4 —— `ToolDescriptor::max_concurrency` **声明生效**的翻转锁
//!
//! 对应架构层：L7 Execution（gqep-executor）
//! 上游决议：`docs/architecture/ADR-234-perf-parallelism-verification-limits.md` D4
//! 翻转裁决：L2 工具层重构方案 §7 W5a（2026-11-17 批）+ ADR-32X —— per-provider
//! 额度 = **min(config 值, descriptor 声明)**（`scheduler.rs::provider_semaphore`）。
//!
//! # 文件名说明
//!
//! 文件名保留历史锁名 `not_enforced`（重命名需同步 bench/gate 引用面，随
//! W7 批处理）；**内容已是生效断言**（语义与文件名相反是刻意的——文件名
//! 记录的是「曾经锁过的状态」，doc 注释记录现状）。
//!
//! # 本文件历史（「未实现」对照 → 「生效」翻转）
//!
//! 本文件原名 `tool_descriptor_concurrency_not_enforced`：证明声明值**无读取方**
//! （两个声明值差 16 倍的 provider 峰值相同且等于 config 额度）。W5a 接通后该
//! 「未实现」状态不复存在——按原文件 §将来真要启用时的裁决（「必须由治理裁决
//! 显式删除/改写本文件」）改写为生效断言；这是**治理裁决驱动的状态翻转**，
//! 不是删测试。
//!
//! # 生效语义的三条判据
//!
//! 1. **字段被读取**：声明值差倍数的两 provider 峰值必然不同（旧对照的等值断言
//!    反转）；
//! 2. **声明收紧**：decl=1 < config=4 ⇒ 峰值恰为 1（声明生效）；
//! 3. **config 兜底**：decl=8 > config=4 ⇒ 峰值恰为 4（min 的 config 侧归因，
//!    向后兼容）。

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

/// per-provider 额度（config 侧）
const PROVIDER_BOUND: usize = 4;
/// 每个 provider 投多少条（须 > PROVIDER_BOUND，否则限流不发生，峰值等于条数）
const PER_PROVIDER_CALLS: usize = 6;
/// 车道额度取大值，确保峰值只可能来自 provider 额度而非车道
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
            description: "declares an enforced max_concurrency (W5a)".to_string(),
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

/// W5a 生效判据：decl=1 与 decl=8（> config）的两 provider 峰值分别为
/// min(config, decl) —— 1 与 4。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declared_max_concurrency_is_enforced() {
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

    // 判据 ①（非空转）：峰值必须真的达到过并发，否则后面的精确断言恒真
    assert!(
        tight_peak >= 1 && loose_peak > 1,
        "阳性对照失效：峰值 {tight_peak}/{loose_peak} 未达到预期并发形态"
    );
    // 判据 ②（字段被读取）：声明收紧生效——decl=1 ⇒ 峰值恰为 1
    assert_eq!(
        tight_peak, 1,
        "decl=1 < config={PROVIDER_BOUND} ⇒ 声明必须收紧（实测 {tight_peak}）"
    );
    // 判据 ③（config 兜底）：decl=8 > config ⇒ 峰值恰为 config 额度（min 归因）
    assert_eq!(
        loose_peak,
        PROVIDER_BOUND,
        "decl={} > config={PROVIDER_BOUND} ⇒ min 取 config（实测 {loose_peak}）",
        PROVIDER_BOUND * 2
    );
}
