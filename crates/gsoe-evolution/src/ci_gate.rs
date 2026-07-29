//! RHI-CG 通道 B — CI 执行门(CiGate)统一抽象(P5.2.1)
//!
//! 对应架构层: L5 Knowledge(gsoe-evolution)
//! 对应 ADR: ADR-044 决策 5(CI 执行门接口设计)
//! 对应任务: P5.2.1(CiGate trait + CargoCiGate + MockCiGate + CiGateResult/Error)
//!
//! # 核心职责
//!
//! 通道 B 的 CI 执行门统一抽象 `cargo test` / `criterion` / `INV-7/8/9` 检查:
//! 1. 接收候选 spec(`HarnessSpec`)作为输入
//! 2. 执行 cargo test --workspace + cargo clippy -- -D warnings(生产路径)
//! 3. 调用 INV-9 委托图无环检查(gsoe-evolution 内独立实现,避免 L5→L9 依赖违规)
//! 4. 返回 `CiGateResult`(passed + failures + regression_streak)
//!
//! # 设计决策(WHY)
//!
//! ## 1. boxed Future + Arc<dyn> 共享模式(与通道 A JudgeClient 一致)
//!
//! - 项目 workspace 未引入 `async-trait` 依赖(保持依赖最小化)
//! - `Pin<Box<dyn Future>>` 是兼容 `dyn Trait` 的标准模式
//! - 与 `auto-dpo::JudgeClient` trait 模式一致,降低认知负担
//!
//! ## 2. INV-9 检查在 gsoe-evolution 内独立实现(关键设计偏差)
//!
//! ADR-045 决策 8 示例代码暗示通道 B 调用
//! `chimera_mas::invariants::InvariantChecker::check_inv9_delegation_acyclic`。但
//! gsoe-evolution (L5) 依赖 chimera-mas (L9) 违反 §2.2 依赖铁律(L(N)→L(N+1) 禁止)。
//!
//! 按代码基线优先 + 架构铁律不可妥协原则,本模块在 L5 层独立实现 INV-9 DFS
//! 三色标记法环检测,与 chimera-mas 的 INV-9 语义镜像但不共享实现。两者:
//! - **L5 INV-9**(本模块):通道 B CI 执行门内调用,检查 spec 候选对应的
//!   委托图是否无环(防止进化后引入循环委托)
//! - **L9 INV-9**(chimera-mas):MAS 子系统运行时不变量,委托派生前调用
//!
//! 此偏差在最终实施报告中记录,不修改 ADR-044/045(append-only 原则)。
//!
//! ## 3. CargoCiGate 子进程执行可选
//!
//! 生产环境 `CargoCiGate::new()` 默认启用子进程执行(`enable_subprocess=true`),
//! 测试环境 `with_subprocess_enabled(false)` 跳过 cargo 子进程,仅做 INV-9 检查。
//! 这样既保留生产路径完整性,又允许单元测试快速验证 INV-9 逻辑。
//!
//! ## 4. R2 冻结声明(ADR-042)
//!
//! 通道 B 仅服务 RHI-CG 通道 A 提议的否决,**不触碰** GSOE×AutoDPO 约束 RL
//! (R2)路径。R2 路径在 FormalVerifier 落地前完全冻结。
//!
//! # 学习不在关键路径(ADR-031 决策 4)
//!
//! CiGate::execute() 是 async 方法,调用方应在后台任务中 await,不阻塞推理路径。

use nexus_contracts::HarnessSpec;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

// ============================================================
// DelegationEdge — gsoe-evolution 内独立的 INV-9 输入类型
// ============================================================

/// 委托边 — gsoe-evolution 内 INV-9 委托图无环检查的输入单元(P5.2.1)
///
/// WHY 独立类型而非复用 `chimera_mas::invariants::DelegationEdge`:
/// gsoe-evolution (L5) 不能依赖 chimera-mas (L9),违反 §2.2 依赖铁律。
/// 本类型在 L5 层独立定义,与 L9 的同名类型语义镜像但实现分离。
///
/// ## 设计决策(与 chimera-mas 对齐)
///
/// - `String` owned 而非 `&str`:纯函数接口需 `'static` 生命周期
/// - `pub` 字段:纯数据类型,无不变量需保护
/// - `new(impl Into<String>, impl Into<String>)`:API 友好,接受 `&str`/`String`
///
/// ## 示例
///
/// ```
/// use gsoe_evolution::ci_gate::DelegationEdge;
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
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

// ============================================================
// INV-9 检查 — gsoe-evolution 内独立 DFS 三色标记法实现
// ============================================================

