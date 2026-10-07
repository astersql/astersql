# `dumpling/export/writer.rs`

## 文件定位

`writer.rs` 是 `astersql-dumpling-export` library crate 的导出落盘协调层。crate 入口 [`dumpling/export/lib.rs`](lib.rs) 在 `ir_impl.rs`、`status.rs` 之后、`dump.rs` 之前通过 `include!("writer.rs")` 将它并入同一个包级命名空间，因此本文件可直接使用 `Config`、`TaskEnum`、`TableDataIR`、`FileFormat`、`Storage` 等相邻文件定义的符号，而不是一个独立的 Rust module。

应用内的直接入口位于 [`dumpling/export/dump.rs`](dump.rs) 的 `Dumper::Dump`：生产端先把库、表及数据分块转为 `TaskEnum` 并写入 channel，消费端构造一个 `Writer`，安装进度回调，逐个接收任务并调用 `Writer::handleTask`。因此本文件位于“任务生成”与“格式编码/外部存储”之间，负责按任务种类选择写入路径、生成文件名并收束资源生命周期。

crate 边界由 [`dumpling/export/Cargo.toml`](Cargo.toml) 确认：包名是 `astersql-dumpling-export`，`lib.rs` 是库入口；writer 间接使用本 crate 的 SQL/CSV 编码依赖、Parquet writer 及 `astersql-objstore-storeapi::WriterOption`。当前 Cargo 注释同时说明该移植面使用本地 SQL/MySQL/storage 等轻量替身，不能把这里的 `Storage`/`Conn` 自动等同于 Go 生产实现的全部能力。

## 核心职责

- `Writer` 保存一次消费循环所需的上下文、配置、数据库连接、目标存储、输出格式、指标和完成回调。
- `NewWriter` 将配置中的 `FileType`（大小写不敏感）解析成 `FileFormat`；未知值保留为 `FileFormatUnknown`，真正写数据时由 `FileFormat::WriteInsert` 返回错误。
- `handleTask` 是任务分派入口：五种元数据任务分别进入 `Write*Meta`，`TableData` 进入 `WriteTableData`，并在最后一个 chunk 成功后触发表完成回调。
- 元数据路径根据 `OutputFileTemplate` 生成 `.sql` 文件，写入 server-specific comments 与规范化后的 DDL。
- 表数据路径启动 `TableDataIR`，必要时从自定义 SQL 的原始结果集推导动态列元数据，再按 CSV、需要切分的 SQL、普通 SQL/Parquet 三条路径编码和落盘。
- `outputFileNamer` 维护 chunk/file 两级索引，保证切文件时文件名稳定且与 Go 的零填充格式一致。
- `countTotalTask` 汇总多个 writer 的接收计数；当前 Rust `Dumper::Dump` 只构造一个 writer，但该 API 保留了池化统计形态。

本文件只协调写出，不实现行值编码、SQL/CSV/Parquet 序列化、模板解析或外部存储本身；这些分别位于 `writer_util.rs`、`prepare.rs` 和 `stubs.rs`/对象存储接口中。

## 主要符号

- `pub struct Writer`：核心状态容器。
  - `id: i64` 仅保存实例编号；本文件目前不使用它记录日志。
  - `tctx: tcontext::Context` 传给 IR 和格式 writer。
  - `conf: Arc<Config>` 与 `ext_storage: Arc<dyn Storage>` 允许共享只读配置和存储句柄。
  - `conn: Option<Conn>` 表示连接可被外层 `take()` 后关闭；数据写入若发现 `None`，返回 `writer conn closed`。
  - `file_fmt` 是构造时解析的输出格式；`metrics` 是可选的、克隆后的指标收集器。
  - `received_task_count` 在每次 `handleTask` 进入时递增，即使后续写入失败也不会回滚。
  - `finish_task_callback`、`finish_table_callback` 是 `Send` 的闭包；构造时均为空操作。前者不由 `handleTask` 调用，而由 `Dumper::Dump` 在任务成功后显式调用；后者由 `handleTask` 在最后一个数据 chunk 成功后调用。
