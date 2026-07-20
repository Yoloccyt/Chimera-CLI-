//! MutualInquirer — 同僚互询(Task 18 §18.5-18.6,§18.9)
//!
//! 架构层归属:L9 Quest(chimera-mas knowledge 子模块)
//! 核心职责:Agent 间通过 EventBus 进行同僚互询,复用 AgentConsultRequested/
//! AgentConsultResponded 事件;`create_safe_summary` 对 raw 上下文进行 PII 脱敏,
//! 确保不泄露文件路径 / IP / 邮箱 / API key(§5.5 ContextIsolationGuard 协同)。
//!
//! ## 脱敏规则(§18.9)
//!
//! | 模式 | 正则 | 替换 |
//! |------|------|------|
//! | 文件路径(Windows) | `[A-Za-z]:[\\/][^\s]+` | `[PATH]` |
//! | 文件路径(Unix) | `/[a-z]+/[^\s]+` | `[PATH]` |
//! | IP 地址 | `\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}` | `[IP]` |
//! | 邮箱 | `[\w.+-]+@[\w.-]+\.\w+` | `[EMAIL]` |
//! | API key | `sk-[A-Za-z0-9]{20,}` | `[API_KEY]` |
//!
//! ## 与 ContextIsolationGuard 的协同(§5.5)
//!
//! `ContextIsolationGuard::create_safe_summary(&AgentContext)` 按 block.name 模式过滤
//! (status/decision/conclusion),处理结构化上下文;
//! `MutualInquirer::create_safe_summary(&str)` 是纯函数,按 PII 模式正则脱敏,
//! 处理 raw 字符串。两者互补:前者保证结构层隔离,后者保证内容层脱敏。
//!
//! ## 关键约束
//!
//! - `tokio::broadcast` 先 subscribe 再 spawn(§4.4 反模式 3)
//! - 不泄露 raw 上下文(§5.5,§6.2 红线)
//! - 单函数 ≤ 200 行(§6.1 红线)

use std::sync::Arc;
use std::time::Duration;

use event_bus::{ConsultUrgency, EventBus, EventMetadata, NexusEvent};
use regex::Regex;
use tokio::time::timeout;

use crate::error::{MasError, Result};

/// 同僚互询默认超时(秒)— 同僚 Agent 响应时限
///
/// WHY 30s:同僚互询不走旗舰模型,响应应快于专家咨询(§18.3 High=15s);
/// 30s 作为兜底,实际由 ConsultUrgency::Medium 控制。
const INQUIRY_DEFAULT_TIMEOUT_S: u64 = 30;

/// 同僚互询器 — 经 EventBus 的 AgentConsultRequested/Responded 与同僚 Agent 通信
///
/// ## 设计要点
///
/// - **复用 EventBus**:不新建 AgentMessageBus(ADR-026 决策 2)
/// - **脱敏前置**:`inquire()` 调用 `create_safe_summary` 对 query 脱敏后再发送
/// - **ContextIsolationGuard 协同**:外层调用方用 Guard 校验所有权,
///   `MutualInquirer` 用正则脱敏 PII,双层防护
#[derive(Clone)]
pub struct MutualInquirer {
    /// 事件总线(复用)
    bus: EventBus,
    /// 当前 Agent ID(咨询方)
    agent_id: String,
    /// 同僚互询超时(秒,默认 30s)
    timeout_s: u64,
    /// 预编译正则(Arc 共享,避免每次调用重新编译)
    /// WHY Arc<Regex>:Regex 编译成本高,OnceLock/cached 风格避免重复编译
    patterns: Arc<SensitivePatterns>,
}

/// 敏感信息正则模式集合 — 预编译,避免重复编译开销
#[derive(Debug)]
struct SensitivePatterns {
    windows_path: Regex,
    unix_path: Regex,
    ip: Regex,
    email: Regex,
    api_key: Regex,
}

impl SensitivePatterns {
    /// 编译所有敏感信息正则
    ///
    /// WHY 集中编译:`create_safe_summary` 多次调用时复用同一组 Regex,
    /// 避免每次调用重新编译(Regex::new 是 O(n) 编译)。
    fn new() -> Self {
        Self {
            // Windows 路径:C:\foo\bar 或 D:/foo/bar
            windows_path: Regex::new(r"[A-Za-z]:[\\/][^\s]+").expect("windows path regex"),
            // Unix 路径:/etc/passwd /usr/local/bin
            unix_path: Regex::new(r"/[a-z]+/[^\s]+").expect("unix path regex"),
            // IP 地址:192.168.1.1
            ip: Regex::new(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}").expect("ip regex"),
            // 邮箱:user@example.com
            email: Regex::new(r"[\w.+-]+@[\w.-]+\.\w+").expect("email regex"),
            // API key:sk-xxxxxxxxxxxxxxxxxxxx(至少 20 个字符)
            api_key: Regex::new(r"sk-[A-Za-z0-9]{20,}").expect("api key regex"),
        }
    }
}

impl MutualInquirer {
    /// 创建同僚互询器
    ///
    /// ## 参数
    /// - `bus`:事件总线(复用,不新建 AgentMessageBus)
    /// - `agent_id`:当前 Agent ID(咨询方)
    pub fn new(bus: EventBus, agent_id: String) -> Self {
        Self {
            bus,
            agent_id,
            timeout_s: INQUIRY_DEFAULT_TIMEOUT_S,
            patterns: Arc::new(SensitivePatterns::new()),
        }
    }

