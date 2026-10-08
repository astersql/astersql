# `pkg/session/runtime/canonical_table_reader.rs`

## 文件定位

目标源码 [`canonical_table_reader.rs`](canonical_table_reader.rs) 属于 `astersql-session` crate 的会话运行时层，由 `pkg/session/runtime.rs` 以私有模块 `canonical_table_reader` 装配。它实现 `CanonicalTableReaderExecutor`，把已经准备好的 `PhysicalTableReader` 转成 TiKV DAG 请求，并通过 `astersql_executor::adapter::ExecExecutor` 接口向上层按 `Chunk` 输出行。

它不是通用 TableReader 的完整复刻，而是 `SessionBoundAdapterOwner::BuildExecutor` 中一个受条件保护的生产快路径：存储客户端必须支持基础 DAG 请求，物理计划必须已准备，`leaf_ranges.len() <= 1`，计划中必须能取得一个 `PhysicalTableReader`，且收集到的 TableScan 恰好只有一个。条件不满足时，`pkg/session/runtime/scan_adapter_runtime.rs` 会回退到 `OpenTypedPhysicalPlanWithBindings`、`OpenTypedPhysicalTableScan` 或快照扫描路径。

## 核心职责

- `CanonicalTableReaderExecutor::new` 把表元数据、列、键范围、快照时间戳、计划 ID 和会话资源组配置固化成一次性的 `kv::Request`。
- `Open` 经 `Domain` 的存储客户端发送请求，取得流式 `kv::Response`，并初始化一次执行周期的缓冲与运行证据。
- `Next`/`NextWithContext` 从 TiKV `SelectResponse` 解码 datum，填充调用方提供的 `chunk::Chunk`；带上下文版本还会在拉取下一响应前检查 SQL kill 信号。
- `fetch_response` 同步累计 `CopRuntimeEvidence`，使返回行与计费/观测证据来自同一批 TiKV DAG 响应；`Close` 或 EOF 时发布最终快照。
- `ChunkConfig`、`NewChunk` 和 `Schema` 将 `ColumnInfo.FieldType` 映射到执行器适配层需要的输出模式。

## 主要符号

- `CanonicalTableReaderExecutor`：`pub(super)` 执行器状态。`domain` 保持存储入口存活；`request` 与 `response` 表示请求发送前后两个阶段；`columns`/`schema` 定义解码宽度和输出类型；`pending` 保存已解码但尚未输出的行；`opened`/`closed` 约束生命周期。
- `CanonicalTableReaderExecutor::new(...) -> Result<Self, SessionError>`：唯一构造入口。调用 `relational_select_request` 建立 TiKV 表扫描请求，调用 `planned_table_reader_select_dag` 写入 DAG 数据，并把 typed key ranges 转为 `kv::KeyRange`。它还设置资源组名与 paging 字节预算。
- `fetch_response(&mut self) -> AdapterResult<bool>`：从 `kv::Response::Next` 拉取一个 subset。返回 `false` 表示 EOF；返回 `true` 表示已处理一个响应，即使该响应没有产生行。
- `publish_evidence(&self)`：把 `(plan_id, evidence)` 覆盖写入共享的 `Mutex<Option<_>>`，供 `StatementRURuntimeEvidence` 汇总。
- `next_inner(...) -> AdapterResult`：`Next` 与 `NextWithContext` 的共同实现，重置输出块、优先排空 `pending`，必要时检查 kill 信号并继续拉取响应。
- `impl ExecExecutor`：实现 `Open`、`Close`、`Next`、`NextWithContext`、chunk/schema 配置、扫描行计数，以及外键/写执行器能力声明。`CalculateNoDelay`、`IsWriteExecutor`、`HasForeignKeyCascades` 均为 `false`；`Detach` 返回 `None`。

## 执行流程

