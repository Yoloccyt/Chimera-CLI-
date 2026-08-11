//! SlashCommandRegistry — 斜杠命令单一事实源(Concord 重构 T1.1,R9 解法)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **独立于 ActionRegistry**:`ActionRegistry` 有 40 项熔断线(`MAX_ACTIONS`),
//!   斜杠命令基线(重构方案 §9 命令总表)若全部注册为 Action 将击穿熔断线。
//!   命令与动作分层:ActionRegistry 只保留可键达动作(≤40),斜杠命令表
//!   不受 40 上限约束;两表通过 `action_id` 做 id 级引用(不复制元数据)。
//! - **三分层 tier**(重构方案 §6.3.3 命令语义分层):
//!   instant(本地执行)/ orchestrated(EventBus 编排)/ agent(提示词模板)。
//!   W2 波次的斜杠解析器按 tier 分流执行路径。
//! - **标题存 i18n key**:与 `ActionDescriptor` 同一模式,展示时经 `i18n::tr`
//!   解析;键表收口(T2.5)前 `tr` 对未命中键优雅降级返回键本身。

use std::collections::HashMap;

/// 命令执行分层 — 决定斜杠命令的执行路径(重构方案 §6.3.3)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlashTier {
    /// ⚡ 即时命令:本地视图/本地状态变更,不出 TUI 进程(如 /theme /chat /status)
    Instant,
    /// 🔄 编排命令:经 `TuiActionRequested`/EventBus 派发后端编排(如 /quest cancel /compact)
    Orchestrated,
    /// 🤖 Agent 命令:提示词模板进入 composer,由 LLM 执行(如 /review /init)
    Agent,
}

/// 命令功能域 — 对应重构方案 §9 七域命令表的域划分
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlashDomain {
    /// 会话域(9.2):/new /clear /compact /resume /fork /side /rename /export /undo /redo /exit
    Session,
    /// 模型与审批域(9.3):/model /mode /plan /permissions
    ModelApproval,
    /// 任务编排域(9.4,Chimera 特有):/quest <sub> /agent <sub>
    Orchestration,
    /// 代码与项目域(9.5):/init /diff /review /mention /wiki /mcp
    CodeProject,
    /// 视图与配置域(9.6):/dashboard /chat /theme /layout /statusline /vim /config 等
    ViewConfig,
    /// 系统与诊断域(9.7):/status /doctor /audit /parliament /help /feedback
    System,
}

impl SlashDomain {
    /// 返回域的稳定标识(用于日志/分组展示)
    pub fn as_str(&self) -> &'static str {
        match self {
            SlashDomain::Session => "session",
            SlashDomain::ModelApproval => "model",
            SlashDomain::Orchestration => "orchestration",
            SlashDomain::CodeProject => "code",
            SlashDomain::ViewConfig => "view",
            SlashDomain::System => "system",
        }
    }
}

/// 斜杠命令元描述 — 命令表条目
///
/// # 字段
/// - `name`:命令词(不含前导 `/`,子命令空格分隔,如 `"quest cancel"`)
/// - `aliases`:同义别名(如 exit 的 `["quit", "q"]`);别名不单独占条目
/// - `tier`:执行分层(instant/orchestrated/agent)
/// - `domain`:功能域
/// - `title_key`:i18n 资源 key(展示时经 `i18n::tr` 解析)
/// - `action_id`:关联的 ActionRegistry 动作 id;`None` 表示纯命令无动作映射
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommandDesc {
    /// 命令词(不含前导 `/`,子命令空格分隔)
    pub name: &'static str,
    /// 同义别名列表(可为空切片)
    pub aliases: &'static [&'static str],
    /// 执行分层
    pub tier: SlashTier,
    /// 功能域
    pub domain: SlashDomain,
    /// 标题的 i18n key
    pub title_key: &'static str,
    /// 关联动作 id(None = 无 ActionRegistry 映射)
    pub action_id: Option<&'static str>,
}

