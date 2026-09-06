//! 外部协议契约 — AppOp/AppEvent 与 Thread/Turn/Item 三原语（WI-01）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts，纯类型零逻辑）
//! 对应工作项: **WI-01 核心-表面分离：nexus-app-server**（v4.0 统一执行总案 §6.1）
//! 对应设计源: Codex CLI app-server 三原语（Thread/Turn/Item，断线重连断点恢复）、
//!             OpenCode serve/attach、DSH headless 五形态
//!
//! # 核心职责
//!
//! 承载**外部稳定协议**的纯类型契约（JSON-RPC v1 语义面）：
//! - **内闭外开（T6）**: 内部 `NexusEvent` 永不进外部协议；对外协议全设
//!   转译层——本模块即转译层两侧的**协议面类型**
//! - **协议三原语映射**: Thread=QuestSession（goal_id+run_id）；
//!   Turn=一次用户请求及后续工作（内含多 Step）；Item=最小 I/O 单元
//! - **断线恢复**: 客户端持 last_item_id，重连后从 session-store 回放增量
//!
//! # 设计约束（ADR-033 + WI-01）
//!
//! - **纯类型零逻辑**: 仅类型定义与构造辅助（无 IO 无状态变更）
//! - **零 crate 依赖**: 仅 `serde` derive（与 L0 铁律一致）
//! - **实验字段逃逸舱**: 各载荷带 `extras: Option<Box<str>>`（JSON 形态），
//!   协议 v1 冻结期（≥3 个月）内新增字段走 extras 而非破坏性变更
//!   （v4.0 §16 WI-01 风险回滚策略）
//! - **枚举密封**: `AppOp`/`AppEvent` 为编译期封闭枚举（协议 v1 语义面），
//!   扩展走版本化 v2（不做开放性枚举——T3 双轨制仅限内部事件）

use serde::{Deserialize, Serialize};

// ============================================================
// 身份 newtype
// ============================================================

/// 会话（Thread）ID — 对应 QuestSession（goal_id + run_id 的组合键编码）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub Box<str>);

impl ThreadId {
    /// 创建会话 ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(Box::from(id.into()))
    }

    /// 底层字符串引用
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 回合（Turn）ID — 一次用户请求及后续工作
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub Box<str>);

impl TurnId {
    /// 创建回合 ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(Box::from(id.into()))
    }

    /// 底层字符串引用
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 条目（Item）ID — 最小 I/O 单元（断点恢复游标）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemId(pub Box<str>);

impl ItemId {
    /// 创建条目 ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(Box::from(id.into()))
    }

    /// 底层字符串引用
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 审批请求 ID — ApprovalRespond 的关联键
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReqId(pub Box<str>);

impl ReqId {
    /// 创建审批请求 ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(Box::from(id.into()))
    }

    /// 底层字符串引用
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ============================================================
// 协议三原语
// ============================================================

/// 用户输入 — Turn 的载荷
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInput {
    /// 输入文本
    pub text: Box<str>,
    /// 实验字段逃逸舱（JSON 形态字符串，协议 v1 冻结期内扩展用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extras: Option<Box<str>>,
}

impl UserInput {
    /// 创建用户输入
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Box::from(text.into()),
            extras: None,
        }
    }

    /// 附加实验字段（逃逸舱）
    pub fn with_extras(mut self, extras: impl Into<String>) -> Self {
        self.extras = Some(Box::from(extras.into()));
        self
    }
}

/// 会话（Thread）— 对应 QuestSession（goal_id + run_id）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    /// 会话 ID
    pub thread_id: ThreadId,
    /// 目标 ID（goal_id，跨 run 共享）
    pub goal_id: Box<str>,
    /// 执行 ID（run_id）
    pub run_id: Box<str>,
    /// 创建时刻（Unix 毫秒）
    pub created_at_ms: u64,
}

impl Thread {
    /// 创建会话
    pub fn new(thread_id: ThreadId, goal_id: &str, run_id: &str, created_at_ms: u64) -> Self {
        Self {
            thread_id,
            goal_id: Box::from(goal_id),
            run_id: Box::from(run_id),
            created_at_ms,
        }
    }
}

/// 条目（Item）— 最小 I/O 单元，状态机驱动
///
/// 状态流转: `started → in_progress → completed / failed`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// 条目 ID（断点恢复游标）
    pub item_id: ItemId,
    /// 所属会话
    pub thread_id: ThreadId,
    /// 所属回合
    pub turn_id: TurnId,
    /// 条目类型标签（如 "message" / "tool_call" / "file_edit"）
    pub kind: Box<str>,
    /// 当前状态
    pub status: ItemStatus,
    /// 载荷（JSON 形态字符串，协议面不解析内部结构）
    pub payload: Box<str>,
    /// 实验字段逃逸舱
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extras: Option<Box<str>>,
}

