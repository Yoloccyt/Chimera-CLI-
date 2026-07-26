//! MAS 子系统不变量检查器 — Task 21 合规增补(INV-7 / INV-8 / INV-9)
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 编码 §21.2 新增不变量,为 ADR-027 Part II 闭环提供编译期可证伪的守护
//!
//! ## 不变量语义(对应设计文档 §21.2)
//!
//! ### INV-7 — 上下文预算界(§15.4)
//!
//! > 单 Agent 驻留 ≤ `effective_capacity(tier)`,全局 `M_total ≤ 130MB × 0.9` 方可派生新 Agent。
//!
//! - **单 Agent 约束**: `agent_resident ≤ effective_capacity`
//!   - `agent_resident`: 该 Agent 当前实际驻留的 Token 数(密集工作集,非 1M 可寻址)
//!   - `effective_capacity`: 该 Agent tier 对应的有效容量上限(L3=128K 热工作集)
//! - **全局约束**: `M_total ≤ 130MB × 0.9`(预留 10% 余量,§15.3 派生准入闸)
//!   - `M_total`: 全 Agent 池聚合内存(MB)
//!   - 130MB: 50 Agent 稳态分布预算上限(ADR-026 决策 7)
//!   - 0.9: 派生前预留 10% 余量,避免 OOM
//! - **失败处理**: 返回 `MasError::TokenBudgetExceeded`(复用 Part I 既有变体,§15.4 明确要求)
//!
//! ### INV-8 — 归档单调性(§17.5)
//!
//! > 记忆只能沿 Hot→Warm→Cold→Ice 单向降级归档,不可反向膨胀。
//!
//! - **tier 顺序**: Hot(0) < Warm(1) < Cold(2) < Ice(3)
//! - **合法操作**: `from_tier.level() < to_tier.level()`(只能升 level,即降级)
//! - **非法操作**: `from_tier.level() >= to_tier.level()`(同层或反向,违反单调性)
//! - **失败处理**: 返回 `MasError::ArchiveMonotonicityViolated { from_tier, to_tier }`
//!
//! ### INV-9 — 委托图无环(P3-W11.3 §21.2)
//!
//! > 委托关系构成的有向图必须无环,防止递归委托死循环(§6.2: 零循环委托)。
//!
//! - **输入**: `&[DelegationEdge]` 委托边列表,每条边 `from → to` 表示父 Agent 委托子 Agent
//! - **算法**: DFS 三色标记法(CLSR §22.3),O(V+E) 时间复杂度
//! - **合法图**: DAG(有向无环图),包括树、森林、多连通分量(各分量无环)
//! - **非法图**: 含任意环(自环 / 多节点环),返回 `MasError::DelegationCycleDetected { cycle_path }`
//! - **失败处理**: 返回带环路径的错误,调用方发布 Critical 事件并阻断涉环 Agent 派生
//!
//! ## 设计原则
//!
//! - **纯函数**: `check_inv7_*` / `check_inv8_*` / `check_inv9_*` 不持有状态,不发布事件,
//!   仅做不变量断言。调用方负责发布 Critical 事件(§6.2 红线:Critical 用 mpsc)。
//! - **无副作用**: 函数返回 `Result<()>`,失败时仅返回错误,不修改输入。
//! - **编译期可证伪**: 通过 `pub struct InvariantChecker` 提供命名空间,
//!   方法为关联函数(非 `&self`),便于在派生准入闸 / 归档降级点直接调用。
//!
//! ## 红线对齐
//!
//! - §4.1: 库层 thiserror,无 unwrap/expect
//! - §4.4 反模式 6: f32 禁止隐式转 f64,全程 f64(MB 计算)
//! - §6.1: 单函数 ≤ 200 行(本模块函数均 < 30 行)
//! - `#![forbid(unsafe_code)]`: crate 级已在 lib.rs 声明,本模块无需重复

use crate::error::{MasError, Result};
use std::collections::{HashMap, HashSet};

// ============================================================
// 常量(SubTask 21.6 REFACTOR — 抽取)
// ============================================================

