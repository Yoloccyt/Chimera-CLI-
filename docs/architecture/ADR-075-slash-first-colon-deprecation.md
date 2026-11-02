# ADR-075:`/` 命令第一公民与 `:` 退役计划(Concord W2 命令翻转)

- **状态**:Accepted(随 Concord 重构 W2 波次实施落地)
- **日期**:2026-08-10
- **决策者**:Concord 重构波 2 实施(《Chimera_CLI_TUI重构方案_v1.0》§15.3 提案编号续写)
- **相关**:ADR-029(TUI v3 交互重构/ActionRegistry)、ADR-034(禁运行时 Feature Flag)、ADR-077(四源 codegen)
- **对应架构层**:L10 Interface(chimera-tui)

## 背景(Context)

重构方案 §2 P1 诊断:TUI 主命令入口为 vi 式 `:` 命令条,与主流五家 Agent CLI
(Codex/Claude Code/opencode/Qoder/Hermes)的 `/` 斜杠第一公民语义全面错位,
新用户第一分钟学习成本最高。同时 `/` 键原承担全局关键字过滤(EnterSearch),
翻转前必须解决搜索语义的去处,且既有 `:` 命令栏的遗留文本命令(面板切换/
quit/refresh/find/filter/level/pause/resume/vote/quest 子命令)不得断裂。

## 决策(Decision)

1. **`/` 翻转为命令第一入口**:`/` 与 `:` 同路由到 `EnterSlash`(新 RouteTarget),
   进入 `InputMode::Slash` 斜杠命令模式;补全列表(SlashCommandSurface)随输入
   实时过滤,Tab 前缀补全,Enter 三分层执行。
2. **搜索语义迁移**:原 EnterSearch 由 `/search <kw>` instant 命令承接(空参清除),
   `RouteTarget::EnterSearch` 与 `EnterCommandBar` 两个目标删除(内部 API,无外部兼容负担)。
3. **`:` 一版本退役窗口(R1 缓解)**:`:` 保留至 v2.27.0;首次经 `:` 进入时展示
   一次性弃用提示(`status.colon_deprecated`,`TuiState.colon_deprecation_shown`
   去重);两入口行为完全一致,不重复打扰。
4. **统一解析 + 遗留回退(零功能断裂)**:提交时先查 SlashCommandRegistry
   (53 命令,精确 + 别名);未命中回退 `CommandPalette::parse_legacy`(原
   parse_command),遗留命令全部保留。冲突消解:`/resume`(会话域)与遗留
   `resume <quest-id>` 同名——带参桥接遗留路径,无参保持会话语义。
5. **三分层执行**:instant 本地效果(主题/布局/面板/过滤/locale/帮助/状态/
   自检/退出);orchestrated 经既有 M4 通道(quest 控制类走 TuiCommand 保留
   确认弹窗);agent 提示词模板预置进 composer。已登记未接线的命令给诚实
   反馈(`slash.todo`),不伪造功能。

## 备选与否决

| 备选 | 否决理由 |
|---|---|
| `:`/`/` 双入口长期并存 | 双心智双文档,P5 型漂移重演(方案 §4 Round 1) |
| `/` 智能双用(命令/搜索分流) | 单键双义交互歧义,与主流语义不符 |
| 移除搜索功能 | 违反"不丢功能"原则,Log/Quest 面板过滤能力受损 |
| 立即废弃 `:` 遗留命令 | 存量使用者功能断裂;一版本窗口 + 回退是更稳的迁移路径 |

## 后果(Consequences)

- 交互表层与主流五家同构,迁移零成本;观测驾驶舱等差异化资产不受影响。
- `InputMode`/`RouterMode` 新增 Slash 变体为纯机械路由态,无业务逻辑;
  ADR-034 合规(无运行时 feature flag)。
- v2.27.0 计划移除 `:` 入口与 parse_legacy 回退通道(届时遗留命令需完成
  斜杠化对齐或明示废弃);词表防退化由 `scripts/check_slash_vocab.{ps1,sh}`
  与 i18n 双语断言守护。

## 验证记录

- `tests/slash_command_test.rs` 15 用例(双入口/弃用提示一次性/三分层/回退/补全/渲染);
- 旧语义测试 9 处同步改写;chimera-tui 全量 1282 passed / 0 failed(2026-08-10);
- 词表抽查脚本 EXIT=0(24 条主流同名命令 tier 全对齐)。
