# ADR-058: RAG 混合检索融合（Reciprocal Rank Fusion）

## 状态

Proposed — 2026-07-30

## 上下文

`repo-wiki` 中 HNSW（dense 向量检索）与 FTS5（sparse 全文检索）并行存在，但无显式融合机制。两路检索结果各自独立返回，由调用方决定使用哪条路径，导致：

1. **召回率受限**：HNSW 可能漏掉语义不相似但关键词精确匹配的文档；FTS5 可能漏掉语义相似但用词不同的文档
2. **无统一排序**：两路结果的分数尺度不同（HNSW 是余弦距离，FTS5 是 BM25/TF-IDF），无法直接比较
3. **用户体验**：调用方需要自行决定使用哪路检索，增加复杂度

### 约束

- 不引入新的外部依赖（RRF 算法简单，可内联实现）
- `#![forbid(unsafe_code)]` 合规
- Top-K 用 `select_nth_unstable`（禁止 sort_by 做 Top-K）
- 降级路径：FTS5 不可用时仅返回 HNSW 结果，不阻断

## 决策

引入 Reciprocal Rank Fusion (RRF) 算法融合 HNSW + FTS5 检索结果。

### RRF 公式

```
score(d) = Σ w_i × 1/(k + rank_i(d))
```

- `k`：平滑参数（默认 60），k 越大对低排名越宽容
- `rank_i(d)`：文档 d 在第 i 路检索中的排名（1-based）
- `w_i`：第 i 路检索的权重（默认 dense=1.0, sparse=1.0）

### 关键设计

1. **纯函数实现**：`rrf_fuse` 是纯函数，接受两路结果（doc_id 列表），返回融合后的 `Vec<HybridSearchResult>`，无锁无 await。

2. **Top-K 用 select_nth_unstable**：融合后排序取 Top-K 使用 `select_nth_unstable_by`（O(n)），仅对 Top-K 部分排序（K log K）。

3. **去重合并**：同一 doc_id 在两路中出现时，RRF 分数累加，dense_rank 和 sparse_rank 分别记录。

4. **配置化**：`HybridSearchConfig`（rrf_k / dense_weight / sparse_weight）集成到 `WikiConfig`，`#[serde(default)]` 向后兼容。

5. **降级路径**：
   - FTS5 不可用（`FtsAvailability::Unavailable`）→ 仅返回 HNSW 结果
   - HNSW 为空 → 仅返回 FTS5 结果
   - 两路均空 → 返回空 Vec（不 panic）

## 影响

- 新增 `crates/repo-wiki/src/search.rs`（hybrid_search + rrf_fuse + HybridSearchConfig + HybridSearchResult）
- 修改 `crates/repo-wiki/src/lib.rs`（模块声明 + prelude 重导出）
- 修改 `crates/repo-wiki/src/types.rs`（WikiConfig 新增 hybrid_search 字段）
- 修改 `crates/repo-wiki/src/config.rs`（builder 方法 + 配置测试）

## 验证

- `cargo test -p repo-wiki` 全部通过（9 个 search_tests + 既有测试）
- `cargo clippy -p repo-wiki --all-targets -- -D warnings` 零警告
- 降级路径全部覆盖（仅 dense / 仅 sparse / 两路均空）
- RRF 分数累积验证（同一文档在两路中出现时分数合并）
