//! Variant Pool — Harness 变体隔离池与规则式任务路由(polish-v2.7 P3-3)
//!
//! 对应架构层:L8 Parliament(子模块)
//! 对应 ADR:ADR-051 决策 1/2/4(落点/规则路由/池容量)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §12.3(小米变体隔离)
//!
//! # 设计决策(WHY)
//!
//! - **池只存 VariantContract**:变体本体(HarnessSpec)在 L5 SpecRegistry,
//!   本池仅存契约引用(ADR-051 否决"双份存储"方案,单一事实源)
//! - **规则式路由非 RL**:确定性三级匹配(精确 → 兜底 → None),
//!   R2 冻结约束下路由升级须走 omega-learner 接缝 + 新 ADR
//! - **容量淘汰**:每任务类型 ≤4 变体 + 全池 ≤64,超限淘汰性能最低者
//!   (防无界增长,小米变体池经验值)

use nexus_contracts::{VariantContract, VariantId};
use tracing::debug;

/// 每任务类型的变体数上限(ADR-051 决策 4,小米变体池经验值)
const PER_TASK_TYPE_LIMIT: usize = 4;

/// 全池变体总数上限(ADR-051 决策 4,防无界增长)
const POOL_TOTAL_LIMIT: usize = 64;

/// 变体隔离池 — 按任务类型隔离的变体契约集合
///
/// # 线程安全
/// 本类型无内部可变性,调用方按 parliament 既有模式以 `RwLock`/`Mutex`
/// 包装共享(与 `RoleRegistry` 同款使用方式)。
#[derive(Debug, Default)]
pub struct VariantPool {
    /// 池内变体契约(Vec 而非 HashMap:池上限 64,线性扫描开销可忽略,
    /// 且路由需按 expected_performance 比较,Vec 遍历语义更直接)
    contracts: Vec<VariantContract>,
}

impl VariantPool {
    /// 创建空变体池
    pub fn new() -> Self {
        Self::default()
    }

    /// 池内变体总数
    pub fn len(&self) -> usize {
        self.contracts.len()
    }

    /// 池是否为空
    pub fn is_empty(&self) -> bool {
        self.contracts.is_empty()
    }

    /// 按变体标识查询契约
    pub fn get(&self, id: &VariantId) -> Option<&VariantContract> {
        self.contracts.iter().find(|c| &c.variant_id == id)
    }

    /// 注册变体契约(经审议通过后调用)
    ///
    /// # 容量淘汰(ADR-051 决策 4)
    /// - 同 VariantId 重复注册 → 覆盖旧契约(幂等更新)
    /// - 任务类型桶超过 4 → 淘汰该桶内 expected_performance 最低者
    /// - 全池超过 64 → 淘汰全池 expected_performance 最低者
    pub fn register(&mut self, contract: VariantContract) {
        // 幂等更新:同 ID 覆盖
        if let Some(existing) = self
            .contracts
            .iter_mut()
            .find(|c| c.variant_id == contract.variant_id)
        {
            *existing = contract;
            return;
        }

        self.contracts.push(contract);

        // 桶级淘汰:新变体的每个任务类型桶都检查容量
        // WHY 快照任务类型:淘汰会移动元素,避免边遍历边删除
        let new_task_types: Vec<String> = self
            .contracts
            .last()
            .map(|c| c.task_types.clone())
            .unwrap_or_default();
        for task_type in &new_task_types {
            self.evict_lowest_if_over(
                |c| c.task_types.iter().any(|t| t == task_type),
                PER_TASK_TYPE_LIMIT,
            );
        }

        // 全池淘汰
        self.evict_lowest_if_over(|_| true, POOL_TOTAL_LIMIT);
    }

    /// 规则式任务路由(ADR-051 决策 2,三级匹配)
    ///
    /// 1. 精确匹配:task_types 含该类型的变体中 expected_performance 最高者
    /// 2. 兜底:task_types 为空的通用变体中 expected_performance 最高者
    /// 3. 无匹配:None(调用方沿用当前活跃 spec)
    pub fn route(&self, task_type: &str) -> Option<&VariantContract> {
        // 级别 1:精确匹配
        let exact: Vec<&VariantContract> = self
            .contracts
            .iter()
            .filter(|c| !c.task_types.is_empty() && c.matches_task_type(task_type))
            .collect();
        if !exact.is_empty() {
            return best_by_perf(exact);
        }

        // 级别 2:通用兜底
        let universal: Vec<&VariantContract> = self
            .contracts
            .iter()
            .filter(|c| c.task_types.is_empty())
            .collect();
        best_by_perf(universal)
    }

    /// 池内指定任务类型的现役最高性能(Skeptic 审议基准)
    pub fn best_performance_for(&self, task_type: &str) -> Option<f32> {
        self.contracts
            .iter()
            .filter(|c| c.matches_task_type(task_type))
            .map(|c| c.expected_performance)
            .fold(None, |acc, p| Some(acc.map_or(p, |a: f32| a.max(p))))
    }

