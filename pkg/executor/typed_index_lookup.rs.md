# `pkg/executor/typed_index_lookup.rs`

## 文件定位

[`typed_index_lookup.rs`](typed_index_lookup.rs) 位于 `astersql-executor` crate（见 [`Cargo.toml`](Cargo.toml) 的 `[package]` 与 `[lib]`），实现 Rust typed physical plan 的索引双读执行器 `TypedIndexLookUp`。模块由 [`lib.rs`](lib.rs) 公开为 `typed_index_lookup`，其唯一生产构造点是 [`builder.rs`](builder.rs) 中 `build_typed_physical_plan` 对 `PhysicalIndexLookUpReader` 的分支：先构造负责表记录解码的 `TypedKVScan`，再创建本执行器，并在外层按物理计划包裹索引侧/表侧选择、其他 pushdown 节点和 `TypedLimit`。

它处于“物理索引范围和 KV 读取绑定已经准备好”到“向上游逐 chunk 返回表记录”之间：索引 KV 只用于恢复 row handle，真正输出的列来自相应 record KV。它不是 Go `IndexLookUpExecutor` 的逐字段复刻，而是 Rust typed 执行链上的同步、惰性实现。

## 核心职责

- 按编码索引键顺序扫描一组半开 `KeyRange`，并支持升序、降序遍历；`new` 会先按方向稳定地排列范围。
- 对每条索引 KV 调用 `astersql_tablecodec::DecodeIndexHandle` 恢复行句柄；若为全局索引的 `kv::PartitionHandle`，使用其中的 `PartitionID` 构造真实分区 record key，否则使用构造时的 `table_id`。
- 通过同一 `kv::Retriever` 对 record key 做点读，并委托 `TypedKVScan::append_decoded_row_for_table` 校验表 ID、解码列值并追加到输出 chunk。
- 保存跨 `Next` 调用的范围位置与游标，保证分页继续而不是重复从范围起点读取；同时累计 `scanned_rows`。
- 为悲观 `SELECT FOR UPDATE` 暴露当前页的规范 record keys；[`adapter.rs`](adapter.rs) 的 `ExecStmt::runPessimisticSelectForUpdate` 会在每页返回后通过 `TakeLockKeys` 消费这些键并调用会话的 `LockKeys`。
- 通过 `NextWithContext` 在开始、范围切换、迭代循环和结束位置检查 `ExecutionContext.sql_killer`，让惰性 KV 循环可被中断。

## 主要符号

- `pub struct TypedIndexLookUp`：唯一生产类型。不可变配置包括 `retriever`、`record_decoder`、`index_columns`、`table_id`、`descending` 和排序后的 `ranges`；可变执行状态包括 `range_index`、`cursor`、`page_keys`、`scanned_rows`、`opened`、`closed`。
- `pub fn new(...) -> Self`：接收共享的 `Arc<dyn kv::Retriever + Send + Sync>`、记录解码器、索引列数、逻辑/物理表 ID、方向和 KV 范围。升序按 `(start, end)` 排列范围，降序按 `(end, start)` 逆序排列。
- `fn next_inner(&mut self, context: Option<&ExecutionContext>, output: &mut chunk::Chunk) -> AdapterResult`：核心状态机；`Next` 和 `NextWithContext` 都委托到此处。
- `impl ExecExecutor for TypedIndexLookUp`：实现 typed 执行器生命周期、chunk/schema 转发、锁键和扫描计数，以及 `Detach`。`CalculateNoDelay`、`IsWriteExecutor` 和外键相关方法表明它是流式只读执行器且自身不产生外键级联。
- `TakeLockKeys`：用 `std::mem::take` 转移当前页的 `page_keys`，所以同一批锁键只会被消费一次。
- `Detach`：克隆共享 retriever、范围和当前位置，并通过 `TypedKVScan::clone_detached` 复制解码状态；新执行器清空 `page_keys`，保留 `scanned_rows`、打开/关闭标志和游标。

## 执行流程

1. `builder.rs::build_typed_physical_plan` 从 `PhysicalIndexLookUpReader` 取得 `IndexPlan`、`TablePlan`、`IndexInfo` 和 `TableInfo`，确定表 ID、索引列数、方向、范围与 chunk 容量，构造 `TypedKVScan` 和 `TypedIndexLookUp`。
2. `Open` 将范围索引、游标、页锁键和扫描计数复位，并设置 `opened = true`、`closed = false`。未打开或已关闭时调用 `Next` 会返回 `index lookup executor is not open`。
3. `next_inner` 先重置输出 chunk 和上一页锁键，再检查 kill signal；随后在 chunk 未满且仍有范围时创建 `Iter` 或 `IterReverse`。
4. 对每条有效索引 KV，先解码 handle。普通 handle 使用 `self.table_id`；`PartitionHandle` 使用其 `PartitionID` 和内部 handle，从而支持全局索引指向物理分区。
5. `EncodeRowKeyWithHandle` 生成规范 record key；`Retriever::Get` 点读记录；`append_decoded_row_for_table` 验证 record key 的表 ID，解码请求列并追加一行。
6. 成功追加后递增 `scanned_rows`、记录锁键并推进游标。升序游标为“当前索引键追加一个 `0` 字节”，用作下一次迭代的排他后继；降序保存当前索引键，依赖 `IterReverse` 的上界语义继续向前。
7. 当前迭代器失效时前进到下一范围并清空游标。每次创建的 KV iterator 都在本轮末尾显式 `Close`；发生循环内错误时也先关闭，再清空部分输出和锁键后返回错误。
8. 所有范围耗尽后返回空 chunk 表示 EOF。外层 `TypedLimit` 可在满足 limit 后停止继续拉取；独立测试证明随后再次 `Next` 不会触发额外索引迭代。

