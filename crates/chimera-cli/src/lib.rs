//! CRATE-CONTRACT BEGIN
//! LAYER:    L10
//! ROLE:     Chimera CLI 鈥?NEXUS-OMEGA AI 缂栫爜浠ｇ悊鐨勫懡浠よ鍏ュ彛
//! BACKEND:  fs
//! PRODUCERS: 14 BudgetExceeded,ClaimIssued,ClaimRetracted,OperationTimedOut,ParentSelected鈥?
//! CONSUMERS: 0 -
//! MATURITY: NOT-IMPL-CN|PLACEHOLDER-EN
//! CRATE-CONTRACT END
//! Chimera CLI 鈥?NEXUS-OMEGA AI 缂栫爜浠ｇ悊鐨勫懡浠よ鍏ュ彛
//!
//! 瀵瑰簲鏋舵瀯灞?L10 Interface
//! 瀵瑰簲鍒涙柊鐐?鏃?鐢ㄦ埛浜や簰鍏ュ彛,浣嗘壙杞?Figment 澶氭簮閰嶇疆鍚堝苟)
//!
//! ## 妯″潡缁勭粐
//! - [`cli`][]:Clap 瀛愬懡浠ゅ畾涔?`Cli`銆乣Commands`銆乣QuestAction`銆乣ConfigAction`)
//! - [`config`][]:Figment 閰嶇疆鍔犺浇(`ChimeraConfig` 鍙婂婧愬悎骞堕€昏緫)
//! - [`commands`][]:鍚勫瓙鍛戒护鐨勪笟鍔￠鏋?`run`/`tui`/`quest`/`config`/`wiki`/`parliament`)
//!
//! ## 閰嶇疆浼樺厛绾?
//! Figment 鍚堝苟椤哄簭(鍚庤€呰鐩栧墠鑰?:
//! 1. 鍐呯疆榛樿鍊?`ChimeraConfig::default`)
//! 2. 閰嶇疆鏂囦欢(榛樿 `~/.chimera/omega.yaml`)
//! 3. 鐜鍙橀噺(鍓嶇紑 `CHIMERA_`,宓屽鐢?`__` 鍒嗛殧,濡?`CHIMERA_QUEST__MAX_TASKS_PER_QUEST`)
//!
//! `--config` 鍙€夋嫨閰嶇疆鏂囦欢璺緞锛屼笉杩涘叆 Figment锛圕LI 鍙傛暟涓嶆槸绗?4 婧愶級銆?
//!
//! ## 鐑姞杞芥柟妗?娉ㄩ噴璇存槑,楠ㄦ灦鏆備笉瀹炵幇)
//! 閰嶇疆鐑姞杞借鍒掗€氳繃涓ょ鏈哄埗瀹炵幇:
//! - **Unix**:鎹曡幏 `SIGHUP` 淇″彿,瑙﹀彂 `ChimeraConfig::load` 閲嶈浇
//! - **璺ㄥ钩鍙?*:浣跨敤 `notify` crate 鐩戝惉 `omega.yaml` 鏂囦欢鍙樻洿,debounce 500ms 鍚庨噸杞?
//!
//!   閲嶈浇鍚庨€氳繃 `event-bus` 骞挎挱 `ConfigReloaded` 浜嬩欢,鍚勫瓙绯荤粺璁㈤槄骞跺簲鐢ㄦ柊閰嶇疆銆?
//!   褰撳墠 Week 8 宸插畬鎴愰潤鎬佸姞杞?鐑姞杞戒负鏈潵澧炲己椤?浼樺厛绾?P3)銆?
//!
//! # 蹇€熺ず渚?
//! WHY 閫夋绀轰緥:灞曠ず鏈€甯哥敤璺緞 鈥斺€?`Cli::parse_from` 瑙ｆ瀽鍙傛暟 + `ChimeraConfig::default` 鍐呯疆榛樿,
//! 瑕嗙洊 CLI 鍏ュ彛涓庨厤缃姞杞戒袱鏉℃牳蹇?API,涓旀棤闇€ IO 鍙湪 doctest 鐩存帴杩愯銆?
//! ```
//! use chimera_cli::{Cli, Commands, QuestAction, ChimeraConfig};
//! use clap::Parser;
//!
//! // Cli 瀹炵幇 clap::Parser,鍙粠瀛楃涓插垏鐗囪В鏋?渚夸簬娴嬭瘯涓庤剼鏈皟鐢?
//! let cli = Cli::parse_from(["chimera", "quest", "list"]);
//! // Quest 鍙樹綋鍚瓙鍛戒护绾?`json` flag(Task 1.7 寮曞叆,涓庡叏灞€ --json 鍏煎鍥為€€),
//! // matches! 妯″紡鐢?`..` 蹇界暐鍏朵粬瀛楁,淇濇寔 doctest 瀵瑰瓧娈垫墿灞曠殑鍋ュ．鎬?
//! assert!(matches!(cli.command, Some(Commands::Quest { action: QuestAction::List, .. })));
//!
//! // ChimeraConfig 瀹炵幇 Default,鎻愪緵鍐呯疆鍏滃簳閰嶇疆(瀵瑰簲 omega.yaml 缂虹渷鍊?
//! let _config = ChimeraConfig::default();
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

