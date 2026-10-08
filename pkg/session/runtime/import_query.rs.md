# `pkg/session/runtime/import_query.rs`

## 文件定位

[`import_query.rs`](import_query.rs) 属于 `astersql-session` crate 的具体会话运行时。父模块 [`pkg/session/runtime.rs`](../runtime.rs) 以私有 `mod import_query;` 装配它，因此文件本身不形成对外 crate API；它通过 `impl ConcreteSession` 增加 `execute_import_query`，供同一运行时的语句分派器调用。

直接入口位于 [`dispatch.rs`](dispatch.rs) 的 `ConcreteSession::execute_statement`：分派器先解析目标库表，拒绝带物化视图日志的基表，并调用 `astersql_executor_importer::CheckImportTableTTL`；当 `ImportIntoStmt.Select` 存在时才进入 `execute_import_query`，否则走文件导入路径。因而本文件负责的是 `IMPORT INTO ... FROM (SELECT ...)` 的同步执行路径，不负责 SQL 解析、TTL/MLog 的公共前置校验或文件导入。

[`pkg/session/Cargo.toml`](../Cargo.toml) 将该目录声明为 `astersql-session`（库入口为 `lib.rs`），并直接依赖本文件使用的 importer、Lightning backend/encode/KV/verification/mydump、KV、元模型、MySQL 模式、tablecodec 与 types crate。当前 crate 的 `nextgen` feature 没有在本文件形成条件编译分支。

## 核心职责

- `ConcreteSession::execute_import_query` 校验查询导入特有的语法形态、选项、目标表空表条件与源/目标列数，然后执行 SELECT，并把结果转换成 importer 的规范 `QueryChunk`。
- `QueryRuntime` 实现 `importer::TableImporterRuntime`，把当前表元信息、SQL mode、DML 类型标志、自增分配器和查询 chunk 接收端适配给通用 importer。
- 该路径复用 Lightning 的表 KV 编码与本地 SST backend：数据不经 SQL `INSERT`/`REPLACE` 写入，而是编码成 data/index engine，关闭后执行物理导入与清理。
- 导入结束后同步运行时 SST 统计，回写实际使用过的自增/自随机/隐式行号上界，并用本地编码 checksum 对照目标表当前 KV 范围。

该实现是聚焦的同步运行时适配，并非 Go `ImportIntoExec`/`TableImporter` 全套控制器的逐对象复刻；当前事实和差异见“与 Go 版本的对应关系”。

## 主要符号

- `struct QueryRuntime`：私有适配器，字段分别为目标 `TableInfo`、`astersql_types::Flags`、数值化 SQL mode、`Allocators` 和由 `Arc<Mutex<mpsc::Receiver<_>>>` 表示的共享查询 chunk 接收端。
- `impl importer::TableImporterRuntime for QueryRuntime`：
  - `DataSourceType` 固定返回 `DataSourceTypeQuery`，使通用 `ProcessChunk` 选择 query processor。
  - `TableInfo` 返回目标表元信息；`GetKeySpace` 当前固定为空字节串。
  - `GetKVEncoder` 从目标表元信息创建 table definition，注入共享 allocators，并用当前 SQL mode、chunk timestamp 和 `CanonicalImportDatumConverter(self.flags)` 创建 `TableKVEncoder`。
  - `GetParser` 恒定报错 `query import has no file parser`；查询路径不应请求 mydump 文件 parser。
  - `TakeQueryChunks` 克隆共享接收端。这里“Take”不会消耗字段本身，串行的 query processor 通过互斥接收器取数据。
- `fn error`：把任意可显示错误统一转换为 `SessionError`。它只保留 `to_string()` 后的消息，不保留原错误类型。
- `ConcreteSession::execute_import_query`：本文件唯一会话入口，返回一列 `Imported_Rows` 的 `ConcreteRecordSet`，其中值是 SELECT 产生的行数。

## 执行流程

