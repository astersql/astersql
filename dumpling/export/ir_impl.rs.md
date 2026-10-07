# [`dumpling/export/ir_impl.rs`](ir_impl.rs)

## 文件定位

`ir_impl.rs` 是 `astersql-dumpling-export` crate 中导出中间表示（IR）的具体实现层。crate 入口 [`dumpling/export/lib.rs`](lib.rs) 先 `include!("ir.rs")` 定义协议，再在第 63 行 `include!("ir_impl.rs")`，最后加载 writer 与 dump 流程；因此本文件不是独立模块，而是和同 crate 的 [`ir.rs`](ir.rs)、[`writer.rs`](writer.rs)、[`dump.rs`](dump.rs) 共享一个包级命名空间。

[`dumpling/export/Cargo.toml`](Cargo.toml) 将该目录声明为库 crate，入口为 `lib.rs`，迁移元数据指向 Go 包 `dumpling/export`。文件直接使用的数据库游标、连接、错误、日志和服务端类型等基础对象来自同 crate 的 [`stubs.rs`](stubs.rs) 及 `lib.rs` 引入的 `astersql-dumpling-context`、`astersql-dumpling-log`；它并不自行负责 CLI、任务调度或存储写入。

## 核心职责

本文件把 `ir.rs` 中的五组抽象落到可运行对象上：

- `rowIter` 和 `multiQueriesChunkIter` 实现 `SQLRowIter`，把一个或多个查询结果表现为“检查、解码、推进、取错、关闭”的统一行流。
- `tableData` 和 `multiQueriesChunk` 实现 `TableDataIR`，分别承载单条查询和多条查询拼接的数据源生命周期。
- `tableMeta` 实现 `TableMeta`，向 SQL/CSV/Parquet writer 提供库表名、列投影、类型、建表语句及统计信息。
- `metaData` 实现 `MetaIR`，统一输出数据库、表、视图、序列等元 SQL，并保证 SQL 以 `;\n` 结束。
- `stringIter` 与 `getSpecialComments` 提供方言相关的前置 SQL 注释流。

它位于“查询构造/元信息探测”和“文件 writer”之间：`sql.rs::SelectAllFromTable`、`dump.rs::dumpSQL`/`dumpTableMeta` 构造这些对象，`writer.rs::Writer::WriteTableData` 和 `writeMetaToFile` 消费它们。

## 主要符号

- `newRowIter(rows, arg_len) -> rowIter`：分配可复用的 `Vec<RawBytes>`，并立即调用一次 `Rows::Next` 预取首行。`HasNext` 只读缓存，不推进游标；`Decode` 委托 `ir.rs::decodeFromRows`。
- `newMultiQueryChunkIter(tctx, conn, queries, arg_len)` / `multiQueriesChunkIter::nextRows`：依序执行 SQL，关闭并检查上一结果集，跳过空结果集，把第一个非空结果集暴露给上层；查询、关闭或结果集错误缓存在 `err`。
- `newStringIter(Vec<String>) -> Box<dyn StringIter>`：构造拥有字符串集合的顺序迭代器；越界 `Next` 返回空串，正确调用约定是先检查 `HasNext`。
- `newTableData(query, col_length, need_col_types)` / `tableData`：保存单 SQL 及可选的动态列类型探测状态。`Start` 执行查询；`Rows` 将结果集包装成 `rowIter`；`RawRows` 允许 `--sql` 路径在消费前推导匿名元信息。
- `tableMeta`：保存 `database`、`table`、选中列与源列两套 `ColumnType`、`selected_field`、特殊注释、建表/建视图 SQL、平均行长和隐式 row ID 标志。`ColumnInfos` 将驱动列类型投影为稳定的导出层 `ColumnInfo`。
- `metaData`：保存目标名、元 SQL 和特殊注释；`MetaSQL` 会原地补齐结尾后返回克隆。
- `newMultiQueriesChunk(queries, col_length)` / `multiQueriesChunk`：`Start` 缓存上下文和连接，`Rows` 才惰性创建跨查询迭代器，`RawRows` 固定为 `None`。
- `getSpecialComments(ServerType)`：MySQL/TiDB 返回关闭外键检查与 `SET NAMES binary` 的版本注释；MariaDB 使用不同的外键检查语句；其他类型返回空集合。

这些结构体和构造函数在当前迁移 crate 中多为 `pub`，但语义上仍是 Go 包内实现；稳定交互边界应视为 `ir.rs` 的 trait，而不是直接依赖字段布局。

## 执行流程

单查询整表路径如下：

