//! LSCT 任务感知策略装配 — 闭环集成测试 + manifest 锁定测试
//!
//! 对应架构层:L3 Storage(cmt-tiering → lsct-tiering 生产依赖边)
//! 对应任务:ADR-160 冻结孤岛偿还(M10,island-batch-promotion-a 岛 4/4)
//!
//! # 测试矩阵
//! - `test_lsct_policy_closed_loop_on_quest_created`:端到端闭环 —
//!   QuestCreated 事件 → 策略腿 reconcile 种子 + `handle_quest_created`
//!   (真实调用 lsct API)→ LsctTierSwitched → 既有订阅腿执行真实迁移
//! - `test_lsct_dep_is_production_not_dev`:manifest 锁定 —
//!   `lsct-tiering` 必须留在 `[dependencies]`(防 dev-dep 回流;
//!   棘轮 [GAP-R] 之外的 manifest 级守卫,双保险)
//! - `test_tier_conversion_roundtrip`:cmt ↔ lsct Tier 双向转换四变体全覆盖
//!
//! WHY 本文件存在(语义非空转论证,ADR-179 判据):
//! LSCT 订阅闭环(CMT 侧 `spawn_lsct_subscriber`)此前只有事件消费腿,
//! 生产图上没有任何 lsct 实例发布 `LsctTierSwitched`。本装配补上决策腿后,
//! "任务负载 → 策略 → 事件 → 实际迁移" 四段链路全部走真实生产调用路径。

use std::sync::Arc;
use std::time::Duration;

use cmt_tiering::lsct_policy::{from_lsct_tier, to_lsct_tier};
use cmt_tiering::{CapabilityEntry, CmtConfig, CmtCoordinator, Tier};
use event_bus::{EventBus, EventMetadata, NexusEvent};

/// 构造指定层级的能力条目(CapabilityEntry::new 的测试便利包装)
fn entry_at(id: &str, tier: Tier) -> CapabilityEntry {
    CapabilityEntry::new(id, "lsct policy test content", tier)
}

/// 端到端闭环:QuestCreated 驱动 LSCT 策略,CMT 订阅者执行真实迁移
///
/// 链路:QuestCreated(title=debug) → reconcile 把 cap-1 注册进 LSCT
/// → handle_quest_created 计算画像(Debug 低强度 → 目标冷层)
/// → LsctTierSwitched(Hot→Warm 逐级)→ apply_lsct_migration 真实迁移
///
/// WHY 从 Hot 出发降级:CmtCoordinator::insert 的公开语义是"写入 Hot 层
/// (跨层查找自动提升的设计基线)",冷层条目由降级/LRU 驱逐产生。
/// 因此闭环验证走 Hot → Warm 降级方向(Debug/Test 类 Quest)。
#[tokio::test]
async fn test_lsct_policy_closed_loop_on_quest_created() {
    let bus = EventBus::new();
    let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus.clone()).unwrap();
    // insert 公开路径恒入 Hot 层(entry.tier 字段由迁移路径维护)
    coord.insert(entry_at("cap-1", Tier::Hot)).await.unwrap();
    let coord = Arc::new(coord);

    // 既有订阅腿(执行迁移)+ 新策略腿(生产决策)—— 两 legs 齐备闭环才成立
    let _subscriber = coord.spawn_lsct_subscriber();
    let policy = coord.spawn_lsct_policy(lsct_tiering::LsctConfig::default());

    // 调试类低强度 Quest:TaskLoadProfile::from_quest_title("debug ...") → Debug/0.2
    // → 目标冷层,cap-1 逐级降温第一步 Hot→Warm
    bus.publish(NexusEvent::QuestCreated {
        metadata: EventMetadata::new("lsct-policy-test"),
        quest_id: "q-1".into(),
        title: "debug flaky failure reproduce".into(),
        task_count: 3,
    })
    .await
    .unwrap();

    // 轮询验证真实迁移效果:cap-1 从 Hot 降温到 Warm(迁移经
    // LsctTierSwitched → apply_lsct_migration,非 insert 语义)
    let mut migrated = false;
    for _ in 0..50 {
        if coord
            .list(Tier::Warm)
            .await
            .unwrap()
            .iter()
            .any(|e| e.id.as_str() == "cap-1")
        {
            migrated = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        migrated,
        "QuestCreated 应驱动 LSCT 策略闭环,把 cap-1 从 Hot 降温到 Warm"
    );

    // 策略装配真实持有 lsct 协调器实例(reconcile 已注册种子,非空)
    assert!(
        !policy.coordinator().is_empty(),
        "reconcile 应把 CMT 已有能力注册进 LSCT 策略层"
    );
    assert_eq!(
        policy.coordinator().get_tier("cap-1"),
        Some(lsct_tiering::Tier::Warm),
        "LSCT 侧 assignment 应随决策更新到 Warm(逐级降温第一步)"
    );
}

/// manifest 锁定:`lsct-tiering` 必须声明在 `[dependencies]` 而非 `[dev-dependencies]`
///
/// 防 dev-dep 回流:若被移回 dev-dependencies,本测试与可达性棘轮([GAP-R])
/// 双门同时变红。解析方式为段级文本扫描(与 scripts/check_declared_dep_usage.py
/// 同源思路,测试内嵌以提供 crate 级近端反馈)。
#[test]
fn test_lsct_dep_is_production_not_dev() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("读取 cmt-tiering Cargo.toml 失败");

    // 按 `[section]` 头切分,定位两个依赖段
    let mut section = "";
    let mut in_deps = false;
    let mut in_dev = false;
    for raw in manifest.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line.trim_matches(|c| c == '[' || c == ']');
            continue;
        }
        if line.starts_with("lsct-tiering") {
            assert_eq!(
                section, "dependencies",
                "lsct-tiering 必须留在 [dependencies](现位于 [{section}]),\
                 dev-dep 回流会使 ADR-160 偿还失效"
            );
            in_deps = true;
        }
        if line.starts_with("lsct-tiering") && section == "dev-dependencies" {
            in_dev = true;
        }
    }
    assert!(
        in_deps,
        "cmt-tiering 必须声明 lsct-tiering 生产依赖边(ADR-160 偿还)"
    );
    assert!(!in_dev, "lsct-tiering 不得出现在 [dev-dependencies]");
}

/// Tier 双向转换:四变体全覆盖 + 往返一致性(事件 payload 字符串契约的载体)
#[test]
fn test_tier_conversion_roundtrip() {
    for tier in [Tier::Hot, Tier::Warm, Tier::Cold, Tier::Ice] {
        let lsct = to_lsct_tier(tier);
        assert_eq!(from_lsct_tier(lsct), tier, "{tier:?} 往返转换应恒等");
        // 事件 payload 字符串契约:两类型 as_str 必须逐字一致
        assert_eq!(lsct.as_str(), tier.as_str());
    }
}