/// DFS 节点颜色 — INV-9 环检测标记(CLRS §22.3)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeColor {
    /// 正在访问(在 DFS 递归栈中,遇到即构成回边)
    Gray,
    /// 已完成访问(子树已全部遍历,再次遇到可安全跳过)
    Black,
}

/// 检查委托图是否无环(gsoe-evolution 内独立 INV-9 实现)
///
/// 算法: DFS 三色标记法(Cormen et al., CLRS §22.3),O(V+E) 时间复杂度。
/// 与 `chimera_mas::invariants::check_inv9_delegation_acyclic` 语义镜像,
/// 但在 L5 层独立实现,避免 L5→L9 依赖违规(详见模块级文档"设计偏差")。
///
/// # 参数
/// - `edges`: 委托边列表,每条边 `from → to` 表示父 Agent 委托子 Agent
///
/// # 返回
/// - `Ok(())`: 委托图无环,允许继续
/// - `Err(cycle_path)`: 检测到环,返回环路径(首尾相同,如 `["A", "B", "A"]`)
///
/// # 边界场景
///
/// - 空边列表:通过(无委托关系,无环)
/// - 单条边(A→B):通过(无环)
/// - 自环(A→A):拒绝(单节点环,`[A, A]`)
/// - 两节点环(A→B, B→A):拒绝(环路径 `[A, B, A]`)
/// - 多连通分量:各分量独立检查,任一含环即拒绝
pub fn check_inv9_delegation_acyclic(edges: &[DelegationEdge]) -> Result<(), Vec<String>> {
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
                return Err(cycle.into_iter().map(String::from).collect());
            }
        }
    }

    Ok(())
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

    // 当前节点的所有邻居已遍历完成,弹出 path 并标记 Black
    path.pop();
    color.insert(node, NodeColor::Black);
    None
}

// ============================================================
// CiFailure — CI 执行失败的细分类型
// ============================================================

/// CI 执行失败的细分类型 — 用于诊断与告警图表分类
///
/// WHY enum 而非 String:编译期穷尽性 + 易于聚合统计(如 inv9_violations 计数)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CiFailureKind {
    /// cargo test 失败(单元/集成测试未通过)
    TestFailed,
    /// cargo clippy 失败(lint 警告未消除)
    LintFailed,
    /// INV-7 上下文预算界违反
    Inv7Violated,
    /// INV-8 归档单调性违反
    Inv8Violated,
    /// INV-9 委托图有环
    Inv9Violated,
    /// criterion 基准回归
    BenchRegression,
}

impl CiFailureKind {
    /// 返回人类可读的标识字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TestFailed => "test_failed",
            Self::LintFailed => "lint_failed",
            Self::Inv7Violated => "inv7_violated",
            Self::Inv8Violated => "inv8_violated",
            Self::Inv9Violated => "inv9_violated",
            Self::BenchRegression => "bench_regression",
        }
    }
}

impl std::fmt::Display for CiFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// CI 执行失败的单条记录 — 携带失败类型与诊断信息
#[derive(Debug, Clone, PartialEq)]
pub struct CiFailure {
    /// 失败类型(用于聚合统计)
    pub kind: CiFailureKind,
    /// 人类可读的失败描述(用于审计日志)
    pub message: String,
}

