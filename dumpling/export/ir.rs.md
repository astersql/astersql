# `dumpling/export/ir.rs` 逻辑说明

## 文件定位

`dumpling/export/ir.rs` 是 `astersql-dumpling-export` crate 的中间表示（IR）协议层。crate 入口 `dumpling/export/lib.rs` 通过 `include!("ir.rs")` 将它拼入同一个包级命名空间；紧邻的 `task.rs` 先定义任务载体，`ir_impl.rs` 再提供这些协议的具体实现。该文件不是独立模块，也不负责查询编排或文件格式编码，而是在任务生产端、数据库结果集适配层和 writer 消费端之间定义稳定边界。

`dumpling/export/Cargo.toml` 将 crate 声明为 library，并以 `package.metadata.porting.go-package = "dumpling/export"` 标明 Go 来源。目标文件直接使用的 `Rows`、`Conn`、`RawBytes`、`Error`、`ServerType` 和 `ColumnInfo` 来自同 crate 的 `stubs.rs`；`tcontext::Context` 来自路径依赖 `astersql-dumpling-context`。Parquet 列元数据在本文件中通过 `ColumnInfo` 暴露，实际格式依赖由 crate 的 `astersql-dumpformat-parquetfile` 承担。

## 核心职责

本文件有三类职责：

1. 用 `TableDataIR`、`SQLRowIter` 和 `RowReceiver` 隔离“启动查询、推进结果集、扫描当前行、关闭资源”与 SQL/CSV/Parquet writer 的具体编码逻辑。
2. 用 `TableMeta`、`MetaIR` 和 `StringIter` 向 writer 提供表结构、DDL、特殊注释及目标名，同时隐藏元数据的具体存储类型。
3. 用 `decodeFromRows` 和 `setTableMetaFromRows` 提供两个共享适配点：前者把当前 `Rows` 行解码到接收器，后者从任意查询结果推导匿名 `TableMeta`，主要服务自定义 SQL 导出路径。

因此，这个文件存在的价值是约束跨层交互和生命周期，而不是实现某一种导出格式。真实运行实现主要位于 `dumpling/export/ir_impl.rs`，格式消费逻辑主要位于 `dumpling/export/writer.rs` 与 `dumpling/export/writer_util.rs`。

## 主要符号

- `TableDataIR: Send`：可跨线程转移的表数据源协议。`Start` 用上下文和连接建立运行态；`Rows` 按值交出 `Box<dyn SQLRowIter>`；`Close` 回收仍由数据源持有的资源；`RawRows` 仅在需要直接检查底层结果集时返回可变借用。主要实现是 `ir_impl.rs` 的 `tableData` 和 `multiQueriesChunk`，测试替身见 `util_for_test.rs::mockTableIR`。
- `TableMeta: Send + Sync`：可在线程间共享的只读表元信息协议。公开 getter 覆盖库表名、列数、列名/类型、SELECT 字段、建表/建视图 SQL、平均行长、隐式 row ID 和 Parquet 所需 `ColumnInfos`。内部兼容方法 `sourceColumnNames`、`sourceColumnTypes` 默认回退到导出列，允许实现覆盖原始列投影；`ir_impl.rs::tableMeta` 会覆盖二者。
- `SQLRowIter: Send`：结果集游标协议。约定 `HasNext` 只观察状态，`Decode` 解码当前行，`Next` 才推进，`Error` 返回延迟错误，`Close` 释放游标。主要实现是 `ir_impl.rs::rowIter` 和 `multiQueriesChunkIter`。
- `RowReceiver`：扫描目标协议，唯一方法 `BindAddress(&mut [RawBytes])`。生产实现 `sql_type.rs::RowReceiverArr` 将 `RawBytes` 复制到 writer 可消费的列缓冲。
- `StringIter: Send`：特殊注释等短字符串序列的最小游标协议。实现 `ir_impl.rs::stringIter` 要求先检查 `HasNext` 再调用 `Next`。
- `MetaIR: Send`：非行数据元信息协议，向 `WriteMeta` 暴露特殊注释、目标名和最终 SQL。实现 `ir_impl.rs::metaData` 会在 `MetaSQL` 中补齐 `;\n`。
- `decodeFromRows`：先调用 `RowReceiver::BindAddress`，再调用 `Rows::Scan`；扫描失败时关闭 `Rows` 并以 `errors_trace` 包装错误；成功后再次绑定，把 Rust 值复制式 adapter 中已填充的 `RawBytes` 写回接收器。
- `setTableMetaFromRows`：读取 `Rows::ColumnTypes` 与 `Rows::Columns`，用 `wrapBackTicks` 转义列名并以逗号连接为 `selected_field`，生成库名、表名及 DDL 均为空的 `tableMeta`，同时通过 `getSpecialComments(server_type)` 注入数据库方言前置语句。