1. `SessionBoundAdapterOwner::BuildExecutor` 在 `pkg/session/runtime/scan_adapter_runtime.rs` 中先清空上一轮 `table_reader_ru_evidence`，再验证 DAG 支持、prepared 状态、范围数量和单 TableScan 形状。
2. `new` 通过 `relational_select_request` 创建带表 ID、快照版本、leader replica read、global txn scope 和 connection ID 的请求。随后 `planned_table_reader_select_dag` 展平 `PhysicalTableReader.TablePlan` 的可下推算子，编码为 `tipb::DagRequest`；该辅助函数还把主键 handle 的类型校正为 signed 64-bit，并要求 DAG 首算子为 TableScan。
3. 构造函数将扫描范围逐项复制进 `request.KeyRanges`，从会话读取 `ResourceGroupName` 与 `Paging.PagingSizeBytes`，并保存列类型、chunk 容量和共享证据槽。
4. `Open` 消费 `request: Option<kv::Request>`，以 `EnableCollectExecutionInfo: true` 调用 `store.GetClient().Send`。成功后保存 response，清空 pending、扫描行数和证据，并进入 `opened = true, closed = false`。
5. `Next` 或 `NextWithContext` 调用 `next_inner`。每次先 `output.Reset()`；若 `pending` 有行，则逐列 `AppendDatum` 并增加 `scanned_rows`；否则（带执行上下文时）调用 `SQLKiller::HandleSignal`，然后由 `fetch_response` 拉取下一 subset。
6. `fetch_response` 先饱和累加 subset 的 `total_keys`、`processed_keys`、`processed_bytes` 和可选 `tikv_response_bytes`，立即发布证据；再把 subset 数据解析为 `tipb::SelectResponse`，检查 TiKV 返回错误，并按 `columns.len()` 个 datum 为一行解码进 `pending`。
7. response EOF 时再次发布证据并使当前 `Next` 返回；输出块为空即向上层表达 EOF。`Close` 清空 pending、取走并关闭 response、发布最终证据；重复 `Close` 不会再次关闭已取走的 response。

## 数据与状态

- `request: Option<kv::Request>` 是一次性资源：`Open` 使用 `take()` 消费。因此同一个实例完成 `Open`/`Close` 后不能重新打开；第二次打开会报请求已消费。仅在已打开且未关闭时重复 `Open` 才是幂等成功。
- `response: Option<Box<dyn kv::Response>>` 在 `Open` 后持有流，`Close` 时被取走。读取采用同步 pull 模式，没有后台预取任务。
- `pending: VecDeque<Vec<Datum>>` 解耦 TiKV response chunk 与上层 chunk 容量：一个响应可解码出多行，跨多次 `Next` 逐步排空。
- 行边界完全由 `columns.len()` 决定。每成功写入上层 chunk 一行，`scanned_rows` 增加一次；仅解码进 `pending` 尚不计数。
- `evidence` 使用 `saturating_add` 累积扫描计数，避免无符号整数溢出；`tikv_response_bytes` 只有收到该字段时才从 `None` 变为 `Some(total)`。
- `shared_evidence` 是会话适配器拥有的跨组件槽。`StatementRURuntimeEvidence` 在 `pkg/session/runtime/scan_adapter_runtime.rs` 中将其转换为对应计划的 `ScanDetail`，并把 TiKV response bytes 纳入语句级网络字节证据。

## 依赖与调用关系

上游直接调用边为 `SessionBoundAdapterOwner::BuildExecutor -> CanonicalTableReaderExecutor::new`（`pkg/session/runtime/scan_adapter_runtime.rs`）。RustCodeGraph 还识别到模块被 `pkg/session/runtime/planning.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs` 和 `pkg/session/runtime_test/planning.rs` 的规划/测试链间接覆盖。

下游关键依赖如下：

- `super::relational_scan::{relational_select_request, planned_table_reader_select_dag}`：建立请求公共字段并从物理计划编码 TiKV DAG。
- `astersql_domain::Domain -> storage() -> kv::Client::Send`：发送请求并获得 `kv::Response`。
- `kv::{Request, Response, ResultSubset, CopRuntimeEvidence}`：传输、流式读取和运行证据边界。
- `tipb::SelectResponse` 与 `protobuf`：解码 TiKV DAG 响应及服务端错误。
- `astersql_tablecodec::codec::DecodeOne`：按列解码行 datum。
- `astersql_executor::adapter::{ExecExecutor, ExecutionContext, ChunkConfig, SchemaColumn}` 与 `astersql_util_chunk`：对接统一执行器生命周期和批量行容器。