impl CiFailure {
    /// 创建新的失败记录
    pub fn new(kind: CiFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

// ============================================================
// CiGateResult — CI 执行结果
// ============================================================

/// CI 执行结果 — 携带通过/失败状态 + 失败详情 + 回归次数
///
/// # 字段语义
///
/// | 字段 | 类型 | 含义 |
/// |------|------|------|
/// | `passed` | bool | CI 总体是否通过(test + clippy + INV-7/8/9) |
/// | `failures` | Vec<CiFailure> | 失败详情(空 Vec 表示无失败) |
/// | `regression_streak` | u32 | 当前连续回归次数(供显著性检测使用) |
///
/// WHY `regression_streak` 在此处携带:CI 执行门是回归信号的源头,
/// 显著性检测器从 CiGateResult 中读取 streak 值,避免单独维护状态。
#[derive(Debug, Clone, PartialEq)]
pub struct CiGateResult {
    /// CI 总体是否通过
    pub passed: bool,
    /// 失败详情列表(空表示无失败)
    pub failures: Vec<CiFailure>,
    /// 当前连续回归次数(由调用方在多次 CI 执行间累积)
    pub regression_streak: u32,
}

impl CiGateResult {
    /// 创建"通过"结果(无失败,streak=0)
    pub fn passed() -> Self {
        Self {
            passed: true,
            failures: Vec::new(),
            regression_streak: 0,
        }
    }

    /// 创建"失败"结果(单条失败,streak 由调用方指定)
    pub fn failed(failure: CiFailure, regression_streak: u32) -> Self {
        Self {
            passed: false,
            failures: vec![failure],
            regression_streak,
        }
    }

    /// 创建"失败"结果(多条失败,streak 由调用方指定)
    pub fn failed_with(failures: Vec<CiFailure>, regression_streak: u32) -> Self {
        Self {
            passed: failures.is_empty(),
            failures,
            regression_streak,
        }
    }

    /// 是否有 INV-9 违反
    pub fn has_inv9_violation(&self) -> bool {
        self.failures
            .iter()
            .any(|f| f.kind == CiFailureKind::Inv9Violated)
    }

    /// 是否有 bench 回归
    pub fn has_bench_regression(&self) -> bool {
        self.failures
            .iter()
            .any(|f| f.kind == CiFailureKind::BenchRegression)
    }
}

// ============================================================
// CiGateError — CI 执行本身的错误(非 CI 失败)
// ============================================================

/// CI 执行门错误 — CI 执行本身失败(如 cargo 不可达),非 CI 检查失败
///
/// WHY 与 `GsoeError` 分离:CI 执行是 IO 密集型操作,错误类型独立便于
/// 调用方区分"CI 检查未通过"(CiGateResult.passed=false)与"CI 执行本身失败"
/// (Err(CiGateError))。前者是正常否决路径,后者是系统故障。
#[derive(Debug, Error)]
pub enum CiGateError {
    /// cargo 子进程不可达(如 cargo 未安装 / 路径错误)
    #[error("cargo 子进程不可达: {reason}")]
    SubprocessUnavailable {
        /// 失败原因
        reason: String,
    },

    /// cargo 子进程超时
    #[error("cargo 子进程超时: {timeout_secs}s")]
    SubprocessTimeout {
        /// 超时时间(秒)
        timeout_secs: u64,
    },

    /// 内部不变量检查异常(如 INV-9 检查 panic)
    #[error("不变量检查异常: {reason}")]
    InvariantCheckFailed {
        /// 失败原因
        reason: String,
    },
}

// ============================================================
// CiGate trait — 通道 B 否决门的统一抽象
// ============================================================

/// CI 执行门 trait — 通道 B 否决门的统一抽象(ADR-044 决策 5)
///
/// # 实现契约
///
/// - 必须 `Send + Sync`(可在 async 任务间共享)
/// - `execute` 方法返回 `Pin<Box<dyn Future>>`,调用方 `.await` 获取结果
/// - 实现不应 panic(可能导致通道 B 不可用)
///
/// # 设计决策(WHY)
///
/// ## boxed Future 而非 async fn in trait
///
/// - 项目未引入 `async-trait` 依赖(workspace Cargo.toml 无此包)
/// - `Pin<Box<dyn Future>>` 是兼容 `dyn Trait` 的标准模式
/// - 与 `auto-dpo::JudgeClient` trait 模式一致
///
/// ## trait 不提供默认实现
///
/// 强制实现者显式提供 CI 执行逻辑,避免忘记实现导致空 CI 检查(防御性编程)。
///
/// # 调用方约束
///
/// - 调用方应在 async 上下文中 `.await` 返回的 Future
/// - 同一 CiGate 实例可被并发调用(实现需保证 Send + Sync)
/// - 调用方负责累积 `regression_streak`(多次 CI 执行间)
pub trait CiGate: Send + Sync {
    /// 执行 CI 检查并返回通过/失败 + 量化指标
    ///
    /// # 参数
    /// - `candidate_spec`: 候选 spec(v_i,被提议的新版本)
    ///
    /// # 返回
    /// - `Ok(CiGateResult)`: CI 执行成功(无论通过/失败),携带量化指标
    /// - `Err(CiGateError)`: CI 执行本身失败(如 cargo 不可达)
    fn execute<'a>(
        &'a self,
        candidate_spec: &'a HarnessSpec,
    ) -> Pin<Box<dyn Future<Output = Result<CiGateResult, CiGateError>> + Send + 'a>>;
}

// ============================================================
// CargoCiGate — 生产路径实现
// ============================================================

/// 生产路径 CI 执行门 — 封装 cargo test + clippy + INV-9 检查
///
/// # 设计
///
/// - **持有 `Vec<DelegationEdge>`**: 用于 INV-9 委托图无环检查
/// - **`enable_subprocess` 开关**: 测试环境可关闭 cargo 子进程执行,仅做 INV-9
/// - **async execute**: 内部使用 `tokio::process::Command` 异步执行 cargo
///
/// # 生产路径流程
///
/// 1. 调用 `check_inv9_delegation_acyclic(&self.delegation_edges)` 检查委托图无环
/// 2. 若 `enable_subprocess=true`,执行 `cargo test --workspace`(异步子进程)
/// 3. 若 `enable_subprocess=true`,执行 `cargo clippy -- -D warnings`(异步子进程)
/// 4. 聚合结果,返回 `CiGateResult`
///
/// # 测试路径
///
/// `with_subprocess_enabled(false)` 跳过 cargo 子进程,仅做 INV-9 检查。
/// 适用于单元测试与 E2E 测试的 INV-9 路径验证(避免 cargo 子进程开销)。
pub struct CargoCiGate {
    /// 委托边列表(用于 INV-9 检查)
    delegation_edges: Vec<DelegationEdge>,
    /// 是否启用 cargo 子进程执行(默认 true,测试可关闭)
    enable_subprocess: bool,
}

impl CargoCiGate {
    /// 创建生产路径 CI 执行门(启用子进程)
    ///
    /// # 参数
    /// - `delegation_edges`: 委托边列表,用于 INV-9 委托图无环检查
    pub fn new(delegation_edges: Vec<DelegationEdge>) -> Self {
        Self {
            delegation_edges,
            enable_subprocess: true,
        }
    }