- `pub fn NewWriter(...) -> Writer`：构造 writer 并解析 `sql`、`csv`、`parquet`；其他字符串映射为 unknown。
- `setFinishTaskCallBack` / `setFinishTableCallBack`：替换进度回调。命名保留 Go 风格，setter 需要 `&mut self`。
- `handleTask(&mut self, &mut TaskEnum) -> Result<()>`：穷举当前六种 `TaskEnum` 变体，构成主要分派表。
- `WritePolicyMeta`、`WriteDatabaseMeta`、`WriteTableMeta`、`WriteViewMeta`、`WriteSequenceMeta`：按不同模板段计算路径并委托 `writeMetaToFile`。视图是唯一生成两个文件的元数据类型。
- `WriteTableData(&mut self, meta, ir, current_chunk)`：数据主入口，管理 `Start`、迭代器、writer 和 IR 的关闭顺序。
- `writeSplitSql(...)`：SQL 在 `FileSize` 或 `StatementSize` 非零时使用的切分循环；复用一个 `SQLRowIter`，每轮消费一个文件的数据。
- `writeMetaToFile(...)`：构造 `metaData`，依次写 special comments 和 `MetaSQL`，最后关闭 lazy writer。
- `countTotalTask(&[Writer]) -> i32`：对 `received_task_count` 求和。
- `outputFileNamer` / `newOutputFileNamer` / `IndexStr` / `NextName`：文件名状态机。`NextName` 先用当前索引执行 `data` 模板，再递增 `FileIndex`，返回“含扩展名路径”和 base；当前调用者只使用前者。

本文件没有 trait、enum、模块级常量或条件编译项。`uploadConcurrency`、`uploadPartSize`、格式常量及 `FileFormat` 均来自 [`dumpling/export/writer_util.rs`](writer_util.rs)。

## 执行流程

1. `Dumper::Dump` 在 [`dump.rs`](dump.rs) 中完成任务生产后，取得外部存储与数据库连接并调用 `NewWriter`。随后安装“完成表”和“完成 chunk”指标回调。
2. consumer 循环通过 `rx.recv()` 获取 `TaskEnum`，先调用 `writer.handleTask(&mut task)`；成功后才由外层调用 `finish_task_callback`。
3. `handleTask` 先递增接收计数，再按任务变体分派：
   - database/table/sequence/policy 各生成一个模板路径；
   - view 分别生成 table/view 路径，并按顺序写两个文件；第一个失败时不会尝试第二个；
   - table data 调用 `WriteTableData`，只有其成功且 `ChunkIndex + 1 == TotalChunks` 时才调用表完成回调。
4. 元数据分支由 `writeMetaToFile` 调用 `getSpecialComments(ServerType)`，逐条加换行写入，然后写 `metaData::MetaSQL()` 的正文并 `Close`。`MetaSQL` 负责 DDL 末尾的分号/换行规范化。
5. 数据分支先检查连接并调用 `ir.Start(tctx, conn)`。当 `conf.SQL` 非空时，从 `ir.RawRows()` 取得原始结果集，通过 `setTableMetaFromRows` 推导列名和类型；缺少 raw rows 或结果集报告错误会中止。
6. `newOutputFileNamer` 从最终采用的 `TableMeta` 捕获库名、表名和当前 chunk，依据 `Rows != 0`、`FileSize != 0` 选择索引格式。
7. 后续按格式执行：
   - CSV：只创建一次 `ir.Rows()`，循环生成 `.csv` 文件；每个文件交给 `writeCSVFile` 消费到大小边界，随后关闭文件。`FileSize == 0` 时只跑一轮，否则继续处理剩余行；循环结束检查迭代器错误并关闭 iterator、IR。
   - split SQL：当格式为 SQL 且 `FileSize != 0` 或 `StatementSize != 0`，进入 `writeSplitSql`。其结构与 CSV 类似，但调用 `writeSQLFile`，最终由外层关闭 IR。
   - 普通 SQL/Parquet/unknown：计算扩展名并只生成一个文件，调用 `FileFormat::WriteInsert`。Parquet 压缩类型映射为 `gz.parquet`、`snappy.parquet`、`zstd.parquet`、`lzo.parquet` 或 `parquet`；unknown 最终报错。
8. `Dumper::Dump` 在 channel 耗尽后停止进度任务，`take()` 并关闭 writer 连接，再写全局结束元数据。

## 数据与状态