`pkg/session/Cargo.toml` 将该文件归入 crate `astersql-session`，并以 workspace 内 path dependency 提供 `astersql-domain`、`astersql-errors`、`astersql-executor`、`astersql-kv`、`astersql-meta-model`、planner、tablecodec、types 和 chunk；protobuf 固定为 `2.8.0`。该模块没有自己的 feature gate。

## 错误处理与边界

- 构造阶段会传播请求/DAG 编码错误，包括缺少 table plan、DAG 首算子不是 TableScan、算子转 protobuf 失败和序列化失败。
- `Open` 在 request 已被消费或客户端返回 `None` response 时返回适配层错误。`Send` 的接口在这里用 `Option` 表达无响应；具体传输错误由存储客户端/response 边界承担。
- 未打开或已关闭时调用 `Next` 会得到 `TableReader executor is not open`；response 缺失时 `fetch_response` 返回 `TableReader response is not open`。
- protobuf 解码错误标记为 `decode TableReader response`；`SelectResponse.error` 转成 `TiKV TableReader failed`。datum 解码错误直接向上传播。
- 解码循环假定每行恰有 `columns.len()` 个 datum。若列数为零且响应 `rows_data` 非空，`encoded` 不会前进；当前入口依赖有效 TableReader schema 避免这一情况，扩展入口时必须显式维持此不变量。
- `publish_evidence` 对 poisoned mutex 使用 `unwrap()`，会 panic；本文件没有恢复 poisoned lock 的策略。
- kill 信号只在 `NextWithContext` 且 pending 已排空、准备拉取下一 subset 时检查。普通 `Next` 不检查，已有 pending 行也不会逐行检查。

## 并发与资源生命周期

执行器本身由 `&mut self` 驱动，不声明可并发调用；`VecDeque`、request 和 response 都是实例内可变状态。`Domain` 与证据槽用 `Arc` 延长共享所有权，其中只有证据槽通过 `std::sync::Mutex` 进行跨组件同步。

资源顺序为“构造未发送请求 -> `Open` 取得 response -> 多次 `Next` -> `Close`”。`Close` 先标记 closed 并丢弃尚未输出的 pending 行，再调用底层 `Response::Close`，无论 close 结果成功与否都发布证据。EOF 会发布证据但不会自动取走或关闭 response，因此正常拥有者仍应调用 `Close`。本类型没有 `Drop` 实现，提前丢弃且未显式 `Close` 时只能依赖底层 response 的析构行为，不能把显式 close 语义视为已保证。

重复 `Close` 对本实例是基本幂等的：第二次没有 response 可关闭，但仍发布同一证据。真实 TiKV 测试 `statement_ru_simple_select_real_tikv_terminal_publication` 进一步验证 result 层重复 `Finish`/`Close` 不会重建最终语句快照，不过该测试覆盖的是完整上层生命周期而非直接构造本类型。

## 与 Go 版本的对应关系

Go 对照实现位于 `pkg/executor/table_reader.go` 的 `TableReaderExecutor`，构造入口主要在 `pkg/executor/builder.go::buildNoRangeTableReader/buildTableReader`。两者共同语义是：由 `PhysicalTableReader` 构造 DAG 请求，在 `Open` 打开分布式读取，在 `Next` 向 chunk 填行，在 `Close` 释放结果，并把表扫描纳入执行统计。

Rust 当前只覆盖 Go 实现的受控子集：单个 TiKV TableScan/可下推子树、最多一个 leaf range 组、leader read、global txn scope、无 stale read。Go 版本还处理 signed/unsigned range 分段与有序合并、分区、TiFlash/MPP/batch cop、dummy 临时或缓存表、相关列重建、虚拟列、索引使用报告、内存 tracker、detach 等；这些能力不能据此文件声称已支持，Rust 入口会在形状不匹配时走其他 typed executor 路径。