本文件没有模块级常量、结构体、枚举、条件编译项或私有函数；六个 trait 和两个函数均处于 crate 根的公开命名空间。

## 执行流程

表数据主链由 `task.rs::TaskTableData` 持有 `Box<dyn TableMeta>` 和 `Box<dyn TableDataIR>`，再由 `TaskEnum::TableData` 交给 `writer.rs::Writer::WriteTableData`：

1. `WriteTableData` 从 writer 取得连接并调用 `TableDataIR::Start`。`tableData::Start` 立即执行单条查询；`multiQueriesChunk::Start` 只缓存上下文与连接，等 `Rows` 时再逐条查询。
2. 自定义 SQL 模式下，writer 在交出游标前通过 `RawRows` 取得底层 `Rows`，调用 `setTableMetaFromRows` 推导动态列元信息；普通表导出继续使用任务携带的 `TableMeta`。
3. writer 根据格式进入 `writeCSVFile`、`writeSQLFile` 或 Parquet 路径。它们通过 `TableDataIR::Rows` 取得游标，以 `HasNext → Decode → Next` 的顺序消费；SQL/CSV/Parquet 的行接收器来自 `sql_type.rs::MakeRowReceiver`。
4. 每次 `Decode` 最终由 `rowIter::Decode` 或 `multiQueriesChunkIter::Decode` 调用 `decodeFromRows`。成功扫描后第二次 `BindAddress` 将当前行从 `RawBytes` 复制到 `RowReceiverArr`。
5. 格式 writer 检查 `SQLRowIter::Error` 并关闭迭代器；`Writer::WriteTableData` 随后关闭输出 writer 和 `TableDataIR`。分文件 SQL/CSV 会复用同一迭代器，从当前位置继续写下一文件。

元信息主链较短：`writer.rs::writeMetaToFile` 构造 `ir_impl.rs::metaData`，依次消费 `MetaIR::SpecialComments` 和 `MetaIR::MetaSQL` 并写入 `LazyStringWriter`；通用等价入口是 `writer_util.rs::WriteMeta`。

## 数据与状态

接口本身不保存状态，但其方法签名明确了实现必须管理的状态：

- `TableDataIR` 的典型状态从“未启动”变为“已持有 rows/运行环境”，再由 `Rows` 把游标所有权交给消费端，最后进入关闭态。当前 Rust 实现的 trait 按值返回迭代器，因此 `tableData::Rows` 和 `multiQueriesChunk::Rows` 实际采用单次取走模型；不能假定同一个实例可重复完整消费。
- `SQLRowIter` 将“当前位置是否有效”缓存在 `has_next`。`newRowIter` 构造时预取首行，所以调用方先检查 `HasNext`，解码后才调用 `Next`。多查询实现还保存查询下标、当前 `Rows` 和终态错误，以把多个结果集表现为一个连续流。
- `decodeFromRows` 接收调用方复用的 `args: &mut [RawBytes]`。这避免每行重新分配扫描槽；`RawBytes(None)` 同时表示 SQL `NULL`。槽位数必须与结果列数一致，否则 `Rows::Scan` 返回列数错误。
- `TableMeta` 返回列名/类型的拥有型 `Vec<String>`，而标量字符串 getter 返回借用，兼顾跨线程任务所有权和读取成本。`SpecialComments` 每次返回新的 boxed iterator，避免不同写出过程共享消费位置。
- `setTableMetaFromRows` 克隆列类型到 `source_col_types` 和 `col_types`；它构造的是查询结果元信息，不含真实库表身份、SHOW CREATE SQL、平均行长或隐式 row ID 信息。

