//! 语义指纹 — mca-gateway 侧 CLV 轻量代理(ADR-069 Task 5 R3 语义缓存)
//!
//! 对应架构层: L10 Interface(mca-gateway)
//!
//! # 定位
//! 真实 CLV(512 维上下文潜在向量)由 L2 nmc-encoder 产生,但 mca-gateway
//! 依赖铁律禁止向上依赖 L2(§2.2 L(N) → L(N+1) 禁止)。本模块是
//! mca-gateway 侧的**轻量代理**:确定性纯函数、零 unsafe、O(文本长度),
//! 输出与 scc-cache `SemanticResponseCache` 的 CLV 维度对齐(512 维),
//! 供语义缓存热路径的语义匹配层(余弦相似度)使用。
//!
//! # 算法
//! 字符级特征哈希:每条消息的每个内容块文本 + 工具声明的
//! (name/description/parameters_schema),按 (字符字节, 位置, 角色盐) 混合
//! 哈希选桶累加,最后 L2 归一化(单位向量)——余弦相似度 = 向量内积。
//!
//! # 性质(测试固化)
//! - 相同请求 → 完全相同指纹(确定性,路由可复现)
//! - 微小文本差异 → 高余弦相似度(> 0.9)
//! - 完全不同文本 → 低余弦相似度(< 0.5)

use nexus_contracts::affinity::{AffinityMessage, ContentBlock, MessageRole, ToolDecl};

/// 指纹维度 — 与 scc-cache SemanticResponseCache 的 CLV 维度对齐(512)
pub const FINGERPRINT_DIM: usize = 512;

/// FNV-1a 偏移基(非零素数,避免全零输入坍缩)
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 质数乘子
const FNV_PRIME: u64 = 0x100_0000_01b3;
/// 位置混合常数(黄金分割比率,分散相邻位置)
const POSITION_SALT: u64 = 0x9e37_79b9_7f4a_7c15;
/// 消息序号混合常数 — 会话历史顺序是语义的一部分("A,B" ≠ "B,A")
const MESSAGE_POS_SALT: u64 = 0x517c_c1b7_2722_0a95;

/// 计算请求的确定性语义指纹(512 维单位向量)
///
/// 对消息内容块文本与工具声明的字符做特征哈希累加,归一化后返回。
/// 纯函数:无随机/时间依赖,相同输入恒产出相同向量(路由可复现)。
pub fn semantic_fingerprint(messages: &[AffinityMessage], tools: &[ToolDecl]) -> Vec<f32> {
    let mut buckets = vec![0.0_f64; FINGERPRINT_DIM];
    // WHY 消息序号参与盐:hash_into 的字符位置是消息内坐标,若不混合消息序号,
    // 交换两条消息的顺序会得到完全相同指纹(顺序是会话语义的一部分,须区分)
    for (msg_idx, msg) in messages.iter().enumerate() {
        let salt = role_salt(msg.role) ^ (msg_idx as u64).wrapping_mul(MESSAGE_POS_SALT);
        for block in &msg.blocks {
            hash_into(&mut buckets, block_text(block), salt);
        }
    }
    for tool in tools {
        // 工具三字段分别盐化:name/description/schema 变更均反映到指纹
        hash_into(&mut buckets, tool.name.as_ref(), 1);
        hash_into(&mut buckets, tool.description.as_ref(), 2);
        hash_into(&mut buckets, tool.parameters_schema.as_ref(), 3);
    }
    // L2 归一化:单位向量(余弦 = 内积);空输入 → 全零向量(相似度 0,永不命中)
    let norm: f64 = buckets.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm == 0.0 {
        return vec![0.0; FINGERPRINT_DIM];
    }
    buckets.into_iter().map(|v| (v / norm) as f32).collect()
}

/// 角色 → 盐(区分同文本不同发言方;unit enum 无 repr,显式 match 映射)
fn role_salt(role: MessageRole) -> u64 {
    match role {
        MessageRole::System => 0,
        MessageRole::User => 1,
        MessageRole::Assistant => 2,
        MessageRole::Tool => 3,
    }
}

/// 内容块 → 参与指纹的文本(Thinking/ToolUse/ToolResult 均承载语义)
fn block_text(block: &ContentBlock) -> &str {
    match block {
        ContentBlock::Text { text } => text,
        ContentBlock::Thinking { thinking, .. } => thinking,
        ContentBlock::ToolUse { input_json, .. } => input_json,
        ContentBlock::ToolResult { content, .. } => content,
    }
}

