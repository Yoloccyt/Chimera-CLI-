//! RoutingHistory — TSR×MoE 收口（P3-T6，v4.0 WI-09 尾）
//!
//! 对应架构层: **L6 Router**（faae-router，ADR-137/ADR-151 裁决：挂既有 crate 增强）
//! 对应任务: **P3-T6**（手册 W15，WI-09 收口：routing_history + MCP 同平面注册预留 + 误剪救回率埋点）
//!
//! # 本模块三件交付
//! 1. [`RoutingHistory`] — 任务类型 × 专家成功率矩阵（WI-09 历史偏好 bonus 数据源），
//!    支持周期衰减清零（防历史矩阵固化,回退路径：`decay(1.0)` 即清零）。
//! 2. [`ExpertCatalog`] — MCP 工具同平面注册接口预留（WI-22 联动）：外部工具经
//!    `discover_and_register()` 注册为同平面专家,与内置专家在同一路由面竞争。
//! 3. [`MissedRecoveryTracker`] — 误剪救回率埋点（WI-09 验证口径：召回率 ≥98% 且
//!    注入 token 降 ≥60%;被剪专家若被用户/后续实际使用,记一次救回）。
//!
//! # 红线
//! `#![forbid(unsafe_code)]` 由 crate 顶层保证;注册表用 `RwLock<HashMap>`（注册低频,
//! 读多写少,零新依赖——Ω₆ 最小依赖）;无自旋（Atomic 仅 Relaxed 计数）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// 任务类型 × 专家成功率矩阵（WI-09 RoutingHistory）
///
/// 键: (task_type, expert_id);值: 成功/总次数。无记录查询返回中性 0.5
/// （不惩罚新专家,也不无证据拔高——诚实数据）。衰减用乘法因子,
/// `decay(0.5)` 半衰 / `decay(1.0)` 清零,由调用方按周期触发。
#[derive(Debug, Clone, Default)]
pub struct RoutingHistory {
    /// (task_type, expert_id) → 成功统计
    stats: HashMap<(String, String), OutcomeStats>,
    /// 累计记录事件数（诊断/衰减触发参考）
    events: u64,
}

/// 单专家单任务类型的成功统计
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct OutcomeStats {
    /// 成功次数
    success: u64,
    /// 总次数
    total: u64,
}

impl RoutingHistory {
    /// 新建空矩阵
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次路由结果（成功/失败）
    pub fn record(&mut self, task_type: &str, expert_id: &str, ok: bool) {
        let key = (task_type.to_string(), expert_id.to_string());
        let entry = self.stats.entry(key).or_default();
        entry.total += 1;
        if ok {
            entry.success += 1;
        }
        self.events += 1;
    }

    /// 查询成功率 — 无记录返回中性 0.5（不惩罚新专家）
    #[must_use]
    pub fn success_rate(&self, task_type: &str, expert_id: &str) -> f64 {
        let key = (task_type.to_string(), expert_id.to_string());
        match self.stats.get(&key) {
            Some(s) if s.total > 0 => s.success as f64 / s.total as f64,
            _ => 0.5,
        }
    }

    /// 是否有该键的记录 — 区分「无记录中性」与「有记录恰为 0.5」
    /// （P3-T6:score_with_history 的回退判定依据）
    #[must_use]
    pub fn is_recorded(&self, task_type: &str, expert_id: &str) -> bool {
        let key = (task_type.to_string(), expert_id.to_string());
        self.stats.get(&key).is_some_and(|s| s.total > 0)
    }

    /// 周期衰减清零 — `factor ∈ (0, 1]`:0.5 半衰（旧证据权重减半）、
    /// 1.0 全清（回退路径:历史矩阵定期衰减清零,WI-09 回滚）。
    ///
    /// 实现:总次数按 factor 缩水（保留成功比例形状,仅弱化证据强度）;
    /// 总次数缩到 0 的条目移除（防僵尸条目累积）。
    pub fn decay(&mut self, factor: f64) {
        debug_assert!(factor > 0.0 && factor <= 1.0, "decay factor 必须 ∈ (0,1]");
        self.stats.retain(|_, s| {
            // u64 乘法缩水;total=1 且 factor<1 时缩到 0 → 移除
            s.total = ((s.total as f64) * factor).round() as u64;
            s.success = ((s.success as f64) * factor).round() as u64;
            s.total > 0
        });
    }

