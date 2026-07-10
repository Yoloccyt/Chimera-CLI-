//! MoE 稀疏门控路由延迟基准 — 对比 O(n) 全量评估 vs O(k) Top-K 门控 vs 五维评分
//!
//! 对应 Task 3 [I1] + v1.3.0 S2:验证 50+ 模型规模下 MoE 门控的路由延迟收益,
//! 以及五维评分(历史维度)相对三维的额外开销。
//!
//! # 基准项
//! - `full_O(n)`:退化门控(threshold=MAX)→ route_auto 全量归一化评估
//!   + n-1 候选列表生成(模拟未启用 MoE 的基线)
//! - `moe_O(k)_3dim`:门控门控(threshold=50,history=None)→ Top-5 粗筛
//!   (三维评分 cost/latency/quality)+ 仅对 5 候选完整评估
//! - `moe_O(k)_5dim`:门控门控(threshold=50,history=Some 充足)→ Top-5 粗筛
//!   (五维评分 cost/latency/quality/success_rate/variance)+ 完整评估
//!
//! # 规模
//! 50 / 100 / 200 模型,覆盖任务规格验收门槛(50+)与更大规模趋势。
//!
//! # 采样约定
//! criterion 默认 100 samples 统计;此处显式 `sample_size(10)` 配合 min-of-N
//! 5 等价语义(降低样本数减少大规模注册表构造开销的噪音,聚焦路由路径延迟)。
//! 详见 `nuxus规则.md §4.1` min-of-N 5 采样约定。
//!
//! # 运行
//! ```bash
//! cargo bench -p model-router --bench moe_bench
//! cargo bench -p model-router --bench moe_bench -- --quick
//! ```

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use model_router::{
    HistoryStore, InMemoryHistoryStore, ModelInfo, ModelRegistry, MoeGate, RoutingRequest,
    RoutingStrategy, SqliteHistoryStore,
};
use nexus_core::{MultimodalInput, UserIntent};

/// 批量生成 n 个差异化模型(与 `tests/moe_test.rs::make_models` 同构)
///
/// WHY 差异化:cost/latency 随 index 递增、quality 递减,确保每个模型评分不同,
/// Top-K 选取有实际区分度,避免排序退化为 model_id 字典序兜底。
fn make_models(n: usize) -> Vec<ModelInfo> {
    (0..n)
        .map(|i| ModelInfo {
            model_id: format!("model-{i:03}"),
            provider: "bench".into(),
            cost_per_1k_tokens: 0.0001 + i as f64 * 0.0001,
            avg_latency_ms: 50 + (i as u64) * 5,
            max_context: 8192,
            quality_score: (0.99 - i as f32 * 0.01).max(0.0),
        })
        .collect()
}

/// 从模型列表构造注册表
fn registry_from(models: &[ModelInfo]) -> ModelRegistry {
    let registry = ModelRegistry::new();
    for m in models {
        // bench 上下文模型 id 唯一(由 make_models 保证),注册不会失败
        registry.register(m.clone()).expect("bench 预注册失败");
    }
    registry
}

/// 构造测试路由请求(Auto 策略)
fn make_request() -> RoutingRequest {
    RoutingRequest {
        quest_id: "q-bench".into(),
        intent: UserIntent {
            intent_id: "i-bench".into(),
            raw_text: "bench".into(),
            multimodal_inputs: vec![MultimodalInput::Text("bench".into())],
            risk_level: 10,
        },
        estimated_tokens: 1000,
        strategy: RoutingStrategy::Auto,
    }
}

/// 构造充足够历史存储(每模型 100 条记录,触发五维评分)
///
/// WHY 100 条:刚好满足 `HISTORY_SUFFICIENT_THRESHOLD`,启用五维评分路径。
/// 延迟 base=200ms + ±10ms 抖动,成功率 85%,模拟真实运行时统计。
fn build_sufficient_history(models: &[ModelInfo]) -> InMemoryHistoryStore {
    let store = InMemoryHistoryStore::new();
    for m in models {
        for i in 0..100u64 {
            let latency = 200.0 + (i as f32 % 10.0);
            let success = i % 10 != 0; // 90% 成功率
            store.record(&m.model_id, latency, success);
        }
    }
    store
}

