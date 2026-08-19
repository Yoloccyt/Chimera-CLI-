//! JSON envelope 渲染快照测试（cargo-insta 试点, P2-7 剩余项）
//!
//! WHY 快照测试:JSON envelope schema(Task 1.7.4, `{ "status": "ok", "data": ... }`)
//! 是 `--json` 程序化消费契约,格式演进必须受控——快照锁定当前格式,
//! 任何字段/缩进/顺序变更都会以测试失败形式暴露,由人工 review 决定接受或拒绝。
//! 与直接捕获 stdout 不同,`render_json` 是纯函数,快照与终端环境完全无关。
//!
//! 更新快照方式:
//!   cargo insta review        # 交互式审查待接受快照
//!   cargo insta accept        # 接受全部新快照
//!   cargo test -p chimera-cli # 常规运行(快照不匹配则失败)

use std::collections::BTreeMap;

use chimera_cli::output::render_json;
/// 辅助:unwrap 渲染结果(测试内序列化不应失败)
fn render<T: serde::Serialize>(data: &T) -> String {
    render_json(data).unwrap_or_else(|e| panic!("render_json 序列化失败: {e}"))
}

/// 正常路径:简单标量载荷(字符串)
#[test]
fn json_envelope_basic_string() {
    let data = "hello world";
    insta::assert_snapshot!(render(&data));
}

/// 正常路径:数字与布尔载荷
#[test]
fn json_envelope_basic_numbers() {
    let data = vec![42u64, 3_600, 7];
    insta::assert_snapshot!(render(&data));
}

/// 嵌套路径:结构化载荷(JSON 对象字面量,验证字段顺序与缩进确定性)
#[test]
fn json_envelope_nested_struct() {
    let payload = serde_json::json!({
        "name": "chimera",
        "version": "2.26.0-omega",
        "layers": ["L0", "L4", "L10"],
        "stats": { "tests": 9954, "crates": 38 },
    });
    insta::assert_snapshot!(render(&payload));
}

/// 边界路径:空容器与空字符串
#[test]
fn json_envelope_empty_edges() {
    let empty_vec: Vec<u32> = vec![];
    insta::assert_snapshot!(render(&empty_vec));
    let empty_map: BTreeMap<String, u32> = BTreeMap::new();
    insta::assert_snapshot!(render(&empty_map));
    let empty_str = "";
    insta::assert_snapshot!(render(&empty_str));
}

/// 不可序列化类型:手动实现 `Serialize` 返回 Err(确定性错误路径)
///
/// WHY 不用 f64::NAN:其序列化行为依赖 serde_json 的 feature 配置
/// (arbitrary_precision 等会改变 NaN 处理),不可靠;自定义类型
/// 保证任何配置下都稳定返回 Err。
struct Unserializable;
impl serde::Serialize for Unserializable {
    fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("故意不可序列化的类型"))
    }
}

/// 异常路径:不可序列化值应返回 Err(而非 panic)
#[test]
fn json_envelope_unserializable_returns_err() {
    let result = render_json(&Unserializable);
    assert!(
        result.is_err(),
        "自定义不可序列化类型应返回 Err,实际: {:?}",
        result
    );
}

/// 异常路径:错误 envelope 结构(JsonError)独立于成功路径
///
/// WHY 直接序列化而非经 render_json:JsonError 由错误输出路径单独构造
/// (不走成功 envelope),此处锁定其自身结构字段。
#[test]
fn json_error_envelope_shape() {
    let detail = chimera_cli::output::JsonErrorDetail {
        kind: "ConfigError",
        message: "配置文件缺失".to_string(),
    };
    let err = chimera_cli::output::JsonError {
        status: "error",
        error: detail,
        exit_code: 3,
    };
    let json = serde_json::to_string_pretty(&err).expect("JsonError 序列化不应失败");
    insta::assert_snapshot!(json);
}