    /// 向同僚 Agent 发起互询 — 经 AgentConsultRequested 事件 + 等待 AgentConsultResponded
    ///
    /// ## 参数
    /// - `peer_agent_id`:被咨询的同僚 Agent ID
    /// - `query`:查询字符串(会被 `create_safe_summary` 脱敏)
    ///
    /// ## 返回
    /// - `Ok(String)`:同僚回答
    /// - `Err(MasError::ExpertUnavailable)`:同僚超时未响应
    /// - `Err(MasError::MessageSendFailed)`:EventBus publish 失败
    pub async fn inquire(&self, peer_agent_id: &str, query: &str) -> Result<String> {
        // 1. 脱敏 query(§5.5,不泄露 raw 上下文)
        let safe_query = Self::create_safe_summary_with_patterns(query, &self.patterns);

        // 2. 构造 AgentConsultRequested 事件
        let request_event = NexusEvent::AgentConsultRequested {
            metadata: EventMetadata::new("chimera-mas/mutual-inquirer"),
            from: self.agent_id.clone(),
            to: peer_agent_id.to_string(),
            question: safe_query,
            context: String::new(),          // 上下文由调用方在外层脱敏后注入
            urgency: ConsultUrgency::Medium, // 同僚互询默认 Medium(§18.3 SLA 30s)
        };

        // 3. 先订阅再 publish(§4.4 反模式 3)
        let mut rx = self.bus.subscribe();

        // 4. publish AgentConsultRequested
        self.bus
            .publish(request_event)
            .await
            .map_err(|e| MasError::MessageSendFailed {
                from: self.agent_id.clone(),
                to: peer_agent_id.to_string(),
                reason: format!("Publish AgentConsultRequested failed: {e}"),
            })?;

        // 5. 等待 AgentConsultResponded,带超时
        let result = timeout(Duration::from_secs(self.timeout_s), async {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let NexusEvent::AgentConsultResponded { to, answer, .. } = &event {
                            if to == &self.agent_id {
                                return Ok(answer.clone());
                            }
                        }
                    }
                    Err(_e) => {
                        return Err(MasError::Internal(
                            "EventBus subscribe channel closed".to_string(),
                        ));
                    }
                }
            }
        })
        .await;

        match result {
            Ok(inner_result) => inner_result,
            Err(_elapsed) => Err(MasError::ExpertUnavailable {
                expert_id: peer_agent_id.to_string(),
                reason: format!("peer inquiry timeout after {}s", self.timeout_s),
            }),
        }
    }

    /// 创建安全摘要 — 对 raw 上下文进行 PII 脱敏(关联函数,纯函数)
    ///
    /// 用预编译正则替换:
    /// - 文件路径 → `[PATH]`
    /// - IP 地址 → `[IP]`
    /// - 邮箱 → `[EMAIL]`
    /// - API key → `[API_KEY]`
    ///
    /// ## 参数
    /// - `raw_context`:原始上下文字符串
    ///
    /// ## 返回
    /// 脱敏后的字符串(可安全用于跨 Agent 通信)
    ///
    /// ## 示例
    ///
    /// ```no_run
    /// use chimera_mas::knowledge::MutualInquirer;
    ///
    /// let raw = "user@example.com accessed /etc/passwd from 192.168.1.1 with sk-abcdef1234567890abcdef1234567890";
    /// let safe = MutualInquirer::create_safe_summary(raw);
    /// assert!(!safe.contains("user@example.com"));
    /// assert!(!safe.contains("/etc/passwd"));
    /// assert!(!safe.contains("192.168.1.1"));
    /// assert!(!safe.contains("sk-abcdef1234567890abcdef1234567890"));
    /// ```
    pub fn create_safe_summary(raw_context: &str) -> String {
        let patterns = SensitivePatterns::new();
        Self::create_safe_summary_with_patterns(raw_context, &Arc::new(patterns))
    }

    /// 内部辅助:用预编译模式集合进行脱敏(避免重复编译)
    fn create_safe_summary_with_patterns(
        raw_context: &str,
        patterns: &Arc<SensitivePatterns>,
    ) -> String {
        // 顺序替换:先替换结构化模式(路径/邮箱/API key),最后替换 IP
        // WHY 顺序:IP 正则可能匹配路径中的数字段(如 /v1.2.3),先替换路径避免误匹配
        let s = patterns.windows_path.replace_all(raw_context, "[PATH]");
        let s = patterns.unix_path.replace_all(&s, "[PATH]");
        let s = patterns.email.replace_all(&s, "[EMAIL]");
        let s = patterns.api_key.replace_all(&s, "[API_KEY]");
        let s = patterns.ip.replace_all(&s, "[IP]");
        s.into_owned()
    }
}

/// 手动实现 Debug — EventBus 未实现 Debug,跳过 bus 字段(§4.1 规范)
impl std::fmt::Debug for MutualInquirer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MutualInquirer")
            .field("agent_id", &self.agent_id)
            .field("timeout_s", &self.timeout_s)
            .field("patterns", &"<compiled regex patterns>")
            .finish()
    }
}