/// 全局内存预算上限(MB)— §15.3 派生准入闸基线
///
/// 来源:50 Agent 稳态分布(30×L0 + 12×L1 + 5×L2 + 3×L3)聚合 ≤ 130MB,
/// 对照暴力加载 305MB 节省 57%(ADR-026 决策 7)。
///
/// WHY 抽取为常量:防止阈值被意外修改导致 INV-7 漂移,
/// 多处调用(派生准入闸 / 监控告警 / proptest)共享同一真值源。
pub const MEMORY_BUDGET_MB: usize = 130;

/// 内存预算利用率上限(0.0-1.0)— §15.3 派生准入闸余量
///
/// 派生前预留 10% 余量,避免 OOM:`M_total ≤ 130MB × 0.9 = 117MB`。
///
/// WHY 用 f64 而非 f32:§4.4 反模式 6,f32 转 f64 精度膨胀导致误判
/// (如 0.9f32 as f64 > 0.9)。全程用 f64 计算避免精度问题。
pub const MEMORY_BUDGET_UTILIZATION: f64 = 0.9;

/// 归档单调性违反的容忍阈值(当前未使用,保留供未来"批量降级"场景)
///
/// 语义:若批量降级中违反比例 < 此阈值,允许部分通过并告警;
/// 当前实现为严格单调(0 容忍),此常量为未来扩展预留(§3.4 GA 后演进)。
#[allow(dead_code)]
pub const ARCHIVE_MONOTONICITY_VIOLATION_THRESHOLD: f64 = 0.1;

// ============================================================
// ArchiveTier — 归档层级枚举
// ============================================================

/// 归档层级 — 对应 CMT 冷热分层(§17.2)
///
/// 顺序严格单调递增:Hot(0) < Warm(1) < Cold(2) < Ice(3)。
/// INV-8 单调性判断依据 `level()` 数值比较。
///
/// ## 与 cmt-tiering 的关系
///
/// 本类型为 chimera-mas 内部不变量检查的语义镜像,
/// **不直接依赖** `cmt-tiering` crate(避免 L9→L3 跨层依赖,§2.2 铁律)。
/// 调用方负责将 cmt-tiering 的 tier 映射为本类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveTier {
    /// 热层 — 高频访问,LRU 256 条上限(§17.2)
    Hot,
    /// 温层 — 中频访问,4096 条上限(§17.2)
    Warm,
    /// 冷层 — 低频访问,65536 条上限,衰减降级(§17.2)
    Cold,
    /// 冰层 — 极低频访问,无上限,只读 KeepForever(§17.2)
    Ice,
}

impl ArchiveTier {
    /// 返回层级数值(0=Hot, 1=Warm, 2=Cold, 3=Ice)
    ///
    /// 用于 INV-8 单调性判断:`from.level() < to.level()` 表示合法降级。
    /// 数值严格递增,确保 `Hot < Warm < Cold < Ice`。
    ///
    /// ## 示例
    ///
    /// ```
    /// use chimera_mas::invariants::ArchiveTier;
    /// assert_eq!(ArchiveTier::Hot.level(), 0);
    /// assert_eq!(ArchiveTier::Ice.level(), 3);
    /// ```
    pub const fn level(self) -> u8 {
        match self {
            ArchiveTier::Hot => 0,
            ArchiveTier::Warm => 1,
            ArchiveTier::Cold => 2,
            ArchiveTier::Ice => 3,
        }
    }
}

// ============================================================
// DelegationEdge — 委托边(INV-9 输入类型)
// ============================================================

/// 委托边 — INV-9 委托图无环检查的输入单元(P3-W11.3 §21.2)
///
/// 表示一条有向委托关系: `from`(委托方/父 Agent) → `to`(被委托方/子 Agent)。
/// 多条 `DelegationEdge` 构成委托有向图,INV-9 要求该图无环。
///
/// ## 设计决策
///
/// - **独立类型而非复用 `AgentTask`**:`AgentTask` 含 `parent_agent_id: Option<String>`,
///   但被委托方 Agent ID 在执行时才生成(`{parent}::sub::{task_id}`),不存在于
///   `AgentTask` 中。独立 `DelegationEdge` 显式表达 from→to 有向边,语义清晰。
/// - **`String` 而非 `&str`**:纯函数接口需 `'static` 生命周期,`String` owned 避免
///   调用方维护引用生命周期。构造开销可接受(委托边数量受 `MAX_AGENT_DEPTH=5` 约束)。
///
/// ## 示例
///
/// ```
/// use chimera_mas::invariants::DelegationEdge;
///
/// let edge = DelegationEdge::new("root-agent", "main-agent-1");
/// assert_eq!(edge.from, "root-agent");
/// assert_eq!(edge.to, "main-agent-1");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DelegationEdge {
    /// 委托方 Agent ID(父 Agent,委托发起方)
    pub from: String,
    /// 被委托方 Agent ID(子 Agent,委托接收方)
    pub to: String,
}

