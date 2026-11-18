//! 路由与阈值方法簇 — L-f 三态路由、HTS 阈值写入、确定性归约委托。
//!
//! 从 `bridge` 单文件按方法簇拆出:本文件只承载热路径查表与其唯一的
//! 写者接口,方法体逐字平移。

use std::sync::Arc;

use super::{decide, ComputeBridge};
use crate::dispatch::{DispatchPlan, TaskKind};
use crate::hts::ThresholdSource;
use crate::reduce::{reduce as reduce_impl, ReduceMode};

impl ComputeBridge {
    /// L-f 核心路由 — 纳秒级查表三态判定（取代 S8 的 50-200μs RuntimeSwitcher,ADR-127）
    ///
    /// 判定顺序:① IO 密集 → [`Async`](DispatchPlan::Async);
    /// ② 条目数 < 阈值 → [`Inline`](DispatchPlan::Inline);
    /// ③ 否则 → [`Rayon`](DispatchPlan::Rayon)。
    ///
    /// 阈值来源:**动态 HTS 表**（T9 升级,替代 T8 的静态查表）——`load()` 拿
    /// arc-swap 无锁快照（~5ns,ADR-128）,`get(kind)` 固定数组索引零分配,
    /// 保持 T8 的零分配语义。无副作用,可并发调用。
    #[must_use]
    pub fn route(&self, kind: TaskKind, n_items: usize) -> DispatchPlan {
        let entry = self.hts.load().get(kind);
        decide(kind, kind.is_io_bound(), entry.threshold, n_items)
    }

    /// 运行期更新阈值表 — T1 测定值灌入 / T14 序贯检验校准的转发接口
    ///
    /// WHY 写者串行而非 arc-swap CAS:旧实现 `load_full → clone → store` 非原子,两个
    /// 并发更新会互相覆盖(lost update)——旧注释"单写者"此前只靠"唯一写者=低频周期器且
    /// 默认 Off"这一运行期约定成立,并非接口层保证(`update_threshold` 是 `pub`)。
    /// `Shared`/`Guard` deref 到内层值而非 `Arc`,指针级 CAS 成功判据在本绑定(1.9.2)易踩
    /// API 陷阱,且 `compare_and_swap` 自 1.6 soft-deprecated 会触发 `-D warnings`。故用
    /// `hts_write_lock` 仅在**写者之间**串行——写者本就低频(T1 测定 / T14 周期器 flush),
    /// **读路径 `route` 仍走 ArcSwap `load` 无锁,热路径零影响**。持锁仅跨一次 clone+store
    /// (同步、无 await,不触"持锁跨 await"红线);中毒经 into_inner 兜底。
    /// `source` 为强制显式参数（诚实数据红线:阈值必须可溯源,见 [`ThresholdSource`]）。
    pub fn update_threshold(
        &self,
        kind: TaskKind,
        threshold: usize,
        chunk: usize,
        source: ThresholdSource,
    ) {
        // 写者串行:独占一次 load→clone→update→store,消除跨写者的 lost-update。
        // 命名 `_guard`(非 `let _`)确保 RAII 持到作用域末;into_inner 兜底中毒(单值语义,无脏数据)。
        let _guard = self
            .hts_write_lock
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let current = self.hts.load_full();
        let mut next = (*current).clone();
        next.update(kind, threshold, chunk, source);
        self.hts.store(Arc::new(next));
    }

    /// 确定性归约 — §11.1 Compute 契约 reduce 方法（ADR-102/106,手册 §10.2）
    ///
    /// 语义:纯 CPU 标量归约,同步内联执行,无错误路径（契约直接返回 f64,
    /// `Saturated`/`Cancelled` 等错误变体不适用）;委托 [`crate::reduce`]。
    ///
    /// # 契约
    /// - [`ReduceMode::Deterministic`]:同一构建内多次调用逐位一致;双构建容差 ≤ 1e-6;
    /// - [`ReduceMode::Audit`]:ReproBLAS 式指数分桶,跨构建逐位比对用
    ///   （审计开销 ≤ 30% 门禁,见 `reduce_bench`）。
    #[must_use]
    pub fn reduce(&self, vals: &[f64], mode: ReduceMode) -> f64 {
        // WHY 不走 spawn_compute_batch:归约是标量级计算,池调度（微秒级）反成主导开销;
        // T14 接线时若输入规模超 HTS 阈值,再考虑 Rayon 分块并行（接口不变,
        // 返回类型 f64 无错误面）
        reduce_impl(vals, mode)
    }
}
