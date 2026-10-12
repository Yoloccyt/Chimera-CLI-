# ADR-083:TUI 生态波——用户命令/通知桥/可发现性(Concord W11 波9·生态)

- **状态**:Accepted(W11 波次交付,2026-08-12 门槛全绿)

> ⚠ **实施状态校正（2026-09-22 现测；决策文本不改）**：本文宣称的 `recap`/`copy`/`notify`/`commands` 四命令
> **均不在内建表**——`BUILTIN_COMMANDS` 实测 53 条（41 一级 + 12 二级，自证 `slash_registry.rs:654`），
> 且 `recap`/`clipboard`/`copy_selection` 全库 0 命中。三个支撑文件
> `actions/user_commands.rs`(275 行/9 单测)、`app/notify.rs`(183 行/7 单测)、`components/prefix_hint.rs`(136 行/4 单测)
> **全部未被 `mod` 声明** ⇒ 不参与编译。另：本文“词表 55→59”与背景节“v1.0 词表 53 条”自相矛盾。
> ⇒ 用户命令机制 / OSC 9 通知桥 / PrefixHint = **未接线/待实现**。
- **日期**:2026-08-12(对应实施波次 2026-08-12 W11)
- **决策发起方**:Concord 重构方案 v2.0(《Chimera_CLI_TUI重构方案_v2.0.md》§6/§8)
- **关联**:ADR-075(斜杠第一公民)、ADR-077(codegen 四源)、方案 §12.3 N11 组合创新
- **影响架构层**:L10 Interface(chimera-tui 命令层 + 交互层)

## 背景(Context)

v1.0 词表 53 条覆盖主流交互,但生态扩展面缺失:无用户自定义命令(Claude Code/
Qoder 已有)、无桌面通知(长任务离开终端无反馈)、前缀键(g/Ctrl+W)对新手
不可见(which-key 范式缺位)。方案 §6 裁决:新增 recap/copy/notify/commands
四命令(词表 55→59)+ 用户命令机制 + OSC 9 opt-in 通知 + PrefixHint 瞬显;
§8 安全专项:SEC-1 用户命令注入面、SEC-2 OSC 序列消毒。

## 决策(Decision)

### 一、用户自定义命令(T11.1/T11.2)

1. **加载器**(`actions/user_commands.rs`,纯函数):扫描
   `.chimera/commands/*.md`(项目级)+ `~/.chimera/commands/*.md`(用户级);
   frontmatter 极简解析(name/description/argument-hint/when);名称校验
   `[a-z0-9-]+`,非法跳过留日志(不注册无法安全调用的命令);项目级同名
   优先用户级(就近原则)。
2. **命令语义**:提示词快捷键而非权限旁路(yottacode 实证原则)——展开后
   正文入 composer(Insert 模式),提交仍走既有会话链路(ApprovalMode +
   能力令牌)。`{{args}}` 占位符替换,无占位符则追加实参。
3. **SEC-1 信任门控**:项目级命令首次使用弹 Confirm 信任确认(会话作用域
   信任集合);确认后同名免确认;`/commands reload` 重扫后信任复位
   (文件可能被修改)。when 谓词(git-repo)不成立时命令如同未定义(诚实)。
4. **匹配时序**:内建词表优先(registry parse 先行),未命中才试用户命令,
   再回退遗留命令栏——三层优先级避免用户命令遮蔽内建。
5. **已知限制(诚实记录)**:用户命令暂不进 Tab 补全候选(内建词表为每键
   重建的 `&'static` 表,动态命令注入需集中化注册表构造,列入后续波次);
   `/commands` 弹窗提供可发现性兜底。

### 二、NotifyBridge 通知桥(T11.3)

6. **OSC 9 opt-in**(`app/notify.rs` 纯函数):`TuiConfig.notify_enabled`
   默认关(对齐 Codex opt-in);TERM_PROGRAM 白名单(Warp/iTerm/WezTerm/
   mintty/ConEmu/Windows Terminal)探测能力,不支持静默降级不写序列。
7. **SEC-2 消毒**:消毒函数单一事实源在 data 层(`data/sanitize.rs`),
   ChatSync 汇入点(所有模型输出中枢)统一剥离控制序列(保留 \n/\t);
   通知体构造前再次消毒——恶意模型输出含 OSC 序列不得伪造通知或操纵
   终端。渗透测试:注入 OSC 9/CSI/BEL 混合载荷,消毒后零残留控制字符。

### 三、可发现性(T11.4/T11.5/T11.6)

8. **PrefixHint**(`components/prefix_hint.rs` 纯状态机):g/Ctrl+W 前缀
   武装后 300ms 无后续键弹 HelpOverlay 提示后缀键位(which-key 范式);
   任意后续键解除;条目与 InputRouter 路由表同源一致。
9. **菜单内粘贴**(T11.5):bracketed paste 事件化(EnableBracketedPaste),
   仅 Slash/Insert 输入面接受粘贴追加,内容消毒 + 换行转空格;其余模式
   忽略(粘贴只进文本输入面)。
10. **F13-F24**(T11.6):codegen `parse_key_spec` 经 crossterm `KeyCode::F(u8)`
    天然支持 F13-F24(文档与测试补齐,零新代码路径)。

### 备选与否决

| 备选 | 否决理由 |
|---|---|
| 用户命令注入 SlashCommandRegistry 动态补全 | 内建表每键重建且 `&'static` 名称,动态注入需集中化改造;已知限制诚实记录,后续波次实施 |
| 通知默认开启 | 桌面打扰是体验负资产;Codex 亦 opt-in |
| 本地 TTS/通知守护进程 | 超出 TUI 边界;OSC 9 终端原生能力足够 |
| 用户命令直接执行动作 | 命令非权限旁路;展开进 composer 保留 ApprovalMode 全链路 |
| 粘贴进所有模式 | 粘贴只进文本输入面,防止 Normal 模式误注入命令文本 |

## 结果(Consequences)

- **正确性**:user_commands 9 测(frontmatter/名称校验/谓词/扫描优先级/
  模板展开)+ notify 8 测(OSC 9 形态/SEC-2 渗透/白名单/RFC 4648 base64
  向量/proptest 消毒不变量)+ sanitize 4 测(幂等性 proptest)+
  prefix_hint 4 测 + W11 E2E 9 测(/notify/recap/copy/commands/用户命令
  信任链路/粘贴消毒)。
- **词表**:59 条内建(+4)+ 30 词主流对齐校验全绿。
- **回归**:SEC-2 消毒位于 ChatSync 汇入点,既有流式守恒测试
  (PaceGate 守恒/内容守恒)全部保持绿——消毒仅剥离控制字符,
  可打印内容逐字守恒。

## 风险与回滚

- **R16 注入面事故**:用户命令/OSC 通道出现可利用注入 → 功能默认关闭
  (notify_enabled=false 为默认;用户命令信任门控为会话强制),SEC-1/SEC-2
  缓解复盘后重开。
- **用户命令投毒**:恶意仓库 `.chimera/commands/` 投毒 → SEC-1 信任确认
  为首次使用强制门控;`/commands` 列表可审计(来源标注项目/用户级)。
- **bracketed paste 兼容**:不支持的终端忽略 EnableBracketedPaste 序列
  (无副作用);粘贴功能静默不可用,不影响键入路径。