impl DelegationEdge {
    /// 创建新的委托边
    ///
    /// ## 参数
    /// - `from`: 委托方 Agent ID
    /// - `to`: 被委托方 Agent ID
    ///
    /// ## 示例
    ///
    /// ```
    /// use chimera_mas::invariants::DelegationEdge;
    ///
    /// let edge = DelegationEdge::new("agent-a", "agent-b");
    /// ```
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

// ============================================================
// InvariantChecker — 不变量检查器(纯函数集合)
// ============================================================

/// 不变量检查器 — INV-7 / INV-8 / INV-9 的纯函数实现
///
/// 设计为关联函数(非 `&self` 方法),因为:
/// 1. 不变量检查无状态,无需实例化
/// 2. 关联函数便于在派生准入闸 / 归档降级点直接调用:
///    `InvariantChecker::check_inv7_context_budget(...)`
/// 3. 提供 `pub struct` 命名空间,使 API 自描述
///
/// ## 使用示例
///
/// ```
/// use chimera_mas::invariants::{ArchiveTier, DelegationEdge, InvariantChecker};
///
/// // INV-7:派生前检查全局内存预算
/// let _ = InvariantChecker::check_inv7_context_budget(64_000, 128_000, 80, 130);
///
/// // INV-8:归档降级前检查单调性
/// let _ = InvariantChecker::check_inv8_archive_monotonicity(
///     ArchiveTier::Hot,
///     ArchiveTier::Warm,
/// );
///
/// // INV-9:委托前检查委托图无环
/// let edges = vec![
///     DelegationEdge::new("root", "agent-a"),
///     DelegationEdge::new("agent-a", "agent-b"),
/// ];
/// let _ = InvariantChecker::check_inv9_delegation_acyclic(&edges);
/// ```
pub struct InvariantChecker;

impl InvariantChecker {
    /// INV-7 — 上下文预算界检查(§15.4)
    ///
    /// 同时校验单 Agent 驻留与全局内存预算两个约束(AND 关系):
    ///
    /// 1. `agent_resident ≤ effective_capacity` — 单 Agent 驻留不超 tier 容量
    /// 2. `m_total ≤ m_budget × MEMORY_BUDGET_UTILIZATION` — 全局内存不超派生阈值
    ///
    /// ## 参数
    ///
    /// - `agent_resident`: 该 Agent 当前实际驻留 Token 数(密集工作集)
    /// - `effective_capacity`: 该 Agent tier 对应的有效容量上限(L3=128K)
    /// - `m_total`: 全 Agent 池聚合内存(MB)
    /// - `m_budget`: 全局内存预算上限(MB,通常为 130)
    ///
    /// ## 返回
    ///
    /// - `Ok(())`: 两个约束均满足,允许派生 / 继续操作
    /// - `Err(MasError::TokenBudgetExceeded)`: 任一约束不满足(§15.4 复用既有变体)
    ///
    /// ## 边界场景
    ///
    /// - `agent_resident == effective_capacity`: 通过(等号允许,恰好满载)
    /// - `m_total == m_budget × 0.9`: 通过(等号允许,恰好达阈值)
    /// - `effective_capacity == 0`: 退化场景,`agent_resident > 0` 即报错
    /// - `m_budget == 0`: 退化场景,`m_total > 0` 即报错
    ///
    /// ## 红线对齐
    ///
    /// - §4.4 反模式 6: 用 f64 计算 `m_budget × 0.9`,避免 f32 精度膨胀
    /// - §15.4: 失败复用 `MasError::TokenBudgetExceeded`,不新建变体
    pub fn check_inv7_context_budget(
        agent_resident: usize,
        effective_capacity: usize,
        m_total: usize,
        m_budget: usize,
    ) -> Result<()> {
        // 约束 1:单 Agent 驻留 ≤ effective_capacity
        if agent_resident > effective_capacity {
            return Err(MasError::TokenBudgetExceeded {
                agent_id: "inv7-check".to_string(),
                current_tokens: agent_resident,
                max_tokens: effective_capacity,
            });
        }
        // 约束 2:全局 M_total ≤ m_budget × 0.9
        // WHY f64 中间值:§4.4 反模式 6,f32 转 f64 精度膨胀(0.9f32 as f64 > 0.9)
        let global_threshold = (m_budget as f64 * MEMORY_BUDGET_UTILIZATION) as usize;
        if m_total > global_threshold {
            return Err(MasError::TokenBudgetExceeded {
                agent_id: "inv7-global".to_string(),
                current_tokens: m_total,
                max_tokens: global_threshold,
            });
        }
        Ok(())
    }