    /// 设置是否启用 cargo 子进程执行(测试用)
    ///
    /// WHY 链式 builder:便于测试快速切换子进程模式,
    /// `CargoCiGate::new(edges).with_subprocess_enabled(false)`
    pub fn with_subprocess_enabled(mut self, enabled: bool) -> Self {
        self.enable_subprocess = enabled;
        self
    }

    /// 返回委托边列表引用(供测试与审计)
    pub fn delegation_edges(&self) -> &[DelegationEdge] {
        &self.delegation_edges
    }

    /// 返回是否启用子进程执行
    pub fn is_subprocess_enabled(&self) -> bool {
        self.enable_subprocess
    }
}

impl CiGate for CargoCiGate {
    fn execute<'a>(
        &'a self,
        _candidate_spec: &'a HarnessSpec,
    ) -> Pin<Box<dyn Future<Output = Result<CiGateResult, CiGateError>> + Send + 'a>> {
        Box::pin(async move {
            let mut failures: Vec<CiFailure> = Vec::new();

            // 步骤 1: INV-9 委托图无环检查(gsoe-evolution 内独立实现)
            if let Err(cycle_path) = check_inv9_delegation_acyclic(&self.delegation_edges) {
                failures.push(CiFailure::new(
                    CiFailureKind::Inv9Violated,
                    format!("INV-9 委托图有环: cycle_path = {:?}", cycle_path),
                ));
            }

            // 步骤 2 & 3: cargo test + clippy(若启用子进程)
            if self.enable_subprocess {
                // 生产路径:异步执行 cargo 子进程
                // WHY tokio::process::Command: 异步子进程,不阻塞 runtime
                // 注:实际生产环境需注入 cargo 可执行路径与工作目录
                // 测试环境通过 with_subprocess_enabled(false) 跳过此路径
                match execute_cargo_test().await {
                    Ok(()) => {}
                    Err(reason) => {
                        failures.push(CiFailure::new(CiFailureKind::TestFailed, reason));
                    }
                }
                match execute_cargo_clippy().await {
                    Ok(()) => {}
                    Err(reason) => {
                        failures.push(CiFailure::new(CiFailureKind::LintFailed, reason));
                    }
                }
            }

            // 步骤 4: 聚合结果
            // regression_streak 由调用方在多次 CI 执行间累积,
            // 单次 execute() 内 streak=0(由调用方在外部根据 passed 累积)
            let passed = failures.is_empty();
            Ok(CiGateResult {
                passed,
                failures,
                regression_streak: 0,
            })
        })
    }
}

