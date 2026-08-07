//! token_estimate — 显式化 token 估算器 + EWMA 动态校准(ADR-070 Token 效率优化)
//!
//! 对应架构层: L10 Interface(mca-gateway)
//!
//! # 为什么显式化
//! 既有实现隐式用 `text.len()/4`(UTF-8 字节宽分类):ASCII 1 字节/4 = 0.25,
//! 中文 3 字节/4 = 0.75 token/字符——字节宽恰好是 BPE 密度的近似代理。
//! 本模块将其显式化为按字符字节宽加权,权重可调、可文档化、可测试:
//!
//! ```text
//! token ≈ Σ 字符字节宽 / 4   (1B:0.25 / 2B:0.5 / 3B:0.75 / 4B:1.0)
//! ```
//!
//! 与既有字节/4 口径完全等价(ASCII/中文/emoji 行为不变),消除隐式巧合。
//!
//! # 校准(EWMA,核心新增价值)
//! 字节宽只是 BPE 密度的近似,各通道存在系统偏差(词表/分词差异)。
//! 估算系数按 (provider, model) 动态校准:厂商真实 usage.input_tokens
//! 与估算值之比经 EWMA(α=0.3)平滑,钳制 [0.5, 1.5] 防失控。
//! 校准是客户端对"字符统计 → BPE"系统误差的最优线性近似,
//! 数据源 = StreamSessionCompleted.input_tokens(厂商真实计量)。
//!
//! # 纯函数与状态分离
//! `estimate_text`/`estimate_messages` 是纯函数(无校准,预算护栏语义
//! 可复现);`TokenEstimator` 持有校准表(校准后的消息估算,invoke 闭环
//! 成本回算用)。两层面职责分离,避免估算器状态污染裁剪决策的确定性。

use dashmap::DashMap;
use nexus_contracts::affinity::AffinityMessage;

/// 校准系数下限 — 真实 token 不低于估算 50%(防估算系统过估导致提前截断)
const CALIBRATION_FLOOR: f32 = 0.5;
/// 校准系数上限 — 真实 token 不高于估算 150%(防估算系统低估导致超发失控)
const CALIBRATION_CEIL: f32 = 1.5;
/// EWMA 平滑因子 — 0.3 对单次异常样本的响应约 3 个样本内收敛(1/0.3)
const EWMA_ALPHA: f32 = 0.3;

/// 估算单段文本的 token 数 — 按 UTF-8 字节宽加权(纯函数,确定性)
///
/// 整数运算:各字节宽字符加权分数合并到 1/4 单位,统一向下取整——
/// 与既有字节/4 口径的"逐条 floor"语义一致(裁剪累加与预算断言口径一致)。
///
/// | 字节宽 | 示例 | 权重 |
/// |--------|------|------|
/// | 1B | ASCII 字母/数字/半角符号 | 0.25 token/字符 |
/// | 2B | 拉丁扩展/希腊文/西里尔文 | 0.5 token/字符 |
/// | 3B | CJK 汉字/韩文/标点 | 0.75 token/字符 |
/// | 4B | emoji/扩展B+ 汉字 | 1.0 token/字符 |
pub fn estimate_text(text: &str) -> u32 {
    let mut quarter_units = 0usize;
    for ch in text.chars() {
        // 权重 = 字节宽/4:合并到 1/4 单位计数,整数除法 floor
        quarter_units += ch.len_utf8();
    }
    (quarter_units / 4) as u32
}

/// 估算单条消息的 token 数 — 内容块文本加权求和(纯函数)
pub fn estimate_message(message: &AffinityMessage) -> u32 {
    message
        .blocks
        .iter()
        .map(|b| match b {
            nexus_contracts::affinity::ContentBlock::Text { text } => estimate_text(text),
            nexus_contracts::affinity::ContentBlock::Thinking { thinking, .. } => {
                estimate_text(thinking)
            }
            nexus_contracts::affinity::ContentBlock::ToolUse { input_json, .. } => {
                estimate_text(input_json)
            }
            nexus_contracts::affinity::ContentBlock::ToolResult { content, .. } => {
                estimate_text(content)
            }
        })
        .sum()
}

/// 估算会话全部消息的 token 数(纯函数,逐条 floor 后求和)
pub fn estimate_messages(messages: &[AffinityMessage]) -> u32 {
    messages.iter().map(estimate_message).sum()
}

/// 动态 token 估算器 — 纯函数估算 + (provider, model) EWMA 校准表
///
/// 线程安全:DashMap 分片锁,calibrate/factor 均为同步原子操作,
/// guard 立即释放(不跨 await,C7 红线)。
#[derive(Debug, Default)]
pub struct TokenEstimator {
    /// (provider, model) → EWMA 校准系数
    factors: DashMap<(Box<str>, Box<str>), f32>,
}