/// 斜杠命令注册表 — 命令表的单一事实源
///
/// WHY 与 ActionRegistry 同构(有序 Vec + HashMap 索引):复用已验证的
/// 注册/查询模式,保持 actions 层内部一致的代码心智。
#[derive(Debug, Clone, Default)]
pub struct SlashCommandRegistry {
    /// 有序命令列表(保留注册顺序,供补全/帮助稳定展示)
    commands: Vec<SlashCommandDesc>,
    /// name → commands 下标索引,提供 O(1) 精确查询
    index: HashMap<&'static str, usize>,
}

/// 内建命令表 — 重构方案 §9 命令总表(六域,别名合并计数)
///
/// WHY 静态表:命令词全部编译期已知,零分配;增删命令只改本表,
/// 补全/帮助/词表抽查脚本全部从注册表派生(W2 波次接线)。
/// 可见性 pub(crate):解析器 proptest 需逐项遍历(crate 内测试用)。
pub(crate) static BUILTIN_COMMANDS: &[SlashCommandDesc] = &[
    // ── 会话域(§9.2,11 条) ─────────────────────────────────────
    SlashCommandDesc {
        name: "new",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::Session,
        title_key: "slash.session.new",
        action_id: None,
    },
    SlashCommandDesc {
        name: "clear",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::Session,
        title_key: "slash.session.clear",
        action_id: None,
    },
    SlashCommandDesc {
        name: "compact",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Session,
        title_key: "slash.session.compact",
        action_id: None,
    },
    SlashCommandDesc {
        name: "resume",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::Session,
        title_key: "slash.session.resume",
        action_id: None,
    },
    SlashCommandDesc {
        name: "fork",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Session,
        title_key: "slash.session.fork",
        action_id: None,
    },
    SlashCommandDesc {
        name: "side",
        aliases: &["btw"],
        tier: SlashTier::Agent,
        domain: SlashDomain::Session,
        title_key: "slash.session.side",
        action_id: None,
    },
    SlashCommandDesc {
        name: "rename",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::Session,
        title_key: "slash.session.rename",
        action_id: None,
    },
    SlashCommandDesc {
        name: "export",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::Session,
        title_key: "slash.session.export",
        action_id: None,
    },
    SlashCommandDesc {
        name: "undo",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Session,
        title_key: "slash.session.undo",
        action_id: None,
    },
    SlashCommandDesc {
        name: "redo",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Session,
        title_key: "slash.session.redo",
        action_id: None,
    },
    SlashCommandDesc {
        name: "exit",
        aliases: &["quit", "q"],
        tier: SlashTier::Instant,
        domain: SlashDomain::Session,
        title_key: "slash.session.exit",
        action_id: None,
    },
    // ── 模型与审批域(§9.3,4 条) ────────────────────────────────
    SlashCommandDesc {
        name: "model",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ModelApproval,
        title_key: "slash.model.model",
        action_id: None,
    },
    SlashCommandDesc {
        name: "mode",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ModelApproval,
        title_key: "slash.model.mode",
        action_id: None,
    },
    SlashCommandDesc {
        name: "plan",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ModelApproval,
        title_key: "slash.model.plan",
        action_id: None,
    },
    SlashCommandDesc {
        name: "permissions",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ModelApproval,
        title_key: "slash.model.permissions",
        action_id: None,
    },
    // ── 任务编排域(§9.4,10 条,Chimera 特有子命令风格) ──────────
    SlashCommandDesc {
        name: "quest list",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.quest.list",
        action_id: None,
    },
    SlashCommandDesc {
        name: "quest show",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.quest.show",
        action_id: None,
    },
    // Concord W2(T2.4):遗留 `:pause/:resume <id>` 的斜杠对等条目,
    // 经 Legacy 桥接到 TuiCommand::RequestQuest*(保留确认弹窗)。
    SlashCommandDesc {
        name: "quest pause",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.quest.pause",
        action_id: None,
    },
    SlashCommandDesc {
        name: "quest resume",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.quest.resume",
        action_id: None,
    },
    SlashCommandDesc {
        name: "quest cancel",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.quest.cancel",
        action_id: None,
    },
    SlashCommandDesc {
        name: "quest checkpoint",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.quest.checkpoint",
        action_id: None,
    },
    SlashCommandDesc {
        name: "quest priority",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.quest.priority",
        action_id: None,
    },
    SlashCommandDesc {
        name: "quest vote",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.quest.vote",
        action_id: None,
    },
    SlashCommandDesc {
        name: "agent list",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.agent.list",
        action_id: None,
    },
    SlashCommandDesc {
        name: "agent spawn",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.agent.spawn",
        action_id: None,
    },
    SlashCommandDesc {
        name: "agent inspect",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.agent.inspect",
        action_id: None,
    },
    SlashCommandDesc {
        name: "agent cancel",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::Orchestration,
        title_key: "slash.agent.cancel",
        action_id: None,
    },
    // ── 代码与项目域(§9.5,6 条) ────────────────────────────────
    SlashCommandDesc {
        name: "init",
        aliases: &[],
        tier: SlashTier::Agent,
        domain: SlashDomain::CodeProject,
        title_key: "slash.code.init",
        action_id: None,
    },
    SlashCommandDesc {
        name: "diff",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::CodeProject,
        title_key: "slash.code.diff",
        action_id: None,
    },
    SlashCommandDesc {
        name: "review",
        aliases: &[],
        tier: SlashTier::Agent,
        domain: SlashDomain::CodeProject,
        title_key: "slash.code.review",
        action_id: None,
    },
    SlashCommandDesc {
        name: "mention",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::CodeProject,
        title_key: "slash.code.mention",
        action_id: None,
    },
    SlashCommandDesc {
        name: "wiki",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::CodeProject,
        title_key: "slash.code.wiki",
        action_id: None,
    },
    SlashCommandDesc {
        name: "mcp",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::CodeProject,
        title_key: "slash.code.mcp",
        action_id: None,
    },
    // ── 视图与配置域(§9.6,10 条) ───────────────────────────────
    SlashCommandDesc {
        name: "dashboard",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.dashboard",
        action_id: None,
    },
    SlashCommandDesc {
        name: "chat",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.chat",
        action_id: None,
    },
    SlashCommandDesc {
        name: "theme",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.theme",
        action_id: None,
    },
    SlashCommandDesc {
        name: "layout",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.layout",
        action_id: None,
    },
    SlashCommandDesc {
        name: "statusline",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.statusline",
        action_id: None,
    },
    SlashCommandDesc {
        name: "vim",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.vim",
        action_id: None,
    },
    SlashCommandDesc {
        name: "config",
        aliases: &["settings"],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.config",
        action_id: None,
    },
    SlashCommandDesc {
        name: "debug-config",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.debug_config",
        action_id: None,
    },
    SlashCommandDesc {
        name: "locale",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.locale",
        action_id: None,
    },
    SlashCommandDesc {
        name: "focus",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.focus",
        action_id: None,
    },
    // Concord W2(T2.4):`/` 搜索迁移与遗留过滤命令斜杠化——/search 承接原
    // EnterSearch 语义(别名 find);filter/level 承接命令栏同名遗留命令。
    SlashCommandDesc {
        name: "search",
        aliases: &["find"],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.search",
        action_id: None,
    },
    SlashCommandDesc {
        name: "filter",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.filter",
        action_id: None,
    },
    SlashCommandDesc {
        name: "level",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.level",
        action_id: None,
    },
    // 参数化面板切换(`/panel <name>`):一条命令覆盖 25 面板,避免逐面板膨胀。
    SlashCommandDesc {
        name: "panel",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::ViewConfig,
        title_key: "slash.view.panel",
        action_id: None,
    },
    // ── 系统与诊断域(§9.7,6 条) ────────────────────────────────
    SlashCommandDesc {
        name: "status",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::System,
        title_key: "slash.system.status",
        action_id: None,
    },
    SlashCommandDesc {
        name: "doctor",
        aliases: &[],
        tier: SlashTier::Instant,
        domain: SlashDomain::System,
        title_key: "slash.system.doctor",
        action_id: None,
    },
    SlashCommandDesc {
        name: "audit",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::System,
        title_key: "slash.system.audit",
        action_id: None,
    },
    SlashCommandDesc {
        name: "parliament",
        aliases: &[],
        tier: SlashTier::Orchestrated,
        domain: SlashDomain::System,
        title_key: "slash.system.parliament",
        action_id: None,
    },
    SlashCommandDesc {
        name: "help",
        aliases: &["?"],
        tier: SlashTier::Instant,
        domain: SlashDomain::System,
        title_key: "slash.system.help",
        action_id: None,
    },
    SlashCommandDesc {
        name: "feedback",
        aliases: &[],
        tier: SlashTier::Agent,
        domain: SlashDomain::System,
        title_key: "slash.system.feedback",
        action_id: None,
    },
];