    /// 变体隔离视图（Milestone D-1：弱模型变体隔离评估的 Rust 侧载体）
    ///
    /// 按变体 ID 标签隔离（约定：ID 含标签子串，如 "weak-qwen3.5-9b-*"），
    /// 返回匹配变体列表；不修改池本身。隔离语义：弱模型变体与强模型
    /// 变体分开评估（不交叉污染性能基线——D-1 变体隔离测试）。
    pub fn isolated(&self, tag: &str) -> Vec<&VariantContract> {
        self.contracts
            .iter()
            .filter(|c| c.variant_id.spec_name.contains(tag))
            .collect()
    }
    /// 谓词命中集合超限时淘汰其中 expected_performance 最低者

    fn evict_lowest_if_over<F: Fn(&VariantContract) -> bool>(&mut self, pred: F, limit: usize) {
        let hits: Vec<usize> = self
            .contracts
            .iter()
            .enumerate()
            .filter(|(_, c)| pred(c))
            .map(|(i, _)| i)
            .collect();
        if hits.len() > limit {
            // 找命中集合中性能最低者的池内索引
            if let Some(&lowest_idx) = hits.iter().min_by(|&&a, &&b| {
                self.contracts[a]
                    .expected_performance
                    .partial_cmp(&self.contracts[b].expected_performance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                let evicted = self.contracts.remove(lowest_idx);
                debug!(variant = %evicted.variant_id, "变体池容量淘汰(性能最低者出局)");
            }
        }
    }
}

/// 候选集内取性能最高者;性能相同取 spec 版本较新者(确定性决胜)
///
/// WHY 独立函数而非闭包:返回引用需显式生命周期标注('a 绑定输入切片),
/// 闭包推断会将引用限制在闭包局部作用域导致编译错误。
fn best_by_perf(mut candidates: Vec<&VariantContract>) -> Option<&VariantContract> {
    candidates.sort_by(|a, b| {
        b.expected_performance
            .partial_cmp(&a.expected_performance)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.variant_id.spec_version.cmp(&a.variant_id.spec_version))
    });
    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(name: &str, version: u32, task_types: Vec<&str>, perf: f32) -> VariantContract {
        VariantContract::new(
            VariantId::new(name, version),
            task_types.into_iter().map(String::from).collect(),
            perf,
            0.1,
        )
    }

    #[test]
    fn test_route_exact_match_prefers_highest_performance() {
        let mut pool = VariantPool::new();
        pool.register(contract("s-a", 1, vec!["code_fix"], 0.7));
        pool.register(contract("s-b", 1, vec!["code_fix"], 0.9));
        pool.register(contract("s-c", 1, vec!["doc_gen"], 0.95));

        let routed = pool.route("code_fix").expect("应命中精确匹配");
        assert_eq!(routed.variant_id.spec_name, "s-b");
    }

    #[test]
    fn test_route_falls_back_to_universal_variant() {
        let mut pool = VariantPool::new();
        pool.register(contract("universal", 2, vec![], 0.6));
        pool.register(contract("specific", 1, vec!["doc_gen"], 0.9));

        // code_fix 无精确匹配 → 兜底通用变体
        let routed = pool.route("code_fix").expect("应命中通用兜底");
        assert_eq!(routed.variant_id.spec_name, "universal");
    }

    #[test]
    fn test_route_none_when_no_match() {
        let mut pool = VariantPool::new();
        pool.register(contract("specific", 1, vec!["doc_gen"], 0.9));
        assert!(pool.route("code_fix").is_none());
    }

    #[test]
    fn test_register_idempotent_update_same_id() {
        let mut pool = VariantPool::new();
        pool.register(contract("s", 1, vec!["a"], 0.5));
        pool.register(contract("s", 1, vec!["a"], 0.8));
        assert_eq!(pool.len(), 1);
        assert_eq!(
            pool.get(&VariantId::new("s", 1))
                .unwrap()
                .expected_performance,
            0.8
        );
    }

    #[test]
    fn test_per_task_type_capacity_evicts_lowest() {
        let mut pool = VariantPool::new();
        // 注册 5 个同任务类型变体(超 4 上限),性能 0.5 最低者应被淘汰
        for (i, perf) in [0.9f32, 0.8, 0.5, 0.7, 0.6].iter().enumerate() {
            pool.register(contract(&format!("s{i}"), 1, vec!["code_fix"], *perf));
        }
        assert_eq!(pool.len(), PER_TASK_TYPE_LIMIT);
        // 性能最低的 s2(0.5)被淘汰
        assert!(pool.get(&VariantId::new("s2", 1)).is_none());
        assert!(pool.get(&VariantId::new("s0", 1)).is_some());
    }

    #[test]
    fn test_best_performance_for_task_type() {
        let mut pool = VariantPool::new();
        pool.register(contract("s-a", 1, vec!["code_fix"], 0.7));
        pool.register(contract("s-b", 1, vec![], 0.85)); // 通用变体也计入
        assert_eq!(pool.best_performance_for("code_fix"), Some(0.85));
        assert_eq!(pool.best_performance_for("unseen"), Some(0.85));
    }
}
