//! RLVR：可验证奖励强化学习（Milestone D-2d，设计 §11.2 目标形态）
//!
//! 语法/逻辑/沙箱三级验证奖励 + 延迟惩罚：
//! `reward = Σ验证档得分 + pass_rate×1.5 − latency_ms/1000`
//!
//! R2 冻结（ADR-042）+ ADR-049 降级裁决 + 项目规范（避免 Box<dyn Trait）：
//! verifier 用 **enum dispatch**（`VerifierKind`）而非设计骨架的
//! `Vec<Box<dyn Verifier>>`——规则式判定，替换为生产实现时接口不变。

/// 规则式验证器（enum dispatch——项目规范：避免 Box<dyn Trait>）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifierKind {
    /// 语法验证：非空且不含 NUL 等非法控制字符
    Syntax,
    /// 逻辑验证：含逻辑结构标记（def/fn/return 等）
    Logic,
    /// 沙箱验证：执行通过标记（模拟沙箱运行 PASS 输出）
    Sandbox,
}

impl VerifierKind {
    /// 规则式判定
    ///
    /// - `Syntax`: 非空且不含 '\0'
    /// - `Logic`: 含逻辑结构标记（`def ` / `fn ` / `return`）
    /// - `Sandbox`: 含执行通过标记 `PASS`
    pub fn verify(self, output: &str) -> bool {
        match self {
            VerifierKind::Syntax => !output.is_empty() && !output.contains('\0'),
            VerifierKind::Logic => {
                output.contains("def ") || output.contains("fn ") || output.contains("return")
            }
            VerifierKind::Sandbox => output.contains("PASS"),
        }
    }
}

/// 测试用例（沙箱验证的通过率判定：`output.contains(expected)`）
#[derive(Debug, Clone, PartialEq)]
pub struct TestCase {
    /// 期望出现在输出中的标记
    pub expected: String,
}

/// RLVR：可验证奖励（设计 §11.2）
#[derive(Debug, Clone)]
pub struct RLVR {
    verifiers: Vec<VerifierKind>,
}

impl RLVR {
    /// 构造：验证器序列（通常 [Syntax, Logic, Sandbox]）
    pub fn new(verifiers: Vec<VerifierKind>) -> Self {
        Self { verifiers }
    }

    /// 计算可验证奖励（设计 §11.2 `compute_reward`）
    ///
    /// - Syntax 档：通过 +0.5 / 失败 −1.0
    /// - Logic 档：通过 +1.0 / 失败 −2.0
    /// - 沙箱档：测试用例通过率 `pass_rate × 1.5`（空用例 → 0）
    /// - 延迟惩罚：`−latency_ms / 1000.0`
    pub fn compute_reward(&self, output: &str, test_cases: &[TestCase], latency_ms: u64) -> f32 {
        let mut reward = 0.0f32;
        for v in &self.verifiers {
            match v {
                VerifierKind::Syntax => reward += if v.verify(output) { 0.5 } else { -1.0 },
                VerifierKind::Logic => reward += if v.verify(output) { 1.0 } else { -2.0 },
                // Sandbox 档由 pass_rate 表达（避免双计）
                VerifierKind::Sandbox => {}
            }
        }
        if !test_cases.is_empty() {
            let passed = test_cases
                .iter()
                .filter(|t| output.contains(&t.expected))
                .count() as f32;
            reward += (passed / test_cases.len() as f32) * 1.5;
        }
        reward -= latency_ms as f32 / 1000.0;
        reward
    }
}
