# ADR-059: mcp-mesh 2PC WAL 持久化

## 状态

Proposed — 2026-07-30

## 上下文

`mcp-mesh` 的 2PC（两阶段提交）协调者原先为纯内存实现，事务状态（Prepare/Commit/Rollback）仅存于 RAM。当协调者进程崩溃后，所有进行中的事务状态丢失，导致：

1. **数据不一致**：Prepare 阶段已 ACK 的参与者等待 Commit 永不到来，资源锁定无法释放
2. **无法恢复**：重启后无法知道哪些事务需要 Commit、哪些需要 Rollback
3. **运维成本**：需人工介入排查参与者状态，手动提交或回滚

### 约束

- `#![forbid(unsafe_code)]` 合规（不引入 sqlite-vec 等需 unsafe binding 的依赖）
- 文件 IO 必须 `spawn_blocking` 包装（§4.4 反模式 #2：rusqlite/文件操作非 async）
- WAL 写入延迟不影响 2PC 主路径的关键超时（`transaction_timeout_ms`）
- 向后兼容：现有 `MeshConfig::default()` 行为可降级为 `durable: false` 禁用 WAL
- 依赖铁律：不引入新的外部 crate（用标准库 `std::fs` + `serde_json`）

## 决策

引入 WAL（Write-Ahead Log）持久化层，基于 JSONL（newline-delimited JSON）append-only 文件，为 2PC 协调者提供崩溃恢复能力。

### 1. WAL 文件格式：JSONL

采用 JSONL 而非 MessagePack 二进制：
- **人类可读**：便于排查崩溃现场（`cat mcp_mesh.wal` 即可查看）
- **天然按行分割**：`BufReader::lines()` 即可解析，无需自定义 framing
- **truncate 简单**：清空文件即可，无需重新编码
- **体积可忽略**：WAL 单条 entry < 200 字节（transaction_id 36 字节 + state + participants + timestamp）

### 2. WalEntry 结构

```rust
pub struct WalEntry {
    pub transaction_id: String,       // UUIDv7
    pub state: TransactionState,      // Prepare / Commit / Rollback
    pub participants_ack: Vec<String>, // 已 ACK 的参与者
    pub timestamp: u64,               // UNIX 毫秒
}
```

### 3. 写入时机（Write-Ahead）

遵循 Gray & Reuter《Transaction Processing》第 7 章的 WAL 原则：
1. **Prepare 成功后** → 写 Prepare entry（参与者已 ACK，崩溃后必须 Commit）
2. **Commit 成功后** → 写 Commit entry（事务终结）
3. **Rollback 完成后** → 写 Rollback entry（事务终结）

### 4. 崩溃恢复（recover_from_wal）

重启时扫描所有 WAL entry：
- **仅有 Prepare entry 的事务**：重新发起 Commit（参与者已 ACK，必须提交）
- **有 Commit/Rollback entry 的事务**：已完成，跳过
- 恢复完成后 `truncate()` 清空 WAL

### 5. fsync 策略

每次 `append` 都 `sync_all()`（fsync），确保崩溃不丢数据：
- 代价：~1-5ms 延迟（SSD）/ ~5-20ms（HDD）
- 2PC 事务串行执行，WAL 写入频率 = 事务频率，通常 < 100 TPS
- 如未来需高吞吐，可引入 group commit（批量 fsync）

### 6. 连接池失败回退

`TcpParticipantClient` 的连接池在复用已关闭连接时，自动回退到新建连接：
- 对端可能在上次请求后关闭连接（如 keep-alive 超时）
- 复用的 stream 首次 write/read 失败时，丢弃旧 stream，用新连接重试一次
- 避免因连接池缓存过期连接而误报网络错误

### 7. DNS rebinding 防御

`TcpParticipantClient::send_request_inner` 在 `connect` 前对 DNS 解析结果做二次校验：
- `register` 阶段只校验字面量 IP/已知内网域名
- `connect` 时通过 `tokio::net::lookup_host` 异步解析域名
- 对每个解析出的 IP 调用 `is_reserved_ip`，任一保留则拒绝（`DnsRebindingBlocked`）

### 8. 配置

```rust
pub struct MeshConfig {
    pub durable: bool,              // 默认 true（启用 WAL）
    pub wal_path: Option<PathBuf>,  // 默认 None（用 ~/.chimera/mcp_mesh.wal）
}
```

测试场景通过 `MeshConfig { durable: false, .. }` 禁用 WAL，避免文件 IO 影响性能断言。

## 影响

### 新增类型
- `WalEntry`：WAL 条目结构（transaction.rs）
- `WalStore`：WAL 持久化管理器（wal.rs）
- `PendingSync`：Lazy 同步缓冲条目（entanglement.rs）
- `QueryRequest` / `QueryResponse`：查询协议消息（participant_client.rs）

### 新增错误变体
- `McpError::WalIoError`：WAL 文件 IO 失败
- `McpError::CompensationFailed`：补偿事务重试耗尽
- `McpError::DnsRebindingBlocked`：DNS rebinding 防御拦截
- `McpError::EventBusPublish`：EntanglementManager Eager 同步失败

### 修改的 API
- `McpMesh::new` / `with_event_bus`：初始化 WAL store
- `McpMesh::prepare_phase` / `commit_phase`：写入 WAL entry
- `McpMesh::recover_from_wal()`：新增，崩溃恢复入口
- `McpMesh::reconcile_pending_transactions()`：新增，补偿事务重试
- `ParticipantClient` trait：新增 `query` 方法（超位置查询）
- `TcpParticipantClient`：新增连接池 + DNS rebinding 防御 + query 实现
- `EntanglementManager`：新增 `with_event_bus` / `sync_state_change` / `flush_lazy_buffer`
- `MeshConfig`：新增 `durable` / `wal_path` 字段
- `McpMeshTransactionCompleted` 事件：新增 `capability_id` 字段（csn-substitutor 关联降级链）

### 性能影响
- WAL 启用时：每次事务增加 ~1-5ms fsync 延迟（可接受，2PC 本身为低吞吐高一致性协议）
- WAL 禁用时：零开销（`wal_store = None`，所有 WAL 操作短路返回）
- 连接池：减少 TCP 握手开销（~1ms LAN / ~30ms WAN），失败自动回退

### 学术支撑
- Gray & Reuter《Transaction Processing》第 7 章：WAL 原则与 2PC 恢复
- Bernstein & Newcomer《Principles of Transaction Processing》：协调者崩溃恢复
- DNS rebinding 防御：OWASP DNS Rebinding 攻击向量文档

## 验证

- `cargo test -p mcp-mesh`：95 lib + 9 integration + 18 SSRF + 2 doctest 全绿
- `cargo clippy -p mcp-mesh --all-targets -- -D warnings`：零警告
- `cargo bench -p mcp-mesh --bench wal_recovery`：WAL 恢复延迟 < 100ms