## 数据与状态

`ranges`、`range_index` 和 `cursor` 共同构成分页状态。范围在构造期排序，执行期不修改；只有当前范围完全失效才递增 `range_index`。`cursor` 是范围内续读位置，跨 chunk 保留，在切换范围时清空。这里没有去重集合，因此正确性依赖上游传入互不造成重复结果的 KV 范围，以及 `Iter`/`IterReverse` 的边界契约。

`page_keys` 只描述最近一次成功 `Next` 输出的记录键：每次调用开始时清空，任一中途错误或最终 kill 检查失败时同时清空，避免为未交付的部分结果加锁。`scanned_rows` 在成功解码并追加每条记录后累加，直到下一次 `Open`；`Detach` 保留累计值但不复制尚未消费的页锁键。

输出 schema、chunk 容量和行编码规则完全委托给 `record_decoder`。这使索引层只关心“索引键到 record key”的映射，而 `TypedKVScan` 负责列 ID 到 datum、PK-is-handle 回填、空 schema 虚拟行等表记录语义。

## 依赖与调用关系

上游生产调用链为 `builder.rs::build_typed_physical_plan` → `TypedIndexLookUp::new` → `ExecExecutor::{Open, NextWithContext/Next, Close}`。通常外层还可能有 `TypedSelection`、`wrap_typed_pushdown_plan` 产生的执行器以及 `TypedLimit`；因此本文件只实现基础双读，不自行解释过滤或 limit 表达式。

主要下游调用为：

- `astersql-kv`：`Retriever::{Iter, IterReverse, Get}`、`Iterator::{Valid, Key, Value, Next, Close}`、`Key`、`PartitionHandle`；
- `astersql-tablecodec`：`DecodeIndexHandle` 与 `EncodeRowKeyWithHandle`；
- `TypedKVScan::append_decoded_row_for_table`：record key 校验与行解码；
- `astersql-util-chunk`：分页输出的 `Chunk`；
- `astersql-errors`：生命周期错误与缺失 handle 错误；
- `adapter::{ExecExecutor, ExecutionContext}`：统一执行生命周期、kill signal、锁键与统计接口。

[`Cargo.toml`](Cargo.toml) 将 `astersql-errors`、`astersql-kv`、`astersql-tablecodec`、`astersql-util-chunk` 都声明为工作区内 path 依赖；本模块没有专属 feature gate，`lib.rs` 无条件公开生产模块，仅以 `#[cfg(test)]` 装配独立测试模块。

## 错误处理与边界

所有 KV、codec、记录解码和 kill-signal 错误均通过 `AdapterResult` 原样向上传播。文件额外产生两类上下文错误：生命周期非法时的 `index lookup executor is not open`，以及索引 KV 无法给出句柄时的 `index KV has no row handle`。`TypedKVScan::append_decoded_row_for_table` 还会拒绝 record key 中 table ID 与预期物理表 ID 不符的记录。

内部迭代循环失败时会关闭 iterator、重置整个输出 chunk 并清空锁键，因此不会把半页结果暴露给调用者。需要注意：创建 `Iter`/`IterReverse` 本身失败时尚无 iterator 可关闭；`Get` 返回记录不存在、索引编码损坏、记录编码损坏都终止当前页，不做跳过或重试。

空范围或耗尽范围会成功返回空 chunk。`index_columns` 必须与实际索引编码中的列数一致；该值由 builder 的 `index_info.Columns.len()` 提供。重叠范围、错误边界或不符合 `Retriever` 游标语义的实现可能产生重复/遗漏，本文件不额外归并或校正。

## 并发与资源生命周期

Rust 实现自身不创建线程、任务或通道。`&mut self` 串行保护游标、输出页锁键和计数器；底层 retriever 通过 `Arc` 共享，并要求 trait object 为 `Send + Sync`，但这不表示同一个执行器可并发调用。

每次范围扫描创建一个短生命周期 iterator，并在成功或循环内错误后显式 `Close`。`Close` 只把执行器标记为关闭，不持有需要等待的后台 worker；重新 `Open` 可复用实例并完全重置进度和统计。`Detach` 产生拥有自身游标状态的执行器，共享 retriever、深拷贝范围和 decoder 配置；原执行器关闭后，detached 实例仍可从复制的游标继续读取，这一行为由 `typed_index_lookup_test.rs` 覆盖。