1. `sql.rs::SelectAllFromTable` 组装 `SELECT`，以 `meta.SelectedLen()` 作为扫描列数调用 `newTableData(..., false)`；`dump.rs::Dumper::dumpWholeTableDirectly` 将它封装为表数据任务。
2. `writer.rs::Writer::WriteTableData` 调用 `TableDataIR::Start`。`tableData::Start` 经 `Conn::QueryContext` 打开 `Rows`，清除旧迭代状态；若 `need_col_types` 为真，还读取真实列数和数据库类型名。
3. writer 在需要行时调用 `Rows`。`newRowIter` 立即预取首行，随后 writer 循环执行 `HasNext -> Decode -> Next`。`Decode` 经 `decodeFromRows` 先绑定扫描槽、扫描，再把扫描结果绑定回接收器。
4. writer 按格式写出数据，并关闭它持有的行迭代器；最后再调用 IR 的 `Close` 做剩余资源回收。

`--sql` 路径由 `dump.rs::dumpSQL` 用 `newTableData(conf.SQL, 0, true)` 创建匿名 `result` 数据源。`WriteTableData` 在 `Start` 后通过 `RawRows` 调用 `ir.rs::setTableMetaFromRows`，取得真实列名和类型，然后才生成输出名并消费数据。

多查询路径中，`multiQueriesChunk::Start` 只缓存环境；第一次 `Rows` 构造 `multiQueriesChunkIter`，其构造函数立即运行 `nextRows`。当前结果集耗尽后，`Next` 再调用 `nextRows`：先关闭旧结果集并检查错误，再执行后续 SQL，自动跳过空集，直至出现一行数据、发生错误或查询全部耗尽。因此 writer 看见的是一个连续行流。

元信息路径中，`writer.rs::writeMetaToFile` 构造 `metaData`，先遍历 `SpecialComments`，再写入经 `MetaSQL` 规范化结尾的正文，最后关闭存储 writer。

## 数据与状态

`rowIter` 的关键不变量是：`has_next` 描述构造或最近一次 `Next` 后底层游标是否停在有效行；`args` 长度必须匹配查询结果列数。重复调用 `HasNext` 不改变位置，`ir_impl_test.rs::test_row_iter` 用 100 次连续检查锁定了这一性质。

`multiQueriesChunkIter::id` 指向下一条尚未启动的查询，`rows` 保存当前结果集，`err` 保存跨结果集切换时的终态错误。所有查询共用一个 `args` 缓冲；前一结果集必须成功关闭且 `Err()` 为空，才允许启动下一查询。

`tableData` 在 `Start` 前只有查询配置；`Start` 后拥有 `rows`；调用 `Rows` 时通过 `Option::take` 把 `Rows` 的所有权移入迭代器，再把 trait object 按值交给调用方。因而 Rust 版的有效数据迭代器是消费式移交：首次取走后，`tableData` 自身不再持有该迭代器；再次调用 `Rows` 会基于缺失的 `rows` 产生空哨兵迭代器，而不是重新执行查询。调用方必须保留并关闭首次返回值。

`multiQueriesChunk::Rows` 同样按值移交迭代器，但在迭代器取走后再次调用会依据已缓存的上下文、连接和查询列表新建另一条查询链；这会重新执行 SQL，不应当被当成可重复读取同一流的接口。`RawRows == None` 也意味着该类型不能服务需要在写入前直接检查原始结果集的 `--sql` 动态元信息路径。

`tableMeta` 区分 `col_types`（实际导出投影）与 `source_col_types`（源表列集合）。`sourceColumnNames/sourceColumnTypes` 供需要保留源 schema 语义的逻辑使用，其余列 getter 基于投影列。所有 getter 返回借用或新建集合，不修改元信息本体；`SpecialComments` 每次克隆数据并创建独立迭代器。

## 依赖与调用关系

RustCodeGraph 给出的关键调用边包括：

- `sql.rs::SelectAllFromTable -> newTableData`；`dump.rs::dumpSQL -> newTableData`。
- `dump.rs::dumpTableMeta`、`dumpSQL`，`ir.rs::setTableMetaFromRows`，`writer.rs::writeMetaToFile` 以及 `parity_test.rs::contract_normal -> getSpecialComments`。
- `writer.rs::Writer::WriteTableData -> TableDataIR::Start/RawRows/Rows/Close`；CSV、分片 SQL 和普通格式分支最终都通过该协议消费数据。
- `writer.rs::writeMetaToFile -> metaData::SpecialComments/MetaSQL`。

