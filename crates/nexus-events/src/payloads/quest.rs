//! Quest / 投票辅助枚举
//!
//! WHY 独立成模块：本组类型只服务于 `Quest / 投票辅助枚举` 一个领域，
//! 与其它域的载荷无耦合（同文件平铺时改任一领域都要翻找全部类型）。
//!
//! 对外路径不变：全部类型经 `payloads/mod.rs` 的 facade 转出，
//! `event_bus::payloads::X` 逐字不变（ADR-314 W6 / BC-03 域下沉）。
//!
//! 类型：`QuestStatus`（Quest 完成状态）、`VoteValue`（议会三值投票）。

use serde::{Deserialize, Serialize};

/// Quest 完成状态 — 用于 `QuestCompleted` 事件(P1.2 实时数据驱动面板)
///
/// WHY 定义在 event-bus 而非 nexus-core:原 nexus-core 仅有 `TaskStatus`,
/// 没有 Quest 级别的结束状态。为不修改核心领域类型(§3.3.1 要求 ADR),
/// 在 event-bus 这一跨层通信契约层定义轻量级状态枚举。
/// 注:此类型属于 P1.2 实时数据面板契约,非 M4 双向控制新增。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QuestStatus {
    /// Quest 成功完成
    Completed,
    /// Quest 执行失败
    Failed,
    /// Quest 被取消
    Cancelled,
}

/// 投票值 — 议会投票的赞成/反对/弃权选项
///
/// WHY 定义在 event-bus:VoteCast 原使用 bool,但控制面板的 :vote 命令
/// 需要显式表达 Abstain,因此在跨层通信契约层定义三值枚举。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum VoteValue {
    /// 赞成
    Yes,
    /// 反对
    No,
    /// 弃权
    Abstain,
}

impl VoteValue {
    /// 返回投票值的小写字符串表示,用于 UI 展示与命令编码。
    pub fn as_str(&self) -> &'static str {
        match self {
            VoteValue::Yes => "yes",
            VoteValue::No => "no",
            VoteValue::Abstain => "abstain",
        }
    }
}

impl std::str::FromStr for VoteValue {
    type Err = ();

    /// 从字符串解析投票值,大小写不敏感。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "yes" => Ok(VoteValue::Yes),
            "no" => Ok(VoteValue::No),
            "abstain" => Ok(VoteValue::Abstain),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn vote_value_as_str_and_from_str_roundtrip() {
        for value in [VoteValue::Yes, VoteValue::No, VoteValue::Abstain] {
            let text = value.as_str();
            assert_eq!(text.parse::<VoteValue>(), Ok(value));
            // 大小写不敏感是命令面板 :vote 的解析约定
            assert_eq!(text.to_uppercase().parse::<VoteValue>(), Ok(value));
        }
        assert_eq!("maybe".parse::<VoteValue>(), Err(()));
    }

    #[test]
    fn vote_and_quest_status_wire_format_has_no_rename() {
        // 无 `#[serde(rename_all)]`:单元变体按变体名字符串编码。
        // 这是跨进程契约,改名/加 rename 会静默破坏在途命令与面板数据。
        assert_eq!(serde_json::to_string(&VoteValue::Yes).unwrap(), "\"Yes\"");
        assert_eq!(
            serde_json::from_str::<VoteValue>("\"Abstain\"").unwrap(),
            VoteValue::Abstain
        );
        assert_eq!(
            serde_json::to_string(&QuestStatus::Completed).unwrap(),
            "\"Completed\""
        );
        assert_eq!(
            serde_json::from_str::<QuestStatus>("\"Cancelled\"").unwrap(),
            QuestStatus::Cancelled
        );
    }
}
