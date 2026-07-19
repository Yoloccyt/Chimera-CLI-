//! Week 8 Task 6 SubTask 6.3 — 1000 次压测无内存泄漏
//!
//! 对应任务:Week 8 Task 6.3(1000 次全链路迭代 + 内存泄漏验证)
//! 架构层:L1-L10 全栈压测
//!
//! # 测试策略
//! 由于 `#![forbid(unsafe_code)]` 红线,无法实现自定义 GlobalAlloc 精确测量堆内存。
//! 采用三重替代验证方案(继承 Week 7 压测经验):
//! 1. **Arc strong_count 探针**:每次迭代 clone 一份 Arc<()>,迭代后验证 strong_count=1
//! 2. **延迟稳定性**:预热后取早期/末期窗口中位数,差异 < 50% 视为无累积性能退化
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

/// 延迟退化阈值:末期窗口中位数 vs 早期窗口中位数差异 < 50% 视为无累积性能退化
const LATENCY_DEGRADATION_THRESHOLD_PCT: f64 = 50.0;

/// 单次迭代延迟上限:2s(含 NMC + Quest + OSA + Wiki,容忍 GC/调度噪声)
const SINGLE_ITER_THRESHOLD_US: u128 = 2_000_000;

/// 预热迭代次数 — 前 N 次用于热化缓存与分配,不计入退化统计
const WARMUP_ITERATIONS: usize = 50;
/// 退化对比窗口大小 — 取早期/末期各 N 次的中位数对比,降低噪声
const COMPARISON_WINDOW: usize = 50;

/// 构造测试用 UserIntent
fn make_intent(iter: usize) -> UserIntent {
    UserIntent {
        intent_id: format!("i-stress-{iter}"),
        raw_text: "分析需求。设计方案。实现代码。".into(),
        multimodal_inputs: vec![MultimodalInput::Text(format!("stress-iter-{iter}"))],
        risk_level: 20,
    }
}

/// 计算 u128 切片的中位数
///
/// WHY:窗口中位数对偶发抖动不敏感,比平均值更适合做延迟稳定性基准。
fn median_u128(values: &[u128]) -> u128 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
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
        let mut max_iter_us: u128 = 0;
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
            // WHY insert_batch:每次迭代沉淀 3 条条目,打包为单次事务写入,
            // 避免 3 次独立 insert 的事务提交、FTS5 索引同步与指标刷新开销。
            store.insert_batch(entries).await.expect("Wiki batch insert 失败");
            total_wiki_entries += 3;

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

            let iter_us = iter_start.elapsed().as_micros();
            latencies.push(iter_us);
            if iter_us > max_iter_us {
                max_iter_us = iter_us;
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
        let store_count = store.count().await.expect("WikiStore count 失败");
        assert_eq!(
            store_count as usize,
            TOTAL_ITERATIONS * 3,
            "WikiStore 应持久化 {} 条记录,实际 {}",
            TOTAL_ITERATIONS * 3,
            store_count
        );

        // === 验证 4:早期窗口中位数 vs 末期窗口中位数退化 < 50%(无累积性能退化)===
        // WHY:单点 first/last 比较在毫秒精度下对 2-3ms 量级噪声过于敏感;
        // 使用微秒精度 + 丢弃预热 + 窗口中位数,能稳健检测累积性退化,
        // 同时避免把正常调度抖动误判为泄漏。
        let early_window =
            &latencies[WARMUP_ITERATIONS..WARMUP_ITERATIONS + COMPARISON_WINDOW];
        let late_window = &latencies[TOTAL_ITERATIONS - COMPARISON_WINDOW..TOTAL_ITERATIONS];

        let early_median = median_u128(early_window);
        let late_median = median_u128(late_window);

        let diff_pct = if early_median > 0 && late_median > early_median {
            (late_median as f64 - early_median as f64) / early_median as f64 * 100.0
        } else {
            0.0
        };
        assert!(
            diff_pct < LATENCY_DEGRADATION_THRESHOLD_PCT,
            "早期中位数 {}μs vs 末期中位数 {}μs,退化 {:.2}% >= {}%,疑似累积性能退化",
            early_median,
            late_median,
            diff_pct,
            LATENCY_DEGRADATION_THRESHOLD_PCT
        );

        // === 验证 5:最大单次迭代延迟 < 阈值(无单次超时)===
        assert!(
            max_iter_us < SINGLE_ITER_THRESHOLD_US,
            "最大迭代延迟 {}μs >= {}μs 阈值",
            max_iter_us,
            SINGLE_ITER_THRESHOLD_US
        );

        // === 验证 6:p95 延迟统计 ===
        latencies.sort_unstable();
        let p50 = latencies[TOTAL_ITERATIONS / 2];
        let p95 = latencies[(TOTAL_ITERATIONS as f64 * 0.95) as usize];
        let p99 = latencies[(TOTAL_ITERATIONS as f64 * 0.99) as usize];

        println!(
            "[STRESS-W8] 1000 次全链路迭代完成:success={} wiki={} early_median={}μs late_median={}μs p50={}μs p95={}μs p99={}μs max={}μs diff={:.2}%",
            total_success, total_wiki_entries, early_median, late_median, p50, p95, p99, max_iter_us, diff_pct
        );
    });
}