/// 字符级特征哈希 — 每字符独立选桶累加(词袋 + 位置 + 盐)
///
/// 逐字符独立哈希而非整段单哈希的原因:整段单哈希下任一字符变化都会
/// 完全改变桶索引(相似请求 → 低相似度);逐字符累加保留大部分共享桶,
/// 微小差异只移动少数桶计数,相似度接近 1.0。
fn hash_into(buckets: &mut [f64], text: &str, salt: u64) {
    for (pos, b) in text.bytes().enumerate() {
        let mut h: u64 = FNV_OFFSET ^ u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
        // 位置 × 盐混合:同位置同盐 → 同桶(确定性);不同位置 → 不同桶区域
        h ^= (pos as u64).wrapping_mul(salt.wrapping_add(POSITION_SALT));
        let idx = (h % buckets.len() as u64) as usize;
        buckets[idx] += 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// 余弦相似度 — mca-gateway 不依赖 nexus-core(依赖铁律),测试内联实现
    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let mut dot = 0.0_f32;
        let mut na = 0.0_f32;
        let mut nb = 0.0_f32;
        for (x, y) in a.iter().zip(b) {
            dot += x * y;
            na += x * x;
            nb += y * y;
        }
        dot / (na.sqrt() * nb.sqrt())
    }

    fn text_message(text: &str) -> AffinityMessage {
        AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    fn tool(name: &str) -> ToolDecl {
        ToolDecl {
            name: name.into(),
            description: "test tool".into(),
            parameters_schema: "{}".into(),
        }
    }

    #[test]
    fn identical_input_produces_identical_fingerprint() {
        // 确定性核心性质:相同请求必须产出完全相同指纹(路由可复现)
        let a = semantic_fingerprint(&[text_message("hello world")], &[]);
        let b = semantic_fingerprint(&[text_message("hello world")], &[]);
        assert_eq!(a, b, "相同输入必须产出完全相同指纹");
        assert_eq!(a.len(), FINGERPRINT_DIM, "维度必须与 CLV 对齐(512)");
        // 单位向量(L2 归一化):|v| ≈ 1
        let norm: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "指纹必须归一化为单位向量");
    }

    #[test]
    fn tiny_text_difference_high_similarity() {
        // 追加 1 字符(局部修改)→ 高相似度 > 0.9(语义等价判定基础)
        let a = semantic_fingerprint(&[text_message("hello world")], &[]);
        let b = semantic_fingerprint(&[text_message("hello world!")], &[]);
        assert!(
            cosine(&a, &b) > 0.9,
            "微小文本差异相似度必须 > 0.9, got {}",
            cosine(&a, &b)
        );
    }

    #[test]
    fn completely_different_text_low_similarity() {
        // 完全不同文本(中英混排长文本)→ 低相似度 < 0.5(避免误命中)
        let a = semantic_fingerprint(
            &[text_message(
                &"alpha beta gamma delta epsilon zeta".repeat(8),
            )],
            &[],
        );
        let b = semantic_fingerprint(
            &[text_message(
                &"数据模型训练需要大量标注样本与算力资源".repeat(8),
            )],
            &[],
        );
        assert!(
            cosine(&a, &b) < 0.5,
            "完全不同文本相似度必须 < 0.5, got {}",
            cosine(&a, &b)
        );
    }

    #[test]
    fn tools_participate_in_fingerprint() {
        // 工具声明参与指纹:同一请求挂不同工具集 → 指纹不同
        let with_tool = semantic_fingerprint(&[text_message("hello")], &[tool("read_file")]);
        let without_tool = semantic_fingerprint(&[text_message("hello")], &[]);
        assert!(
            cosine(&with_tool, &without_tool) < 0.999,
            "工具声明必须参与指纹(键空间区分)"
        );
        // 同一工具集(字段值相同)→ 指纹确定
        let again = semantic_fingerprint(&[text_message("hello")], &[tool("read_file")]);
        assert_eq!(with_tool, again);
    }

    #[test]
    fn message_order_participates_in_fingerprint() {
        // 位置盐:消息顺序不同 → 指纹不同(历史顺序是语义的一部分)
        let m1 = semantic_fingerprint(&[text_message("first"), text_message("second")], &[]);
        let m2 = semantic_fingerprint(&[text_message("second"), text_message("first")], &[]);
        assert!(
            cosine(&m1, &m2) < 1.0,
            "消息顺序必须参与指纹(位置盐), got {}",
            cosine(&m1, &m2)
        );
    }

    // ============================================================
    // proptest(ADR-069 Task 5.4)
    // ============================================================

    proptest::proptest! {
        /// 确定性:随机文本构造两次必须产出完全相同的指纹(路由可复现)
        #[test]
        fn fingerprint_deterministic(s in "[a-zA-Z0-9_ ]{0,64}") {
            let a = semantic_fingerprint(&[text_message(&s)], &[]);
            let b = semantic_fingerprint(&[text_message(&s)], &[]);
            prop_assert_eq!(a, b, "随机输入 {:?} 必须产出相同指纹", s);
        }

        /// 微小差异(追加 1 字符)→ 相似度恒 > 0.9:
        /// 共享桶比例 = n/(n+1) 的下界,长度 ≥ 5 时恒 > 0.83→ 需验证 ≥ 0.9
        /// 实际由位置/字符独立选桶保证共享桶 = 全部 n 个(位置不变)。
        #[test]
        fn fingerprint_append_char_high_similarity(s in "[a-zA-Z0-9_ ]{5,32}") {
            let a = semantic_fingerprint(&[text_message(&s)], &[]);
            let mut extended = s.clone();
            extended.push('x');
            let b = semantic_fingerprint(&[text_message(&extended)], &[]);
            let sim = cosine(&a, &b);
            prop_assert!(sim > 0.9, "追加字符相似度必须 > 0.9, got {sim}, s = {s:?}");
        }
    }
}