`Writer` 的可变状态主要是连接、计数和两个回调。配置和存储通过 `Arc` 共享，但 `handleTask`/`WriteTableData` 都要求 `&mut self`，因此单个 writer 不会在本文件内并行处理多个任务。`metrics: Option<metrics>` 在构造时从引用克隆，随后作为共享收集器的句柄传给格式 writer。

`TaskEnum::TableData` 持有 `Box<dyn TableMeta>` 与 `Box<dyn TableDataIR>`；`handleTask` 借用 metadata、可变借用 data。动态 SQL 模式会临时构造一个 `Box<dyn TableMeta>`，只在当前 `WriteTableData` 调用内覆盖原 metadata，不写回 task。

`outputFileNamer` 的索引不变量来自 `IndexStr`：

- 同时启用 row limit 和 file-size split：`{ChunkIndex:09}{FileIndex:04}`；
- 仅启用 file-size split：只使用 `{FileIndex:09}`，首文件因此是 `000000000`，不体现 chunk；
- 其他情况：只使用 `{ChunkIndex:09}`。

`NextName` 无论模板执行成功与否都会在执行之后递增 `FileIndex`；由于错误立即向上传播，当前调用链不会在同一 namer 上重试该失败索引。`received_task_count` 同样是“收到/尝试”计数，不是成功计数。

## 依赖与调用关系

上游直接关系：

- [`dumpling/export/lib.rs`](lib.rs) 将本文件编入 crate，并挂载独立的 `writer_test.rs`、`writer_serial_test.rs`。
- [`dumpling/export/dump.rs`](dump.rs) 调用 `NewWriter`、两个 callback setter、`handleTask`，并在成功返回后调用 `finish_task_callback`。RustCodeGraph 对这些上游边未产出 callers 结果，因此此部分由源码调用点核验。
- [`dumpling/export/task.rs`](task.rs) 定义 `TaskEnum` 及各任务载荷，数据任务的 chunk 字段决定表完成回调时机。

下游直接关系由 RustCodeGraph 的 file-scoped callees 与源码共同确认：

- `handleTask → WritePolicyMeta / WriteDatabaseMeta / WriteTableMeta / WriteViewMeta / WriteSequenceMeta / WriteTableData`。
- `WriteTableData → TableDataIR::Start/Rows/Close、setTableMetaFromRows、newOutputFileNamer、outputFileNamer::NextName、writeSplitSql`；动态 trait 分派和 `FileFormat::WriteInsert` 等边需结合源码确认。
- `writeSplitSql → TableDataIR::Rows、SQLRowIter::HasNext/Error/Close、writeSQLFile、LazyStringWriter`。
- `writeMetaToFile → metaData、getSpecialComments、MetaIR::MetaSQL、LazyStringWriter`。
- `NextName → IndexStr → OutputTemplate::Execute`。

格式实现位于 [`writer_util.rs`](writer_util.rs)：`writeSQLFile`、`writeCSVFile` 消费 caller-owned iterator，`FileFormat::WriteInsert` 分派 SQL/CSV/Parquet，`LazyStringWriter` 在首次 `Write` 时才调用 `Storage::CreateWithOptions`。SQL 和 CSV 数据文件设置 `WriterOption { Concurrency: 4, PartSize: 5 MiB }`；Parquet 与 metadata 不设置该选项。

## 错误处理与边界

所有写路径返回 crate 的 `Result<()>`，使用 `?` 保留首个关键错误。需要注意具体优先级：

- 模板执行失败时还没有创建文件；元数据 view 的第二个模板在任何写入前就计算完成，但第一个文件成功、第二个文件写入失败时仍可能留下第一个文件。
- `WriteTableData` 在连接已被取走时立即报 `writer conn closed`；自定义 SQL 模式缺少 `RawRows` 时报 `raw rows unavailable for SQL query metadata`。
- CSV 和 split SQL 每个文件都先保存写入结果，再调用 `LazyStringWriter::Close`，之后按“写入错误优先、关闭错误其次”传播。整个循环后再关闭 iterator 和 IR；但源码使用顺序 `result?; closed_iter?; closed_ir`，所以当写入失败时，关闭动作虽已执行，其错误不会覆盖原错误。
- 普通格式路径也总会执行 writer 与 IR 的关闭，再按 `write_result → close_writer_result → close_ir_result` 的顺序返回错误。
- `ir.Start` 失败发生在任何显式关闭 guard 建立之前，本函数不会再调用 `ir.Close`。动态 metadata 推导失败或 `rows.Err()` 非空也会在进入后续关闭逻辑前直接返回，因此当前 Rust 代码在这些早退点不保证调用 `ir.Close`。
- `writeMetaToFile` 在 comment 或 SQL `Write` 失败时会立即返回，因而不会执行末尾的 `lazy.Close()`；这与数据路径显式保存关闭结果的做法不同。
- unknown format 仍会先生成 `unknown_format` 文件名并构造 lazy writer，但 `FileFormat::WriteInsert` 在首次写入前报错，所以按 lazy 语义不会创建空文件。
- 空数据由格式 helper 与 lazy create 共同处理：未发生 `Write` 时不创建底层对象。CSV/split SQL 的循环是否进入取决于 `iter.HasNext()`。

