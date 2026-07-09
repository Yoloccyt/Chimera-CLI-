//! 懒加载配置(LazyConfig)集成测试 — Task 4 (E1)
//!
//! 验收点:
//! - 14 section 按需懒加载(未访问不解析,以错误探针验证隔离性)
//! - 重复访问返回缓存(同一引用)
//! - 既有 eager API(load / default_config / ChimeraConfig)向后兼容不变

#![forbid(unsafe_code)]

use std::io::Write;

use tempfile::TempDir;

use chimera_cli::config::{self, LazyConfig};
use chimera_cli::ChimeraConfig;

/// 辅助:在临时目录写入 yaml 并构建 LazyConfig。
fn lazy_from_yaml(yaml: &str) -> (TempDir, LazyConfig) {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let path = tmp.path().join("omega.yaml");
    let mut f = std::fs::File::create(&path).expect("创建文件失败");
    f.write_all(yaml.as_bytes()).expect("写入失败");
    let lazy = LazyConfig::new(Some(path)).expect("LazyConfig::new 失败");
    (tmp, lazy)
}

/// 既有 eager API(load / default_config / init_config_file / ChimeraConfig)
/// 签名与行为不变 —— 向后兼容验证。
#[test]
fn test_backward_compatible_api() {
    // default_config 仍返回完整 ChimeraConfig
    let cfg = config::default_config();
    assert!(!cfg.nexus.version.is_empty());

    // default_config_path 仍返回 omega.yaml 路径
    assert!(config::default_config_path()
        .to_string_lossy()
        .contains("omega.yaml"));

    // load 仍返回完整 ChimeraConfig(全量解析)
    let tmp = TempDir::new().expect("创建临时目录失败");
    let p = tmp.path().join("omega.yaml");
    config::init_config_file(&p).expect("生成配置失败");
    let loaded = config::load(Some(p)).expect("加载失败");
    assert_eq!(loaded.nexus.version, "1.0.0-omega");
}

/// 未访问的 section 不被解析(错误探针验证隔离性)。
///
/// 策略:配置中 `quest.max_tasks_per_quest` 写成非法字符串。
/// - 若 `LazyConfig::new` 全量解析,`new` 即应失败;
/// - 若为真正 section 级懒加载,访问 `nexus` 成功、访问 `quest` 失败,
///   证明 `quest` 仅在访问时才解析。
#[test]
fn test_lazy_load_unaccessed_section_not_parsed() {
    let yaml = r#"
nexus:
  version: "1.0.0-omega"
quest:
  max_tasks_per_quest: "NOT_A_NUMBER"
"#;
    let (_tmp, lazy) = lazy_from_yaml(yaml);

    // nexus 合法 → 访问成功,证明未因 quest 错误而整体失败
    let nexus = lazy.nexus().expect("nexus 应可解析");
    assert_eq!(nexus.version, "1.0.0-omega");

    // quest 字段类型错误 → 访问失败,证明懒加载触发了 quest 解析
    let err = lazy.quest().expect_err("quest 应解析失败");
    assert!(
        err.to_string().contains("quest"),
        "错误应指向 quest 解析失败: {err}"
    );
}

/// 重复访问返回缓存(同一引用,缓存命中)。
#[test]
fn test_repeated_access_returns_cached() {
    let yaml = r#"
nexus:
  version: "9.9.9-test"
quest:
  auto_decompose: false
  max_tasks_per_quest: 7
"#;
    let (_tmp, lazy) = lazy_from_yaml(yaml);

    let q1 = lazy.quest().expect("首次访问 quest");
    let q2 = lazy.quest().expect("二次访问 quest");
    // 同一引用 —— 证明缓存命中,未重复解析
    assert!(std::ptr::eq(q1, q2), "重复访问应返回同一引用(缓存命中)");
    assert_eq!(q1.max_tasks_per_quest, 7);
    assert!(!q1.auto_decompose);

    let n1 = lazy.nexus().expect("访问 nexus");
    let n2 = lazy.nexus().expect("再次访问 nexus");
    assert!(std::ptr::eq(n1, n2), "nexus 重复访问应返回同一引用");
    assert_eq!(n1.version, "9.9.9-test");
}

/// 14 个顶层 section 全部可懒加载访问(逐个触发解析)。
#[test]
fn test_all_14_sections_lazy_accessible() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let path = tmp.path().join("omega.yaml");
    config::init_config_file(&path).expect("生成配置失败");
    let lazy = LazyConfig::new(Some(path)).expect("LazyConfig::new 失败");

    assert!(!lazy.nexus().expect("nexus").version.is_empty());
    assert!(lazy.quest().expect("quest").auto_decompose);
    assert!(!lazy
        .thinking_toggle()
        .expect("thinking_toggle")
        .default_mode
        .is_empty());
    assert!(lazy.repo_wiki().expect("repo_wiki").auto_generate);
    assert!(!lazy
        .model_router()
        .expect("model_router")
        .providers
        .is_empty());
    assert!(!lazy.osa().expect("osa").dimensions.is_empty());
    assert!(lazy.kvbsr().expect("kvbsr").max_blocks > 0);
    assert!(lazy.pvl().expect("pvl").max_retry > 0);
    assert!(lazy.mtpe().expect("mtpe").default_prediction_depth > 0);
    assert!(lazy.gqep().expect("gqep").batch_size > 0);
    assert!(!lazy.seccore().expect("seccore").sandbox.is_empty());
    assert!(!lazy.mcp().expect("mcp").servers.is_empty());
    assert!(lazy.evolution().expect("evolution").enabled);
    assert!(lazy.monitoring().expect("monitoring").prometheus.enabled);
}

/// `to_chimera_config` 聚合全部 14 section 为完整 [`ChimeraConfig`]。
#[test]
fn test_lazy_to_chimera_config() {
    let yaml = r#"
nexus:
  version: "7.7.7-merge"
"#;
    let (_tmp, lazy) = lazy_from_yaml(yaml);
    let full: ChimeraConfig = lazy.to_chimera_config().expect("聚合失败");
    assert_eq!(full.nexus.version, "7.7.7-merge");
    // 未在 yaml 写的 section 回退默认值(extract_inner 对缺失 key 用 serde default)
    assert!(full.quest.auto_decompose);
    assert!(!full.model_router.providers.is_empty());
}
