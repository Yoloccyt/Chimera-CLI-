//! R11 W4e:w4 开/关对照实验(蓝图 P2 验收,ADR-295 D7)
//!
//! 蓝图 §七 P2 行验收原文:「w4 开/关对照(成功率/成本双指标,RSI 三件套:
//! 单调保留)」。本测试以**受控语料**机证该对照:
//! - 语料 = 30 工具响应 + 10 用户指令,两者**元数据完全同质**(同 token 数/
//!   同 access_count 族/同时近性)⇒ w4 来源降权是两跑之间唯一差异源;
//! - 质量指标 = 保留集中的用户指令数(高价值面占比;蓝图 §5.4「tool-response
//!   类条目降权」的预期方向);
//! - 成本指标 = 压缩后装载 token(双跑都须贴预算上限,不越线)。
//!
//! # 方向断言的适用边界(诚实声明)
//! 断言只在「工具响应为 filler、用户指令为高价值」的受控语料下成立——
//! 真实工作负载中工具响应可能关键(w4 误伤场景,蓝图 §九风险 4)。该场景
//! 的弃权通道 = `CHIMERA_W4_DISABLED`(共同哲学 6),随批保留。
//! 机制级 A/B(配置层 source_weight 0.36 vs 0.0)与运行时 env 开关
//! (effective_source_weight)共用同一消费点,配置层对照即开关语义的机证。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::sync::Arc;

use chrono::{Duration, Utc};
use hcw_window::{CompressionReport, ContextCompressor, ContextEntry, ContextSource, HcwConfig};

const TOOL_COUNT: usize = 30;
const USER_COUNT: usize = 10;
const ENTRY_TOKENS: usize = 100;
/// 全量 4000 token 的 50% —— 强制驱逐,使来源权重差异进入保留决策
const BUDGET: usize = 2000;

/// 元数据完全同质的条目(count/age 一致 ⇒ recency/frequency 不构成组间差异)
fn make(id: &str, count: u32, age_ms: i64, source: ContextSource) -> ContextEntry {
    let mut e = ContextEntry::new(id, "file-1", format!("content-{id}"), ENTRY_TOKENS);
    e.access_count = count;
    e.last_accessed_at = Utc::now() - Duration::milliseconds(age_ms);
    e.source = Some(source);
    e
}

fn to_arc(entries: Vec<ContextEntry>) -> Vec<Arc<ContextEntry>> {
    entries.into_iter().map(Arc::new).collect()
}

fn run(config: &HcwConfig) -> CompressionReport {
    let entries: Vec<ContextEntry> = (0..TOOL_COUNT)
        .map(|i| make(&format!("tool-{i}"), 2, 500, ContextSource::Tool))
        .chain((0..USER_COUNT).map(|i| make(&format!("user-{i}"), 1, 500, ContextSource::User)))
        .collect();
    ContextCompressor::compress(config, &to_arc(entries), BUDGET, None, Utc::now())
}

fn user_kept(r: &CompressionReport) -> usize {
    r.retained_entries
        .iter()
        .filter(|e| e.source == Some(ContextSource::User))
        .count()
}

fn tool_kept(r: &CompressionReport) -> usize {
    r.retained_entries
        .iter()
        .filter(|e| e.source == Some(ContextSource::Tool))
        .count()
}

#[test]
fn w4_ab_tool_heavy_workload_dual_metrics() {
    // ON = R9b WP-D 默认档(0.36);OFF = 三维旧公式(逐位等价 w4 关)
    let on_cfg = HcwConfig {
        source_weight: 0.36,
        ..HcwConfig::default()
    };
    let off_cfg = HcwConfig {
        source_weight: 0.0,
        ..HcwConfig::default()
    };

    let on = run(&on_cfg);
    let off = run(&off_cfg);
    let (q_on, q_off) = (user_kept(&on), user_kept(&off));
    let (t_on, t_off) = (tool_kept(&on), tool_kept(&off));

    // 质量方向:w4 ON ⇒ 用户指令保留数严格增加(降权把预算让给高价值面)
    assert!(
        q_on > q_off,
        "w4 ON 必须提升高价值面保留: on={q_on} off={q_off}"
    );
    // 结构性证据:工具响应保留数相应下降(降权生效,不是凭空多出预算)
    assert!(
        t_on < t_off,
        "w4 ON 必须降低工具响应保留数: on={t_on} off={t_off}"
    );
    // 成本方向:双跑都不得越预算(成本不因 w4 恶化)
    assert!(
        on.compressed_size <= BUDGET && off.compressed_size <= BUDGET,
        "双跑都不得越预算: on={} off={} budget={BUDGET}",
        on.compressed_size,
        off.compressed_size
    );

    println!(
        "w4 A/B 双指标(蓝图 P2): 质量=用户保留 on={q_on}/off={q_off}; \
         工具保留 on={t_on}/off={t_off}; 成本=装载 token on={}/off={}(预算 {BUDGET})",
        on.compressed_size, off.compressed_size
    );
}
