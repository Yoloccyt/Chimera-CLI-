//! CRATE-CONTRACT BEGIN
//! LAYER:    L9
//! ROLE:     闀挎湡浠诲姟寮曟搸 鈥?Quest 鍒嗚В銆佹鏌ョ偣鎸佷箙鍖栦笌鎬濊€冩ā寮忔不鐞?
//! BACKEND:  fs
//! PRODUCERS: 12 CheckpointLoaded,CheckpointSaved,CoordinationRatioReported,ExecutionCompleted,NexusStateChanged鈥?
//! CONSUMERS: 2 chimera-cli,nexus-app-server
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! 闀挎湡浠诲姟寮曟搸 鈥?Quest 鍒嗚В銆佹鏌ョ偣鎸佷箙鍖栦笌鎬濊€冩ā寮忔不鐞?
//!
//! 瀵瑰簲鏋舵瀯灞?L9 Quest
//! 瀵瑰簲鍒涙柊鐐?TTG(Thinking Toggle Governance)+ LHQP(Long-Horizon Quest Persistence)
//!
//! # 鏍稿績鑱岃矗
//! - 浠?`UserIntent` 鍒嗚В浠诲姟鍥?DAG),鏍￠獙鏃犵幆鍚庡垱寤?Quest
//! - 缁存姢 Task 鐘舵€佹満(Pending鈫扲unning鈫扖ompleted/Failed),骞挎挱杩涘害浜嬩欢
//! - TTG 鑷姩鎬濊€冩ā寮忔不鐞?鍩轰簬 Quest 澶嶆潅搴︿笌 DECB 棰勭畻妗ｄ綅鑷姩閫夋嫨 Fast/Standard/Deep
//! - 妯″紡鍒囨崲閫氳繃 EventBus 鍙戝竷 ThinkingModeSwitched 浜嬩欢,渚?Parliament 绛変笅娓歌闃?
//! - 瀹屾垚 Quest 鏃跺箍鎾?ExecutionCompleted 浜嬩欢
//! - LHQP 妫€鏌ョ偣鎸佷箙鍖?Quest 鐘舵€佸簭鍒楀寲涓?MessagePack 钀界洏,宕╂簝鍚庡彲鎭㈠
//!
//! # 蹇€熺ず渚?
//! ```
//! use event_bus::EventBus;
//! use nexus_core::{UserIntent, MultimodalInput};
//! use quest_engine::QuestEngine;
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let engine = QuestEngine::new(bus);
//! let intent = UserIntent {
//!     intent_id: "i-1".into(),
//!     raw_text: "鍒嗘瀽闇€姹傘€傝璁℃柟妗堛€?.into(),
//!     multimodal_inputs: vec![MultimodalInput::Text("...".into())],
//!     risk_level: 10,
//! };
//! let quest = engine.create_quest(intent).await.unwrap();
//! assert_eq!(quest.tasks.len(), 2);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod ambient_mode;
pub mod arbitration;
pub mod checkpoint;
pub mod config;
pub mod control;
pub mod coordination_metrics;
pub mod dag;
/// WP-E(R7 鎵规): 浠诲姟渚濊禆鎺ㄦ柇绛栫暐鎺ュ彛(鏂规 C:绛栫暐 + 閰嶇疆寮€鍏?榛樿 Linear)
pub mod decompose;
pub mod engine;
pub mod error;
/// Phase 9 搂14.2: 闀夸换鍔″湴鍥撅紙TencentDB 鏈哄埗锛氱煭鎽樿鍏ヤ笂涓嬫枃 + 璇︽儏澶栫疆 + 鍦板浘娉ㄥ叆锛?
pub mod long_task_map;
/// Phase 9 浜屾瀹℃煡澧炲己 搂14.4: 闀挎椂绋嬩俊鐢ㄥ垎閰嶅櫒锛圫HARP 缁熻鐗堟椂闂寸淮搴︼級
pub mod long_term_credit;
/// Phase 9 浜屾瀹℃煡澧炲己: L2 璁板繂灞傚崗鍚屾帴鍙ｏ紙MemorySyncHook 渚濊禆鍊掔疆锛?
pub mod memory_sync_hook;
pub mod metrics_sync;
/// Phase 9 搂14.1: OpenMLE 鎼滅储鏍戠鐞嗗櫒锛堢粡楠屽崱鐗囪繘鍖栨爲锛屼笌 dag.rs 浠诲姟 DAG 璇箟鍒嗙锛?
pub mod search_tree;

// P3-T13c: Loop 缁堟璁板垎鍗★紙v4.0 WI-32:鏀舵暃脳杈归檯脳閰嶉涓夌淮鎵撳垎锛?
pub mod scorecard;
/// P1-1: NexusStateChanged EventBus 閫傞厤鍣?L1 Core 娣卞害浼樺寲,渚濊禆鍊掔疆鎺ョ嚎)
pub mod state_listener;
pub mod trajectory_exporter;
pub mod ttg;
pub mod types;
pub mod wal;