这些边界是当前源码事实；尤其不能把 Go 的 retry/connection rebuild/error metric 行为推断到 Rust 实现中。

## 并发与资源生命周期

`Writer` 字段满足跨线程所需的若干约束（`Storage: Send + Sync`、`TableDataIR: Send`、回调为 `Send`），但本文件不创建线程、不持有锁、不操作 channel。当前 [`dump.rs`](dump.rs) 用一个标准 mpsc receiver 和一个 writer 串行消费全部任务；这与 Go 注释所述 writer pool/work goroutine 不同。

连接由 `NewWriter` 放入 `Some(conn)`，在每个数据 IR 的 `Start` 中借用；整个消费循环结束后，`Dumper::Dump` 用 `writer.conn.take()` 获取所有权并关闭。Rust writer 不在失败后重建连接，也没有 Go 的 `rebuildConnFn`。

数据资源的预期正常生命周期为 `ir.Start → ir.Rows/写出 → iterator.Close → ir.Close`。CSV 和 split SQL 跨多个输出文件复用同一 iterator，文件级 `LazyStringWriter` 则每轮新建并关闭。普通 SQL/Parquet 把 IR 交给 `FileFormat::WriteInsert`；部分格式 helper 自己创建/关闭 iterator，随后 `WriteTableData` 仍负责关闭 IR。

`LazyStringWriter` 只在第一次写字节时打开对象，`Close` 通过 `Option::take` 保证底层 writer 最多关闭一次。SQL/CSV 的 multipart 参数是资源/性能契约：并发度 4、part size 5 MiB。这里的“并发度”属于对象存储单文件上传选项，不代表多个 `TaskEnum` 并行执行。

## 与 Go 版本的对应关系

直接对照文件是 [`dumpling/export/writer.go`](writer.go)，独立回归是 `writer_test.go`、`writer_serial_test.go` 与对应 Rust 测试。已对齐的主要语义包括：

- `NewWriter` 的格式解析与默认回调；六类任务的分派；最后一个 chunk 才触发表完成回调。
- schema/table/view/sequence/policy 模板选择及 `.sql` 后缀；view 输出两份 DDL。
- output namer 的三种零填充索引规则，以及每次 `NextName` 后增加 file index。
- SQL/CSV/Parquet 格式分派、Parquet 压缩后缀、SQL/CSV 的 4 路和 5 MiB 上传选项。
- custom SQL 根据结果集推导 metadata；切分输出复用 iterator 继续消费剩余行。

当前 Rust 与 Go 的重要差异/迁移缺口：

- Go `Writer.run` 监听 context 和 task channel，支持多个 writer；Rust 的接收循环在 `Dumper::Dump`，目前是单 writer 串行执行，且没有 context-cancel 分支。
- Go `WriteTableData` 包含 `utils.WithRetry`、backoff、失败指标和 `rebuildConnFn`；Rust 只尝试一次，连接失败后不会重建。
- Go 统一经 `buildInterceptFileWriter`/`buildFileWriter` 接入 `CompressType`、teardown 和“是否真正写入”的检测；Rust 直接使用 `LazyStringWriter`。数据文件仅按 Parquet 压缩类型改变扩展名，`Config::CompressType` 并未在本文件落到压缩 writer；metadata 也未接压缩。
- Go `tryToWriteTableData` 用 `SomethingIsWritten` 避免空轮次并记录日志；Rust 依赖 `HasNext` 和 lazy create，无相同日志。
- Go 有 unsupported dynamic task type 的 default 分支；Rust `TaskEnum` 是封闭枚举，`match` 穷举，因此新增变体会在编译期要求更新。
- Go `outputFileNamer` 还含 `Policy` 和独立 `render`；Rust policy 直接把参数传给 `OutputTemplate::Execute`，namer 仅服务数据文件。