impl TokenEstimator {
    /// 创建空校准表的估算器(全部通道系数 = 1.0,即纯函数口径)
    pub fn new() -> Self {
        Self::default()
    }

    /// 校准 — 以厂商真实 usage 与估算值之比更新 EWMA 系数
    ///
    /// `real`/`estimated` 均为 0 时跳过(无信息量,防除零)。
    /// 新系数 = 旧系数×(1-α) + ratio×α,钳制 [0.5, 1.5]。
    /// WHY f64 中间值: 大 token 数的 ratio 计算用 f64 避免 f32 精度损失
    /// (u64 大数百分比计算红线同源)。
    pub fn calibrate(&self, provider: &str, model: &str, real: u64, estimated: u64) {
        if real == 0 || estimated == 0 {
            return;
        }
        let ratio = (real as f64 / estimated as f64) as f32;
        let key = (provider.into(), model.into());
        let mut entry = self.factors.entry(key).or_insert(1.0);
        let next = *entry * (1.0 - EWMA_ALPHA) + ratio * EWMA_ALPHA;
        *entry = next.clamp(CALIBRATION_FLOOR, CALIBRATION_CEIL);
    }

    /// 查询校准系数(无记录 = 1.0,纯函数口径)
    pub fn factor(&self, provider: &str, model: &str) -> f32 {
        self.factors
            .get(&(provider.into(), model.into()))
            .map(|v| *v)
            .unwrap_or(1.0)
    }

    /// 校准后的消息估算 — 纯估算 × 校准系数
    pub fn estimate_messages_calibrated(
        &self,
        provider: &str,
        model: &str,
        messages: &[AffinityMessage],
    ) -> u32 {
        let base = estimate_messages(messages);
        // f64 中间值:base ≤ 数百万 token,f32 精度(2^24)足够,仍用 f64 保持一致口径
        (base as f64 * f64::from(self.factor(provider, model))) as u32
    }

