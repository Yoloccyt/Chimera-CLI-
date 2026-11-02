//! GQEP 配置定义
//!
//! 控制聚集执行的超时、并发度与原子性行为。
//! 配置项默认值经过权衡,适合大多数 L6-L10 层操作场景。

use serde::{Deserialize, Serialize};

/// 单次 gather 允许的**最大在飞并发上界（静态帽）**——ADR-202 I2。
///
/// WHY 定义在本 crate而非 `nexus-contracts`（ADR-202 §11.4-D1 有据偏离）：
/// 机械门 G-91（`scripts/gate_manifest.toml:1002`）按**消费方 crate 数**统计
/// L0 pub 项，`< 2` 者必入 `contract_consumer_baseline.txt` 只减台账且
/// 「NEW 未登记必红」。本常量只有 gqep-executor 一个消费者，入 L0 即必然
/// 新增一行只减台账，违反「台账只减不增」与优先序「删除>合并>下沉>抽象>新增」。
///
/// WHY 取 256（ADR-202 §11.4-D2 推导，非拍脑袋）：
/// - 必须 `>= 100`：现默认 `max_concurrency = 100`，帽低于默认会静默改写默认语义，
///   违 I8「bound >= N 逐位等价」向后兼容判据。
/// - 先例对齐：仓内既有上限常量同值 `nexus-contracts/src/quota.rs:97 MAX_TASK_QUEUE_LENGTH = 256`。
/// - 职责界定：本帽的职责是**截断 `10^9` 型误配置**（那种值下限流形同虚设），
///   它**不是资源帽**——资源帽属 I3 内存自适应，因 gqep 无 per-op 实测预算而本批未做。
/// - 不按 RL-08（130MB / 50 agent）反推：那条红线量的是 MAS 50-agent 路径，
///   不是 gqep 单操作；无实测不得推数（v2.0 §8-bis 未实测数字禁入 perf 三表）。
pub const MAX_GATHER_FANOUT: usize = 256;

/// 编译期不变量：静态帽必须 `>= 1` 且 `>= ` 现默认 `max_concurrency`（100）。
///
/// WHY 放在常量旁而不是测试里：`clamp(1, MAX_GATHER_FANOUT)` 在 `min > max` 时会 panic，
/// 而帽与默认值都是 `const`——这类不变量能在编译期钉住就不应拖到运行期断言
/// （否则只能靠人记得跑测试）。改小到 <100 时本行直接编译失败。
const _: () = assert!(MAX_GATHER_FANOUT >= 100);

/// GQEP 执行器配置
///
/// 控制聚集执行的超时、并发度与原子性行为。
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GqepConfig {
    /// 默认操作超时(毫秒)
    ///
    /// 单个操作超过此时间未完成则触发 `OperationTimeout`。
    /// 默认 5000ms,平衡响应性与长操作容忍度。
    ///
    /// WHY 5000ms:L6-L10 层操作(如沙箱执行、模型调用、Wiki 沉淀)
    /// 通常在 1-3 秒内完成,5 秒超时足够覆盖正常波动,
    /// 同时避免长时间挂起(对应尸检教训:无超时导致永久挂起)。
    pub default_timeout_ms: u64,

    /// 最大并发操作数（**已生效的硬上界**）
    ///
    /// 限制单次 gather 同时在飞的 entangled future 数，防止资源耗尽
    /// （对应架构红线 §6.1「1M Token 暴力加载」）。默认 100。
    ///
    /// 生效路径：`gatherer` 按本值分批入流、每完成一个补一个（ADR-202 Option B），
    /// 与 `pvl-layer/src/concurrency.rs:77-89` 已生效的真限流同构；
    /// 载体始终是 `FuturesUnordered`（不违 `agents.md` 「并发收集用 FuturesUnordered」红线）。
    ///
    /// WHY 不用 `buffer_unordered`：其未拉取的源项**从未被 poll**，而 `entangle`
    /// 的注册与 `OrphanGuard` 创建都发生在 async 体首次执行
    /// （`qeep-protocol/src/protocol.rs:283-284` 源码自述）⇒ 被全局超时丢弃时
    /// **零归因静默消失**，直接破坏 UNLEARNABLE #3「零孤儿调用保证不可绕过」
    /// 与 `types.rs` 的书面承诺。ADR-202 因此否决 Option A。
    ///
    /// 有界语义：实际使用值经 [`GqepConfig::effective_bound`] 钳制到
    /// `[1, MAX_GATHER_FANOUT]`，故 `0` 等价串行而非「无限制」。
    pub max_concurrency: usize,

    /// 全局 gather 超时阈值(毫秒) — 整个聚集流程的 deadline
    ///
    /// 与 `default_timeout_ms`(单操作超时)构成双层超时防护:
    /// - `default_timeout_ms`:保护单个 future(entangle 内 tokio::time::timeout)
    /// - `gather_deadline_ms`:保护整个 gather 流程,包裹 stream.next() 循环,
    ///   防止大规模 gather 时单操作超时累积导致整体执行时间失控。
    ///
    /// WHY 5000ms 默认值:5s 平衡了大规模 gather 的容错性与系统延迟。
    /// L6-L10 层典型 gather(沙箱执行/模型调用/Wiki 沉淀)多在 1-3s 内完成,
    /// 5s deadline 足以覆盖正常波动,同时为异常场景兜底,避免永久挂起。
    ///
    /// 特殊值:`0` 表示禁用全局超时(向后兼容,行为与无全局超时一致),
    /// 此时仅单操作超时生效。与 `with_timeout` 的 `timeout_ms=0` 语义保持一致。
    pub gather_deadline_ms: u64,

    /// 是否启用批量原子性
    ///
    /// 启用后,`gather_atomic` 中任一操作失败将触发回滚。
    /// 默认 true,保证批量操作的一致性。
    pub batch_atomic_enabled: bool,
}