## 依赖与调用关系

上游调用与持有关系：

- `task.rs::TaskTableData` 是 `TableMeta`/`TableDataIR` 的任务边界；`dump.rs` 构造表元信息和数据任务，`sql.rs::SelectAllFromTable` 等入口返回 `Box<dyn TableDataIR>`。
- `writer.rs::Writer::WriteTableData` 是 `TableDataIR::Start`、`RawRows` 和动态 `setTableMetaFromRows` 的直接生产调用者；`writer_util.rs` 的 SQL/CSV/Parquet 写出函数消费 `Rows`、`TableMeta`、`SQLRowIter` 和 `RowReceiver`。
- `writer_util.rs::WriteMeta` 和 `writer.rs::writeMetaToFile` 消费 `MetaIR`/`StringIter`；后者当前也直接调用 `metaData` 的相同接口。

下游实现与函数调用：

- `decodeFromRows → RowReceiver::BindAddress → Rows::Scan/Close → errors_trace`。直接调用者在 `ir_impl.rs` 中是 `rowIter::Decode` 与 `multiQueriesChunkIter::Decode`。
- `setTableMetaFromRows → Rows::ColumnTypes/Columns → wrapBackTicks → getSpecialComments → tableMeta`。直接生产调用点是 `writer.rs::Writer::WriteTableData` 的自定义 SQL 分支。
- `TableMeta::ColumnInfos` 最终被 `writer_util.rs::parquet_columns` 转换为 `astersql_dumpformat_parquetfile::ColumnInfo`；`ColumnTypes`、`ColumnNames` 和 `SelectedField` 则驱动 SQL/CSV 编码和表头。

RustCodeGraph 的文件节点显示 `ir.rs` 被 `dump.rs`、`sql.rs`、`writer.rs`、`writer_util.rs`、相关测试等 10 个文件使用，并识别 `ir.rs::decodeFromRows` 与 `ir.rs::setTableMetaFromRows` 为独立 Rust 函数。由于本地 `callers` 子命令对这两个符号查询持续超过 60 秒无结果，具体调用边以限定在 `dumpling/export` 的符号搜索和上述调用点源码复核为准。

## 错误处理与边界

- `decodeFromRows` 保证第一次绑定发生在扫描之前，即使 `Scan` 失败也保留该可观察副作用；失败后忽略 `Rows::Close` 自身的返回值，优先返回被追踪的扫描错误。这一优先级与 Go `ir.go::decodeFromRows` 一致。
- Rust 的 `RawBytes` adapter 不像 Go `database/sql` 那样绑定指针，所以成功扫描后必须第二次调用 `BindAddress` 才能把值复制进接收器。这是 Rust 相对 Go 的必要实现差异，删除第二次绑定会导致 writer 看到扫描前的空值或旧值。
- `setTableMetaFromRows` 对 `ColumnTypes` 或 `Columns` 的错误立即用 `?` 传播，不返回部分元信息。列名通过 `wrapBackTicks` 转义内部反引号；结果拼接不额外插入空格，与 Go `strings.Join(nms, ",")` 一致。
- trait 没有强制调用顺序，但当前实现要求 `Start` 先于有效的 `Rows`/`RawRows`，并要求消费循环遵守 `HasNext → Decode → Next`。在没有当前行、rows 已关闭或扫描槽数不匹配时，`Decode` 会失败。
- `SQLRowIter::Error` 与 `Decode` 错误是两条通道：前者承载推进/结果集终态错误，后者承载当前行扫描错误。writer 必须同时处理，不能只依赖循环退出。
- `StringIter::Next` 的接口不返回错误或 `Option`；当前 `stringIter` 越界返回空串。因此安全扩展应继续要求调用方先检查 `HasNext`，不能用空串作为终止信号。

