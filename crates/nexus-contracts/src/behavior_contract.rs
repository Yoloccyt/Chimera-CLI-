//! 行为契约 — 类型在特定场景下的使用规范契约(polish-v2.7 P1-3)
//!
//! 对应架构层: L0 Contracts(新建)
//! 对应 ADR: ADR-049 决策 1(rl-types 归 L0,不建第二个契约 crate)
//! 对应设计源: `chimera_ultimate_polish_v2.7.md` §4.2(北大 NL2Pipeline gap + Qoder 证据纪律)
//!
//! # 设计决策(WHY)
//!
//! - **纯类型零逻辑**: 遵循 ADR-033 约束,仅类型定义与基础构造函数,
//!   契约的运行时校验由消费层(L9 efficiency-monitor RuntimeAuditor)实现
//! - **证据可验证**: `BehaviorContract` 的前置/后置/不变量均为可被
//!   Runtime Auditor 审计的文本断言,配套 `ContractExample` 提供可执行示例
//! - **消费层**: L9 efficiency-monitor(审计)/ L5 gsoe-evolution(AEGIS Evolver
//!   生成变体时的行为约束输入,Phase 2)
//!
//! # 完整实现时机
//!
//! 当前文件定义**类型骨架**(Phase 1),契约的自动提取与运行时审计
//! 在 Phase 2(AEGIS)与后续版本渐进落地。

use serde::{Deserialize, Serialize};

/// 契约适用场景 — 声明契约在哪类上下文中生效
///
/// WHY 枚举而非自由字符串:场景集合是封闭的(运行时/测试/进化三类),
/// 枚举提供编译期穷尽检查,避免消费层字符串模糊匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractContext {
    /// 生产运行时:契约违反应产出审计 Finding
    Runtime,
    /// 测试环境:契约作为测试断言来源
    Test,
    /// 进化流程:AEGIS Evolver 生成变体时的硬约束(违反即丢弃候选)
    Evolution,
}

/// 契约示例 — 展示契约的正确/错误用法
///
/// WHY 携带示例:契约文本断言是给人和 LLM 读的,示例提供可对照的
/// 具体形态,降低消费方(AEGIS Planner / 开发者)的误读率。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractExample {
    /// 示例描述(说明该示例演示哪条约束)
    pub description: String,
    /// 示例代码或配置片段
    pub snippet: String,
    /// 是否为正例(true = 符合契约,false = 违反契约的反例)
    pub is_positive: bool,
}

/// 行为契约 — 定义某类型在特定场景下的使用规范
///
/// # 核心语义
///
/// 契约由三类断言构成(均为人类可读文本,可被 Runtime Auditor 审计):
/// - **前置条件**: 使用目标类型前必须满足的状态
/// - **后置条件**: 使用完成后必须成立的状态
/// - **不变量**: 全程必须保持的性质
///
/// # 示例
///
/// ```
/// use nexus_contracts::{BehaviorContract, ContractContext};
///
/// let contract = BehaviorContract::new(
///     "bc-eventbus-subscribe-order",
///     "event_bus::EventBus",
///     ContractContext::Runtime,
/// )
/// .with_precondition("subscribe() 必须在 tokio::spawn() 之前同步调用")
/// .with_invariant("broadcast 不缓存历史消息,晚订阅者收不到早期事件");
///
/// assert_eq!(contract.preconditions.len(), 1);
/// assert_eq!(contract.invariants.len(), 1);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BehaviorContract {
    /// 契约唯一标识(建议 "bc-" 前缀 + kebab-case)
    pub contract_id: String,
    /// 目标类型的完整路径(如 "event_bus::EventBus")
    pub target_type: String,
    /// 适用场景
    pub context: ContractContext,
    /// 前置条件断言集合
    pub preconditions: Vec<String>,
    /// 后置条件断言集合
    pub postconditions: Vec<String>,
    /// 不变量断言集合
    pub invariants: Vec<String>,
    /// 正/反示例集合
    pub examples: Vec<ContractExample>,
}