因此本文描述的是当前 Rust 行为，不应仅因名字对应就宣称 retry、并行 writer 或通用压缩已完成移植。

## 扩展指南

- 新增任务类型：先在 `task.rs::TaskEnum` 增加载荷，再在 `Writer::handleTask` 添加分支；若属于表级完成语义，明确 callback 的成功条件，并在独立的 `writer_test.rs` 增加分派和计数回归。
- 新增输出格式：同步扩展 `writer_util.rs::FileFormat`、`NewWriter` 字符串映射、扩展名与 `WriteInsert` 分派；在 `WriteTableData` 判断它是否需要分文件、multipart option 或特殊后缀。未知格式必须继续显式失败。
- 修改切分策略或命名：集中修改 `newOutputFileNamer`、`IndexStr`、`NextName`，同步 `test_output_file_namer_matches_go_split_indices`，并核对 Go `outputFileNamer`。改变命名是兼容性风险，会影响增量消费、对象覆盖和恢复脚本。
- 接入 retry/连接重建：最可能改动 `Writer` 状态和 `WriteTableData`；必须定义每次重试前后的 IR 可重启性、已创建文件的覆盖/清理、计数与 callback 幂等性，不能只包一层重试循环。
- 接入压缩：应复用/补齐统一 writer builder，而不是仅改后缀；metadata、SQL、CSV、Parquet 的压缩配置含义不同，需与 Go 的 `buildFileWriter`/`buildInterceptFileWriter` 对照。
- 调整关闭逻辑：保持“尝试关闭所有已获得资源，同时优先返回业务写错误”的原则，并为 `Start` 后 metadata 推导失败、write 失败、close 失败分别增加 fault-injection 测试。
- 性能关注点：文件大小切分循环、每文件重新创建对象、multipart 参数和 iterator 复用直接影响内存、对象数量与吞吐。不要让 `Rows()` 在同一数据流上被无意重复创建。

Rust 测试逻辑应继续放在独立的 [`writer_test.rs`](writer_test.rs) 或 [`writer_serial_test.rs`](writer_serial_test.rs)，不要嵌回生产文件；并应尽量与 Go 的 `writer_test.go` / `writer_serial_test.go` 测试意图一致。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter dumpling/export/writer.rs` 确认目标文件有 25 个符号。
- RustCodeGraph 源码读取：`node --file dumpling/export/writer.rs --offset 1 --limit 800` 覆盖目标文件 1–379 行。
- RustCodeGraph 精确查询：`query` 确认 `NewWriter`、`handleTask`、`WriteTableData`、`writeSplitSql`、`writeMetaToFile`、`countTotalTask`、`newOutputFileNamer`、`IndexStr`、`NextName` 的定义位置；`callees --file dumpling/export/writer.rs` 验证 `handleTask` 的六条分派边、`WriteTableData` 的 IR/namer 边、`writeMetaToFile` 的 metadata 边和 `NextName → IndexStr`。file-scoped callers 对部分符号无输出，故上游调用由 `dump.rs` 的直接调用点补证。
- 已读生产代码：`dumpling/export/writer.rs`、`lib.rs`、`dump.rs`、`task.rs`、`ir.rs`、`writer_util.rs`、`stubs.rs`、`Cargo.toml`；Go 对照为 `dumpling/export/writer.go`，并通过 `dump.go` 核对 writer 的上游位置。
- 已读测试：`dumpling/export/writer_test.rs` 覆盖 metadata 内容、SQL 数据写入、file/statement split、namer、最后 chunk callback、CSV 分片完整性和 multipart option；`writer_serial_test.rs` 覆盖 metadata/SQL/CSV 编码及错误与指标。对应 Go 证据来自 `writer_test.go`、`writer_serial_test.go`。
- 人工复核结论：本文件存在于任务消费与格式 writer 之间；运行时由 `Dumper::Dump` 驱动；安全扩展必须同时维护任务分派、命名、资源关闭、独立 Rust 测试与 Go 语义对照。