1. 从 `ImportIntoStmt.Select` 取 SELECT；缺失时返回 `query import requires SELECT`。显式列/用户变量映射或列赋值均被拒绝，因为当前查询路径没有配置映射语义。
2. 使用显式 schema，或回退到 `current_database()`；`resolve_runtime_table` 找不到目标时返回错误。
3. 遍历 `statement.Options`，以 `EvalExpr` 求值有值选项。当前识别 `disable_precheck`、`disable_tikv_import_mode`、`thread`、`disk_quota`、`checksum_table`：flag 不允许携值，`thread` 必须能解析成正整数，quota 必须能解析且大于零，checksum 交给 `PostOpLevel::FromStringValue`。未识别选项立即失败。需注意：`thread` 当前仅校验而未控制 worker 数；`disable_tikv_import_mode` 当前仅接受而未改变导入模式。
4. 用 `GenTablePrefix(table.ID)` 生成目标表 KV 范围。未设置 `disable_precheck` 时，在 `kv::MaxVersion` snapshot 上检查 `[prefix, prefix.PrefixNext())` 是否存在任意 KV；非空则拒绝导入。迭代器在成功路径中显式 `Close`。
5. 调用 `execute_insert_select_node` 在当前会话关系执行器上取得 SELECT 结果。把 SELECT 列数与目标表非隐藏列数比较；不一致即失败。
6. 将全部已物化行按 1024 条分批。每个结果值按列名读取：缺失或 SQL NULL 变为 `encode::Datum::Null`；能由 `row_codec::binary_runtime_bytes` 识别的值变为 bytes，否则保留为 string。每批构造 `QueryChunk`，`row_id_offset` 为 `batch_index * 1024`。发送完成后丢弃 sender，以 channel 断开表示 EOF。
7. 从会话状态解析 SQL mode，创建 `NewPanickingAllocators(false)`，组装 `QueryRuntime`；然后用 `import_sst::Backend` 创建物理 backend 和 engine manager，打开编号 `1` 的 data engine 与 `IndexEngineID` index engine。
8. 创建空 keyspace 的 `KVGroupChecksum`，调用一次 `importer::ProcessChunk`。通用 [`engine_process.rs`](../../executor/importer/engine_process.rs) 根据 `DataSourceTypeQuery` 调用 `TakeQueryChunks` 和 `NewQueryChunkProcessor`，消费 channel、编码表记录及索引 KV，并写入两个本地 engine。
9. 取得 `physical.disk_quota_pressure(quota)` guard 后，依次关闭 data/index engine，以固定 region split size `96 MiB` 和 split keys `960000` 执行 `Import`，随后 `Cleanup`。完成后把 backend 的 `stats` 快照写入 `self.import_files.sst_stats`。
10. 对确实适用的 `AutoIncrementType`、`AutoRandomType`、`RowIDAllocType` 读取 allocator base；上界大于零时调用 `allocate_runtime_auto_id` 回写，避免后续正常写入复用已导入 ID。
11. 合并本地 checksum，再扫描目标表完整 KV 前缀生成 `RemoteChecksum`，交给 `VerifyChecksum` 按用户选择的 `checksum_level` 校验。成功后返回 SELECT 行数。

## 数据与状态

- 只读输入包括 `ImportIntoStmt`、会话当前数据库、目标 `TableInfo`、`state.sql_mode`、DML 编码 flags，以及 SELECT 的列名与字符串化行值。
- `selected` 在进入 importer 前已包含完整 SELECT 结果；之后才按 1024 条分批。这意味着当前实现的峰值内存与结果集大小相关，channel 本身并未形成 SELECT 与编码之间的流式背压。
- `QueryChunk.row_id_offset` 是每批首行的全局零基偏移；最后一批虽可能少于 1024 行，前序整批固定为 1024，因此该公式连续。
- `QueryRuntime.allocators` 与入口局部变量共享同一 allocator 状态：编码器分配 ID，导入后入口读取 base 并推进会话运行时的持久 ID 状态。
- `checksum` 在 encoder/process 与最终验证之间通过 `Arc<Mutex<_>>` 共享；`MergedChecksum` 形成一次本地快照，远端值则来自导入完成后的目标表 KV 扫描。
- 可观察写状态包括目标表 data/index KV、自动 ID、`import_files.sst_stats`；成功返回的记录集只报告 SELECT 行数，不读取 backend 的实际 imported KV/row count。

## 依赖与调用关系

上游主链为：SQL 解析得到 `ast::ImportIntoStmt` → [`dispatch.rs`](dispatch.rs) 做目标表、MLog、TTL 公共检查 → `ConcreteSession::execute_import_query`。另一个直接 SQL 生产者是 [`system_session.rs`](system_session.rs)，物化视图初始化构造带 `disable_precheck`、`thread`、`disk_quota` 的 `IMPORT INTO ... FROM (...)`，最终仍经分派器进入本文件。

主要下游关系如下：

- `execute_insert_select_node`：执行关系 SELECT，并提供列集合和已物化行。
- `row_codec::binary_runtime_bytes` 与 `CanonicalImportDatumConverter`：把运行时值转换成 importer 可稳定编码的 Datum。
- `importer::ProcessChunk` → `ProcessChunkWithWriterAndLogger` → query processor：创建 data/index writer，调用 `GetKVEncoder`，获取共享 channel，并完成编码和 writer 收尾。
- `import_sst::Backend`、`backend::MakeEngineManager`：承载本地 SST engine 生命周期及向存储层的 split/write/ingest。
- `allocate_runtime_auto_id`：把编码阶段 allocator 的最大值同步回运行时元数据。
- `verification::{KVGroupChecksum, KVChecksum}` 与 `importer::VerifyChecksum`：连接本地编码统计和导入后存储快照。