impl Item {
    /// 创建条目
    pub fn new(
        item_id: ItemId,
        thread_id: ThreadId,
        turn_id: TurnId,
        kind: &str,
        status: ItemStatus,
        payload: &str,
    ) -> Self {
        Self {
            item_id,
            thread_id,
            turn_id,
            kind: Box::from(kind),
            status,
            payload: Box::from(payload),
            extras: None,
        }
    }

    /// 附加实验字段（逃逸舱）
    pub fn with_extras(mut self, extras: impl Into<String>) -> Self {
        self.extras = Some(Box::from(extras.into()));
        self
    }
}

/// 条目状态 — started → in_progress → completed / failed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// 已启动
    Started,
    /// 进行中
    InProgress,
    /// 已完成
    Completed,
    /// 已失败
    Failed,
}

/// 协议面 Token 用量 — TurnCompleted 的载荷
///
/// # 命名隔离（融合裁决 F2 同源）
/// 既有 `omni_message::TokenUsage` 为模型调用 Token 用量（L0 同层已有类型），
/// 本类型为**协议面** Token 用量（含缓存读写——WI-03 LPA 前缀稳定性埋点），
/// 命名 `AppTokenUsage` 避免类型命名空间冲突（E0252）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AppTokenUsage {
    /// 输入 token 数
    pub input_tokens: u64,
    /// 输出 token 数
    pub output_tokens: u64,
    /// 缓存读 token 数（WI-03 LPA 前缀稳定性埋点）
    pub cache_read_tokens: u64,
    /// 缓存写 token 数
    pub cache_creation_tokens: u64,
}

impl AppTokenUsage {
    /// 创建协议面 Token 用量
    pub fn new(
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
        }
    }
}

// ============================================================
// 审批
// ============================================================

/// 审批请求 — 客户端需用户裁决的操作
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// 请求 ID（ApprovalRespond 的关联键）
    pub request_id: ReqId,
    /// 操作描述（人类可读）
    pub description: Box<str>,
    /// 操作类别（只读/幂等写/破坏写/副作用）
    pub category: Box<str>,
    /// 关联条目 ID（可回放到具体 I/O 单元）
    pub item_id: Option<ItemId>,
}

impl ApprovalRequest {
    /// 创建审批请求
    pub fn new(
        request_id: ReqId,
        description: &str,
        category: &str,
        item_id: Option<ItemId>,
    ) -> Self {
        Self {
            request_id,
            description: Box::from(description),
            category: Box::from(category),
            item_id,
        }
    }
}

/// 审批裁决 — ApprovalRespond 的载荷
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// 批准
    Allow,
    /// 拒绝
    Deny,
    /// 本次批准（单次提权，WI-23 语义对齐）
    AllowOnce,
}

// ============================================================
// 权限模式
// ============================================================

/// 权限模式 — 六模式谱系（WI-23 execpolicy 语义对齐，L0 仅类型）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// 规划模式: 全只读 + DryRun 投影
    Plan,
    /// 自动批准写（writable_patterns 白名单）
    AcceptEdits,
    /// 默认: ask 规则触发审批
    Default,
    /// 仅预批准清单（headless）
    DontAsk,
    /// 分类器裁决（默认不启用 + 全量审计）
    Auto,
    /// 仅容器/CI（isolated=true）
    BypassPermissions,
}

// ============================================================
// 操作原语 AppOp — 客户端 → 服务端
// ============================================================

/// 客户端操作原语 — 协议 v1 封闭枚举（WI-01 §6.1）
///
/// # 内闭外开纪律（T6）
/// 本枚举是**外部协议面**：`NexusEvent` 内部变体永不进入此面；
/// 服务端经转译层将 AppOp 映射为内部 CoreOp 驱动核心。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppOp {
    /// 启动会话（ThreadStart）
    ThreadStart(ThreadStartParams),
    /// 提交回合输入
    TurnSubmit {
        /// 会话 ID
        thread_id: ThreadId,
        /// 用户输入
        input: UserInput,
    },
    /// 中断回合
    TurnInterrupt {
        /// 回合 ID
        turn_id: TurnId,
    },
    /// 审批裁决
    ApprovalRespond {
        /// 请求 ID
        request_id: ReqId,
        /// 裁决
        decision: ApprovalDecision,
    },
    /// 会话分叉（WI-18 会话树语义的前置协议面）
    ThreadFork {
        /// 源会话 ID
        thread_id: ThreadId,
        /// 分叉起点条目 ID
        at: ItemId,
    },
    /// 设置权限模式
    ModeSet {
        /// 目标模式
        mode: PermissionMode,
    },
}

