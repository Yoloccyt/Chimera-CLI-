//! WI-G5 指标字段登记表锁 — BusLogger 导出面 ⇔ `scripts/bus_logger_metrics.txt` 双向恒等
//!
//! # 语义
//! 指标字段登记表（`scripts/bus_logger_metrics.txt`）是 BusLogger Prometheus 导出面的
//! **权威清单**；本测试把两侧互锁：
//! - `render_metrics()` 的 `# TYPE` 家族集 ⇔ 登记册 family 集（新增/删除/改名任一单向漂移即红）；
//! - 每行声明的 exposition-series 必须真实出现在输出中（`_total` 后缀由
//!   prometheus-client 对 Counter 自动追加，登记册记录**输出名**，防“登记名≠导出名”失配）。
//!
//! # 回滚
//! 删除本文件 + `scripts/bus_logger_metrics.txt`（零生产面）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::collections::BTreeMap;
use std::time::Duration;

use event_bus::{BusLogger, EventMetadata, NexusEvent};

/// 指标字段登记表（include_str：编译期钉入，缺失即编译失败）
const REGISTRY: &str = include_str!("../../../scripts/bus_logger_metrics.txt");

/// 登记册数据行 → (family, type, series, labels)
fn registry_rows() -> Vec<(String, String, String, String)> {
    REGISTRY
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| !l.starts_with("family|type|")) // 表头
        .map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            assert_eq!(
                f.len(),
                5,
                "登记行 schema: family|type|series|labels|note — {l}"
            );
            (f[0].into(), f[1].into(), f[2].into(), f[3].into())
        })
        .collect()
}

/// render_metrics 输出 → `# TYPE` 家族集
fn output_type_map(output: &str) -> BTreeMap<String, String> {
    output
        .lines()
        .filter_map(|l| l.strip_prefix("# TYPE "))
        .filter_map(|rest| {
            let mut it = rest.split_whitespace();
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .collect()
}

fn sample_events() -> (NexusEvent, NexusEvent) {
    let quest = NexusEvent::QuestCreated {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q-1".into(),
        title: "t".into(),
        task_count: 1,
    };
    let veto = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-veto".into(),
        veto_reason: "unsafe shell injection".into(),
        frozen_capabilities: vec!["shell-exec".into()],
    };
    (quest, veto)
}

/// 双向恒等：家族集与类型逐项对拍 + 每行出场序列真实存在于输出。
#[test]
fn registry_matches_render_metrics() {
    let rows = registry_rows();
    assert!(!rows.is_empty(), "登记册不得为空");

    // 驱动一次采集（Counter/Histogram 需至少一条样本才出现在输出）
    let logger = BusLogger::new("test-module");
    let (quest, veto) = sample_events();
    logger.log_publish(&quest, 1, Duration::from_micros(50));
    logger.log_publish(&veto, 1, Duration::from_micros(80));

    let output = logger.render_metrics();
    let types = output_type_map(&output);

    let mut registry_families: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    registry_families.sort();
    let mut output_families: Vec<String> = types.keys().cloned().collect();
    output_families.sort();
    assert_eq!(
        registry_families, output_families,
        "导出面家族集 ⇔ 登记册恒等（新增未登记/登记未接线均翻红）"
    );

    for (family, typ, series, _labels) in &rows {
        assert_eq!(
            types.get(family).map(String::as_str),
            Some(typ.as_str()),
            "类型失配：{family} 应为 {typ}"
        );
        assert!(
            output.contains(series.as_str()),
            "登记输出名 {series} 未在 render_metrics 中出现:\n{output}"
        );
    }
}

/// 登记册 schema 守卫：类型枚举合法 + 行数自洽。
#[test]
fn registry_is_wellformed() {
    let rows = registry_rows();
    for (family, typ, series, labels) in &rows {
        assert!(
            matches!(typ.as_str(), "counter" | "histogram"),
            "未知指标类型 {typ:?}（family={family}）"
        );
        assert!(!family.is_empty() && !series.is_empty());
        assert!(!labels.is_empty(), "labels 列必填（无标签写 '-'）");
    }
    assert_eq!(
        rows.len(),
        3,
        "现行导出面 = 3 指标（wave 变更须同批更新本断言与登记册）"
    );
}