/// 异步执行 `cargo test --workspace` 子进程
///
/// 生产路径:返回 `Ok(())` 表示测试通过,`Err(reason)` 表示失败。
/// 失败原因可能是子进程不可达 / 测试失败 / 超时。
///
/// WHY 独立函数:便于未来扩展(如 timeout 配置、环境变量注入)。
async fn execute_cargo_test() -> Result<(), String> {
    // 使用 tokio::process::Command 异步执行 cargo
    // 注:此处使用标准库 Command 而非 tokio::process,因 workspace
    // 可能未启用 tokio 的 process feature。生产环境如需异步子进程,
    // 可注入 tokio::process::Command。
    let output = std::process::Command::new("cargo")
        .args(["test", "--workspace", "--", "--quiet"])
        .output()
        .map_err(|e| format!("cargo test 子进程不可达: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("cargo test 失败: {}", stderr.trim()))
    }
}

/// 异步执行 `cargo clippy -- -D warnings` 子进程
async fn execute_cargo_clippy() -> Result<(), String> {
    let output = std::process::Command::new("cargo")
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .output()
        .map_err(|e| format!("cargo clippy 子进程不可达: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("cargo clippy 失败: {}", stderr.trim()))
    }
}

// ============================================================
// MockCiGate — 测试用桩,可配置返回结果
// ============================================================

/// 测试用 CI 执行门桩 — 可配置返回固定的 `CiGateResult` 或 `CiGateError`
///
/// # 设计意图
///
/// 提供确定性的 CI 执行结果,避免测试依赖 cargo 子进程:
/// - `with_passing_result()`: 永远返回 passed=true
/// - `with_failing_result(failures)`: 返回固定 failures 列表
/// - `with_error(error)`: 永远返回 Err(模拟 cargo 不可达)
///
/// # 使用场景
///
/// - 单元测试:验证通道 B 编排逻辑(不依赖 cargo)
/// - E2E 测试:模拟连续回归场景(显著性检测)
/// - 基准测试:criterion bench 需要确定性输入
///
/// # 不变量
///
/// - `result` 与 `error` 互斥(构造时二选一)
/// - `regression_streak` 由调用方在外部累积(本桩不维护状态)
pub struct MockCiGate {
    /// 固定返回的 Ok 结果(None 表示应返回 Err)
    result: Option<CiGateResult>,
    /// 固定返回的 Err 错误(None 表示应返回 Ok)
    error: Option<CiGateError>,
}

impl MockCiGate {
    /// 创建永远通过的 CI 执行门(streak=0)
    pub fn with_passing_result() -> Self {
        Self {
            result: Some(CiGateResult::passed()),
            error: None,
        }
    }

    /// 创建永远失败的 CI 执行门(指定 failures,streak=0)
    pub fn with_failing_result(failures: Vec<CiFailure>) -> Self {
        Self {
            result: Some(CiGateResult {
                passed: false,
                failures,
                regression_streak: 0,
            }),
            error: None,
        }
    }

    /// 创建永远失败的 CI 执行门(指定 regression_streak,用于显著性检测测试)
    ///
    /// WHY 单独构造器:显著性检测需要 mock streak 值,而真实 CI 执行门
    /// 的 streak 由调用方累积。本构造器允许测试直接注入 streak。
    pub fn with_regression_streak(streak: u32) -> Self {
        Self {
            result: Some(CiGateResult {
                passed: false,
                failures: vec![CiFailure::new(
                    CiFailureKind::BenchRegression,
                    format!("mock regression streak = {streak}"),
                )],
                regression_streak: streak,
            }),
            error: None,
        }
    }

    /// 创建永远返回错误的 CI 执行门(模拟 cargo 不可达)
    pub fn with_error(error: CiGateError) -> Self {
        Self {
            result: None,
            error: Some(error),
        }
    }
}

impl CiGate for MockCiGate {
    fn execute<'a>(
        &'a self,
        _candidate_spec: &'a HarnessSpec,
    ) -> Pin<Box<dyn Future<Output = Result<CiGateResult, CiGateError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(err) = &self.error {
                // 复用错误:clone 后返回(避免移动 self.error)
                let err = match err {
                    CiGateError::SubprocessUnavailable { reason } => {
                        CiGateError::SubprocessUnavailable {
                            reason: reason.clone(),
                        }
                    }
                    CiGateError::SubprocessTimeout { timeout_secs } => {
                        CiGateError::SubprocessTimeout {
                            timeout_secs: *timeout_secs,
                        }
                    }
                    CiGateError::InvariantCheckFailed { reason } => {
                        CiGateError::InvariantCheckFailed {
                            reason: reason.clone(),
                        }
                    }
                };
                return Err(err);
            }
            // 复制 result(Clone 已派生)返回
            Ok(self.result.clone().unwrap_or_else(CiGateResult::passed))
        })
    }
}