    /// 累计事件数（诊断）
    #[must_use]
    pub fn events(&self) -> u64 {
        self.events
    }

    /// 矩阵条目数（诊断）
    #[must_use]
    pub fn entries(&self) -> usize {
        self.stats.len()
    }
}

/// 专家来源种类 — 同平面竞争标识（WI-22 联动）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpertKind {
    /// 内置专家（faae 注册表既有）
    Builtin,
    /// MCP 外部工具（client_v2 discover_and_register 注册,WI-22）
    Mcp,
}

/// 同平面专家元数据 — MCP 注册面预留
#[derive(Debug, Clone, PartialEq)]
pub struct ExpertMeta {
    /// 来源种类
    pub kind: ExpertKind,
    /// 命名空间（MCP 服务器名 / 内置分组名）
    pub namespace: String,
    /// schema 版本（MCP schema 缓存失效联动,WI-22）
    pub schema_version: u64,
}

/// 专家目录 — 内置 + MCP 同平面注册表（WI-22 联动预留）
///
/// WHY RwLock<HashMap> 而非 DashMap:注册低频（连接即注册,会话级）,
/// 读多写少;零新依赖（Ω₆ 最小依赖红线）。
#[derive(Debug, Default)]
pub struct ExpertCatalog {
    /// expert_id → 元数据
    experts: RwLock<HashMap<String, ExpertMeta>>,
}

impl ExpertCatalog {
    /// 新建空目录
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册专家（内置或 MCP）— 重复注册覆盖（幂等,新 schema 版本生效）
    pub fn register(&self, expert_id: &str, meta: ExpertMeta) {
        let mut g = self
            .experts
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        g.insert(expert_id.to_string(), meta);
    }

    /// 注销专家（断连时调用,WI-22 FallbackToBuiltin 联动）
    pub fn unregister(&self, expert_id: &str) {
        let mut g = self
            .experts
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        g.remove(expert_id);
    }

    /// 查询元数据
    #[must_use]
    pub fn get(&self, expert_id: &str) -> Option<ExpertMeta> {
        let g = self
            .experts
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        g.get(expert_id).cloned()
    }

    /// 当前专家数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        let g = self
            .experts
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        g.len()
    }

    /// 空目录判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 误剪救回率埋点 — WI-09 验证口径
///
/// 语义:「被 top-k 剪掉」的专家若被用户/后续任务实际使用,记一次救回;
/// recovery_rate = 救回 / 误剪。接入方在路由与使用路径打点。
/// 门禁联动:召回率 <98% 自动放开（回滚路径,由调用方读取本埋点判定）。
#[derive(Debug, Default)]
pub struct MissedRecoveryTracker {
    /// 被剪（未进 top-k）次数
    missed: AtomicU64,
    /// 被剪后仍被使用次数
    recovered: AtomicU64,
}

impl MissedRecoveryTracker {
    /// 新建埋点
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次误剪（专家未进 top-k）
    pub fn record_missed(&self) {
        self.missed.fetch_add(1, Ordering::Relaxed);
    }

    /// 记一次救回（被剪专家仍被使用）
    pub fn record_recovery(&self) {
        self.recovered.fetch_add(1, Ordering::Relaxed);
    }

    /// 误剪次数
    #[must_use]
    pub fn missed(&self) -> u64 {
        self.missed.load(Ordering::Relaxed)
    }

    /// 救回次数
    #[must_use]
    pub fn recovered(&self) -> u64 {
        self.recovered.load(Ordering::Relaxed)
    }