impl BehaviorContract {
    /// 创建空断言的契约骨架(builder 起点)
    pub fn new(
        contract_id: impl Into<String>,
        target_type: impl Into<String>,
        context: ContractContext,
    ) -> Self {
        Self {
            contract_id: contract_id.into(),
            target_type: target_type.into(),
            context,
            preconditions: Vec::new(),
            postconditions: Vec::new(),
            invariants: Vec::new(),
            examples: Vec::new(),
        }
    }

    /// 追加一条前置条件(builder 风格,链式调用)
    #[must_use]
    pub fn with_precondition(mut self, assertion: impl Into<String>) -> Self {
        self.preconditions.push(assertion.into());
        self
    }

    /// 追加一条后置条件
    #[must_use]
    pub fn with_postcondition(mut self, assertion: impl Into<String>) -> Self {
        self.postconditions.push(assertion.into());
        self
    }

    /// 追加一条不变量
    #[must_use]
    pub fn with_invariant(mut self, assertion: impl Into<String>) -> Self {
        self.invariants.push(assertion.into());
        self
    }

    /// 追加一个示例
    #[must_use]
    pub fn with_example(mut self, example: ContractExample) -> Self {
        self.examples.push(example);
        self
    }

    /// 契约是否为空(三类断言均无)— 空契约无审计价值
    pub fn is_empty(&self) -> bool {
        self.preconditions.is_empty()
            && self.postconditions.is_empty()
            && self.invariants.is_empty()
    }

    /// 强制层校验（Milestone B-3c，九层防御 L0 补齐）
    ///
    /// 给定执行观测到的"已满足断言"集合，校验契约全部断言（前置 + 后置 +
    /// 不变量）是否被覆盖。观测条目按**包含匹配**（断言为自然语言，观测
    /// 侧可能携带上下文后缀）。
    ///
    /// # 返回
    /// - `Satisfied`：全部断言被观测覆盖
    /// - `Violated { missing }`：未被覆盖的断言列表（消费方应发布
    ///   FormalViolation 事件并走审议，见 parliament::formal_violation）
    ///
    /// # 复杂度
    /// O(断言数 × 观测数)——契约断言数量级小（个位数），线性足够。
    pub fn enforce(&self, observed: &[String]) -> ContractCheckOutcome {
        let covered = |assertion: &str| observed.iter().any(|o| o.contains(assertion));
        let missing: Vec<String> = self
            .preconditions
            .iter()
            .chain(self.postconditions.iter())
            .chain(self.invariants.iter())
            .filter(|a| !covered(a))
            .cloned()
            .collect();
        if missing.is_empty() {
            ContractCheckOutcome::Satisfied
        } else {
            ContractCheckOutcome::Violated { missing }
        }
    }
}

/// 强制层校验结果（Milestone B-3c）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractCheckOutcome {
    /// 全部断言满足
    Satisfied,
    /// 存在未覆盖断言（消费方应发布 FormalViolation 事件）
    Violated {
        /// 未被观测覆盖的断言列表
        missing: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_accumulates_assertions() {
        let contract = BehaviorContract::new("bc-test", "my::Type", ContractContext::Runtime)
            .with_precondition("pre-1")
            .with_precondition("pre-2")
            .with_postcondition("post-1")
            .with_invariant("inv-1");
        assert_eq!(contract.preconditions, vec!["pre-1", "pre-2"]);
        assert_eq!(contract.postconditions, vec!["post-1"]);
        assert_eq!(contract.invariants, vec!["inv-1"]);
        assert!(!contract.is_empty());
    }

    #[test]
    fn test_empty_contract_detected() {
        let contract = BehaviorContract::new("bc-empty", "my::Type", ContractContext::Test);
        assert!(contract.is_empty());
    }

    #[test]
    fn test_serde_roundtrip() {
        let contract = BehaviorContract::new("bc-rt", "my::Type", ContractContext::Evolution)
            .with_invariant("inv")
            .with_example(ContractExample {
                description: "正例".into(),
                snippet: "let x = 1;".into(),
                is_positive: true,
            });
        let json = serde_json::to_string(&contract).expect("序列化失败");
        let back: BehaviorContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(contract, back);
        // 场景枚举 snake_case 序列化约定
        assert!(json.contains("\"evolution\""));
    }
}