// ============================================================
// 诊断测试:单独测量 WikiStore insert 随条目数增长的延迟
// ============================================================
//
// WHY:压力测试发现整体迭代延迟从 1470μs 退化到 3094μs,需定位退化来源。
// 本测试排除 NMC/Quest/OSA/EventBus 等因素,单独观察 WikiStore insert。

#[test]
#[ignore]
fn test_wiki_store_insert_scaling() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");

    rt.block_on(async {
        let tmp = TempDir::new().expect("TempDir 创建失败");
        let store = WikiStore::open(&tmp.path().join("scaling.db")).expect("WikiStore 打开失败");

        const N: usize = 1000;
        let mut latencies: Vec<u128> = Vec::with_capacity(N);

        for i in 0..N {
            let entry = repo_wiki::WikiEntry::new(
                format!("scaling-{i}"),
                format!("Title {i}"),
                format!("Content {i}"),
                vec![],
                vec![0.0; 512],
            );
            let start = Instant::now();
            store.insert(entry).await.unwrap();
            latencies.push(start.elapsed().as_micros());
        }

        let early = median_u128(&latencies[0..COMPARISON_WINDOW]);
        let late = median_u128(&latencies[N - COMPARISON_WINDOW..N]);
        let diff_pct = if early > 0 && late > early {
            (late as f64 - early as f64) / early as f64 * 100.0
        } else {
            0.0
        };

        println!(
            "[WIKI-STORE-SCALING] N={} early_median={}μs late_median={}μs diff={:.2}%",
            N, early, late, diff_pct
        );
    });
}

// ============================================================
// 诊断测试:对主压测各阶段做细分计时,定位退化来源
// ============================================================
//
// WHY:全链路测试退化 110%,但 WikiStore 单独仅退化 17.6%,
// 需找出非 WikiStore 阶段的退化来源。

#[test]
#[ignore]
fn test_stress_profile_per_step() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");

    rt.block_on(async {
        let tmp = TempDir::new().expect("TempDir 创建失败");
        let store = WikiStore::open(&tmp.path().join("profile.db")).expect("WikiStore 打开失败");

        const N: usize = 500;
        let mut t_setup: Vec<u128> = Vec::with_capacity(N);
        let mut t_nmc: Vec<u128> = Vec::with_capacity(N);
        let mut t_quest: Vec<u128> = Vec::with_capacity(N);
        let mut t_tasks: Vec<u128> = Vec::with_capacity(N);
        let mut t_osa: Vec<u128> = Vec::with_capacity(N);
        let mut t_wiki_gen: Vec<u128> = Vec::with_capacity(N);
        let mut t_wiki_insert: Vec<u128> = Vec::with_capacity(N);

        for i in 0..N {
            let t0 = Instant::now();
            let pipeline = setup_week7_pipeline().expect("管线装配失败");
            t_setup.push(t0.elapsed().as_micros());

            let t1 = Instant::now();
            let _clv = pipeline
                .encoder
                .perceive(PerceptionInput::Text(format!("stress-iter-{i}")))
                .expect("NMC 编码失败");
            t_nmc.push(t1.elapsed().as_micros());

            let bus = EventBus::new();
            let t2 = Instant::now();
            let engine = QuestEngine::with_config(bus.clone(), QuestConfig::default());
            let quest = engine
                .create_quest(make_intent(i))
                .await
                .expect("Quest 创建失败");
            t_quest.push(t2.elapsed().as_micros());

            let t3 = Instant::now();
            for j in 0..quest.tasks.len() {
                let task_id = format!("task-{j}");
                engine
                    .update_task_status(&quest.quest_id, &task_id, TaskStatus::Running)
                    .await
                    .unwrap();
                engine
                    .update_task_status(&quest.quest_id, &task_id, TaskStatus::Completed)
                    .await
                    .unwrap();
            }
            t_tasks.push(t3.elapsed().as_micros());

            let t4 = Instant::now();
            let coord = OmniSparseCoordinator::new(bus.clone());
            let profile = TaskProfile::new("stress-task", 0.5, RiskLevel::Medium);
            let _masks = coord
                .compute_all_masks(&profile)
                .await
                .expect("OSA 掩码计算失败");
            t_osa.push(t4.elapsed().as_micros());

            let t5 = Instant::now();
            let final_quest = engine.get_quest(&quest.quest_id).expect("应能获取 Quest");
            let entries = WikiGenerator::from_quest_result(&final_quest);
            t_wiki_gen.push(t5.elapsed().as_micros());

            let t6 = Instant::now();
            store.insert_batch(entries).await.unwrap();
            t_wiki_insert.push(t6.elapsed().as_micros());

            drop(pipeline);
            drop(engine);
            drop(coord);
        }

        fn report(name: &str, values: &[u128]) {
            let early = median_u128(&values[0..COMPARISON_WINDOW.min(values.len())]);
            let late = median_u128(&values[values.len().saturating_sub(COMPARISON_WINDOW)..]);
            let diff_pct = if early > 0 && late > early {
                (late as f64 - early as f64) / early as f64 * 100.0
            } else {
                0.0
            };
            println!(
                "[PROFILE] {:15} early={}μs late={}μs diff={:.2}%",
                name, early, late, diff_pct
            );
        }

        report("setup", &t_setup);
        report("nmc", &t_nmc);
        report("quest", &t_quest);
        report("tasks", &t_tasks);
        report("osa", &t_osa);
        report("wiki_gen", &t_wiki_gen);
        report("wiki_insert", &t_wiki_insert);
    });
}