    /// INV-8 — 归档单调性检查(§17.5)
    ///
    /// 验证归档操作沿 Hot→Warm→Cold→Ice 单向降级:
    ///
    /// - 合法: `from_tier.level() < to_tier.level()`(降级,level 升高)
    /// - 非法: `from_tier.level() >= to_tier.level()`(同层或反向膨胀)
    ///
    /// ## 参数
    ///
    /// - `from_tier`: 源归档级
    /// - `to_tier`: 目标归档级
    ///
    /// ## 返回
    ///
    /// - `Ok(())`: 合法降级,允许归档操作
    /// - `Err(MasError::ArchiveMonotonicityViolated)`: 同层或反向,拒绝
    ///
    /// ## 边界场景
    ///
    /// - `Hot → Warm`: 通过(level 0→1)
    /// - `Hot → Ice`: 通过(跨级降级 level 0→3)
    /// - `Hot → Hot`: 拒绝(同层,level 0→0)
    /// - `Ice → Hot`: 拒绝(反向膨胀,level 3→0)
    ///
    /// ## 错误字段
    ///
    /// 错误变体的 `from_tier`/`to_tier` 字段为 `format!("{tier:?}")` 形式
    /// (如 `"Hot"`/`"Ice"`),便于调用方无需导入 `ArchiveTier` 即可理解错误。
    pub fn check_inv8_archive_monotonicity(
        from_tier: ArchiveTier,
        to_tier: ArchiveTier,
    ) -> Result<()> {
        // 严格降级:to.level() 必须严格大于 from.level()
        if to_tier.level() <= from_tier.level() {
            return Err(MasError::ArchiveMonotonicityViolated {
                from_tier: format!("{from_tier:?}"),
                to_tier: format!("{to_tier:?}"),
            });
        }
        Ok(())
    }