RustCodeGraph 已索引本文件的 15 个符号并报告其被运行时/恢复等文件引用；但对 `execute_import_query`、`ProcessChunk` 这类常见/trait 分派符号未返回可靠的精确 caller/callee 边。因此上述关键边以文件限定的源码入口和调用表达式复核，没有把同名的全仓搜索结果当作调用证据。

## 错误处理与边界

- 所有入口错误都以 `SessionResult` 向分派器传播；多数外部 `String`/KV 错误经 `error` 降格为仅含消息的 `SessionError`。
- 语法/配置边界包括：必须存在 SELECT；不支持 `ColumnsAndUserVars` 和 `ColumnAssignments`；flag 不可带值；thread/quota 必须为正；未知选项失败；源列数必须等于目标非隐藏列数。
- `disable_precheck` 只跳过本文件的“目标表为空”扫描，不绕过分派器更早执行的 TTL 与 MLog 检查。独立测试 `import_select_rejects_enabled_ttl_even_with_disable_precheck` 明确固定了这一边界。
- `disable_tikv_import_mode` 被识别为无值 flag，但当前实现没有保存或消费它；`thread` 也只验证。扩展文档或调用者不应把二者描述成当前已生效。
- data engine 成功导入而 index engine 关闭/导入失败时，本文件没有事务性回滚已 ingest 的 data KV；错误会向上传播。类似地，打开 data engine 后若打开 index engine、`ProcessChunk` 或后处理失败，本文件没有统一的 defer/RAII 清理块。是否由 backend 内部收尾不能从本文件确认，故应视为需要专项验证的故障边界。
- `Mutex::lock().unwrap()` 用于 backend stats 与 checksum；锁中毒会 panic，而不是转为 `SessionError`。`NewPanickingAllocators` 也表明不支持的 allocator 访问可能按设计 panic。
- 目标空表检查与最终导入之间没有在本文件中建立原子隔离；并发写入的协调依赖上游约束或存储/ingest 冲突检测，本文件自身未证明该竞态被排除。

## 并发与资源生命周期

当前实现是同步单调用流程。与 Go 的两个 goroutine 不同，Rust 先完整执行 SELECT，再把所有 chunk 写入无界 `std::sync::mpsc`，关闭 sender 后才调用一次 `ProcessChunk` 消费；因此不存在 producer/consumer 并行，也没有 `thread` 个 importer worker。

receiver 被包装成 `Arc<Mutex<_>>`，满足通用 importer 的共享接收接口；本文件只调用一次 `ProcessChunk`，实际消费仍为单路串行。`sender` 的显式 `drop` 是 EOF 生命周期边界，否则 query processor 可能持续等待。

快照迭代器在正常扫描后显式关闭。engine 生命周期按 `OpenEngine → writer/process → Close → Import → Cleanup` 顺序执行；只有所有这些步骤成功后才进行 ID 与 checksum 后处理。`disk_quota_pressure(quota)` guard 仅覆盖最终两个 engine 的 close/import/cleanup 区段，本文件没有启动 Go/Rust `TableImporter::StartDiskQuotaCheck` 那种后台周期线程。

`Arc<Mutex<KVGroupChecksum>>` 和 backend stats mutex 的临界区很短，但锁中毒未恢复。会话内部的 `state`、`import_files` 使用 `RefCell` 可变借用，体现该具体 session 路径本身不是跨线程共享对象。

## 与 Go 版本的对应关系

Go 主入口在 [`pkg/executor/import_into.go`](../../executor/import_into.go) 的 `ImportIntoExec` 查询导入分支：它创建新 session，运行控制器前置检查和 TiKV 配置，用有界 channel 连接两个 `errgroup` goroutine；一个执行 SELECT 并流式生产 `QueryChunk`，另一个调用 `TableImporter.ImportSelectedRows`。成功后还刷新表统计、设置 affected rows 和 statement message。

Go [`TableImporter.ImportSelectedRows`](../../executor/importer/table_import.go) 与本文件共享核心阶段：打开 data/index engine、query chunk 编码、磁盘 quota 检查、关闭并导入 engine、同步 allocator、checksum/post-process。Go 的 query processor 定义在 [`chunk_process.go`](../../executor/importer/chunk_process.go)，同样把 query rows 编为 data/index KV。

