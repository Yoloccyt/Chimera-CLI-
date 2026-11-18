//! 计算池构造 — 全局 rayon 池的私有构造入口（单例初始化 / 同模块测试）。
//!
//! 从 `bridge` 单文件按方法簇拆出:构造与字段初始化集中于此,
//! 其余方法簇见 [`super::probe`] / [`super::route`] / [`super::batch`]。

use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{ComputeBridge, STACK_SIZE_BYTES};
use crate::hts::HtsTable;
use crate::telemetry::KindCounters;
use crate::utilization::ActiveCounter;

impl ComputeBridge {
    /// 构造计算池 — 供单例惰性初始化调用,不直接暴露（用 [`bridge()`]）
    ///
    /// WHY 私有构造:池配置（线程数/栈/线程名）是全局预算,不允许调用方按需定制
    /// 导致多池并存（纪律④ 桥接唯一）。
    pub(super) fn new(threads: usize) -> Self {
        // WHY 显式 match + panic 而非 unwrap/expect(红线:错误用 ?/match):
        // rayon build 失败仅当 OS 拒绝创建线程(进程级资源耗尽),此时计算桥不可用
        // 即进程不可用,无运行期降级路径(与 D15 运行期熔断杀池不同)——这是进程
        // 启动期致命前提的失败,panic 沿 OnceLock 惰性初始化传播(§11.1 不变式
        // "计算任务永不 panic 到调用方"仅约束运行期批处理,不覆盖单例初始化前提)
        let pool = match rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .stack_size(STACK_SIZE_BYTES)
            .thread_name(|idx| format!("chimera-compute-{idx}"))
            .build()
        {
            Ok(p) => p,
            Err(e) => panic!("ComputeBridge 初始化失败(系统线程资源耗尽): {e}"),
        };
        Self {
            pool,
            // HTS 初始表:dispatch.rs 静态值迁移（S9 离线测定预填,W1 复测,§8.4）
            hts: ArcSwap::from_pointee(HtsTable::default()),
            hts_write_lock: std::sync::Mutex::new(()),
            active: Arc::new(ActiveCounter::default()),
            thread_count: threads,
            counters: KindCounters::default(),
            sat_bp_override: std::sync::atomic::AtomicU32::new(0),
            mode_override: std::sync::atomic::AtomicU8::new(u8::MAX),
            cal_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 同模块测试专用桥构造 —— 不碰全局单例,零串扰
    ///
    /// WHY 集中构造:测试需多个独立实例验证饱和/降级/探针行为,
    /// 字段集会随功能演进,集中一处避免多处同步修改。
    // 本函数仅在 cfg(test) 下编译,池构建失败 = 测试环境前提不成立,expect 在此合法
    // （E-5 红线的适用域是生产代码,与 `mod tests` 顶部的同类放行一致）
    #[allow(clippy::expect_used)]
    #[cfg(test)]
    pub(crate) fn test_bridge(threads: usize) -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("测试池构建失败");
        Self {
            pool,
            hts: ArcSwap::from_pointee(HtsTable::default()),
            hts_write_lock: std::sync::Mutex::new(()),
            active: Arc::new(ActiveCounter::default()),
            thread_count: threads,
            counters: KindCounters::default(),
            sat_bp_override: std::sync::atomic::AtomicU32::new(0),
            mode_override: std::sync::atomic::AtomicU8::new(u8::MAX),
            cal_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }
}