    /// 当前校准表大小(诊断用)
    pub fn calibrated_channels(&self) -> usize {
        self.factors.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{ContentBlock, MessageRole};

    fn msg(text: &str) -> AffinityMessage {
        AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    // ============================================================
    // 纯函数:CJK-aware 加权(3.1 失败测试先行)
    // ============================================================

    #[test]
    fn ascii_matches_legacy_char4() {
        // ASCII 权重 0.25/字符:与旧字符/4 口径完全一致(兼容既有裁剪测试)
        assert_eq!(estimate_text("abcd"), 1);
        assert_eq!(estimate_text("abc"), 0);
        assert_eq!(estimate_text("hello world"), 2); // 11/4 = 2
    }

    #[test]
    fn cjk_weighted_three_quarters() {
        // 中文权重 0.75/字符:"你好世界" = 4×0.75 = 3
        // 旧字符/4 估算 = 1(低估 3 倍)——本实现修正系统性低估
        assert_eq!(estimate_text("你好世界"), 3);
        assert_eq!(estimate_text("你好"), 1); // 2×0.75 = 1.5 → floor 1
    }

    #[test]
    fn mixed_cjk_ascii() {
        // "修复bug" = 6 字节(中文) + 3 字节(ASCII) = 9 字节 / 4 → 2
        assert_eq!(estimate_text("修复bug"), 2);
        // 全角标点计入 3 字节宽:"你好，世界" = 5 字符 × 3 字节 = 15 / 4 → 3
        assert_eq!(estimate_text("你好，世界"), 3);
    }

    #[test]
    fn estimate_message_sums_blocks() {
        let m = AffinityMessage {
            role: MessageRole::User,
            blocks: vec![
                ContentBlock::Text {
                    text: "abcd".into(),
                },
                ContentBlock::Text {
                    text: "你好".into(),
                },
            ],
        };
        // 1 + 1 = 2(逐块 floor 后求和,与旧口径一致)
        assert_eq!(estimate_message(&m), 2);
    }

    #[test]
    fn estimate_messages_sums_per_message_floor() {
        // 逐条 floor 后求和:3 条 5 字符 ASCII = 3×1 = 3(与旧口径一致)
        let three = vec![msg("12345"), msg("abcde"), msg("ABCDE")];
        assert_eq!(estimate_messages(&three), 3);
        // 中文消息:2 条 4 字符 = 2×3 = 6
        let zh = vec![msg("你好世界"), msg("代码重构")];
        assert_eq!(estimate_messages(&zh), 6);
    }

    // ============================================================
    // 校准表:EWMA 收敛与钳制(3.2)
    // ============================================================

    #[test]
    fn calibrate_converges_to_true_ratio() {
        // 注入真实 ratio = 1.3(钳制范围内):50 样本内收敛到 1.3 附近
        let est = TokenEstimator::new();
        for _ in 0..50 {
            est.calibrate("deep_seek", "m", 130, 100);
        }
        let k = est.factor("deep_seek", "m");
        assert!((k - 1.3).abs() < 0.05, "EWMA 应收敛到 1.3, got {k}");
        // 钳制上限 1.5:真实 ratio 5.0 被钳制,防估算系统失控
        let est2 = TokenEstimator::new();
        for _ in 0..10 {
            est2.calibrate("zhipu", "m", 500, 100);
        }
        let k2 = est2.factor("zhipu", "m");
        assert!((k2 - 1.5).abs() < 1e-6, "系数必须钳制在 1.5, got {k2}");
    }

    #[test]
    fn calibrate_skips_zero_inputs() {
        // real/estimated 为 0 时不产生记录(防除零)
        let est = TokenEstimator::new();
        est.calibrate("deep_seek", "m", 0, 100);
        est.calibrate("deep_seek", "m", 100, 0);
        assert_eq!(est.factor("deep_seek", "m"), 1.0);
        assert_eq!(est.calibrated_channels(), 0);
    }

    #[test]
    fn calibrated_estimate_applies_factor() {
        // 校准系数 1.5 × 纯估算 4 = 6
        let est = TokenEstimator::new();
        for _ in 0..50 {
            est.calibrate("deep_seek", "m", 300, 200);
        }
        let messages = vec![msg("你好世界")]; // 纯估算 12 字节/4 = 3
        assert_eq!(
            est.estimate_messages_calibrated("deep_seek", "m", &messages),
            4
        );
        // 无校准通道 = 纯函数口径
        assert_eq!(
            est.estimate_messages_calibrated("unknown", "m", &messages),
            3
        );
    }

    #[test]
    fn factors_are_per_channel() {
        // (provider, model) 独立校准:不同通道互不干扰
        let est = TokenEstimator::new();
        for _ in 0..50 {
            est.calibrate("deep_seek", "flash", 200, 100);
        }
        assert!((est.factor("deep_seek", "flash") - 1.5).abs() < 1e-6);
        assert_eq!(est.factor("deep_seek", "pro"), 1.0, "同厂商不同模型独立");
        assert_eq!(est.factor("zhipu", "flash"), 1.0, "不同厂商独立");
    }

    #[test]
    fn calibrated_estimate_within_20pct_on_mixed_corpus() {
        // Phase 1 验收基准:中英混合语料端到端,校准后估算与真实 token
        // 偏差必须 ≤ ±20%(SMART 测量层要求;模拟真实 BPE 与字节/4 存在
        // 1.3 倍系统偏差的场景——校准应把偏差收敛到阈值内)
        let est = TokenEstimator::new();
        let corpus = [
            msg("实现一个基于 Tokio 的异步 HTTP 服务器,支持路由与中间件"),
            msg("fn main() { let rt = tokio::runtime::Runtime::new().unwrap(); }"),
            msg("数据模型需要设计合理的索引与缓存策略,以应对高并发读场景"),
            msg("pub async fn handle_request(req: Request) -> Response { .. }"),
        ];
        let estimated = u64::from(estimate_messages(&corpus));
        assert!(estimated > 0, "混合语料估算必须非零");
        // 模拟真实 token:字节/4 的 1.3 倍(中文 BPE 压缩率高于字节假设)
        let real = (f64::from(estimated as u32) * 1.3).round() as u64;
        // 多轮校准(50 样本)收敛到 1.3(钳制范围内)
        for _ in 0..50 {
            est.calibrate("deep_seek", "flash", real, estimated);
        }
        let calibrated = u64::from(est.estimate_messages_calibrated("deep_seek", "flash", &corpus));
        let deviation = (calibrated as f64 - real as f64) / real as f64;
        assert!(
            deviation.abs() <= 0.20,
            "校准后偏差必须 ≤ ±20%, got {deviation:.3} (calibrated={calibrated}, real={real})"
        );
    }

    // ============================================================
    // proptest:确定性(估算纯函数无随机/时钟依赖)
    // ============================================================

    use proptest::prelude::*;

    proptest::proptest! {
        #[test]
        fn estimate_text_is_deterministic(s in "[\u{4E00}-\u{9FFF}a-zA-Z0-9 ，。！？]{0,64}") {
            let a = estimate_text(&s);
            let b = estimate_text(&s);
            prop_assert_eq!(a, b, "相同文本必须产出相同估算");
        }

        #[test]
        fn estimate_text_never_overflows(s in "[\u{4E00}-\u{9FFF}a-zA-Z0-9]{0,256}") {
            // 加权估算上界:全 CJK 时 0.75/字符 ≤ 字符数——u32 域内恒安全
            let est = estimate_text(&s);
            prop_assert!(est <= s.chars().count() as u32, "估算不得超过字符数");
        }
    }
}
