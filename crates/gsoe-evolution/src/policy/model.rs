//! 策略模型调用注入接口（Milestone C-2，Week-7 TODO 闭合）
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §6 C-2
//!
//! # 职责
//!
//! 将 grpo/fitness/mutation 的占位（Lcg 模拟 logits / 规则评分）替换为
//! 可注入的真实模型调用：`PolicyModelInvoker` trait 由编排器（L10）接线
//! 真实模型实现，L5 gsoe-evolution 不依赖 L10（依赖铁律合规，注入式）。
//!
//! # 回退语义（R2 冻结面外）
//!
//! 未注入 invoker 时回退 `LcgModelInvoker`（确定性 PRNG，现行为保持不变）：
//! - grpo 采样：Lcg 扰动（Week-7 前行为）
//! - fitness 评分：规则 (reward+1)/2
//! - mutation 扰动：Lcg 随机
//!
//! # 硬约束
//!
//! trait 同步方法（`Send + Sync`）；真实模型调用的异步/网络由编排器侧
//! 封装（内部 spawn_blocking），保持 L5 调用链同步不变。

use crate::policy::grpo::Lcg;

/// 策略模型调用器 — grpo 采样 / fitness 评判 / mutation 引导的统一注入点
pub trait PolicyModelInvoker: Send + Sync {
    /// 生成 logits 向量（grpo 采样：动作 = 基准 + mutation_rate × logits）
    ///
    /// `seed` 为确定性回退的随机种子；真实实现可忽略 seed 直接采样。
    fn logits(&self, seed: u64, dim: usize) -> Vec<f32>;

    /// 模型评判 fitness（替代规则 (reward+1)/2）
    ///
    /// 输入原始奖励与动作数，返回 [0, 1] 评判分。
    fn judge_fitness(&self, reward: f32, action_count: usize) -> f32;

    /// 变异方向引导（可选；返回 [-1, 1] 方向，None 语义由调用方回退 Lcg）
    fn mutate_direction(&self, seed: u64) -> f32 {
        // 默认：无引导（调用方回退 Lcg 随机扰动）
        let _ = seed;
        0.0
    }
}

/// 确定性实现 — 测试/编排器 mock（固定值 logits / 固定评判分 / 固定方向）
///
/// WHY 具名实现：测试与编排器 mock 复用；真实模型实现由编排器提供
/// （L10 侧实现 trait，内部封装 MCP Mesh / HTTP 调用）。
#[derive(Debug, Clone)]
pub struct DeterministicInvoker {
    /// 固定 logits / 评判分 / 方向值
    value: f32,
}

impl DeterministicInvoker {
    /// 创建固定值 invoker（logits 每维 = value；judge = value clamp [0,1]）
    pub fn new(value: f32) -> Self {
        Self { value }
    }
}

impl PolicyModelInvoker for DeterministicInvoker {
    fn logits(&self, _seed: u64, dim: usize) -> Vec<f32> {
        vec![self.value; dim]
    }

    fn judge_fitness(&self, _reward: f32, _action_count: usize) -> f32 {
        self.value.clamp(0.0, 1.0)
    }

    fn mutate_direction(&self, _seed: u64) -> f32 {
        self.value
    }
}

/// 默认回退实现 — Lcg 确定性 PRNG（Week-7 前行为）
///
/// 未注入 invoker 时的等价实现；保持既有测试与行为不变量。
#[derive(Debug, Default)]
pub struct LcgModelInvoker;

impl PolicyModelInvoker for LcgModelInvoker {
    fn logits(&self, seed: u64, dim: usize) -> Vec<f32> {
        let mut rng = Lcg::new(seed);
        (0..dim).map(|_| rng.next_f32()).collect()
    }

    fn judge_fitness(&self, reward: f32, _action_count: usize) -> f32 {
        ((reward + 1.0) / 2.0).clamp(0.0, 1.0)
    }

    fn mutate_direction(&self, seed: u64) -> f32 {
        let mut rng = Lcg::new(seed);
        rng.next_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_invoker_is_constant() {
        let invoker = DeterministicInvoker::new(0.5);
        assert_eq!(invoker.logits(1, 4), vec![0.5; 4]);
        assert_eq!(invoker.judge_fitness(0.0, 10), 0.5);
        assert_eq!(invoker.mutate_direction(1), 0.5);
    }

    #[test]
    fn lcg_invoker_matches_rule_fitness() {
        let invoker = LcgModelInvoker;
        assert!((invoker.judge_fitness(1.0, 0) - 1.0).abs() < 1e-6);
        assert!((invoker.judge_fitness(-1.0, 0) - 0.0).abs() < 1e-6);
        assert_eq!(invoker.logits(42, 3).len(), 3);
    }
}