下游核心依赖为 `Conn::QueryContext`、`Rows::{Next,Scan,Close,Err,Columns,ColumnTypes}`、`ir.rs::decodeFromRows`、`ColumnType`/`ColumnInfo`、`tcontext::Context` 日志和 errors 辅助函数。上层不需要知道 `rowIter` 的字段布局，只依赖 `SQLRowIter`。

RustCodeGraph 对 Rust 源只找到 `newMultiQueriesChunk` 的自定义义边，没有发现 `dump.rs` 等生产调用者；Go 版则由 `dump.go::buildConcatTask` 调用。故在当前 Rust 索引所代表的代码中，多查询类型已有实现但未证实接入生产导出主链，不能据 Go 调用关系宣称 Rust 已启用该路径。

## 错误处理与边界

`tableData::Start` 为查询启动错误和启动后 `Rows::Err` 添加具体 SQL 上下文。动态列探测中的 `Columns`/`ColumnTypes` 错误直接传播。`rowIter::Decode` 和多查询 `Decode` 会在已关闭的 `Rows` 上返回 `sql: Rows are closed`；扫描失败的关闭行为由 `ir.rs::decodeFromRows` 承担。

多查询迭代器把以下情况视为终态：关闭上一结果集失败、上一结果集的延迟错误、新结果集启动失败、新结果集立即报告错误。错误存入 `err` 后 `has_next=false`，`Error`、`Decode` 和 `Close` 都能向调用方暴露它；没有有效结果集时 `Decode` 返回带当前 `id` 的错误。空结果集本身不是错误，会继续下一 SQL。

边界约定包括：

- `newRowIter` 会预取，因此创建迭代器本身已经改变游标位置。
- `Decode` 只能在 `HasNext` 为真且结果集未关闭时调用；`stringIter::Next` 也要求先检查 `HasNext`。
- 扫描槽数量必须与查询列数匹配；正常表路径取自 `TableMeta::SelectedLen`，动态 SQL 路径在 `Start` 中重新计算。
- `metaData::MetaSQL` 只判断严格后缀 `;\n`；其他结尾（包括单独分号）会再追加 `;\n`。
- `ColumnInfos` 使用 `ColumnType::Nullable` 和 `DecimalSize` 的返回值构造信息；Rust 当前实现忽略这些 API 的“信息是否可用”辅助标志，需由对应 stub/驱动语义保证默认值合理。
- `tableData::Rows` 内部对未初始化状态使用默认空 `Rows`，而 `multiQueriesChunk::Rows` 未经 `Start` 时会回退到后台 context 和默认连接；这避免部分 panic，但不代表这种调用顺序是有效业务用法。

## 并发与资源生命周期

`TableDataIR`、`SQLRowIter`、`StringIter` 要求 `Send`，`TableMeta` 要求 `Send + Sync`，允许任务或只读元信息跨线程传递；具体实现没有内部锁，游标推进、`err`、`id` 和 `iter` 都要求单一可变持有者顺序访问。`tableMeta` 的并发安全来自只读 getter 和拥有型字段，而不是运行时同步。

连接生命周期由 writer/Dumper 管理，本文件只克隆或借用 `Conn`。单查询结果集从 `tableData::Start` 打开，随后所有权移交给 `rowIter`；多查询结果集由 `multiQueriesChunkIter` 逐个打开，并在切换前关闭上一项。writer 的 CSV 和分片 SQL 分支显式保存、关闭从 `Rows()` 取得的迭代器；由于 Rust trait 按值返回，迭代器一旦移交，随后 `ir.Close()` 无法替代调用方关闭它。

`Close` 被设计为无资源时成功，但不应误解为所有路径都可任意重复关闭：底层 `Rows::Close` 的幂等性由其实现决定，而且多查询 `Close` 在已有终态错误时优先返回该错误。没有后台线程、异步任务或通道在本文件中创建；背压完全由同步的 `HasNext/Decode/Next` 循环形成。

## 与 Go 版本的对应关系

直接对照文件是 [`dumpling/export/ir_impl.go`](ir_impl.go)，独立测试是 [`ir_impl_test.go`](ir_impl_test.go) 与 [`ir_impl_test.rs`](ir_impl_test.rs)。Rust 保留了 Go 的类型划分、Go 风格方法名、首行预取、跨空查询跳转、元 SQL 后缀修正、列元信息投影和不同服务端 special comments。

主要语言适配如下：Go 的 `*sql.Rows`/`*sql.Conn` 对应 Rust `Rows`/`Conn`；Go `[]any` 扫描目的地对应复用的 `Vec<RawBytes>`；Go `error` 对应 `Result` 与 `Option<Error>`；Go 接口嵌入对应 Rust 的显式 `Option<Box<dyn SQLRowIter>>` 和 trait 实现。

