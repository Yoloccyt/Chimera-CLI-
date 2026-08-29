//! HTS 动态阈值表性能基准(HtsTable::get 热路径查询 + update 运行期更新)
//!
//! 对应任务:P1-T9(手册 §8.4 / ADR-103 / ADR-128),route 决策门禁基准
//! 对应架构层:L1 Core
//!
//! # 重建说明
//! ⚠️ 本文件于 **2026-08-28 磁盘 ENOSPC 事故中数据丢失**,现基于源码
//! (`compute/hts.rs`) 公共 API 做功能重建。
//!
//! # 基准场景
//! - `hts_table_get`:六类 `TaskKind` 的 [`HtsTable::get`] 热路径查询延迟
//!   (常数数组索引 + 零分配,`Entry` 全 Copy 值返回);
//! - `hts_table_update`:运行期 [`HtsTable::update`] 校准延迟(T1/T14 低频写)。
//!
//! # 参考样板
//! 语法结构参照同目录完整保留的 `clv_bench.rs`。

#![forbid(unsafe_code)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::compute::hts::{HtsTable, ThresholdSource};
use nexus_core::compute::TaskKind;

/// bench 1:六类任务查表延迟
///
/// WHY 穷举六类:get 走 `kind_index` 常数数组索引(零分配、零锁),
/// 验证每类映射槽位的查表延迟与 cache 布局无关(失败即回归)。
fn hts_table_get(c: &mut Criterion) {
    let table = HtsTable::default();
    let mut group = c.benchmark_group("hts_table_get");
    for kind in TaskKind::ALL {
        group.bench_function(BenchmarkId::new("kind", format!("{kind:?}")), |be| {
            be.iter(|| {
                let e = table.get(kind);
                criterion::black_box(e);
            });
        });
    }
    group.finish();
}

/// bench 2:运行期阈值校准延迟
///
/// WHY 更新低频(T1 测定灌入 / T14 序贯校准),RCU 全表替换无关热路径;
/// 此基准锚定写路径成本,验证 update 不引入不合理的分配放大。
fn hts_table_update(c: &mut Criterion) {
    let mut group = c.benchmark_group("hts_table_update");
    for kind in TaskKind::ALL {
        group.bench_function(BenchmarkId::new("kind", format!("{kind:?}")), |be| {
            be.iter_batched(
                HtsTable::default,
                |mut table| {
                    table.update(kind, 4096, 256, ThresholdSource::ConservativeDefault);
                    criterion::black_box(&table);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, hts_table_get, hts_table_update);
criterion_main!(benches);