    /// INV-9 — 委托图无环不变量检查(P3-W11.3 §21.2)
    ///
    /// 验证委托关系构成的有向图无环,防止循环委托导致递归死循环
    /// (§6.2 红线:零循环委托)。
    ///
    /// ## 算法
    ///
    /// DFS 三色标记法(Cormen et al., CLRS §22.3):
    /// - `White`(HashMap 缺失键): 未访问
    /// - `Gray`: 正在访问(在 DFS 递归栈中)
    /// - `Black`: 已完成访问
    ///
    /// 当 DFS 遇到 `Gray` 节点时,存在回边(back edge)→ 检测到环。
    /// 环路径从该 `Gray` 节点到当前节点,再加上该节点自身闭合。
    ///
    /// ## 时间复杂度
    ///
    /// O(V + E),其中 V = Agent 数,E = 委托边数。
    /// 委托图受 `MAX_AGENT_DEPTH=5` 约束,规模有限。
    ///
    /// ## 参数
    ///
    /// - `edges`: 委托边列表,每条边 `from → to` 表示父 Agent 委托给子 Agent
    ///
    /// ## 返回
    ///
    /// - `Ok(())`: 委托图无环,允许继续委托
    /// - `Err(MasError::DelegationCycleDetected)`: 检测到环,`cycle_path` 携带环路径
    ///
    /// ## 边界场景
    ///
    /// - 空边列表:通过(无委托关系,无环)
    /// - 单条边(A→B):通过(无环)
    /// - 自环(A→A):拒绝(单节点环,[A, A])
    /// - 两节点环(A→B, B→A):拒绝(环路径 [A, B, A])
    /// - 多连通分量:各分量独立检查,任一含环即拒绝
    ///
    /// ## 红线对齐
    ///
    /// - §6.2: 零循环委托(INV-9 无环)
    /// - §6.1: 单函数 ≤ 200 行(本方法 < 30 行,核心逻辑委托 `dfs_visit_cycle`)
    /// - 纯函数:不持有状态,不发布事件,调用方负责发布 Critical 事件
    pub fn check_inv9_delegation_acyclic(edges: &[DelegationEdge]) -> Result<()> {
        // 空图:无委托关系,直接通过
        if edges.is_empty() {
            return Ok(());
        }

        // 构建邻接表 + 收集所有节点(from / to 均纳入)
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut nodes: HashSet<&str> = HashSet::new();
        for edge in edges {
            adj.entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
            nodes.insert(edge.from.as_str());
            nodes.insert(edge.to.as_str());
        }

        // DFS 三色标记法:遍历所有节点,每个未访问节点(HashMap 缺失 = White)启动 DFS
        let mut color: HashMap<&str, NodeColor> = HashMap::new();
        let mut path: Vec<&str> = Vec::new();

        for &node in &nodes {
            // 仅对未访问节点(White)启动 DFS,已完成的(Black)跳过
            if !color.contains_key(node) {
                if let Some(cycle) = dfs_visit_cycle(node, &adj, &mut color, &mut path) {
                    return Err(MasError::DelegationCycleDetected {
                        cycle_path: cycle.into_iter().map(String::from).collect(),
                    });
                }
            }
        }

        Ok(())
    }
}

// ============================================================
// INV-9 辅助:DFS 三色标记法环检测
// ============================================================

/// DFS 节点颜色 — INV-9 环检测标记
///
/// 三色标记法(CLSR §22.3):
/// - `None`(HashMap 缺失键): 未访问(等价 CLRS `White`)
/// - `Gray`: 正在访问(在 DFS 递归栈中,遇到即构成回边)
/// - `Black`: 已完成访问(子树已全部遍历,再次遇到可安全跳过)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeColor {
    Gray,
    Black,
}

/// DFS 遍历单个节点,检测环(递归实现)
///
/// ## 执行流程
///
/// 1. 将当前节点标记为 `Gray`,压入 path(DFS 栈)
/// 2. 遍历所有邻居:
///    - `Gray` 邻居 → 回边 → 从 path 截取环路径返回
///    - `None`(未访问)→ 递归 DFS
///    - `Black`(已完成)→ 跳过
/// 3. path 弹出当前节点,标记为 `Black`
///
/// ## 返回
///
/// - `Some(Vec<&str>)`: 检测到环,返回环路径(首尾相同,如 `["A", "B", "A"]`)
/// - `None`: 从该节点可达的子图无环
///
/// ## 算法复杂度
///
/// O(V + E) 总计(所有节点各被访问一次),空间 O(V)(递归栈 + path)
fn dfs_visit_cycle<'a>(
    node: &'a str,
    adj: &HashMap<&'a str, Vec<&'a str>>,
    color: &mut HashMap<&'a str, NodeColor>,
    path: &mut Vec<&'a str>,
) -> Option<Vec<&'a str>> {
    color.insert(node, NodeColor::Gray);
    path.push(node);

    if let Some(neighbors) = adj.get(node) {
        for &neighbor in neighbors {
            match color.get(neighbor) {
                // 回边:neighbor 在当前 DFS 栈中(Gray)→ 检测到环
                Some(&NodeColor::Gray) => {
                    // 从 path 中找到 neighbor 的位置,截取环路径
                    // 环 = [neighbor, ..., node, neighbor]
                    let cycle_start = path.iter().position(|&n| n == neighbor)?;
                    let mut cycle: Vec<&'a str> = path[cycle_start..].to_vec();
                    cycle.push(neighbor); // 闭合环(首尾相同)
                    return Some(cycle);
                }
                // 未访问(None = White):递归 DFS
                None => {
                    if let Some(cycle) = dfs_visit_cycle(neighbor, adj, color, path) {
                        return Some(cycle);
                    }
                }
                // 已完成(Black):跳过,该子树已确认无环
                Some(&NodeColor::Black) => {}
            }
        }
    }

    // 当前节点的所有邻居已遍历完毕,标记为 Black 并从 path 弹出
    path.pop();
    color.insert(node, NodeColor::Black);
    None
}

