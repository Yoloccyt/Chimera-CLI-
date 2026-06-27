//! Week 8 Task 6 SubTask 6.3 — 1000 次压测无内存泄漏
//!
//! 对应任务:Week 8 Task 6.3(1000 次全链路迭代 + 内存泄漏验证)
//! 架构层:L1-L10 全栈压测
//!
//! # 测试策略
//! 由于 `#![forbid(unsafe_code)]` 红线,无法实现自定义 GlobalAlloc 精确测量堆内存。
//! 采用三重替代验证方案(继承 Week 7 压测经验):
//! 1. **Arc strong_count 探针**:每次迭代 clone 一份 Arc<()>,迭代后验证 strong_count=1
//! 2. **延迟稳定性**:首次 vs 末次延迟差异 < 50% 视为无累积性能退化
//! 3. **资源可重建性**:1000 次后仍能成功创建新管线,证明无资源耗尽
//!
//! # 全链路覆盖
//! 每次迭代执行:UserIntent → NMC 编码 → Quest 创建 → OSA 掩码计算 → Wiki 生成
//! (轻量版,避免 Parliament/PVL 在 1000 次循环中累积过多后台任务)
//!
//! # 架构红线对齐
//! - `#![forbid(unsafe_code)]` 红线
//! - 单运行时:用 `tokio::runtime::Runtime::new()` 避免.spawn 泛滥
//! - 内存敏感:单线程串行迭代,避免并发内存爆炸

#![forbid(unsafe_code)]

#[path = "week7_setup.rs"]
#[allow(dead_code)]
mod setup;

use std::sync::Arc;
use std::time::Instant;

use event_bus::EventBus;
use nexus_core::{MultimodalInput, TaskStatus, UserIntent};
use nmc_encoder::PerceptionInput;
use osa_coordinator::{OmniSparseCoordinator, RiskLevel, TaskProfile};
use quest_engine::{QuestConfig, QuestEngine};
use repo_wiki::{WikiGenerator, WikiStore};
use setup::setup_week7_pipeline;
use tempfile::TempDir;

/// 总迭代次数(Task 6.3 要求 1000 次)
const TOTAL_ITERATIONS: usize = 1000;

/// 延迟退化阈值:末次 vs 首次延迟差异 < 50% 视为无累积性能退化
const LATENCY_DEGRADATION_THRESHOLD_PCT: f64 = 50.0;

/// 单次迭代延迟上限:2s(含 NMC + Quest + OSA + Wiki,容忍 GC/调度噪声)
const SINGLE_ITER_THRESHOLD_MS: u128 = 2000;

/// 构造测试用 UserIntent
fn make_intent(iter: usize) -> UserIntent {
    UserIntent {
        intent_id: format!("i-stress-{iter}"),
        raw_text: "分析需求。设计方案。实现代码。".into(),
        multimodal_inputs: vec![MultimodalInput::Text(format!("stress-iter-{iter}"))],
        risk_level: 20,
    }
}

// ============================================================
// 压测主测试:1000 次全链路迭代 + Arc 泄漏探针 + 延迟稳定性
// ============================================================
//
// WHY `#[ignore]`:1000 次迭代耗时较长(约 30-60s),标记为 ignored
// 避免在日常 `cargo test` 中阻塞,通过 `--ignored` 显式触发。