需要特别注意的差异是 Go 的 `tableData.Rows()` 返回并缓存同一个接口值，Rust 因 trait 签名返回 `Box` 所有权而用 `Option::take` 移交，重复调用语义不等价。Go 的 `multiQueriesChunk` 生产路径由 `buildConcatTask` 接线，但当前 Rust 调用图未证明同等接线。Rust 还显式检查 closed 状态，并在部分无资源状态返回成功或空哨兵，属于迁移时为所有权/桩类型增加的防护；扩展时应以可观察行为和独立测试为准，不能仅按结构逐行翻译 Go。

两侧 `TestRowIter/test_row_iter` 都验证首行预取、重复 `HasNext` 不推进、解码与 `Next` 的顺序；`TestChunkRowIter/test_chunk_row_iter` 都验证部分消费后仍有下一行，以及关闭底层结果集后解码报错。现有同名测试没有直接覆盖多查询切换、`tableData` 重复 `Rows`、`metaData` 后缀和 special comments；相关修改必须补充独立 Rust 测试，不能把测试内嵌回生产文件。

## 扩展指南

新增单查询数据源行为时优先修改 `tableData::{Start,Rows,Close,RawRows}`，并同步检查 `Writer::WriteTableData` 的四类格式分支和 `--sql` 动态元信息路径。若改变迭代协议，应同时修改 `ir.rs::SQLRowIter`/`TableDataIR`、`decodeFromRows` 及所有 writer 消费循环，明确谁拥有并关闭返回的迭代器。

新增 chunk 拼接策略时应以 `multiQueriesChunkIter::nextRows` 为唯一查询切换点，保持“先关闭并检查旧 rows、空集继续、错误终止、查询顺序不变”的不变量；还应先确认 Rust 生产任务构造处是否需要真正接入 `newMultiQueriesChunk`。需要覆盖的独立测试至少包括：首个/中间/末尾空集、查询启动失败、关闭失败、延迟 `Rows::Err`、全部为空、第二次 `Rows` 的明确契约及资源关闭次数。

新增列属性时应扩展 `ColumnInfo` 与 `tableMeta::ColumnInfos`，同时检查 SQL、CSV、Parquet writer 对 `ColumnTypes/ColumnNames/sourceColumn*` 的使用，避免混淆源列与投影列。新增服务端方言时修改 `getSpecialComments`，并为语句内容和顺序增加 parity/单元测试；这些语句会进入导出文件，存在导入兼容和安全风险。

性能上应继续复用 `args`，避免逐行分配；不要在热路径的 `HasNext` 或 `Next` 中克隆查询/元信息。正确性风险集中在预取造成的游标偏移、列数不匹配、错误被 `Close` 掩盖和迭代器所有权丢失。Rust 单元测试继续放在 `dumpling/export/ir_impl_test.rs`，Go 对照回归放在 `ir_impl_test.go`，不要把测试加入本源文件。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、7032 个 Rust 文件；目标 `dumpling/export/ir_impl.rs` 已索引，共 507 行，显示被 `dump.rs`、`http_handler.rs`、`ir.rs`、`ir_impl_test.rs` 等文件使用。
- 通过 RustCodeGraph 完整读取：`dumpling/export/ir_impl.rs`、协议文件 `dumpling/export/ir.rs`、crate 入口 `dumpling/export/lib.rs`、Rust 测试 `dumpling/export/ir_impl_test.rs`、Go 实现 `dumpling/export/ir_impl.go`、Go 测试 `dumpling/export/ir_impl_test.go`。
- 通过 RustCodeGraph 核对主链：`dumpling/export/sql.rs::SelectAllFromTable`，`dump.rs::{dumpWholeTableDirectly,dumpSQL,dumpTableMeta}`，`writer.rs::{Writer::WriteTableData,writeMetaToFile}`；调用图明确给出 `newTableData`、`getSpecialComments` 与 trait 方法的上述调用边。
- 通过 `dumpling/export/Cargo.toml` 核对 crate 名、`lib.rs` 入口、Go 包映射、本地 Dumpling 依赖和 writer 格式相关依赖；该文件无 feature 条件，`ir_impl.rs` 也没有条件编译项、模块级常量、独立 trait 或枚举。
- 测试事实：`ir_impl_test.rs::{test_row_iter,test_chunk_row_iter}` 与 Go 的 `TestRowIter/TestChunkRowIter` 覆盖预取、稳定 `HasNext`、逐行解码推进及关闭后解码失败。本任务按计划不运行 Cargo；结构校验命令及结果在交付时记录。