## 并发与资源生命周期

`TableDataIR`、`SQLRowIter`、`StringIter` 和 `MetaIR` 要求 `Send`，`TableMeta` 额外要求 `Sync`。这使任务及其数据/元信息对象可进入 dumpling 的并发 writer 工作流；它不表示单个可变迭代器可被多个线程同时推进。`&mut self` 和按值交出的 `Box<dyn SQLRowIter>` 保持单一消费者语义。

资源所有权沿 `TableDataIR::Start → Rows/RawRows → SQLRowIter::Close → TableDataIR::Close` 转移。`decodeFromRows` 在扫描失败时主动关闭底层 rows；正常路径由格式 writer 关闭迭代器，再由 `Writer::WriteTableData` 关闭 IR。当前实现力求关闭幂等，但 trait 本身没有声明幂等保证，新实现仍应支持 writer 的兜底关闭顺序，并确保早退错误不泄漏结果集。

分片/多查询数据源要特别注意所有权：`multiQueriesChunkIter` 切换查询前关闭上一结果集，并把关闭、查询或 rows 错误缓存为终态错误；`TableDataIR::Rows` 交出迭代器后，IR 可能不再持有该资源，所以外部消费者必须负责关闭。`TableMeta` 只读且 `Send + Sync`，适合在任务编排中共享；`SpecialComments` 返回独立迭代器，避免共享可变游标。

## 与 Go 版本的对应关系

`dumpling/export/ir.go` 是直接语义基线。六个 Rust trait 对应六个 Go interface，两个 Rust helper 对应同名 Go 函数；方法名和总体调用顺序刻意保留 Go 风格。关键差异如下：

- Go 的 `TableDataIR.Rows()` 返回 interface 引用语义，Rust 返回拥有型 `Box<dyn SQLRowIter>`；Rust 实现因此明确采用取走/单消费者模型。
- Go `TableMeta.ColumnInfos()` 返回 `[]*ColumnInfo`，Rust 返回拥有型 `Vec<ColumnInfo>`；Rust 的 `ColumnInfo` 来自本地适配类型，之后再投影到 Parquet crate 类型。
- Rust 为 `TableMeta` 增加了带默认实现的 `sourceColumnNames`/`sourceColumnTypes`，用于保留原始列与导出列之间的投影；Go 当前在 `ir_impl.go` 通过包级 helper `tableSourceColumnNames`/`tableSourceColumnTypes` 处理等价兼容逻辑。
- Go 的 `BindAddress` 把 `sql.Rows.Scan` 目标绑定到接收器地址，扫描后数据自然可见；Rust stubs 使用值复制，故 `decodeFromRows` 成功扫描后多一次 `BindAddress`。失败前绑定与关闭 rows 的行为仍与 Go 对齐。
- Go 使用 `[]any` 和 `*sql.Rows`，Rust 使用 `&mut [RawBytes]` 与本地 `Rows`；错误分别通过 `error`/`errors.Trace` 和 `Result`/`errors_trace` 传播。
- `setTableMetaFromRows` 两端都复制列类型、反引号包裹列名、以逗号连接字段并注入特殊注释；Rust 显式填写其余 `tableMeta` 字段的零值，以满足结构体完整初始化。

Go 没有独立的 `ir_test.go`；共享迭代语义主要由 `dumpling/export/ir_impl_test.go` 覆盖。Rust 新增 `ir_test.rs` 专门锁定值复制 adapter 下“扫描失败前已绑定且 rows 被关闭”的回归行为，这是实现差异的验证，不是功能简化。