impl SlashCommandRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建并装入重构方案 §9 全部内建命令(生产入口)
    pub fn with_builtin_commands() -> Self {
        let mut reg = Self::new();
        for desc in BUILTIN_COMMANDS {
            reg.register(*desc);
        }
        reg
    }

    /// 注册一条命令;若 name 或别名与既有条目冲突则忽略并返回 `false`
    ///
    /// WHY 别名冲突也拒绝:别名是命令的等价入口,两个命令共享别名会造成
    /// 补全歧义与执行分流不确定;与 ActionRegistry 相同,拒绝优于静默覆盖。
    pub fn register(&mut self, desc: SlashCommandDesc) -> bool {
        if self.index.contains_key(desc.name) {
            return false;
        }
        if desc.aliases.iter().any(|a| self.index.contains_key(a)) {
            return false;
        }
        let idx = self.commands.len();
        self.index.insert(desc.name, idx);
        for alias in desc.aliases {
            self.index.insert(alias, idx);
        }
        self.commands.push(desc);
        true
    }

    /// 按命令词或别名精确查询(不含前导 `/`)
    pub fn get(&self, name: &str) -> Option<&SlashCommandDesc> {
        self.index.get(name).map(|&i| &self.commands[i])
    }

    /// 返回全部命令(注册顺序)
    pub fn all(&self) -> &[SlashCommandDesc] {
        &self.commands
    }

    /// 命令总数(别名不单独计数)
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// 注册表是否为空
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// 返回指定执行分层的全部命令(W2 解析器按 tier 分流的取数入口)
    pub fn by_tier(&self, tier: SlashTier) -> Vec<&SlashCommandDesc> {
        self.commands.iter().filter(|c| c.tier == tier).collect()
    }

    /// 返回指定功能域的全部命令
    pub fn by_domain(&self, domain: SlashDomain) -> Vec<&SlashCommandDesc> {
        self.commands
            .iter()
            .filter(|c| c.domain == domain)
            .collect()
    }

    /// 模糊搜索 — 供 SlashCommandSurface 补全面板检索(W2 接线)
    ///
    /// 匹配规则(大小写不敏感):query 命中 name / 别名 / 解析后的标题任一即返回。
    /// WHY 三路匹配:与 `ActionRegistry::fuzzy_search` 同一心智(用户可能记得
    /// 命令词、别名或界面标题);空 query 返回全部(补全面板打开时展示完整列表)。
    pub fn fuzzy(&self, query: &str) -> Vec<&SlashCommandDesc> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.commands.iter().collect();
        }
        self.commands
            .iter()
            .filter(|c| Self::matches(c, &q))
            .collect()
    }

    /// 判断单条命令是否命中查询(name/别名/标题三路)
    fn matches(desc: &SlashCommandDesc, query_lower: &str) -> bool {
        if desc.name.to_lowercase().contains(query_lower) {
            return true;
        }
        if desc
            .aliases
            .iter()
            .any(|a| a.to_lowercase().contains(query_lower))
        {
            return true;
        }
        // 标题经 i18n 解析后按当前 locale 匹配;键表收口前 tr 未命中返回键本身,
        // 此时按键名匹配(slash.* 键名含命令域信息,仍具可发现性)
        crate::i18n::tr(desc.title_key)
            .to_lowercase()
            .contains(query_lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::registry::{ActionRegistry, MAX_ACTIONS};
    use proptest::prelude::*;

    #[test]
    fn builtin_table_registers_full_baseline() {
        let reg = SlashCommandRegistry::with_builtin_commands();
        // 重构方案 §9 六域基线 47 条 + Concord W2 补齐 6 条(quest pause/resume、
        // search/filter/level、panel)= 53 条规范条目
        assert_eq!(reg.len(), 53, "内建命令表规模应与 §9 基线 + W2 补齐一致");
        assert!(!reg.is_empty());
    }

    #[test]
    fn command_names_are_unique() {
        let mut reg = SlashCommandRegistry::new();
        for d in BUILTIN_COMMANDS {
            assert!(reg.register(*d), "首次注册 {} 应成功", d.name);
        }
        for d in BUILTIN_COMMANDS {
            assert!(!reg.register(*d), "重复注册 {} 应被拒绝", d.name);
        }
    }

    #[test]
    fn alias_collision_is_rejected() {
        let mut reg = SlashCommandRegistry::new();
        // exit 占用别名 "q";另一条命令再以 "q" 为名或别名注册必须被拒绝
        assert!(reg.get("exit").is_none());
        let reg2 = SlashCommandRegistry::with_builtin_commands();
        assert!(reg2.get("q").is_some(), "别名应可精确查询");
        assert!(reg2.get("quit").is_some());
        assert_eq!(reg2.get("q").unwrap().name, "exit");
        let mut reg3 = SlashCommandRegistry::with_builtin_commands();
        let colliding = SlashCommandDesc {
            name: "q_alias_cmd",
            aliases: &["q"],
            tier: SlashTier::Instant,
            domain: SlashDomain::System,
            title_key: "slash.test.collide",
            action_id: None,
        };
        assert!(!reg3.register(colliding), "与既有别名冲突应被拒绝");
        let _ = reg.register(SlashCommandDesc {
            name: "solo",
            aliases: &[],
            tier: SlashTier::Instant,
            domain: SlashDomain::System,
            title_key: "slash.test.solo",
            action_id: None,
        });
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn tier_partition_is_exhaustive_and_disjoint() {
        let reg = SlashCommandRegistry::with_builtin_commands();
        let instant = reg.by_tier(SlashTier::Instant).len();
        let orchestrated = reg.by_tier(SlashTier::Orchestrated).len();
        let agent = reg.by_tier(SlashTier::Agent).len();
        // 三分层划分无遗漏无重叠
        assert_eq!(instant + orchestrated + agent, reg.len());
        // 每层均非空(三种执行路径都有命令覆盖)
        assert!(instant > 0 && orchestrated > 0 && agent > 0);
    }

    #[test]
    fn by_domain_partitions_commands() {
        let reg = SlashCommandRegistry::with_builtin_commands();
        let session = reg.by_domain(SlashDomain::Session);
        assert_eq!(session.len(), 11, "会话域应为 11 条(§9.2)");
        assert!(session.iter().all(|c| c.domain == SlashDomain::Session));
        // 六域合计 = 总数
        let total: usize = [
            SlashDomain::Session,
            SlashDomain::ModelApproval,
            SlashDomain::Orchestration,
            SlashDomain::CodeProject,
            SlashDomain::ViewConfig,
            SlashDomain::System,
        ]
        .iter()
        .map(|d| reg.by_domain(*d).len())
        .sum();
        assert_eq!(total, reg.len());
    }

    #[test]
    fn every_command_discoverable_by_own_name() {
        let reg = SlashCommandRegistry::with_builtin_commands();
        for c in reg.all() {
            let hits = reg.fuzzy(c.name);
            assert!(
                hits.iter().any(|h| h.name == c.name),
                "命令 {} 应能被自身名称搜索命中",
                c.name
            );
        }
    }

    #[test]
    fn fuzzy_empty_query_returns_all_and_alias_hits() {
        let reg = SlashCommandRegistry::with_builtin_commands();
        assert_eq!(reg.fuzzy("").len(), reg.len());
        assert_eq!(
            reg.fuzzy("   ").len(),
            reg.len(),
            "纯空白 query 等价空 query"
        );
        // 别名片段命中
        assert!(reg.fuzzy("quit").iter().any(|c| c.name == "exit"));
        // 大小写不敏感
        assert!(reg
            .fuzzy("QUEST CANCEL")
            .iter()
            .any(|c| c.name == "quest cancel"));
    }

    #[test]
    fn r9_action_budget_not_breached_by_slash_table() {
        // R9 解法验证:斜杠命令表(53 条 > 40)装入后,ActionRegistry 规模不受影响
        let slash = SlashCommandRegistry::with_builtin_commands();
        assert!(slash.len() > MAX_ACTIONS, "命令表规模确已超出动作熔断线");
        let actions = ActionRegistry::with_builtin_domains();
        assert!(
            actions.len() <= MAX_ACTIONS,
            "ActionRegistry 必须保持在 40 熔断线内(不受命令表影响)"
        );
        assert!(!actions.is_over_budget());
    }

    proptest! {
        /// 属性:内建命令表的任意子集注册后,注册数 = 子集大小、tier 划分守恒、
        /// 空 query 补全返回全部 —— 保证补全面板在任意命令表规模下行为一致。
        #[test]
        fn subset_registration_invariants(picks in proptest::collection::vec(any::<bool>(), 0..BUILTIN_COMMANDS.len())) {
            let mut reg = SlashCommandRegistry::new();
            let mut expected = 0usize;
            for (pick, desc) in picks.iter().zip(BUILTIN_COMMANDS.iter()) {
                if *pick {
                    prop_assert!(reg.register(*desc));
                    expected += 1;
                }
            }
            prop_assert_eq!(reg.len(), expected);
            let tier_sum = reg.by_tier(SlashTier::Instant).len()
                + reg.by_tier(SlashTier::Orchestrated).len()
                + reg.by_tier(SlashTier::Agent).len();
            prop_assert_eq!(tier_sum, reg.len());
            prop_assert_eq!(reg.fuzzy("").len(), reg.len());
        }

        /// 属性:任意命令表规模下 ActionRegistry 预算守恒(R9 的参数化证明)
        #[test]
        fn action_budget_invariant_under_any_slash_size(n in 0usize..=BUILTIN_COMMANDS.len()) {
            let mut slash = SlashCommandRegistry::new();
            for desc in BUILTIN_COMMANDS.iter().take(n) {
                prop_assert!(slash.register(*desc));
            }
            prop_assert_eq!(slash.len(), n);
            let actions = ActionRegistry::with_builtin_domains();
            prop_assert!(actions.len() <= MAX_ACTIONS);
        }
    }

    /// Concord W2(T2.5 i18n 门禁):每条命令的 title_key 必须在中文与英文
    /// 两语言表均命中(tr 不回退键本身)——新增命令漏译时本测试即时拦截。
    #[test]
    fn all_command_title_keys_resolve_in_both_locales() {
        let _locale_guard = crate::i18n::locale_test_guard();
        let reg = SlashCommandRegistry::with_builtin_commands();

        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        for c in reg.all() {
            assert_ne!(
                crate::i18n::tr(c.title_key),
                c.title_key,
                "zh 表缺失命令键 {}(命令 {})",
                c.title_key,
                c.name
            );
        }

        crate::i18n::set_locale(crate::i18n::Locale::En);
        for c in reg.all() {
            assert_ne!(
                crate::i18n::tr(c.title_key),
                c.title_key,
                "en 表缺失命令键 {}(命令 {})",
                c.title_key,
                c.name
            );
        }
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
    }

    /// Concord W2 性能门禁:补全 fuzzy 查询单帧开销 <1ms
    ///
    /// WHY 断言式用例:补全面板每次按键都重算候选,53 条命令表的三路
    /// 匹配必须在键入预算内(帧预算 16ms 的零头);回退即回归。
    #[test]
    fn fuzzy_query_within_frame_budget() {
        let reg = SlashCommandRegistry::with_builtin_commands();
        let start = std::time::Instant::now();
        // 100 次混合查询(空/前缀/子串/两词)取总耗时,避免单次抖动误判
        for i in 0..100 {
            let q = match i % 4 {
                0 => "",
                1 => "que",
                2 => "theme",
                _ => "quest pause",
            };
            std::hint::black_box(reg.fuzzy(q));
        }
        let total = start.elapsed();
        assert!(
            total < std::time::Duration::from_millis(100),
            "100 次 fuzzy 查询总耗时 {total:?} 超预算(单次均值应 ≪ 1ms)"
        );
    }
}
