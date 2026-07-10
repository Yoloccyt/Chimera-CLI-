//! 集成测试 — `LazyConfig` 懒加载机制(Task 4 / E1)
//!
//! 覆盖三类不变量:
//! 1. **懒加载**:未访问的 section 不被解析(malformed section 仅在访问时报错)
//! 2. **缓存**:重复访问同一 section 返回同一引用(OnceLock 缓存命中)
//! 3. **向后兼容 / 行为等价**:`LazyConfig` 各 section 值与 `config::load` 急切加载一致
//!
//! 设计依据(已经验证):figment 的 `extract_inner::<T>(key)` 与 `extract::<ChimeraConfig>()`
//! 对同一 provider 链产出等价值;`Yaml::file(缺失)` 返回 `Ok(empty)`,回退默认值,
//! 故 `LazyConfig::new` 与 `config::load` 在缺失文件场景行为一致。

use std::path::PathBuf;

use chimera_cli::config;
use chimera_cli::LazyConfig;
use serde::Serialize;
use tempfile::TempDir;

/// 测试夹具:生成默认 omega.yaml,同时构造懒加载与急切加载两份配置,便于逐 section 比对。
///
/// `_tmp` 保活以确保配置文件在测试期间存在;`TempDir` drop 时自动清理。
struct Setup {
    _tmp: TempDir,
    lazy: LazyConfig,
    eager: chimera_cli::ChimeraConfig,
}

fn setup() -> Setup {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let path: PathBuf = tmp.path().join("omega.yaml");
    config::init_config_file(&path).expect("生成配置文件失败");

    let lazy = LazyConfig::new(Some(path.clone())).expect("LazyConfig 构建失败");
    let eager = config::load(Some(path)).expect("急切加载失败");
    Setup {
        _tmp: tmp,
        lazy,
        eager,
    }
}

/// 比对单个 section 的懒加载值与急切加载值(经 JSON 序列化后字符串相等)。
///
/// WHY 用 JSON 字符串比对而非 PartialEq:14 个 section 类型未派生 PartialEq
/// (定义在 nexus-core,RC 阶段不修改核心类型),JSON 往返比对可零侵入验证等价性,
/// 与现有 `test_default_config_roundtrip` 同策略。
fn assert_section_eq<T: Serialize>(lazy: &T, eager: &T, name: &str) {
    let l = serde_json::to_string(lazy).expect("序列化 lazy section 失败");
    let e = serde_json::to_string(eager).expect("序列化 eager section 失败");
    assert_eq!(l, e, "section {name}: lazy 与 eager 不一致");
}

// ===== 核心测试(3) =====

/// 核心不变量 1 — 懒加载:未访问的 section 不被解析。
///
/// 构造一个 `quest.max_tasks_per_quest` 类型错误的配置文件(语法合法但语义错误)。
/// 若 `LazyConfig::new` 急切解析全部 section,则构造会失败;
/// 懒加载下构造成功,且 `nexus`(合法)可访问,仅 `quest`(非法)访问时报错。
/// 这直接证明 section 解析延迟到首次访问。
#[test]
fn test_lazy_config_section_not_extracted_until_accessed() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let path = tmp.path().join("malformed.yaml");
    // quest 子字段类型错误:u32 期望收到字符串。YAML 语法合法,故文件解析阶段不报错。
    std::fs::write(&path, "quest:\n  max_tasks_per_quest: \"NOT_A_NUMBER\"\n")
        .expect("写入测试配置失败");

    // build 不解析任何 section,故即便 quest 非法也能成功
    let lazy = LazyConfig::new(Some(path)).expect("build 不应解析 section");

    // nexus 合法,可正常懒加载
    let nexus = lazy.nexus().expect("nexus 应可懒加载");
    assert!(!nexus.version.is_empty(), "nexus.version 非空");

    // quest 非法,仅访问时报错 —— 这是懒加载的直接证据
    let quest_err = lazy.quest();
    assert!(
        quest_err.is_err(),
        "malformed quest 应在访问时报错,实际:{quest_err:?}"
    );
}

/// 核心不变量 2 — 缓存:重复访问同一 section 返回同一引用(OnceLock 命中)。
#[test]
fn test_lazy_config_repeated_access_returns_cached() {
    let s = setup();

    // 两次独立访问应返回指向同一内存的引用(OnceLock 只初始化一次)
    let first = s.lazy.nexus().expect("首次访问 nexus");
    let second = s.lazy.nexus().expect("二次访问 nexus");
    assert!(
        std::ptr::eq(first, second),
        "重复访问应返回缓存引用(同一地址)"
    );
}

