//! Skills 渐进加载器集成测试 — 顶层 API + CLV 门控 + 缓存闭环（v3.4.0 §11.1）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ 相似度门控端到端 /
//! Index First + Body on Demand 分界 / body_provider 注入 / proptest 门控不变量 /
//! 技能全链组合（SkillGraph 进化 → 索引映射 → 渐进加载）

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use nexus_core::CLV;
use proptest::prelude::*;
use repo_wiki::{LoadedSkill, ProgressiveSkillLoader, SkillBody, SkillMetadata};

/// 构造指定维度方向为 1.0 的单位 CLV（其余 0）
fn unit_clv(dim: usize) -> CLV {
    // 算法体已收敛到 L1 `nexus_core::CLV::basis`(单一权威构造器),
    // 此处仅保留本地签名,避免改动本文件内数十处调用点。
    // basis 越界返回 None;夹具若下标非法则该测试无效,直接 expect 暴露。
    CLV::basis(dim).expect("测试夹具:下标须在 CLV::DIMENSION 内")
}

fn meta(id: &str, dim: usize) -> SkillMetadata {
    SkillMetadata {
        skill_id: id.to_string(),
        name: format!("name-{id}"),
        description: format!("desc-{id}"),
        embedding: unit_clv(dim),
        tags: vec![id.to_string()],
        body_size: 100,
        last_used: None,
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let loader = ProgressiveSkillLoader::new(0.7);
    assert_eq!(loader.index_count(), 0);
    assert_eq!(loader.similarity_threshold(), 0.7);
}

// ----------------------------------------------------------
// 端到端: Index First → Body on Demand
// ----------------------------------------------------------

#[tokio::test]
async fn end_to_end_index_first_body_on_demand() {
    let mut loader = ProgressiveSkillLoader::new(0.6);
    // 5 个技能: 2 个与任务相关（dim 0），3 个正交（dim 10+）
    loader.register_index(vec![
        meta("rel-1", 0),
        meta("rel-2", 0),
        meta("orth-1", 10),
        meta("orth-2", 11),
        meta("orth-3", 12),
    ]);
    assert_eq!(loader.index_count(), 5);

    // 索引全量 10，full-load 上限 1
    let loaded: Vec<LoadedSkill> = loader.load_skills(&unit_clv(0), 10, 1).await;
    // 正交技能被门控过滤（cos=0 < 0.6）
    assert_eq!(loaded.len(), 2);
    // 第一个 full-load，第二个仅索引占位（铁律5 懒加载）
    assert!(!loaded[0].body.code.starts_with("// Not loaded"));
    assert!(loaded[1].body.code.starts_with("// Not loaded"));
    // 占位 body 保留描述（索引信息可用）
    assert!(loaded[1].body.documentation.starts_with("desc-"));

    // 内存节省: 5 索引仅 1 body → saved = 1 - 1/5 = 0.8
    let stats = loader.get_stats().await;
    assert_eq!(stats.total_indexed, 5);
    assert_eq!(stats.bodies_loaded, 1);
    assert!((stats.memory_saved_ratio - 0.8).abs() < 1e-6);
}

#[tokio::test]
async fn body_provider_injection_end_to_end() {
    // 注入真实内容来源（模拟 WikiStore 接线）
    let mut loader = ProgressiveSkillLoader::new(0.5).with_body_provider(|id| SkillBody {
        skill_id: id.to_string(),
        code: format!("fn skill_{id}() {{}}"),
        examples: vec![format!("example-{id}")],
        tests: vec![format!("test-{id}")],
        documentation: format!("docs-{id}"),
    });
    loader.register_index(vec![meta("parse", 0)]);
    let loaded = loader.load_skills(&unit_clv(0), 1, 1).await;
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].body.code, "fn skill_parse() {}");
    assert_eq!(loaded[0].body.examples, vec!["example-parse".to_string()]);
}

#[tokio::test]
async fn max_index_count_limits_candidates() {
    let mut loader = ProgressiveSkillLoader::new(0.5);
    // 全部相关（同维度 cos=1.0）
    loader.register_index((0..10).map(|i| meta(&format!("s{i}"), 0)).collect());
    // 索引上限 3 → 仅 3 个候选
    let loaded = loader.load_skills(&unit_clv(0), 3, 1).await;
    assert_eq!(loaded.len(), 3);
    let stats = loader.get_stats().await;
    assert_eq!(stats.bodies_loaded, 1);
}