impl Default for GqepConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 5000,
            max_concurrency: 100,
            gather_deadline_ms: 5000,
            batch_atomic_enabled: true,
        }
    }
}

impl GqepConfig {
    /// 本次 gather 实际采用的并发上界，钳制到 `[1, MAX_GATHER_FANOUT]`。
    ///
    /// WHY 钳在**读取点**而非构造点：`GqepConfig` 可经 serde 直接反序列化
    /// （无构造钩子可拦，见 `test_config_serde_roundtrip`），所以任何绕过
    /// `Default` 的赋值路径都必须被读取点兼容。
    ///
    /// 同时满足两条不变量：
    /// - I1 下界：`0` 不得 panic/死锁，也不得等价「无限制」→ 下限 1
    /// - I2 上界：防「配置写 `10^9` 形同无界」→ 上限 `MAX_GATHER_FANOUT`
    ///
    /// WHY 用 `clamp` 而非 `max(1).min(cap)`：语义等价（clippy::manual_clamp 亦如此要求），
    /// 但 `clamp` 把「区间钳制」这一意图写在函数名上；其隐含前提
    /// `min <= max` 由 `MAX_GATHER_FANOUT >= 100` 的既有断言测试守住，
    /// 否则 `clamp` 会 panic（不得靠运气管）。
    #[must_use]
    pub fn effective_bound(&self) -> usize {
        self.max_concurrency.clamp(1, MAX_GATHER_FANOUT)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GqepConfig::default();
        assert_eq!(config.default_timeout_ms, 5000);
        assert_eq!(config.max_concurrency, 100);
        assert_eq!(config.gather_deadline_ms, 5000);
        assert!(config.batch_atomic_enabled);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = GqepConfig {
            default_timeout_ms: 3000,
            max_concurrency: 50,
            gather_deadline_ms: 8000,
            batch_atomic_enabled: false,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: GqepConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.default_timeout_ms, 3000);
        assert_eq!(restored.max_concurrency, 50);
        assert_eq!(restored.gather_deadline_ms, 8000);
        assert!(!restored.batch_atomic_enabled);
    }

    #[test]
    fn test_config_clone() {
        let config = GqepConfig::default();
        let cloned = config.clone();
        assert_eq!(config.default_timeout_ms, cloned.default_timeout_ms);
        assert_eq!(config.gather_deadline_ms, cloned.gather_deadline_ms);
    }

    // ============================================================
    // ADR-202 I1 / I2：并发上界钳制的两端必须有界
    // ============================================================

    /// I1 下界：`max_concurrency = 0` 不得等价「无限制」，必须钳到 1（串行）。
    ///
    /// 守住的具体退化：若钳制写成 `.min(cap)` 而漏 `.max(1)`，则 0 会一路传到
    /// 分批循环，首波入流 0 个、永不补波 → gather 卡死或返回空而**谎报全成功**。
    #[test]
    fn test_max_concurrency_zero_clamps_to_one() {
        let config = GqepConfig {
            max_concurrency: 0,
            ..GqepConfig::default()
        };
        assert_eq!(config.effective_bound(), 1, "0 必须钳到 1，不得等价无限制");
    }

    /// I1 下界（serde 路径）：绕过 `Default` 直接反序列化出的 0 同样被钳。
    ///
    /// WHY 单独一条：`GqepConfig` 无构造钩子，serde 可以把 0 直送字段，
    /// 钳制只能发生在读取点——本测试就是那条注释的断言形式。
    #[test]
    fn test_max_concurrency_zero_from_serde_clamps_to_one() {
        let json = r#"{"default_timeout_ms":5000,"max_concurrency":0,"gather_deadline_ms":5000,"batch_atomic_enabled":true}"#;
        let restored: GqepConfig = serde_json::from_str(json).expect("反序列化成功");
        assert_eq!(
            restored.max_concurrency, 0,
            "字段本身保留用户给定值（不改写配置）"
        );
        assert_eq!(restored.effective_bound(), 1, "但读取点必须钳到 1");
    }

    /// I2 上界：`10^9` 型误配置被截到静态帽，不得形同无界。
    #[test]
    fn test_max_concurrency_capped_to_fanout_ceiling() {
        let config = GqepConfig {
            max_concurrency: usize::MAX,
            ..GqepConfig::default()
        };
        assert_eq!(config.effective_bound(), MAX_GATHER_FANOUT);
        let big = GqepConfig {
            max_concurrency: 1_000_000_000,
            ..GqepConfig::default()
        };
        assert_eq!(big.effective_bound(), MAX_GATHER_FANOUT, "10^9 必须被截断");
    }

    /// I8 前置：默认配置（100）不被本帽静默改写。
    ///
    /// WHY 断的是行为而不是常量关系：「帽 >= 默认」这条已由 `config.rs` 的
    /// `const _: () = assert!(...)` 在编译期钉住（clippy 禁止恒定值的运行期断言）；
    /// 本测试则验证**实际行为**：默认配置下的有效上界就是 100，未被钳低。
    #[test]
    fn test_default_bound_not_silently_reduced_by_ceiling() {
        let default = GqepConfig::default();
        assert_eq!(
            default.effective_bound(),
            default.max_concurrency,
            "默认配置的有效上界必须等于其配置值，否则静态帽静默改写了默认并发语义（违 I8）"
        );
        assert_eq!(
            GqepConfig::default().effective_bound(),
            100,
            "默认路径逐位不变"
        );
    }
}