/// Action 缂栨帓鍣?娑堣垂 TuiActionRequested,璺敱 quest.* 鍩熷埌 QuestEngine 鐪熷疄鎵ц)
pub mod action_orchestrator;
/// 鍚姩 banner 杈撳嚭(鍝佺墝 ASCII art,鍙€氳繃 `--no-banner` 鍏抽棴)
pub mod banner;
/// `/bench` 涓€鏈燂細build.rs 闈欐€佽鏁?provider锛堜緵 commands::tui 瑁呴厤 BenchPanel锛夈€?
mod bench_provider;
/// 缁熶竴璋冪敤棰勭畻鍖呰 鈥?瓒呮椂 + CancellationToken 娉ㄥ叆(M2,M1-鏂瑰悜5:
/// chat/quest 涓昏矾寰?LLM/闀挎搷浣滅殑鏈眰鍏滃簳闃叉姢,瓒呮椂鍙?OperationTimedOut 浜嬩欢)
pub mod call_budget;
/// Clap 瀛愬懡浠ゅ畾涔?
pub mod cli;
/// 瀛愬懡浠や笟鍔￠鏋?
pub mod commands;
/// 闆嗕腑缁勫悎鏍?鈥?AppContext/鍗忚瀹夸富鐪熷疄瑁呴厤锛圕12 + C1锛岃瘎瀹℃尝娆?1锛?
pub mod composition;
/// Figment 澶氭簮閰嶇疆鍔犺浇
pub mod config;
/// 鍚屾 DB 鎵撳紑鐨勭粺涓€鍑哄彛(Phase 6A/搂4.4 绾㈢嚎 2:rusqlite open 蹇呰蛋 blocking 姹?
pub mod db_open;
/// CLI 閿欒绫诲瀷(缁撴瀯鍖栭敊璇爜,Task 0.2 灏嗘嵁姝ゆ槧灏?ExitCode 鐭╅樀)
pub mod error;
/// `exec --plan` 宸ュ叿鎵ц鍐呮牳鎺ョ嚎锛圗1鈥诧細鍐呯疆宸ュ叿 + 鍥涚幆閾?+ 涓夊眰瓒呮椂 + Scc 鍥炲啓锛?
pub mod exec_plan;
/// 搂16.1 缁忛獙鍗＄墖闂幆缁勫悎鏍规帴绾?Phase 10 瀹¤淇 Wave 1:
/// ExperienceCardBus 涓婚摼 + L3 鍙屾祦鎸佷箙鍖?+ L2/L6 娑堣垂 + 浜旂淮瀹¤)
pub mod experience_loop;
/// TUI 鈫?缂栨帓鍣ㄥ崗璁彙鎵嬫湇鍔＄(Concord W10,ADR-082:TuiHello 搴旂瓟 + SEC-4 涓€娆℃€?
pub mod handshake;
/// LLM 宸ュ叿寰幆(Phase 7,ADR-232:妯″瀷 鈫?宸ュ叿澶氳疆闂幆,榛樿鍏抽棴)
pub mod llm;
/// McpToolExecutor 鈥?`mcp.<server>.<tool>` 鎺ュ叆鍥涚幆浼犺緭鐜?ADR-256:澶嶅悎鎵ц鍣?
/// 鍓嶇紑鍒嗘淳 + 缁勫悎鏍瑰敮涓€瑁呴厤;浼犺緭鎭?InProcessClient 闆剁綉缁滃壇浣滅敤)
pub mod mcp_tool_executor;
/// 鎵归噺褰掑洜鎸囨爣 Sink(W3/F-03/ADR-248 C-A:TelemetryBatchSink 鈫?BusLogger 璁℃暟鍣?
pub mod metrics_sink;
/// Quest 鍒嗚В缂栨帓鍣?娑堣垂 TuiChatSubmitted,缁?QuestEngine 鐪熷疄鍒嗚В骞舵祦寮忓洖鍙?
pub mod orchestrator;
/// P1 鍊欓€夆憿锛圓DR-211 Accepted锛夛細`agent spawn` 鍓?mas-sched claim 鍑嗗叆
/// 锛坋nv 涓夋€?`CHIMERA_SCHED_CLAIM`锛宱ff 榛樿閫愪綅绛変环锛?
/// + `ClaimIssued`/`ClaimRetracted` 鐢熶骇鍙戝嚭鐐癸紙Normal/Parliament Topic锛夈€?
mod sched_claim;
// PROBE P3.2: 瓒呯獥鍏滃簳妗ワ紙kvbsr鈫抮epo-wiki鈫抙cw 涓ょ骇妫€绱㈢湡瀹為摼璺級
pub mod overwindow_bridge;
// PROBE P2.2: 閫夋嫨鍣ㄥ涔犵紪鎺掑櫒锛圫4 鈫?holder 鈫?window 娉ㄥ叆閾撅級
/// 缁熶竴杈撳嚭 helper(Task 1.7 JSON / Task 1.12 褰╄壊 + 琛ㄦ牸 + 杩涘害)
pub mod output;
/// Permission prompt 鏈哄埗(Task 1.11)
pub mod permission;
/// Quest 鎵ц闈?Phase 5/ADR-231):浠诲姟绾х绾?+ DAG 灏辩华闃熷垪 + 宸ュ叿璁″垝鎵ц
pub mod quest_exec;
/// Quest 鐢熷懡鍛ㄦ湡缁勪欢妗?Phase 10 瀹¤淇 Wave 2:
/// LongTaskMap/SearchTreeManager/LongTermCreditAssigner 浜嬩欢妗ユ帴瑁呴厤)
pub mod quest_loop;
// Milestone C-3: rl-client 閫氶亾锛圚ttpRlClient feature-gated锛孨oopRlClient 甯搁┗锛?
pub mod rl_client;
pub mod selector_orchestrator;
/// 鍛戒护瑁呴厤鍒嗙被(缂栬瘧鏈熺┓灏?鍐冲畾鏄惁瑁呴厤 AppContext)
pub mod spec;
/// Tool execution plane 鐨?L10 provider 閫傞厤鍣?+ env 閰嶇疆锛圥hase 3锛?
pub mod tool_plane;

// === 鍏紑 API 閲嶅鍑?===
pub use cli::{Cli, Commands, ConfigAction, QuestAction};
pub use config::{ChimeraConfig, LazyConfig};
pub use error::ChimeraCliError;

/// Crate 鐗堟湰(浠?workspace.package.version 娲剧敓)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