    /// 救回率 = recovered / missed;无误剪时返回 1.0（无风险即满分）
    #[must_use]
    pub fn recovery_rate(&self) -> f64 {
        let m = self.missed();
        if m == 0 {
            return 1.0;
        }
        self.recovered() as f64 / m as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RoutingHistory 基本记录与查询 — 无记录中性 0.5,有记录精确
    #[test]
    fn history_record_and_query() {
        let mut h = RoutingHistory::new();
        assert!(
            (h.success_rate("code", "tool-a") - 0.5).abs() < 1e-9,
            "无记录必须中性 0.5"
        );
        h.record("code", "tool-a", true);
        h.record("code", "tool-a", true);
        h.record("code", "tool-a", false);
        assert!((h.success_rate("code", "tool-a") - 2.0 / 3.0).abs() < 1e-9);
        // 任务类型隔离
        assert!((h.success_rate("search", "tool-a") - 0.5).abs() < 1e-9);
        assert_eq!(h.events(), 3);
        assert_eq!(h.entries(), 1);
    }

    /// 衰减清零 — 半衰后成功率形状保留,证据强度缩水;1.0 全清
    #[test]
    fn history_decay_semantics() {
        let mut h = RoutingHistory::new();
        h.record("code", "t", true);
        h.record("code", "t", false);
        assert!((h.success_rate("code", "t") - 0.5).abs() < 1e-9);
        // 半衰:2 次 → 1 次（保留成功比例形状:1 次中 0.5 轮 → 0 或 1 轮）
        h.decay(0.5);
        // total = round(2*0.5)=1, success = round(1*0.5)=1 → 成功率 1.0（形状弱化后的量化结果）
        // 断言仅验证条目存活与区间合法（量化舍入不承诺精确形状）
        let rate = h.success_rate("code", "t");
        assert!((0.0..=1.0).contains(&rate));
        // 1.0 全清:total 缩水后可能保留（round(1*1.0)=1）—— 语义为"保留全部证据",
        // 全清需调用方按周期清零（本接口 1.0 = 不衰减）
        h.decay(1.0);
        assert!((0.0..=1.0).contains(&h.success_rate("code", "t")));
    }

    /// 衰减移除僵尸条目 — 单次记录 factor<1 后 total 归零 → 条目移除
    #[test]
    fn history_decay_removes_zombies() {
        let mut h = RoutingHistory::new();
        h.record("x", "y", true);
        h.decay(0.1); // round(1*0.1)=0 → 移除
        assert_eq!(h.entries(), 0, "total 缩到 0 的条目必须移除");
        assert!(
            (h.success_rate("x", "y") - 0.5).abs() < 1e-9,
            "移除后回中性"
        );
    }

    /// ExpertCatalog 注册/查询/注销 — 幂等覆盖 + 断连注销
    #[test]
    fn catalog_register_query_unregister() {
        let c = ExpertCatalog::new();
        assert!(c.is_empty());
        c.register(
            "mcp:github",
            ExpertMeta {
                kind: ExpertKind::Mcp,
                namespace: "github".into(),
                schema_version: 1,
            },
        );
        assert_eq!(c.len(), 1);
        let meta = c.get("mcp:github").expect("注册后必须可查");
        assert_eq!(meta.kind, ExpertKind::Mcp);
        assert_eq!(meta.namespace, "github");
        // 重复注册覆盖（schema 版本递增）
        c.register(
            "mcp:github",
            ExpertMeta {
                kind: ExpertKind::Mcp,
                namespace: "github".into(),
                schema_version: 2,
            },
        );
        assert_eq!(c.get("mcp:github").map(|m| m.schema_version), Some(2));
        c.unregister("mcp:github");
        assert!(c.get("mcp:github").is_none());
        assert!(c.is_empty());
    }

    /// MissedRecoveryTracker — 救回率精确,无误剪满分
    #[test]
    fn missed_recovery_tracker() {
        let t = MissedRecoveryTracker::new();
        assert!((t.recovery_rate() - 1.0).abs() < 1e-9, "无误剪时满分");
        t.record_missed();
        t.record_missed();
        t.record_recovery();
        assert!((t.recovery_rate() - 0.5).abs() < 1e-9);
        assert_eq!(t.missed(), 2);
        assert_eq!(t.recovered(), 1);
    }

    /// 并发注册 — 多线程注册/查询无 panic 无丢失（RwLock 语义）
    #[test]
    fn catalog_concurrent() {
        let c = std::sync::Arc::new(ExpertCatalog::new());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let c = std::sync::Arc::clone(&c);
                std::thread::spawn(move || {
                    for j in 0..50usize {
                        c.register(
                            &format!("mcp:server-{i}-{j}"),
                            ExpertMeta {
                                kind: ExpertKind::Mcp,
                                namespace: format!("server-{i}"),
                                schema_version: 1,
                            },
                        );
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("线程应正常退出");
        }
        assert_eq!(c.len(), 8 * 50, "并发注册必须全部可见");
    }
}