## 扩展指南

新增数据源时应实现 `TableDataIR`，并优先复用 `rowIter` 或实现完整的 `SQLRowIter`。必须明确：`Start` 何时执行查询、`Rows` 是否只能调用一次、`RawRows` 是否可用、游标交出后由谁关闭、推进错误如何通过 `Error` 暴露。对应测试应放在独立的 `*_test.rs`，可扩展 `ir_impl_test.rs`；跨 writer 的资源与错误传播应同步扩展 `writer_serial_test.rs` 或 `writer_util_test.rs`，不要把测试嵌入生产文件。

新增输出格式所需元数据时，先判断它属于所有表的稳定属性还是格式私有投影。稳定属性可扩展 `TableMeta`，但必须同步更新 `ir_impl.rs::tableMeta`、`util_for_test.rs::mockTableIR` 以及所有自定义实现；格式私有信息更适合在 writer 侧从 `ColumnInfos` 投影，以避免扩大所有实现者的兼容成本。修改列投影还要同步核对 `setTableMetaFromRows` 的匿名元信息和 Go 对照。

修改 `decodeFromRows` 时应保持三项不变量：扫描前绑定的可观察顺序、扫描失败关闭 rows 并返回原扫描错误、成功后将 Rust `RawBytes` 复制回接收器。至少同步 `ir_test.rs` 的错误回归、`ir_impl_test.rs::test_row_iter` 的成功解码，以及 `sql_type_test.rs` 对 `RowReceiverArr` 的 NULL/二进制值覆盖。

修改生命周期或并发边界时，兼容风险高于局部语法风险：放宽/收紧 `Send + Sync` 会影响任务跨线程传递；改变 `Rows` 的所有权会影响分文件续写；改变 `Close` 错误优先级可能掩盖原始扫描或推进错误。性能上应保留每个迭代器复用 `args` 的策略，避免逐行分配，并避免让 `ColumnTypes`/`ColumnNames` 的克隆进入更内层的逐列热循环。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件；`files --filter dumpling/export` 包含 `ir.rs`、`ir_impl.rs`、writer 与测试；`node --file dumpling/export/ir.rs` 读取完整 114 行并报告 10 个使用文件；`query decodeFromRows` 与 `query setTableMetaFromRows` 分别定位 Go/Rust 同名函数。精确 `callers` 查询超过 60 秒无输出后终止，调用边改由限定目录搜索和源码调用点验证。
- 源与 crate 边界：`dumpling/export/ir.rs`、`dumpling/export/lib.rs`、`dumpling/export/Cargo.toml`、`dumpling/export/stubs.rs`。
- 直接实现与调用边：`dumpling/export/ir_impl.rs`、`dumpling/export/task.rs`、`dumpling/export/writer.rs`、`dumpling/export/writer_util.rs`、`dumpling/export/sql_type.rs`、`dumpling/export/sql.rs`、`dumpling/export/dump.rs`。
- Go 对照：`dumpling/export/ir.go`、`dumpling/export/ir_impl.go`、`dumpling/export/writer.go`、`dumpling/export/writer_util.go`、`dumpling/export/ir_impl_test.go`。
- Rust 测试证据：`dumpling/export/ir_test.rs::decode_from_rows_binds_receiver_before_scan_error` 验证失败前绑定、关闭和错误文本；`ir_impl_test.rs::test_row_iter` 验证预取、稳定 `HasNext`、解码及推进；`ir_impl_test.rs::test_chunk_row_iter` 验证关闭后解码失败；`writer_serial_test.rs` 的错误迭代器验证行级终态错误传播；`sql_type_test.rs` 验证接收器的 NULL 与原始字节复制。
- 人工复核结论：本文已回答该文件为何存在、如何沿任务—writer—迭代器主链运行、数据/错误/关闭状态如何流动，以及新增实现或元数据字段时必须同步的生产符号和独立测试。任务为纯文档分析，按计划未运行 Cargo。
