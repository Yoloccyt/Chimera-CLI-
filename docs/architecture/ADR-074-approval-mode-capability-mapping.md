# ADR-074:ApprovalMode × CapabilityToken 映射表(Concord W4 审批模式交互层)

- **状态**:Accepted(随 Concord 重构 W4 波次实施落地)
- **日期**:2026-08-11
- **决策者**:Concord 重构波 4 实施(《Chimera_CLI_TUI重构方案_v1.0》§4 Round 3 与 §7.4 键位表裁定)
- **相关**:ADR-034(禁用运行时 Feature Flag)、ADR-075(/ 第一公民)、ADR-076(双模式默认分层)
- **对应架构层**:L10 Interface(chimera-tui)× L4 Security 能力语义声明

## 背景(Context)

方案文档 P3:CapabilityToken 四态存在于能力层,但 TUI 无一键模式切换,用户无法在会话中
翻转"只读规划 ? 自动执行"。对标五家主流 Agent CLI(Claude Code Shift+Tab 三态、Kiro
Shift+Tab plan、Codex /mode + /plan、yottacode normal→auto→plan 循环 + 常驻 banner),
交互层需要审批模式表达。Round 3 结论:新增 ApprovalMode 三态(Normal/Plan/Auto),
Shift+Tab 循环 + statusline 常驻徽标;三态为 TuiState 会话字段,不进编译期。

## 决策(Decision)

### 1. 三态 × CapabilityToken 语义映射表

| ApprovalMode | 交互层语义 | CapabilityToken 映射语义 | Critical 9 类 mpsc 事件 |
|---|---|---|---|
| Normal(默认) | 执行前确认 | 对应 Provisional 档语义:每次执行类动作需确认通道在场 | 不豁免 |
| Plan | 只读规划:orchestrated/agent 命令被交互层诚实拦截,仅 instant 可执行 | 对应只读档语义:复用 quest checkpoint 机制表达"生成计划不执行" | 不豁免 |
| Auto | 自动执行档 | 对应 AutoApprove 档语义:常规动作免确认自动放行 | **不豁免**(SkepticVeto/RedTeamAudit/BudgetExceeded/AgentTaskFailed/AsaIntervention/AffinityQuotaExhausted/R2FreezeViolation/R2FreezeRollbackFailed/FormalViolation 九类旁路清单不动) |

### 2. 分层职责边界(依赖铁律合规)

- ApprovalMode 是 L10 交互层的**会话运行策略字段**(TuiState.approval_mode),
  仅承担表达与交互层拦截(Plan 态对 orchestrated/agent 命令诚实提示);
- 真实执行门控归 L4/L9 能力层(CapabilityToken/SecCore/Decay 机制),
  TUI 不越层伪造门控;映射表为**语义声明**,执行接缝在能力层既有机制中;
- `capability_policy_label()` 为纯表达函数(仅返回 i18n 键),零能力层依赖。

### 3. 正交性论证

- 与 RouterMode 正交:RouterMode 是输入语法状态机(Normal/Insert/Slash 等),
  ApprovalMode 是权限语义状态机,二者互不影响转移;
- 与 ViewMode 正交:Chat/Dashboard 是视图布局状态;唯一耦合点为键位分义
  (Chat 视图 Normal 态 Shift+Tab 循环审批模式,Dashboard 保留焦点环,方案 §7.4);
- 三态切换不改写任何 NexusEvent 变体,不触达 Critical mpsc 旁路清单。

### 4. ADR-034 合规声明

ApprovalMode 与 ViewMode 同为会话状态字段,非编译期/运行时 feature flag;
模式切换不启用/禁用任何代码路径的功能开关,仅改变交互层执行策略与表达。

## 后果(Consequences)

**正面**:
- 用户会话中一键触达权限/自主度翻转,对齐五家事实标准(P3 闭环);
- statusline 徽标三态色彩区分(Normal 绿/Plan 黄/Auto 红),模式状态常驻可见;
- Plan 模式"仅规划不执行"在交互层可验证(集成用例 plan_mode_blocks_orchestrated_command)。

**代价与边界**:
- Auto 档的执行层自动放行语义待能力层消费 approval 状态后完整闭环
  (当前 TUI 层为表达 + 交互拦截;接缝声明于本 ADR 映射表);
- Plan 态拦截限于斜杠命令 orchestrated/agent 两层;遗留命令栏直发路径
  (ParsedSlash::Legacy)不受限——该路径为 W2 兼容窗口产物(v2.27.0 移除后自然收敛)。

## 验证(Verification,2026-08-11)

- approval_mode.rs 状态机 8 单测(cycle 序/serde 往返/默认值/标签键/拦截谓词);
- approval_mode_test.rs 8 集成用例(Shift+Tab 按视图分流/Plan 拦截/instant 放行/
  徽标渲染/持久化往返/旧文件兼容);
- chimera-tui 全量 1336 passed / 0 failed。