// ----------------------------------------------------------
// 技能全链组合（L5 内部协同 W3）: SkillGraph 进化 → 索引映射 → 渐进加载
// ----------------------------------------------------------

/// 锁定 L5 技能三链路的组合可用性: 图进化产出节点 → `skill_metadata_from_graph`
/// 映射为索引 → loader 按 CLV 门控渐进加载。生产上游（轨迹 → SkillUsagePattern
/// 生产者,受 R2 冻结治理）接线前,本测试保证链路本身随时可用、无需适配层改动;
/// 同时验证无 body_provider 时占位**显式声明未接线**(不伪装已加载)。
#[tokio::test]
async fn skill_full_chain_graph_to_loader_composition() {
    use repo_wiki::skill_graph::{SkillGraph, SkillUsagePattern};
    use repo_wiki::skill_metadata_from_graph;

    // 1. 图进化: 两个高频技能入图(频次 ≥ 3),序列相邻建依赖(parse → compile)
    let mut graph = SkillGraph::new();
    graph.evolve_with_patterns(&[
        SkillUsagePattern {
            skill_id: "parse".into(),
            embedding: unit_clv(0),
            frequency: 5,
            success_rate: 0.9,
            sequence: Vec::new(),
        },
        SkillUsagePattern {
            skill_id: "compile".into(),
            embedding: unit_clv(1),
            frequency: 5,
            success_rate: 0.8,
            sequence: vec!["parse".into(), "compile".into()],
        },
    ]);
    assert_eq!(graph.len(), 2, "频次 ≥ 3 的技能应入图");

    // 2. 索引映射: 依赖沉淀为 tags,统计沉淀为 description
    let index = skill_metadata_from_graph(&graph);
    assert_eq!(index.len(), 2);
    let compile = index
        .iter()
        .find(|m| m.skill_id == "compile")
        .expect("compile 应在索引中");
    assert_eq!(
        compile.tags,
        vec!["parse".to_string()],
        "序列相邻依赖应沉淀为 tags"
    );
    assert!(
        compile.description.contains("reuse_count=5"),
        "复用统计应沉淀进 description,实际: {}",
        compile.description
    );

    // 3. 渐进加载: 相似度门控选出 parse(dim 0 与任务同向),compile 正交被过滤
    let mut loader = ProgressiveSkillLoader::new(0.5);
    loader.register_index(index);
    let loaded = loader.load_skills(&unit_clv(0), 10, 1).await;
    assert_eq!(loaded.len(), 1, "正交技能应被门控过滤");
    assert_eq!(loaded[0].metadata.skill_id, "parse");
    // 无 body_provider: 占位必须显式声明未接线,不得伪装已加载
    assert!(
        loaded[0].body.code.contains("body_provider not wired"),
        "占位 body 应显式声明未接线,实际: {}",
        loaded[0].body.code
    );
}

// ----------------------------------------------------------
// proptest: 门控不变量
// ----------------------------------------------------------

proptest! {
    /// 任意阈值与索引规模: 返回结果数 ≤ max_index_count，
    /// 且 memory_saved_ratio ∈ [0, 1]（不除零）
    #[test]
    fn gating_invariants(
        threshold in 0.0f32..1.0,
        n_skills in 0usize..20,
        max_index in 0usize..15,
        max_load in 0usize..10,
    ) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        // async block 返回 Result 以使 prop_assert! 的 early-return 生效
        let result: Result<(), proptest::test_runner::TestCaseError> = rt.block_on(async {
            let mut loader = ProgressiveSkillLoader::new(threshold);
            loader.register_index((0..n_skills).map(|i| meta(&format!("s{i}"), i % 8)).collect());
            let loaded = loader.load_skills(&unit_clv(0), max_index, max_load).await;
            prop_assert!(loaded.len() <= max_index);
            prop_assert!(loaded.len() <= n_skills);
            let stats = loader.get_stats().await;
            prop_assert!(stats.memory_saved_ratio >= 0.0);
            prop_assert!(stats.memory_saved_ratio <= 1.0);
            Ok(())
        });
        result?;
    }
}