Rust 当前保持的语义包括：SELECT 来源判定、目标元数据驱动编码、query 专用 chunk、data/index 分离、重复键由物理导入暴露、allocator 后处理与 checksum。明确差异包括：

- Rust 在单一 `ConcreteSession` 中同步执行并预先物化全部 SELECT 行；Go 使用新 session、并发 producer/importer 和有界 channel。
- Rust 固定一次 `ProcessChunk`，`thread` 只校验；Go 按 `ThreadCnt` 启动多个 worker。
- Rust 接受 `checksum_table` 和 `disable_tikv_import_mode`；当前 Go 的 `TestInitOptionsFromQueryNegativeCase` 把这两项列为 query source 不允许选项。Rust 对后者不产生行为。
- Rust 返回 `Imported_Rows` record set；Go 更新 statement context 的 affected rows/message，并 best-effort 刷新表统计。
- Rust `GetKeySpace` 为空，且固定 region split 参数；Go 从 importer/controller 配置取得 keyspace、split size/keys 等。

这些差异是当前源码事实，不应在没有相应行为与回归测试时假定已经与 Go 等价。

## 扩展指南

- 新增或调整查询导入选项时，优先修改 `execute_import_query` 的选项解析，并同步核对 Go `Plan.initOptions` 的允许集合、默认值、重复选项和类型规则。若选项应产生行为，必须把解析值继续接到 runtime/backend；仅“接受但忽略”会造成错误兼容承诺。
- 若要让 `thread` 生效或实现流式导入，需要同时重构 SELECT 生产、channel 容量、多个 `ProcessChunk` worker、错误取消和 receiver 共享策略；还要保证 row ID offset、allocator、checksum 合并与 writer 关闭顺序在并发下正确。
- 若扩展列映射、用户变量或 assignment，接入点是前置拒绝分支和行到 Datum 的转换环节；需对齐 Go planner/importer 的列映射、默认值、生成列与类型转换规则，不能只移除拒绝检查。
- 若改变 engine 生命周期，应为“data 已导入/index 失败”“ProcessChunk 中途失败”“checksum 失败”和 cleanup 失败分别定义可恢复策略，避免留下部分物理结果。
- Rust 测试不要内嵌到本文件。会话端到端回归应扩展同目录独立文件 [`normal_ddl_create_materialized_view_test.rs`](normal_ddl_create_materialized_view_test.rs)，或新增同目录 `*_test.rs` 并在 `runtime.rs` 的 `#[cfg(test)] mod` 中装配；通用 query processor/option 行为则应扩展 [`pkg/executor/importer`](../../executor/importer/) 下的独立测试。
- 性能变更要重点量化 SELECT 全量物化、1024 固定批次、无界 channel、单 worker 编码、全表 checksum 扫描和固定 region split 参数。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/session/runtime/import_query.rs` 确认目标文件入图且含 15 个符号；`node --file ... --offset 1 --limit 400` 完整读取 295 行源码。精确 caller/callee 查询未产生可用边，已用限定路径源码补证，未采用通用名称的噪声结果。
- 目标源码与模块边界：[`import_query.rs`](import_query.rs)、[`runtime.rs`](../runtime.rs)、[`dispatch.rs`](dispatch.rs)、[`system_session.rs`](system_session.rs)、[`pkg/session/Cargo.toml`](../Cargo.toml)。`pkg/session` 下未发现 `doc.go`，因此没有额外包契约文件可读。
- Rust 下游实现：[`engine_process.rs`](../../executor/importer/engine_process.rs)、[`table_import.rs`](../../executor/importer/table_import.rs)、[`import_sst.rs`](import_sst.rs)。
- Go 对照：[`import_into.go`](../../executor/import_into.go)、[`table_import.go`](../../executor/importer/table_import.go)、[`chunk_process.go`](../../executor/importer/chunk_process.go)、[`import_test.go`](../../executor/importer/import_test.go)。
- 独立 Rust 测试：[`normal_ddl_create_materialized_view_test.rs`](normal_ddl_create_materialized_view_test.rs) 的 `normal_ddl_plan_create_materialized_view_2_import_select` 验证成功导入，`normal_ddl_plan_create_materialized_view_2_import_errors` 验证重复键不留行及非空目标拒绝，`import_select_rejects_enabled_ttl_even_with_disable_precheck` 验证 TTL 检查不能被该选项绕过。同名专用 `import_query_test.rs` 当前不存在。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证文件存在且恰有 11 个固定二级章节，并人工复核所有链接指向真实文件、差异描述没有提升为未证实的支持结论。