/// 会话启动参数 — ThreadStart 的载荷
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadStartParams {
    /// 目标 ID（goal_id）
    pub goal_id: Box<str>,
    /// 执行 ID（run_id）
    pub run_id: Box<str>,
    /// 初始用户输入（可选——空会话启动）
    pub initial_input: Option<UserInput>,
    /// 实验字段逃逸舱
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extras: Option<Box<str>>,
}

impl ThreadStartParams {
    /// 创建会话启动参数
    pub fn new(goal_id: &str, run_id: &str) -> Self {
        Self {
            goal_id: Box::from(goal_id),
            run_id: Box::from(run_id),
            initial_input: None,
            extras: None,
        }
    }

    /// 附带初始输入
    pub fn with_initial_input(mut self, input: UserInput) -> Self {
        self.initial_input = Some(input);
        self
    }
}

// ============================================================
// 事件原语 AppEvent — 服务端 → 客户端
// ============================================================

/// 服务端事件原语 — 协议 v1 封闭枚举（WI-01 §6.1）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppEvent {
    /// 会话已启动
    ThreadStarted {
        /// 会话
        thread: Thread,
    },
    /// 条目状态变更（started → in_progress → completed / failed）
    ItemChanged {
        /// 条目（含最新状态）
        item: Item,
    },
    /// 审批请求
    ApprovalRequested {
        /// 审批请求
        request: ApprovalRequest,
    },
    /// 回合完成
    TurnCompleted {
        /// 回合 ID
        turn_id: TurnId,
        /// 协议面 Token 用量（含缓存读写）
        usage: AppTokenUsage,
    },
    /// 错误（应用层人类可读消息，结构化错误见 L0 errors 模块）
    Error {
        /// 错误代码（与 `NexusError` 变体名对应）
        code: Box<str>,
        /// 人类可读消息
        message: Box<str>,
    },
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_op_json_roundtrip() {
        let op = AppOp::TurnSubmit {
            thread_id: ThreadId::new("t-1"),
            input: UserInput::new("修复编译错误"),
        };
        let json = serde_json::to_string(&op).expect("JSON 序列化失败");
        let decoded: AppOp = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, op);
    }

    #[test]
    fn app_event_json_roundtrip() {
        let item = Item::new(
            ItemId::new("i-1"),
            ThreadId::new("t-1"),
            TurnId::new("r-1"),
            "message",
            ItemStatus::Completed,
            "{}",
        );
        let ev = AppEvent::ItemChanged { item };
        let json = serde_json::to_string(&ev).expect("JSON 序列化失败");
        let decoded: AppEvent = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, ev);
    }

    #[test]
    fn app_op_wire_format_frozen() {
        let op = AppOp::ModeSet {
            mode: PermissionMode::Plan,
        };
        let json = serde_json::to_string(&op).expect("JSON 序列化失败");
        assert!(json.contains("\"plan\""));
        // 枚举 tag 形态: externally tagged（serde 默认）
        assert!(json.starts_with("{\"ModeSet\":"));
    }

    #[test]
    fn item_status_lifecycle() {
        // started → in_progress → completed 状态机（协议面语义）
        let started = ItemStatus::Started;
        let in_progress = ItemStatus::InProgress;
        let completed = ItemStatus::Completed;
        assert_ne!(started, in_progress);
        assert_ne!(in_progress, completed);
        assert_ne!(started, completed);
    }

    #[test]
    fn approval_flow_roundtrip() {
        let req = ApprovalRequest::new(
            ReqId::new("req-1"),
            "运行 cargo build",
            "idempotent_write",
            Some(ItemId::new("i-9")),
        );
        let respond = AppOp::ApprovalRespond {
            request_id: req.request_id.clone(),
            decision: ApprovalDecision::AllowOnce,
        };
        let json = serde_json::to_string(&respond).expect("JSON 序列化失败");
        let decoded: AppOp = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert!(matches!(
            decoded,
            AppOp::ApprovalRespond {
                decision: ApprovalDecision::AllowOnce,
                ..
            }
        ));
    }

    #[test]
    fn thread_start_params_extras_escape_hatch() {
        // 逃逸舱: extras 字段可携带协议 v1 冻结期内的实验扩展
        let params =
            ThreadStartParams::new("goal-1", "run-1").with_initial_input(UserInput::new("你好"));
        let json = serde_json::to_string(&params).expect("JSON 序列化失败");
        assert!(json.contains("\"goal_id\":\"goal-1\""));
        assert!(json.contains("\"initial_input\":"));
    }

    #[test]
    fn thread_fork_semantics() {
        // WI-18 会话树分叉的协议面: 源会话 + 分叉起点
        let op = AppOp::ThreadFork {
            thread_id: ThreadId::new("t-1"),
            at: ItemId::new("i-5"),
        };
        let json = serde_json::to_string(&op).expect("JSON 序列化失败");
        let decoded: AppOp = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, op);
    }
}