/// 核心不变量 3 — 向后兼容:既有急切加载 API 链路不受影响,且 LazyConfig 与之等价。
#[test]
fn test_backward_compatible_api_unchanged() {
    // 既有 API 仍可用(类型与函数签名未变)
    let cfg = chimera_cli::ChimeraConfig::default();
    assert!(
        !cfg.nexus.version.is_empty(),
        "ChimeraConfig::default 仍可用"
    );

    let default_cfg = config::default_config();
    assert!(
        !default_cfg.nexus.version.is_empty(),
        "default_config 仍可用"
    );

    let path = config::default_config_path();
    assert!(
        path.to_string_lossy().contains("omega.yaml"),
        "default_config_path 仍可用"
    );

    // init_config_file + load 急切链路不变
    let tmp = TempDir::new().expect("创建临时目录失败");
    let p = tmp.path().join("omega.yaml");
    config::init_config_file(&p).expect("init_config_file 仍可用");
    let loaded = config::load(Some(p.clone())).expect("load 仍可用");
    assert_eq!(loaded.nexus.version, "1.0.0-omega");

    // 新增 LazyConfig 与既有 API 共存,且对同一文件产出等价值
    let lazy = LazyConfig::new(Some(p)).expect("LazyConfig 可与既有 API 共存");
    assert_eq!(lazy.nexus().unwrap().version, "1.0.0-omega");
}

// ===== 14 个 section 逐一等价性测试 =====

#[test]
fn test_lazy_section_nexus() {
    let s = setup();
    assert_section_eq(s.lazy.nexus().unwrap(), &s.eager.nexus, "nexus");
}

#[test]
fn test_lazy_section_quest() {
    let s = setup();
    assert_section_eq(s.lazy.quest().unwrap(), &s.eager.quest, "quest");
}

#[test]
fn test_lazy_section_thinking_toggle() {
    let s = setup();
    assert_section_eq(
        s.lazy.thinking_toggle().unwrap(),
        &s.eager.thinking_toggle,
        "thinking_toggle",
    );
}

#[test]
fn test_lazy_section_repo_wiki() {
    let s = setup();
    assert_section_eq(s.lazy.repo_wiki().unwrap(), &s.eager.repo_wiki, "repo_wiki");
}

#[test]
fn test_lazy_section_model_router() {
    let s = setup();
    assert_section_eq(
        s.lazy.model_router().unwrap(),
        &s.eager.model_router,
        "model_router",
    );
}

#[test]
fn test_lazy_section_osa() {
    let s = setup();
    assert_section_eq(s.lazy.osa().unwrap(), &s.eager.osa, "osa");
}

#[test]
fn test_lazy_section_kvbsr() {
    let s = setup();
    assert_section_eq(s.lazy.kvbsr().unwrap(), &s.eager.kvbsr, "kvbsr");
}

#[test]
fn test_lazy_section_pvl() {
    let s = setup();
    assert_section_eq(s.lazy.pvl().unwrap(), &s.eager.pvl, "pvl");
}

#[test]
fn test_lazy_section_mtpe() {
    let s = setup();
    assert_section_eq(s.lazy.mtpe().unwrap(), &s.eager.mtpe, "mtpe");
}

#[test]
fn test_lazy_section_gqep() {
    let s = setup();
    assert_section_eq(s.lazy.gqep().unwrap(), &s.eager.gqep, "gqep");
}

#[test]
fn test_lazy_section_seccore() {
    let s = setup();
    assert_section_eq(s.lazy.seccore().unwrap(), &s.eager.seccore, "seccore");
}

#[test]
fn test_lazy_section_mcp() {
    let s = setup();
    assert_section_eq(s.lazy.mcp().unwrap(), &s.eager.mcp, "mcp");
}

#[test]
fn test_lazy_section_evolution() {
    let s = setup();
    assert_section_eq(s.lazy.evolution().unwrap(), &s.eager.evolution, "evolution");
}

#[test]
fn test_lazy_section_monitoring() {
    let s = setup();
    assert_section_eq(
        s.lazy.monitoring().unwrap(),
        &s.eager.monitoring,
        "monitoring",
    );
}