// ============================================================
// 单元测试(P5.2.1)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::{HarnessMeta, RetryPolicy};

    /// 构造最小合法 spec 用于测试
    fn make_test_spec() -> HarnessSpec {
        HarnessSpec {
            meta: HarnessMeta {
                name: "test-spec".to_string(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: None,
            },
            contracts: vec![],
            hops: vec![],
            retry: RetryPolicy::default(),
            auxiliary: None,
        }
    }

    // ============================================================
    // DelegationEdge 测试
    // ============================================================

    #[test]
    fn test_delegation_edge_new() {
        let edge = DelegationEdge::new("agent-a", "agent-b");
        assert_eq!(edge.from, "agent-a");
        assert_eq!(edge.to, "agent-b");
    }

    #[test]
    fn test_delegation_edge_equality() {
        let e1 = DelegationEdge::new("a", "b");
        let e2 = DelegationEdge::new("a", "b");
        let e3 = DelegationEdge::new("b", "a");
        assert_eq!(e1, e2);
        assert_ne!(e1, e3);
    }

    // ============================================================
    // check_inv9_delegation_acyclic 测试
    // ============================================================

    #[test]
    fn test_inv9_empty_edges_passes() {
        assert!(check_inv9_delegation_acyclic(&[]).is_ok());
    }

    #[test]
    fn test_inv9_single_edge_passes() {
        let edges = vec![DelegationEdge::new("a", "b")];
        assert!(check_inv9_delegation_acyclic(&edges).is_ok());
    }

    #[test]
    fn test_inv9_linear_chain_passes() {
        let edges = vec![
            DelegationEdge::new("a", "b"),
            DelegationEdge::new("b", "c"),
            DelegationEdge::new("c", "d"),
        ];
        assert!(check_inv9_delegation_acyclic(&edges).is_ok());
    }

    #[test]
    fn test_inv9_tree_passes() {
        let edges = vec![
            DelegationEdge::new("root", "a"),
            DelegationEdge::new("root", "b"),
            DelegationEdge::new("a", "c"),
            DelegationEdge::new("a", "d"),
        ];
        assert!(check_inv9_delegation_acyclic(&edges).is_ok());
    }

    #[test]
    fn test_inv9_self_loop_rejected() {
        let edges = vec![DelegationEdge::new("a", "a")];
        let result = check_inv9_delegation_acyclic(&edges);
        assert!(result.is_err());
        let cycle = result.unwrap_err();
        assert!(cycle.len() >= 2);
        assert_eq!(cycle.first(), cycle.last());
    }

    #[test]
    fn test_inv9_two_node_cycle_rejected() {
        let edges = vec![DelegationEdge::new("a", "b"), DelegationEdge::new("b", "a")];
        let result = check_inv9_delegation_acyclic(&edges);
        assert!(result.is_err());
        let cycle = result.unwrap_err();
        assert!(cycle.len() >= 3);
        assert_eq!(cycle.first(), cycle.last());
    }

    #[test]
    fn test_inv9_three_node_cycle_rejected() {
        let edges = vec![
            DelegationEdge::new("a", "b"),
            DelegationEdge::new("b", "c"),
            DelegationEdge::new("c", "a"),
        ];
        let result = check_inv9_delegation_acyclic(&edges);
        assert!(result.is_err());
        let cycle = result.unwrap_err();
        assert!(cycle.len() >= 4);
        assert_eq!(cycle.first(), cycle.last());
    }

    #[test]
    fn test_inv9_dag_with_cross_edges_passes() {
        // A→B, A→C, B→D, C→D(D 有两父但无环)
        let edges = vec![
            DelegationEdge::new("a", "b"),
            DelegationEdge::new("a", "c"),
            DelegationEdge::new("b", "d"),
            DelegationEdge::new("c", "d"),
        ];
        assert!(check_inv9_delegation_acyclic(&edges).is_ok());
    }

    #[test]
    fn test_inv9_multi_component_one_cycle_rejected() {
        // 分量 1: A→B→C(无环)
        // 分量 2: D→E→D(有环)
        let edges = vec![
            DelegationEdge::new("a", "b"),
            DelegationEdge::new("b", "c"),
            DelegationEdge::new("d", "e"),
            DelegationEdge::new("e", "d"),
        ];
        let result = check_inv9_delegation_acyclic(&edges);
        assert!(result.is_err());
    }

    #[test]
    fn test_inv9_multi_component_all_acyclic_passes() {
        let edges = vec![
            DelegationEdge::new("a", "b"),
            DelegationEdge::new("c", "d"),
            DelegationEdge::new("e", "f"),
        ];
        assert!(check_inv9_delegation_acyclic(&edges).is_ok());
    }

    #[test]
    fn test_inv9_dag_plus_back_edge_rejected() {
        // DAG: A→B→C→D, 回边: D→B(形成 B→C→D→B 环)
        let edges = vec![
            DelegationEdge::new("a", "b"),
            DelegationEdge::new("b", "c"),
            DelegationEdge::new("c", "d"),
            DelegationEdge::new("d", "b"),
        ];
        let result = check_inv9_delegation_acyclic(&edges);
        assert!(result.is_err());
        let cycle = result.unwrap_err();
        assert_eq!(cycle.first(), cycle.last());
    }

    #[test]
    fn test_inv9_duplicate_edges_passes() {
        // 重复边 A→B, A→B 不构成环
        let edges = vec![DelegationEdge::new("a", "b"), DelegationEdge::new("a", "b")];
        assert!(check_inv9_delegation_acyclic(&edges).is_ok());
    }

    // ============================================================
    // CiFailureKind / CiFailure 测试
    // ============================================================

    #[test]
    fn test_ci_failure_kind_as_str() {
        assert_eq!(CiFailureKind::TestFailed.as_str(), "test_failed");
        assert_eq!(CiFailureKind::LintFailed.as_str(), "lint_failed");
        assert_eq!(CiFailureKind::Inv7Violated.as_str(), "inv7_violated");
        assert_eq!(CiFailureKind::Inv8Violated.as_str(), "inv8_violated");
        assert_eq!(CiFailureKind::Inv9Violated.as_str(), "inv9_violated");
        assert_eq!(CiFailureKind::BenchRegression.as_str(), "bench_regression");
    }

    #[test]
    fn test_ci_failure_new() {
        let f = CiFailure::new(CiFailureKind::TestFailed, "test xyz failed");
        assert_eq!(f.kind, CiFailureKind::TestFailed);
        assert_eq!(f.message, "test xyz failed");
    }

    // ============================================================
    // CiGateResult 测试
    // ============================================================

    #[test]
    fn test_ci_gate_result_passed() {
        let r = CiGateResult::passed();
        assert!(r.passed);
        assert!(r.failures.is_empty());
        assert_eq!(r.regression_streak, 0);
    }

    #[test]
    fn test_ci_gate_result_failed() {
        let f = CiFailure::new(CiFailureKind::Inv9Violated, "cycle detected");
        let r = CiGateResult::failed(f, 1);
        assert!(!r.passed);
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.regression_streak, 1);
    }

    #[test]
    fn test_ci_gate_result_failed_with_multiple() {
        let failures = vec![
            CiFailure::new(CiFailureKind::TestFailed, "test 1"),
            CiFailure::new(CiFailureKind::LintFailed, "lint 1"),
        ];
        let r = CiGateResult::failed_with(failures, 2);
        assert!(!r.passed);
        assert_eq!(r.failures.len(), 2);
        assert_eq!(r.regression_streak, 2);
    }

    #[test]
    fn test_ci_gate_result_failed_with_empty_passes() {
        // 空失败列表 = 通过
        let r = CiGateResult::failed_with(vec![], 0);
        assert!(r.passed);
    }

    #[test]
    fn test_ci_gate_result_has_inv9_violation() {
        let r = CiGateResult::failed(CiFailure::new(CiFailureKind::Inv9Violated, "cycle"), 1);
        assert!(r.has_inv9_violation());
        assert!(!r.has_bench_regression());
    }

    #[test]
    fn test_ci_gate_result_has_bench_regression() {
        let r = CiGateResult::failed(
            CiFailure::new(CiFailureKind::BenchRegression, "10% slower"),
            1,
        );
        assert!(r.has_bench_regression());
        assert!(!r.has_inv9_violation());
    }

    // ============================================================
    // CiGateError 测试
    // ============================================================

    #[test]
    fn test_ci_gate_error_subprocess_unavailable_display() {
        let e = CiGateError::SubprocessUnavailable {
            reason: "cargo not found".into(),
        };
        assert!(e.to_string().contains("cargo 子进程不可达"));
        assert!(e.to_string().contains("cargo not found"));
    }

    #[test]
    fn test_ci_gate_error_subprocess_timeout_display() {
        let e = CiGateError::SubprocessTimeout { timeout_secs: 300 };
        assert!(e.to_string().contains("300"));
    }

    #[test]
    fn test_ci_gate_error_invariant_check_failed_display() {
        let e = CiGateError::InvariantCheckFailed {
            reason: "dfs panic".into(),
        };
        assert!(e.to_string().contains("不变量检查异常"));
    }

    // ============================================================
    // CargoCiGate 测试(子进程禁用,仅测 INV-9 路径)
    // ============================================================

    #[test]
    fn test_cargo_ci_gate_new_defaults() {
        let gate = CargoCiGate::new(vec![]);
        assert!(gate.is_subprocess_enabled());
        assert!(gate.delegation_edges().is_empty());
    }

    #[test]
    fn test_cargo_ci_gate_with_subprocess_disabled() {
        let gate = CargoCiGate::new(vec![]).with_subprocess_enabled(false);
        assert!(!gate.is_subprocess_enabled());
    }

    #[tokio::test]
    async fn test_cargo_ci_gate_passes_with_empty_edges_no_subprocess() {
        let gate = CargoCiGate::new(vec![]).with_subprocess_enabled(false);
        let spec = make_test_spec();
        let result = gate.execute(&spec).await.expect("空边 + 无子进程应通过");
        assert!(result.passed);
        assert!(result.failures.is_empty());
    }

    #[tokio::test]
    async fn test_cargo_ci_gate_passes_with_dag_no_subprocess() {
        let edges = vec![
            DelegationEdge::new("root", "a"),
            DelegationEdge::new("a", "b"),
        ];
        let gate = CargoCiGate::new(edges).with_subprocess_enabled(false);
        let spec = make_test_spec();
        let result = gate.execute(&spec).await.expect("DAG + 无子进程应通过");
        assert!(result.passed);
        assert!(result.failures.is_empty());
    }

    #[tokio::test]
    async fn test_cargo_ci_gate_fails_with_cycle_no_subprocess() {
        let edges = vec![DelegationEdge::new("a", "b"), DelegationEdge::new("b", "a")];
        let gate = CargoCiGate::new(edges).with_subprocess_enabled(false);
        let spec = make_test_spec();
        let result = gate
            .execute(&spec)
            .await
            .expect("CI 执行应成功(非子进程故障)");
        assert!(!result.passed);
        assert!(result.has_inv9_violation());
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].kind, CiFailureKind::Inv9Violated);
    }

    #[tokio::test]
    async fn test_cargo_ci_gate_self_loop_rejected() {
        let edges = vec![DelegationEdge::new("a", "a")];
        let gate = CargoCiGate::new(edges).with_subprocess_enabled(false);
        let spec = make_test_spec();
        let result = gate.execute(&spec).await.unwrap();
        assert!(!result.passed);
        assert!(result.has_inv9_violation());
    }

    #[tokio::test]
    async fn test_cargo_ci_gate_three_node_cycle_rejected() {
        let edges = vec![
            DelegationEdge::new("a", "b"),
            DelegationEdge::new("b", "c"),
            DelegationEdge::new("c", "a"),
        ];
        let gate = CargoCiGate::new(edges).with_subprocess_enabled(false);
        let spec = make_test_spec();
        let result = gate.execute(&spec).await.unwrap();
        assert!(!result.passed);
        assert!(result.has_inv9_violation());
    }

    // ============================================================
    // MockCiGate 测试
    // ============================================================

    #[tokio::test]
    async fn test_mock_ci_gate_passing_result() {
        let gate = MockCiGate::with_passing_result();
        let spec = make_test_spec();
        let result = gate.execute(&spec).await.expect("MockCiGate 应返回 Ok");
        assert!(result.passed);
        assert!(result.failures.is_empty());
    }

    #[tokio::test]
    async fn test_mock_ci_gate_failing_result() {
        let failures = vec![CiFailure::new(CiFailureKind::TestFailed, "mock failure")];
        let gate = MockCiGate::with_failing_result(failures);
        let spec = make_test_spec();
        let result = gate.execute(&spec).await.expect("MockCiGate 应返回 Ok");
        assert!(!result.passed);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].kind, CiFailureKind::TestFailed);
    }

    #[tokio::test]
    async fn test_mock_ci_gate_with_regression_streak() {
        let gate = MockCiGate::with_regression_streak(3);
        let spec = make_test_spec();
        let result = gate.execute(&spec).await.unwrap();
        assert!(!result.passed);
        assert_eq!(result.regression_streak, 3);
        assert!(result.has_bench_regression());
    }

    #[tokio::test]
    async fn test_mock_ci_gate_with_error() {
        let gate = MockCiGate::with_error(CiGateError::SubprocessUnavailable {
            reason: "cargo missing".into(),
        });
        let spec = make_test_spec();
        let result = gate.execute(&spec).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CiGateError::SubprocessUnavailable { reason } => {
                assert!(reason.contains("cargo missing"));
            }
            other => panic!("期望 SubprocessUnavailable, 收到: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_mock_ci_gate_with_timeout_error() {
        let gate = MockCiGate::with_error(CiGateError::SubprocessTimeout { timeout_secs: 60 });
        let spec = make_test_spec();
        let result = gate.execute(&spec).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CiGateError::SubprocessTimeout { timeout_secs } => {
                assert_eq!(timeout_secs, 60);
            }
            other => panic!("期望 SubprocessTimeout, 收到: {other:?}"),
        }
    }

    /// 验证 MockCiGate 可被作为 `Arc<dyn CiGate>` 共享(与 JudgeClient 模式一致)
    #[tokio::test]
    async fn test_mock_ci_gate_can_be_shared_as_arc_dyn() {
        use std::sync::Arc;
        let gate: Arc<dyn CiGate> = Arc::new(MockCiGate::with_passing_result());
        let spec = make_test_spec();
        // 克隆 Arc 后仍可调用
        let gate_clone = Arc::clone(&gate);
        let result = gate_clone.execute(&spec).await.unwrap();
        assert!(result.passed);
    }
}