## 与 Go 版本的对应关系

Go 的直接语义对照是 [`distsql.go`](distsql.go) 中 `IndexLookUpExecutor`，两者都实现索引双读：先从索引侧取得 handle，再读取表记录，并按索引任务顺序向上游返回。两者也都支持分区/全局索引、升降序、推下 limit 的计划语义和 Detach 概念。

当前 Rust 版本是聚焦 typed 本地 KV 绑定的同步实现，不能把 Go 已有能力自动视为 Rust 已支持。Go 实现会构造 DistSQL 请求，启动 `indexWorker`/`tableWorker`，通过 `resultCh` 和 `lookupTableTask` 并发批量查表，并包含内存 tracker、运行时统计、索引使用统计、adaptive limit、分页、关联列重建、grouped ranges/merge sort、union scan dummy 模式、弱一致性及 worker 取消/等待等机制；Rust 本文件逐条索引、逐条 `Get`，没有这些 worker、通道、统计和内存治理设施。

Go `Close` 会取消 worker、清空 channel 并等待两个 wait group；Rust `Close` 仅改变状态。Go `Detach` 复制 executor 并 detach session context；Rust `Detach` 复制纯 typed 执行状态且显式丢弃当前页锁键。扩展 Rust 行为时应保持实际双读顺序、分区 handle 和锁键语义与 Go 一致，但不应在没有相应架构证据时照搬 Go 的并发字段。

## 扩展指南

- 新增索引编码或 handle 形态时，优先修改 `next_inner` 中 `DecodeIndexHandle` 后的 table/handle 分派，并在独立的 [`typed_index_lookup_test.rs`](typed_index_lookup_test.rs) 增加对应 canonical codec 用例；不要把测试嵌入生产源文件。
- 新增范围合并、去重、降序分页或批量 point-get 时，应先明确 `kv::Retriever` 的半开区间和 reverse 上界契约，再修改 `new` 的排序及 `cursor` 推进；重点防范跨 chunk 重复、遗漏和索引顺序变化。
- 新增列解码语义应落在 `TypedKVScan`，本文件只选择正确的 record key/物理表 ID；同时更新 `typed_kv_scan_test.rs` 与 lookup 的集成边界测试。
- 新增取消点时沿用 `NextWithContext` 的 `sql_killer`，并保证错误路径仍关闭 iterator、清空部分 chunk 与锁键。
- 若引入批量或并行双读，需设计结果重排、背压、取消、内存计量和 Close/Detach 所有权，不能只并发化 `Get`；Go `distsql.go::IndexLookUpExecutor` 可作为语义参考，但 Rust typed runtime 的 `ExecExecutor` 契约是直接边界。
- 修改锁键生成时同步检查 `adapter.rs::runPessimisticSelectForUpdate` 的逐页消费方式；返回索引键而非规范 record key 会锁错对象。
- 修改 builder 接线时同步覆盖 `typed_index_lookup_builder_streams_pushed_limit_and_returned_lock_key`，确认过滤/limit 包裹顺序和 EOF 后不再访问存储。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/typed_index_lookup.rs` 确认目标文件被索引，`node --file ...` 完整核对 238 行生产源码，`query TypedIndexLookUp`、`query next_inner`、`query append_decoded_row_for_table` 核对主要符号。精确 `callers/callees` 对该实现未给出可用边，故调用关系由下列源码入口交叉验证。
- [`builder.rs`](builder.rs)：`PhysicalIndexLookUpReader` 分支是生产构造入口，提供 retriever、record decoder、索引列数、表 ID、方向和 ranges，并负责外层 filter/pushdown/limit。
- [`adapter.rs`](adapter.rs)：`ExecExecutor` 定义生命周期与扩展方法；`runPessimisticSelectForUpdate` 证明 `TakeLockKeys` 返回值用于真实行锁。
- [`typed_kv_scan.rs`](typed_kv_scan.rs)：`append_decoded_row_for_table` 证明 record key table ID 校验、row codec 解码、主键 handle 回填及 chunk 追加语义。
- [`typed_index_lookup_test.rs`](typed_index_lookup_test.rs)：三个独立测试分别证明规范索引顺序下的分页双读/Detach/扫描计数/锁键、全局索引按分区 ID 读取、builder 接线下 pushed limit 与 EOF 不再迭代。
- [`distsql.go`](distsql.go) 与 [`detach.go`](detach.go)：核对 Go `IndexLookUpExecutor` 的双读职责、并发 worker 生命周期、结果任务消费、Close 和 Detach，并据此标出 Rust 当前未移植能力。
- [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)：核对 crate 名、Go 包映射、直接依赖和模块/独立测试装配。
- 本任务是纯文档分析，按计划不运行 Cargo；交付只执行固定 11 章节结构检查和 Git 差异范围检查。