Go 的 `Next` 由 `tableResultHandler`/`distsql.SelectResult` 解码并填充 chunk，Rust 则直接迭代 `kv::Response`、解析 `tipb::SelectResponse` 与 `DecodeOne`。Rust 额外把同一 subset 上的 `CopRuntimeEvidence` 放入会话共享槽，以适配当前 Rust 语句 RU 汇总链；相关最终行为由 `pkg/session/runtime/scan_adapter_runtime.rs::StatementRURuntimeEvidence` 消费。

## 扩展指南

- 扩大适用计划形状时，先修改 `SessionBoundAdapterOwner::BuildExecutor` 的选择条件，并同步证明回退路径没有被错误截获；不要仅放宽 `new`。分区、多范围组合、IndexReader、TiFlash 或 MPP 应分别对照 Go 的 range/result handler 语义实现，不能把多个范围简单拼接后宣称等价。
- 新增下推算子或输出列变化时，优先检查 `planned_table_reader_select_dag` 的 executor 顺序、`output_offsets` 和 handle 类型修正，同时保证 `columns.len()` 与 TiKV 编码的每行 datum 数严格一致。
- 改动 response 解码时，应在独立测试文件 `pkg/session/runtime/scan_adapter_runtime_test.rs` 增加回归测试，至少覆盖多 subset、单 subset 多行、chunk 容量截断、空响应、畸形 protobuf、TiKV error、datum 截断、kill 信号以及 EOF/Close 证据发布。不要把测试内嵌进生产 `.rs`。
- 改动 RU 证据时，同时核对 `StatementRURuntimeEvidence` 和 `pkg/executor/statement_ru_plan_walk.rs` 的消费约定；真实 transport bytes 的端到端验证入口是被忽略的 RealTiKV 测试 `statement_ru_simple_select_real_tikv_terminal_publication`。
- 若要支持可重开实例，需要重新设计一次性 request 的所有权与 response 清理；当前 `Option::take` 明确禁止 close 后 reopen。
- 性能变更应关注每行 `Vec<Datum>` 分配、pending 峰值、响应整块预解码和逐 datum append 成本；兼容性变更应对照 Go TableReader 的 chunk、range ordering、virtual column 与 execution stats 语义。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点；`files --filter pkg/session/runtime/canonical_table_reader.rs` 确认目标文件已索引；`node --file ... --offset 1 --limit 500` 读取全部 304 行并识别 30 个符号；文件关系报告直接使用者为 `planning.rs`、`scan_adapter_runtime.rs`、`scan_adapter_runtime_test.rs`、`runtime_test/planning.rs`。`query CanonicalTableReaderExecutor --kind struct` 将定义定位到第 34 行。图对同名 `new`/`next_inner`/`fetch_response` 的无文件限定查询存在歧义，因此调用边再以已索引文件指向的直接入口核对。
- Rust 源码：`pkg/session/runtime/canonical_table_reader.rs`；模块入口 `pkg/session/runtime.rs`；直接构造与证据消费 `pkg/session/runtime/scan_adapter_runtime.rs`；共享字段定义 `pkg/session/runtime/typed_adapter_bridge.rs`；请求/DAG 辅助实现 `pkg/session/runtime/relational_scan.rs`。
- crate 边界：`pkg/session/Cargo.toml`。
- Rust 独立测试：`pkg/session/runtime/scan_adapter_runtime_test.rs` 的 prepared/explain/execute statement RU terminal 测试，以及被忽略的 `statement_ru_simple_select_real_tikv_terminal_publication`。未发现直接以 `CanonicalTableReaderExecutor` 名称构造的同名专用单元测试；现有证据主要通过完整 adapter 路径覆盖。
- Go 对照：`pkg/executor/table_reader.go` 的 `TableReaderExecutor::{Open, Next, Close}` 与 `pkg/executor/builder.go::{buildNoRangeTableReader, buildTableReader}`；RU/扫描统计语义另由 `pkg/executor/statement_ru_plan_walk.go` 及其独立测试覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证命令及最终退出码在任务交付时记录。