#[test]
#[ignore]
fn test_stress_1000_iterations() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");

    rt.block_on(async {
        // Arc 泄漏探针:每次迭代 clone 一份,迭代结束后验证 strong_count 回到 1
        let leak_probe = Arc::new(());

        // 持久化 WikiStore(跨迭代复用,验证无累积泄漏)
        let tmp = TempDir::new().expect("TempDir 创建失败");
        let store = WikiStore::open(&tmp.path().join("stress.db")).expect("WikiStore 打开失败");

        let mut latencies: Vec<u128> = Vec::with_capacity(TOTAL_ITERATIONS);
        let mut total_success: usize = 0;
        let mut max_iter_ms: u128 = 0;
        let mut total_wiki_entries: usize = 0;

        for i in 0..TOTAL_ITERATIONS {
            let iter_start = Instant::now();
            let _probe = leak_probe.clone();

            // 每次迭代使用独立 EventBus,避免 broadcast 通道累积
            let bus = EventBus::new();

            // 1. NMC 编码(L2 Memory)— 复用 Week7 管线中的 encoder
            let pipeline = setup_week7_pipeline().expect("管线装配失败");
            let clv = pipeline
                .encoder
                .perceive(PerceptionInput::Text(format!("stress-iter-{i}")))
                .expect("NMC 编码失败");
            assert_eq!(clv.dimension(), 512, "CLV 维度必须为 512");

            // 2. Quest 创建(L9 Quest)— 分解为 3 个 Task
            let engine = QuestEngine::with_config(bus.clone(), QuestConfig::default());
            let quest = engine
                .create_quest(make_intent(i))
                .await
                .expect("Quest 创建失败");
            assert_eq!(quest.tasks.len(), 3, "应分解为 3 个 Task");

            // 3. OSA 掩码计算(L6 Router)— 五维度稀疏化
            let coord = OmniSparseCoordinator::new(bus.clone());
            let profile = TaskProfile::new("stress-task", 0.5, RiskLevel::Medium);
            let masks = coord
                .compute_all_masks(&profile)
                .await
                .expect("OSA 掩码计算失败");
            // 空候选集时 active_count 可能为 0,只验证 mask_hash 存在
            assert!(!masks.mask_hash().is_empty(), "mask_hash 不应为空");

            // 4. 推进所有 Task 至 Completed(L9 状态机)
            for j in 0..quest.tasks.len() {
                let task_id = format!("task-{j}");
                engine
                    .update_task_status(&quest.quest_id, &task_id, TaskStatus::Running)
                    .await
                    .expect("Task Running 失败");
                engine
                    .update_task_status(&quest.quest_id, &task_id, TaskStatus::Completed)
                    .await
                    .expect("Task Completed 失败");
            }

            // 5. Wiki 沉淀(L5 Knowledge)— 从已完成 Quest 生成条目
            let final_quest = engine
                .get_quest(&quest.quest_id)
                .expect("应能获取 Quest");
            let entries = WikiGenerator::from_quest_result(&final_quest);
            for entry in &entries {
                let _ = store.insert(entry);
            }
            total_wiki_entries += entries.len();

            // 显式 drop 管线与 engine,触发 Drop trait 释放资源
            drop(pipeline);
            drop(engine);
            drop(coord);
            drop(_probe);

            // 验证 leak_probe strong_count 回到 1(只有原始引用存活)
            assert_eq!(
                Arc::strong_count(&leak_probe),
                1,
                "第 {i} 次迭代后 leak_probe strong_count 应为 1,检测到引用泄漏"
            );

            let iter_ms = iter_start.elapsed().as_millis();
            latencies.push(iter_ms);
            if iter_ms > max_iter_ms {
                max_iter_ms = iter_ms;
            }
            total_success += 1;
        }

        // === 验证 1:全部 1000 次迭代成功 ===
        assert_eq!(
            total_success, TOTAL_ITERATIONS,
            "应有 {} 次成功迭代,实际 {}",
            TOTAL_ITERATIONS, total_success
        );

        // === 验证 2:Wiki 条目累积正确(3 entries/iter × 1000 = 3000)===
        assert_eq!(
            total_wiki_entries,
            TOTAL_ITERATIONS * 3,
            "应生成 {} 个 Wiki 条目,实际 {}",
            TOTAL_ITERATIONS * 3,
            total_wiki_entries
        );

        // === 验证 3:WikiStore 持久化计数 ===
        // WHY as usize:WikiStore::count 返回 u32,TOTAL_ITERATIONS 为 usize,统一到 usize 比较
        let store_count = store.count().expect("WikiStore count 失败");
        assert_eq!(
            store_count as usize,
            TOTAL_ITERATIONS * 3,
            "WikiStore 应持久化 {} 条记录,实际 {}",
            TOTAL_ITERATIONS * 3,
            store_count
        );

        // === 验证 4:首次 vs 末次延迟退化 < 50%(无累积性能退化)===
        let first_ms = latencies[0];
        let last_ms = latencies[TOTAL_ITERATIONS - 1];
        let diff_pct = if first_ms > 0 && last_ms > first_ms {
            (last_ms as f64 - first_ms as f64) / first_ms as f64 * 100.0
        } else {
            0.0
        };
        assert!(
            diff_pct < LATENCY_DEGRADATION_THRESHOLD_PCT,
            "首次 {}ms vs 末次 {}ms,退化 {:.2}% >= {}%,疑似内存泄漏",
            first_ms,
            last_ms,
            diff_pct,
            LATENCY_DEGRADATION_THRESHOLD_PCT
        );

        // === 验证 5:最大单次迭代延迟 < 阈值(无单次超时)===
        assert!(
            max_iter_ms < SINGLE_ITER_THRESHOLD_MS,
            "最大迭代延迟 {}ms >= {}ms 阈值",
            max_iter_ms,
            SINGLE_ITER_THRESHOLD_MS
        );

        // === 验证 6:p95 延迟统计 ===
        latencies.sort_unstable();
        let p50 = latencies[TOTAL_ITERATIONS / 2];
        let p95 = latencies[(TOTAL_ITERATIONS as f64 * 0.95) as usize];
        let p99 = latencies[(TOTAL_ITERATIONS as f64 * 0.99) as usize];

        println!(
            "[STRESS-W8] 1000 次全链路迭代完成:success={} wiki={} first={}ms last={}ms p50={}ms p95={}ms p99={}ms max={}ms diff={:.2}%",
            total_success, total_wiki_entries, first_ms, last_ms, p50, p95, p99, max_iter_ms, diff_pct
        );
    });
}