/// MoE 路由延迟基准:对比全量评估 / 三维门控 / 五维门控
///
/// WHY 三个对比维度:
/// - `full_O(n)`:全量评估基线(threshold=MAX,history=None)
/// - `moe_O(k)_3dim`:三维门控(threshold=50,history=None)— v1.2.0 行为
/// - `moe_O(k)_5dim`:五维门控(threshold=50,history=Some 充足)— v1.3.0 行为
///
/// 三者调用同一 `route_auto_with_gate`,仅 gate/history 配置不同,确保对比公平
/// (消除注册表构造、请求构造等无关变量,只测路由路径差异)。
fn route_latency_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("moe_route_latency");
    // min-of-N 5 采样:降低样本数聚焦路由路径(注册表规模大时单次构造昂贵)
    group.sample_size(10);

    for n in [50usize, 100, 200] {
        let models = make_models(n);
        let registry = registry_from(&models);
        let req = make_request();

        // 退化门控:全量评估基线(threshold 极大永不触发稀疏化)
        let full_gate = MoeGate::new(usize::MAX, 5);
        // MoE 门控:50+ 规模触发 Top-5 稀疏化
        let moe_gate = MoeGate::default();
        // 五维历史存储:每模型 100 条记录(充足,触发五维评分)
        let history = build_sufficient_history(&models);

        group.bench_with_input(BenchmarkId::new("full_O(n)", n), &n, |b, _| {
            b.iter(|| {
                let _decision = model_router::strategies::route_auto_with_gate(
                    black_box(&registry),
                    black_box(&req),
                    black_box(&full_gate),
                    None,
                )
                .expect("全量路由不应失败");
            });
        });

        group.bench_with_input(BenchmarkId::new("moe_O(k)_3dim", n), &n, |b, _| {
            b.iter(|| {
                let _decision = model_router::strategies::route_auto_with_gate(
                    black_box(&registry),
                    black_box(&req),
                    black_box(&moe_gate),
                    None,
                )
                .expect("三维门控路由不应失败");
            });
        });

        group.bench_with_input(BenchmarkId::new("moe_O(k)_5dim", n), &n, |b, _| {
            b.iter(|| {
                let _decision = model_router::strategies::route_auto_with_gate(
                    black_box(&registry),
                    black_box(&req),
                    black_box(&moe_gate),
                    Some(black_box(&history) as &dyn HistoryStore),
                )
                .expect("五维门控路由不应失败");
            });
        });
    }
    group.finish();
}

/// HistoryStore record 延迟基准:对比 InMemory vs SQLite 的单次 record 延迟
///
/// WHY 对比维度:量化 SQLite 持久化相对内存的延迟开销,为是否启用 SQLite
/// (M2 RL 路由触发条件)提供数据支撑。SQLite record 包含 SELECT 旧值 +
/// 反序列化 BLOB + 合并 + 序列化 BLOB + INSERT OR REPLACE 全流程,
/// 反映真实 UPSERT 场景(同一 model_id 多次 record,数据累积)。
///
/// WHY 累积 record 而非每次新 model_id:真实使用场景是同一模型多次路由,
/// 历史数据累积;bench 测量的是 UPSERT 延迟(含 SELECT 旧值合并),
/// 而非仅 INSERT(后者无合并开销,不反映生产场景)。
///
/// # 运行
/// ```bash
/// cargo bench -p model-router --bench moe_bench -- history_store_record
/// ```
fn history_store_record_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("history_store_record");
    // min-of-N 5 采样(与 route_latency_bench 一致)
    group.sample_size(10);

    // Memory 实现:DashMap entry().or_default() + HistoryRecord::record
    // WHY 不用 black_box:record 返回 (),无值可被优化掉;直接调用即可
    let memory = InMemoryHistoryStore::new();
    group.bench_function("memory", |b| {
        b.iter(|| memory.record("bench-model", 100.0, true));
    });

    // SQLite 实现:SELECT 旧值 + 合并 + INSERT OR REPLACE
    // WHY tempdir 生命周期绑定到 bench 函数:避免 tempdir drop 删除数据库文件
    let tmp = tempfile::tempdir().expect("tempdir 创建失败");
    let db_path = tmp.path().join("bench_record.db");
    let sqlite = SqliteHistoryStore::new(&db_path).expect("SqliteHistoryStore 打开失败");
    group.bench_function("sqlite", |b| {
        b.iter(|| sqlite.record("bench-model", 100.0, true));
    });

    group.finish();
}

/// HistoryStore get 延迟基准:对比 InMemory vs SQLite 的单次 get 延迟
///
/// WHY 预填充 100 条记录:模拟真实路由场景(模型已有充足历史,触发五维评分
/// 路径),get 返回非空 HistoryRecord(含 100 个 latency_samples)。
/// SQLite get 包含 SELECT + 反序列化 BLOB → VecDeque<f32> 全流程。
///
/// # 运行
/// ```bash
/// cargo bench -p model-router --bench moe_bench -- history_store_get
/// ```
fn history_store_get_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("history_store_get");
    group.sample_size(10);

    // 预填充数据(每模型 100 条记录,latency_samples 满窗口)
    let memory = InMemoryHistoryStore::new();
    let tmp = tempfile::tempdir().expect("tempdir 创建失败");
    let db_path = tmp.path().join("bench_get.db");
    let sqlite = SqliteHistoryStore::new(&db_path).expect("SqliteHistoryStore 打开失败");
    for i in 0..100u32 {
        let latency = i as f32 * 1.0;
        let success = i % 2 == 0;
        memory.record("bench-model", latency, success);
        sqlite.record("bench-model", latency, success);
    }

    group.bench_function("memory", |b| {
        b.iter(|| black_box(memory.get("bench-model")));
    });
    group.bench_function("sqlite", |b| {
        b.iter(|| black_box(sqlite.get("bench-model")));
    });

    group.finish();
}

criterion_group!(
    benches,
    route_latency_bench,
    history_store_record_bench,
    history_store_get_bench
);
criterion_main!(benches);