// === 鍏抽敭绫诲瀷閲嶅鍑?绠€鍖栧閮ㄥ鍏?===
pub use arbitration::ArbitrationLayer;
// Milestone B-2: Ambient Mode锛堝悗鍙板父椹诲畧鎶ゅ惊鐜?璧勬簮鐪嬮棬鐙?璁板繂鏁寸悊/妫€鏌ョ偣璋冨害锛?
pub use ambient_mode::{
    spawn_ambient_subscriber, spawn_ambient_subscriber_with_receivers, AmbientModeConfig,
    MemoryTidyHook, NoopTidyHook,
};
// 鍗槦鎬佹帴绾垮伐浣滃寘:妫€鏌ョ偣鍏宠仈缁勪欢鎬佸揩鐓?tree/map bytes)鍏紑 API
pub use checkpoint::{CheckpointManager, SatelliteState};
pub use config::{DagMode, QuestConfig};
// WP-E(R7 鎵规): 渚濊禆鎺ㄦ柇绛栫暐鍏紑 API(鏂规 C,chimera-cli run --execute 娉㈡寮€鍏虫秷璐?DependencyMode)
pub use decompose::{
    dependency_inference, DependencyInference, DependencyMode, HeuristicDag, LinearChain,
};
// P2-1: 鍗忚皟鎴愭湰/鎺ㄧ悊澧炵泭姣斿€煎害閲?涓夐噸鎮栬鎺ㄧ悊鎮栬绾㈢嚎)
pub use control::{
    handle_control_event, spawn_control_subscriber, spawn_control_subscriber_with_receiver,
};
pub use coordination_metrics::{
    CoordinationCostSample, CoordinationMetricsCollector, CoordinationMetricsConfig,
    CoordinationToGainRatio, InferenceGainSample,
};
pub use engine::QuestEngine;
pub use error::QuestError;
// Phase 9 搂14.2: 闀夸换鍔″湴鍥惧叕寮€ API 閲嶅鍑?
pub use long_task_map::{
    ExternalStorage, InMemoryExternalStorage, LongTaskMap, NodeStatus, StepResult, TaskEdge,
    TaskMapRef, TaskNode,
};
// Phase 9 搂14.1: OpenMLE 鎼滅储鏍戝叕寮€ API 閲嶅鍑?
pub use search_tree::{SearchTreeManager, TreeError, TreeStats};
// Phase 9 浜屾瀹℃煡澧炲己 搂14.4: 闀挎椂绋嬩俊鐢ㄥ垎閰嶅櫒鍏紑 API 閲嶅鍑?
pub use long_term_credit::{CreditAssignment, CreditStep, LongTermCreditAssigner};
// Phase 9 浜屾瀹℃煡澧炲己: L2 璁板繂灞傚崗鍚屾帴鍙ｅ叕寮€ API 閲嶅鍑?
pub use memory_sync_hook::{MemorySyncHook, NoopMemorySyncHook};
// 鍗忚皟搴﹂噺鎺ョ嚎闂幆:寰呭悎骞舵牱鏈笌瑙傛祴浜嬩欢璁㈤槄鍣?
// (娑堣垂 DebateCompleted / DelegationCompleted,濉厖鍗忚皟鎴愭湰 Option 瀛楁)
pub use metrics_sync::{
    spawn_metrics_subscriber, spawn_metrics_subscriber_with_receiver, PendingCoordSample,
};
// P1-1: NexusState 鐘舵€佸彉鏇?鈫?NexusStateChanged 浜嬩欢閫傞厤鍣?
pub use state_listener::{BusStateChangeListener, STATE_LISTENER_SOURCE};
// P4-W16.1.3: Quest 杞ㄨ抗瀵煎嚭鍣ㄧ被鍨嬮噸瀵煎嚭(渚?L5 omega-learner / L9 efficiency-monitor 浣跨敤)
pub use trajectory_exporter::{
    export_trajectory, export_trajectory_from_quest, ContextSummary, QuestTrajectory, TaskProgress,
    TrajectoryAction, TrajectoryReward, TrajectoryState,
};
pub use ttg::{ComplexityScore, ModeSwitchReason, TtgConfig, TtgGovernor};
pub use types::{CheckpointMeta, TaskResult};

/// 榛樿 Quest 閰嶇疆
///
/// 鐢熶骇鎺ㄨ崘鍊?
/// - max_tasks_per_quest: 16(涓?GQEP 鎵瑰鐞嗙獥鍙ｅ榻?
/// - checkpoint_interval: 3(姣?3 涓?Task 瀹屾垚瑙﹀彂妫€鏌ョ偣)
pub fn default_config() -> QuestConfig {
    QuestConfig::default()
}