// ============================================================
// 单元测试(模块内,与集成测试 tests/invariants_test.rs 互补)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// INV-7 单 Agent 边界:驻留 = 容量(等号允许)
    #[test]
    fn inv7_agent_resident_equal_to_capacity_is_ok() {
        let result = InvariantChecker::check_inv7_context_budget(128_000, 128_000, 0, 130);
        assert!(result.is_ok());
    }

    /// INV-7 单 Agent 边界:驻留 < 容量(尚未满载)
    #[test]
    fn inv7_agent_resident_below_capacity_is_ok() {
        let result = InvariantChecker::check_inv7_context_budget(64_000, 128_000, 0, 130);
        assert!(result.is_ok());
    }

    /// INV-7 单 Agent 超限:返回 TokenBudgetExceeded
    #[test]
    fn inv7_agent_resident_exceeds_capacity_returns_error() {
        let result = InvariantChecker::check_inv7_context_budget(200_000, 128_000, 0, 130);
        assert!(matches!(result, Err(MasError::TokenBudgetExceeded { .. })));
    }

    /// INV-7 全局边界:M_total = 130×0.9 = 117(等号允许)
    #[test]
    fn inv7_global_at_boundary_is_ok() {
        let m_total = (130_f64 * 0.9) as usize;
        let result = InvariantChecker::check_inv7_context_budget(0, 128_000, m_total, 130);
        assert!(result.is_ok());
    }

    /// INV-7 全局超限:M_total = 118 > 117
    #[test]
    fn inv7_global_exceeds_budget_returns_error() {
        let result = InvariantChecker::check_inv7_context_budget(0, 128_000, 118, 130);
        assert!(matches!(result, Err(MasError::TokenBudgetExceeded { .. })));
    }

    /// INV-8 合法降级:Hot→Warm→Cold→Ice 全链路
    #[test]
    fn inv8_monotonic_demotion_all_ok() {
        assert!(InvariantChecker::check_inv8_archive_monotonicity(
            ArchiveTier::Hot,
            ArchiveTier::Warm,
        )
        .is_ok());
        assert!(InvariantChecker::check_inv8_archive_monotonicity(
            ArchiveTier::Warm,
            ArchiveTier::Cold,
        )
        .is_ok());
        assert!(InvariantChecker::check_inv8_archive_monotonicity(
            ArchiveTier::Cold,
            ArchiveTier::Ice,
        )
        .is_ok());
        // 跨级降级(Hot→Ice)也应合法
        assert!(InvariantChecker::check_inv8_archive_monotonicity(
            ArchiveTier::Hot,
            ArchiveTier::Ice,
        )
        .is_ok());
    }

    /// INV-8 反向膨胀:Ice→Hot 等均拒绝
    #[test]
    fn inv8_reverse_promotion_returns_error() {
        let result =
            InvariantChecker::check_inv8_archive_monotonicity(ArchiveTier::Ice, ArchiveTier::Hot);
        match result {
            Err(MasError::ArchiveMonotonicityViolated { from_tier, to_tier }) => {
                assert_eq!(from_tier, "Ice");
                assert_eq!(to_tier, "Hot");
            }
            other => panic!("期望 ArchiveMonotonicityViolated,实际: {other:?}"),
        }
    }

    /// INV-8 同层操作:Hot→Hot 等拒绝
    #[test]
    fn inv8_same_tier_returns_error() {
        let result =
            InvariantChecker::check_inv8_archive_monotonicity(ArchiveTier::Hot, ArchiveTier::Hot);
        assert!(matches!(
            result,
            Err(MasError::ArchiveMonotonicityViolated { .. })
        ));
    }

    /// ArchiveTier::level() 严格递增
    #[test]
    fn archive_tier_level_strictly_increasing() {
        assert!(ArchiveTier::Hot.level() < ArchiveTier::Warm.level());
        assert!(ArchiveTier::Warm.level() < ArchiveTier::Cold.level());
        assert!(ArchiveTier::Cold.level() < ArchiveTier::Ice.level());
    }

    /// 常量稳定性:防止阈值被意外修改
    #[test]
    fn constants_are_stable() {
        assert_eq!(MEMORY_BUDGET_MB, 130);
        assert_eq!(MEMORY_BUDGET_UTILIZATION, 0.9);
        assert_eq!((130_f64 * 0.9) as usize, 117);
    }

    // ============================================================
    // INV-9 委托图无环不变量测试(P3-W11.3)
    // ============================================================

    /// INV-9 空边列表:无委托关系,直接通过
    #[test]
    fn inv9_empty_edges_is_ok() {
        let edges: Vec<DelegationEdge> = vec![];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(result.is_ok(), "空委托图应无环");
    }

    /// INV-9 单条边(A→B):无环,通过
    #[test]
    fn inv9_single_edge_is_ok() {
        let edges = vec![DelegationEdge::new("root", "agent-a")];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(result.is_ok(), "单条委托边应无环");
    }

    /// INV-9 线性链(A→B→C→D):无环,通过
    #[test]
    fn inv9_linear_chain_is_ok() {
        let edges = vec![
            DelegationEdge::new("root", "agent-a"),
            DelegationEdge::new("agent-a", "agent-b"),
            DelegationEdge::new("agent-b", "agent-c"),
            DelegationEdge::new("agent-c", "agent-d"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(result.is_ok(), "线性委托链应无环");
    }

    /// INV-9 树结构(root→A, root→B, A→C, A→D):无环,通过
    #[test]
    fn inv9_tree_structure_is_ok() {
        let edges = vec![
            DelegationEdge::new("root", "agent-a"),
            DelegationEdge::new("root", "agent-b"),
            DelegationEdge::new("agent-a", "agent-c"),
            DelegationEdge::new("agent-a", "agent-d"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(result.is_ok(), "树形委托结构应无环");
    }

    /// INV-9 自环(A→A):拒绝,环路径 [A, A]
    #[test]
    fn inv9_self_loop_is_rejected() {
        let edges = vec![DelegationEdge::new("agent-a", "agent-a")];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        match result {
            Err(MasError::DelegationCycleDetected { cycle_path }) => {
                assert_eq!(
                    cycle_path,
                    vec!["agent-a", "agent-a"],
                    "自环路径应为 [A, A]"
                );
            }
            other => panic!("自环应返回 DelegationCycleDetected, 实际: {other:?}"),
        }
    }

    /// INV-9 两节点环(A→B, B→A):拒绝,环路径 [A, B, A]
    #[test]
    fn inv9_two_node_cycle_is_rejected() {
        let edges = vec![
            DelegationEdge::new("agent-a", "agent-b"),
            DelegationEdge::new("agent-b", "agent-a"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        match result {
            Err(MasError::DelegationCycleDetected { cycle_path }) => {
                // 环路径首尾相同,中间为环上节点
                assert_eq!(cycle_path.first(), cycle_path.last(), "环路径首尾应相同");
                assert_eq!(cycle_path.len(), 3, "两节点环路径长度应为 3(首+中+尾)");
                // 验证环包含 A 和 B
                assert!(
                    cycle_path.contains(&"agent-a".to_string()),
                    "环应包含 agent-a"
                );
                assert!(
                    cycle_path.contains(&"agent-b".to_string()),
                    "环应包含 agent-b"
                );
            }
            other => panic!("两节点环应返回 DelegationCycleDetected, 实际: {other:?}"),
        }
    }

    /// INV-9 三节点环(A→B→C→A):拒绝,环路径长度 4
    #[test]
    fn inv9_three_node_cycle_is_rejected() {
        let edges = vec![
            DelegationEdge::new("agent-a", "agent-b"),
            DelegationEdge::new("agent-b", "agent-c"),
            DelegationEdge::new("agent-c", "agent-a"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        match result {
            Err(MasError::DelegationCycleDetected { cycle_path }) => {
                assert_eq!(cycle_path.first(), cycle_path.last(), "环路径首尾应相同");
                assert_eq!(cycle_path.len(), 4, "三节点环路径长度应为 4");
                // 验证环包含所有三个节点
                assert!(cycle_path.contains(&"agent-a".to_string()));
                assert!(cycle_path.contains(&"agent-b".to_string()));
                assert!(cycle_path.contains(&"agent-c".to_string()));
            }
            other => panic!("三节点环应返回 DelegationCycleDetected, 实际: {other:?}"),
        }
    }

    /// INV-9 多连通分量,全部无环:通过
    #[test]
    fn inv9_multiple_components_all_acyclic_is_ok() {
        // 两个独立连通分量:root→A→B 和 root2→C→D
        let edges = vec![
            DelegationEdge::new("root", "agent-a"),
            DelegationEdge::new("agent-a", "agent-b"),
            DelegationEdge::new("root2", "agent-c"),
            DelegationEdge::new("agent-c", "agent-d"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(result.is_ok(), "多连通分量均无环时应通过");
    }

    /// INV-9 多连通分量,其中一个有环:拒绝
    #[test]
    fn inv9_multiple_components_one_cyclic_is_rejected() {
        // 分量 1: root→A→B(无环)
        // 分量 2: C→D→C(有环)
        let edges = vec![
            DelegationEdge::new("root", "agent-a"),
            DelegationEdge::new("agent-a", "agent-b"),
            DelegationEdge::new("agent-c", "agent-d"),
            DelegationEdge::new("agent-d", "agent-c"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(
            matches!(result, Err(MasError::DelegationCycleDetected { .. })),
            "任一连通分量含环应拒绝"
        );
    }

    /// INV-9 深度 5 的线性链(匹配 MAX_AGENT_DEPTH):通过
    #[test]
    fn inv9_depth_5_chain_is_ok() {
        // root(0) → A(1) → B(2) → C(3) → D(4) → E(5)
        let edges = vec![
            DelegationEdge::new("root", "d1"),
            DelegationEdge::new("d1", "d2"),
            DelegationEdge::new("d2", "d3"),
            DelegationEdge::new("d3", "d4"),
            DelegationEdge::new("d4", "d5"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(result.is_ok(), "深度 5 的线性委托链应无环");
    }

    /// INV-9 DAG(含交叉边但无环):通过
    #[test]
    fn inv9_dag_with_cross_edges_is_ok() {
        // A→B, A→C, B→D, C→D(D 有两个父,但无环)
        let edges = vec![
            DelegationEdge::new("a", "b"),
            DelegationEdge::new("a", "c"),
            DelegationEdge::new("b", "d"),
            DelegationEdge::new("c", "d"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(result.is_ok(), "DAG(含交叉边但无环)应通过");
    }

    /// INV-9 环路径闭合性验证:首尾必须相同
    #[test]
    fn inv9_cycle_path_is_closed() {
        // 构造一个明确的三节点环
        let edges = vec![
            DelegationEdge::new("x", "y"),
            DelegationEdge::new("y", "z"),
            DelegationEdge::new("z", "x"),
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        match result {
            Err(MasError::DelegationCycleDetected { cycle_path }) => {
                // 环路径首尾相同(闭合)
                assert_eq!(
                    cycle_path.first(),
                    cycle_path.last(),
                    "环路径首尾应相同(闭合), 实际: {cycle_path:?}"
                );
                // 环路径长度 = 环上节点数 + 1(首尾重复)
                assert!(cycle_path.len() >= 2, "环路径至少含首尾 2 个元素");
            }
            Ok(()) => panic!("含环图应被拒绝"),
            Err(other) => panic!("应返回 DelegationCycleDetected, 实际: {other:?}"),
        }
    }

    /// INV-9 重复边(A→B, A→B):不构成环,通过
    #[test]
    fn inv9_duplicate_edges_no_cycle_is_ok() {
        let edges = vec![
            DelegationEdge::new("agent-a", "agent-b"),
            DelegationEdge::new("agent-a", "agent-b"), // 重复边
        ];
        let result = InvariantChecker::check_inv9_delegation_acyclic(&edges);
        assert!(result.is_ok(), "重复边不构成环,应通过");
    }
}
