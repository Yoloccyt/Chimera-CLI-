//! W3-1 等价性锁：`config::load` 去 `Serialized::defaults` 后，
//! 缺失/部分/仅 env/全键面四种夹具必须与 `ChimeraConfig::default()` 同值。
//!
//! WHY 独立测试文件：`load` 的默认值来源从「全量序列化默认结构」改为
//! 「`nexus-core` 容器级 `#[serde(default)]`」是一次语义等价替换，
//! 需要一组**值级**夹具把「缺失键回填同源」钉死（字段级断言会漏掉
//! 未来新增段）。本文件在改动前后都应全绿——改动前是 golden 基线，
//! 改动后是回归护栏。
//!
//! 注：env 夹具会写进程级环境变量，故全部用例持有同一把锁串行执行；
//! 否则并行用例会把 `CHIMERA_QUEST__AUTO_DECOMPOSE` 读进自己的加载结果。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard};

use chimera_cli::config;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 串行化同二进制内的配置加载夹具（理由见模块头）。
fn lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 序列化为 JSON 值做深比较（`ChimeraConfig` 未实现 `PartialEq`）。
fn as_value(cfg: &config::ChimeraConfig) -> serde_json::Value {
    serde_json::to_value(cfg).expect("ChimeraConfig 应可序列化为 JSON")
}

/// 缺失文件：与内置默认值逐字段同值。
#[test]
fn load_missing_file_matches_default() {
    let _guard = lock();
    let tmp = TempDir::new().expect("创建临时目录失败");
    let loaded = config::load(Some(tmp.path().join("nope.yaml"))).expect("缺失文件应回退默认值");
    assert_eq!(
        as_value(&loaded),
        as_value(&config::default_config()),
        "缺失文件加载结果必须与 ChimeraConfig::default() 同值"
    );
}

/// 部分 YAML（仅顶层段的一个字段）：其余字段由 serde default 回填。
#[test]
fn load_partial_yaml_matches_default_plus_override() {
    let _guard = lock();
    let tmp = TempDir::new().expect("创建临时目录失败");
    let path = tmp.path().join("partial.yaml");
    std::fs::write(&path, "quest:\n  auto_decompose: false\n").expect("写入夹具失败");

    let loaded = config::load(Some(path)).expect("部分配置应可加载");
    let mut expected = config::default_config();
    expected.quest.auto_decompose = false;
    assert_eq!(
        as_value(&loaded),
        as_value(&expected),
        "部分 YAML 只覆盖 quest.auto_decompose，其余字段应与默认值同值"
    );
}

/// 部分嵌套 YAML：父段与子段都只给一个键，兄弟段保持默认。
#[test]
fn load_partial_nested_yaml_matches_default_plus_override() {
    let _guard = lock();
    let tmp = TempDir::new().expect("创建临时目录失败");
    let path = tmp.path().join("nested.yaml");
    std::fs::write(&path, "model_router:\n  strategy: \"Off\"\n").expect("写入夹具失败");

    let loaded = config::load(Some(path)).expect("部分嵌套配置应可加载");
    let mut expected = config::default_config();
    expected.model_router.strategy = "Off".to_string();
    assert_eq!(
        as_value(&loaded),
        as_value(&expected),
        "嵌套部分覆盖不得清空兄弟字段"
    );
}

/// 仅 env（无文件）：env 覆盖与默认值回填同源。
#[test]
fn load_env_only_matches_default_plus_override() {
    let _guard = lock();
    std::env::set_var("CHIMERA_QUEST__AUTO_DECOMPOSE", "false");
    let tmp = TempDir::new().expect("创建临时目录失败");
    let result = config::load(Some(tmp.path().join("absent.yaml")));
    std::env::remove_var("CHIMERA_QUEST__AUTO_DECOMPOSE");

    let loaded = result.expect("仅 env 配置应可加载");
    let mut expected = config::default_config();
    expected.quest.auto_decompose = false;
    assert_eq!(
        as_value(&loaded),
        as_value(&expected),
        "env 覆盖后其余字段应与默认值同值"
    );
}

/// 全键面夹具：把 `default()` 的完整序列化写成 YAML 再加载，必须原值返回。
///
/// WHY 用全键面而非 `config init` 模板：模板是用户面派生物，其 `nexus.version`
/// 与部分路径/指标名历来与 `Default` impl 不一致（既有事实，非本轮变量）；
/// 全键面夹具才是「所有键都存在时零值漂移」的严格锁。
#[test]
fn load_full_surface_fixture_matches_default() {
    let _guard = lock();
    let tmp = TempDir::new().expect("创建临时目录失败");
    let path = tmp.path().join("full.yaml");
    // JSON 是 YAML 子集，figment 的 Yaml provider 可直接解析。
    let full =
        serde_json::to_string_pretty(&config::default_config()).expect("default 应可序列化为 JSON");
    std::fs::write(&path, full).expect("写入夹具失败");

    let loaded = config::load(Some(path)).expect("全键面夹具应可加载");
    assert_eq!(
        as_value(&loaded),
        as_value(&config::default_config()),
        "全键面夹具加载结果必须与 ChimeraConfig::default() 同值"
    );
}

/// 坏 YAML 仍必须报错（`main` 侧 → exit 1 的契约前提，不得因去 defaults 而放宽）。
#[test]
fn load_bad_yaml_still_errors() {
    let _guard = lock();
    let tmp = TempDir::new().expect("创建临时目录失败");
    let path = tmp.path().join("bad.yaml");
    std::fs::write(&path, "quest:\n  auto_decompose: \"not-a-bool\"\n").expect("写入夹具失败");

    assert!(
        config::load(Some(path)).is_err(),
        "类型错误的配置必须保持 Err（main 侧 exit 1 契约）"
    );
}
