# `dumpling/export/writer_util.rs`

## 文件定位

本文件属于 `astersql-dumpling-export` library crate；`dumpling/export/Cargo.toml` 把 crate 根设为 `lib.rs`，而 `lib.rs` 通过 `include!("writer_util.rs")` 将本文件放进与 Go `dumpling/export` 包相似的单包命名空间。它位于导出执行面的格式化与落盘边界：上游 `Writer::WriteTableData` 负责启动 `TableDataIR`、命名文件和选择分片策略，本文件把 `SQLRowIter` 中的行转换为 SQL、CSV 或 Parquet 字节，并写入 `ObjectWriter`。

生产数据主链是 `Writer::handleTask` → `Writer::WriteTableData` → `FileFormat::WriteInsert` → `WriteInsertSQL` / `WriteInsertInCsv` / `WriteInsertInParquet`。开启 SQL/CSV 文件大小分片时，`writer.rs` 会持有同一个迭代器并直接反复调用私有的 `writeSQLFile` 或 `writeCSVFile`，使下一文件从尚未消费的下一行继续。`WriteMeta` 与辅助函数 `write` 虽是公开符号，但当前 Rust 生产路径 `Writer::writeMetaToFile` 自行完成同类写入；仓库内对 `WriteMeta` 的直接调用来自 `writer_serial_test.rs`，因此不能把它描述为已接线的元数据生产入口。

## 核心职责

- `FileFormat` 统一表示 SQL、CSV、Parquet 和未知格式，并提供日志名称、文件扩展名及格式分派。
- `WriteInsertSQL` / `writeSQLFile` 生成 special comments 与多行 `INSERT ... VALUES`，按 `StatementSize` 切 SQL statement，按 `FileSize` 停在当前输出文件边界。
- `WriteInsertInCsv` / `writeCSVFile` 将 CSV dialect、NULL、二进制编码和表头配置交给 `astersql-dumpformat-csvfile`，同时处理“全部列均为生成列”的空行输出。
- `WriteInsertInParquet` 将数据库列信息映射为 Parquet schema，按近似内存字节阈值写 row group，并将编码结果流式写入对象存储。
- `CSVObjectWriter`、`ParquetObjectWriter` 和 `csv_io_error` 在 crate 的 `ObjectWriter`/`Error` 与标准 `std::io::Write`/`std::io::Error` 之间适配，并保留对象存储 multipart 上限错误身份。
- `LazyStringWriter` 延迟调用 `Storage::CreateWithOptions`，避免未产生任何字节时创建空文件；`WriteMeta` 则提供 special comments 加 DDL 正文的基础串行写出能力。

文件还定义 `lengthLimit = 1 MiB`，但当前目标文件及 Rust crate 中没有消费该常量；`lib.rs` 允许 `dead_code`，所以它是迁移期保留接口，而不是当前生效的 flush 阈值。实际 SQL/CSV 缓冲与切分由 dumpformat writer、`StatementSize` 和 `FileSize` 控制。

## 主要符号

- `FileFormat`：`#[repr(i32)]` 的公开枚举，值域与 Go iota 顺序一致。`String()` 返回日志文本，`Extension()` 返回文件后缀，`WriteInsert(...) -> Result<()>` 按变体分派；`FileFormatUnknown` 明确返回 `unknown file format`。
- `FileFormatSQLTextString`、`FileFormatCSVString`、`FileFormatParquetString`：配置值和普通扩展名常量；`writer.rs::NewWriter` 用它们解析 `Config.FileType`。
- `wrapBackTicks`、`escapeString`：前者总是在标识符外加反引号并把内部反引号加倍，后者只做内部反引号加倍。SQL 查询构造还在 `sql.rs`、`dump.rs`、`ir.rs`、`column_filter.rs` 和 `schema_projection.rs` 复用这两个公开函数。
- `WriteInsertSQL`：取得 `TableDataIR::Rows()`，调用 `writeSQLFile`，并且无论写入结果成功与否都先调用 iterator `Close()`；写入错误优先于关闭错误返回。
- `writeSQLFile`：单个 SQL 输出文件的真实循环。它先写 `TableMeta::SpecialComments`，构造 SQL prefix 和列种类，再逐行 `Decode`、`SQLWriter::write`、更新指标并检查文件大小。
- `WriteInsertInCsv` / `writeCSVFile`：结构与 SQL 路径相同；私有函数允许 `writer.rs` 跨文件分片时复用已有 iterator。
- `parquet_columns`、`parquet_schema`、`write_parquet_group`：依次完成列元数据转换、Parquet schema 构建和列式 row-group 写入。`write_parquet_group` 校验行列数量、NULL 可选性、逻辑值与物理 writer 类型是否一致。
- `WriteInsertInParquet`：建立 `SerializedFileWriter`，缓冲原始行，达到 `ParquetRowGroupSize`、`FileSize` 或迭代结束时写 row group，最后关闭 Parquet writer 并记录指标。
- `CSVObjectWriter<'a>`：借用 `&mut dyn ObjectWriter` 的轻量适配器，`flush` 为 no-op；SQL 与 CSV dumpformat writer 均通过它写入。
- `ParquetObjectWriter<'a>`：除委托写入外，用 `Arc<AtomicU64>` 记录实际已写字节，供文件大小判断和指标使用。
- `annotatePartLimit`：仅当 `Error.exceed_upload_parts` 哨兵为真时添加约 48.83 GiB 单对象限制及 `--filesize (-F)` 建议，不靠错误字符串猜测类型。
- `LazyStringWriter`：保存 `Arc<dyn Storage>`、相对路径、可选底层 writer 和 `WriterOption`；`ensure` 只在第一次 `Write` 时创建，`Close` 通过 `take()` 确保最多关闭一次。
- `WriteMeta`、`write`：前者依次写特殊注释和 `MetaSQL`，后者只把字符串字节转发给 writer；目前主要由独立测试覆盖。

## 执行流程

1. `Writer::NewWriter` 将 `Config.FileType` 解析为 `FileFormat`；`Writer::WriteTableData` 启动 IR、确定动态或静态 `TableMeta`、生成输出名并建立 `LazyStringWriter`。
2. 非专用分片路径调用 `FileFormat::WriteInsert`。未知格式立即报错；已知格式各自取得一份 `SQLRowIter`。SQL/CSV 公共包装在格式化结束后关闭 iterator，外层 `Writer::WriteTableData` 再关闭 lazy writer 和整个 IR。
3. SQL 路径先根据 `SelectedField` 生成带列清单或不带列清单的 `INSERT` prefix，并将 special comments 直接写入 sink。每行通过 `RowReceiver` 解码后交给 SQL writer；空 `SelectedField` 时仍消费源行，但写出空 tuple `()`。每 1000 行增量更新 rows/bytes 指标，推进 iterator 后检查 `FileSize`。
4. CSV 路径把分隔符、包围符、反斜杠转义、行终止符、NULL 字面量和 binary dialect 转成 csvfile 配置。只有存在选中字段且未设置 `NoHeader` 时写表头；空选中字段时每个源行写一个空 CSV record，所以仍保留行终止符和行数。
5. SQL 或 CSV 分片由 `writer.rs` 在同一 iterator 上循环调用私有单文件函数。单文件函数在超过软大小阈值后停止，已经消费的行不回退；下一次调用从下一行开始。
6. Parquet 路径从 `ColumnInfos` 构造 dumpformat 列模型和 parquet-rs schema，配置压缩与 data page size。每行原始值先缓存在 `rows`，累计原始 payload 字节达到 `ParquetRowGroupSize`，或估算达到 `FileSize`，或到达末行时，按列解析并写一个 row group；最后 `SerializedFileWriter::close` 写 footer。
7. 所有格式最终经 `ObjectWriter::Write` 落到 `LazyStringWriter`。首次非空或空 slice 的 `Write` 调用都会触发 `ensure` 创建对象；完全没有写入则 `Close` 成功且不创建文件。

## 数据与状态

输入状态来自 `Config`、`TableMeta`、`TableDataIR`/`SQLRowIter` 和可选 `metrics`。`SelectedField` 同时控制 SQL 列清单、是否解码行、CSV 表头和全生成列行为；它不是由 `CompleteInsert` 在本文件中重新判断。`ColumnTypes` 经 `columnKinds` 分为 bytes、number、string，影响 SQL/CSV 引号及二进制处理；Parquet 则使用更丰富的 `ColumnInfos`（名称、数据库类型、nullable、precision、scale）。

SQL 单文件状态包括 `preamble`、`count`、`last_count`、`finished_size` 和复用的 `RowReceiver`；CSV 对应保存 `count`、`counted`、`finished_size`、复用 raw vector 和 csv writer。成功时 gauge 反映本次文件行数/字节数；失败时只回滚本次已经提交给 gauge 的增量。`metrics: None` 是被支持的路径，不执行观测更新。

Parquet 的 `rows: Vec<Vec<Option<Vec<u8>>>>` 保存一个 row group 的原始值，`buffered_bytes` 只计算字段 payload，不含 Parquet 编码、页头、footer 等开销；`written: Arc<AtomicU64>` 记录底层实际写入量。因此 `FileSize` 是写完当前缓冲组前的近似软阈值，不能保证最终文件严格不超过配置值。`lengthLimit` 当前不参与任何状态转换。

`LazyStringWriter.w` 是资源状态机：`None` 表示尚未创建或已经被 `Close` 取走，`Some` 表示底层 writer 已打开。`option` 必须在首次 `Write` 前设置；创建以后再修改不会影响既有 writer。

## 依赖与调用关系

上游生产调用者以 `writer.rs` 为核心：`Writer::WriteTableData` 调用 `FileFormat::WriteInsert`；SQL/CSV 分片分支分别直接调用 `writeSQLFile`、`writeCSVFile`；三个分支都创建并关闭 `LazyStringWriter`。RustCodeGraph 对目标符号的查询还显示 `WriteInsertSQL` 与 `WriteInsertInCsv` 被 `writer_serial_test.rs` 调用，CSV 入口被 `dump_test.rs` 的列投影测试调用，`WriteInsertInParquet` 当前除分派器外主要由本文件测试覆盖。

下游 crate 依赖由 `dumpling/export/Cargo.toml` 明确声明：SQL 编码使用 `astersql-dumpformat-sqlfile`，CSV 使用 `astersql-dumpformat-csvfile`，Parquet 的数据库类型解析使用 `astersql-dumpformat-parquetfile`，实际标准文件编码使用带固定 tag `astersql-parquet-v60.0.0-streaming-pages.1` 的 `parquet` Git 依赖，对象写入选项类型来自 `astersql-objstore-storeapi`。本文件的 `Config`、IR traits、metrics、错误和 storage traits 因 `lib.rs::include!` 单包布局从 crate 作用域直接可见。

标识符工具的上游范围比写文件更广：`sql.rs` 构造 SHOW/SELECT 等查询，`dump.rs` 和 `schema_projection.rs` 拼接过滤或投影 SQL，`ir.rs` 处理选中字段。修改其语义会同时影响导出查询与输出文本，不应只按 writer 局部工具看待。

## 错误处理与边界

- 空 iterator：SQL/CSV 的单文件函数返回 iterator 现有错误或成功写零行；公开包装随后关闭 iterator。Parquet 会在发现现有 iterator 错误时尽力关闭并返回原错误，无错误时直接返回关闭结果。
- SQL/CSV：decode、格式 writer、底层写入、writer close 和 iterator error 均向上传播。公开包装先保存写入结果，再调用 iterator `Close`，所以写入失败也会尝试释放 iterator；若写入与关闭都失败，写入错误优先。
- CSV/SQL 的标准 I/O 适配先用 `annotatePartLimit` 保留 multipart 哨兵，再由 `csv_io_error` 从 `std::io::Error` 的 inner error 恢复 crate `Error`。普通同文本错误不会误获 `--filesize` 注解；这一点由 `writer_util_test.rs` 覆盖。
- SQL 文件大小包含 special-comment preamble；CSV 使用 encoder 估算大小。二者均在完整写出一行后判断阈值，因此允许单行使文件超过限制。
- Parquet 对 precision/scale 的 `i64 → i32` 越界、schema 构造、原始值解析、必填列收到 NULL、行列数不匹配、物理 writer 类型不匹配和 parquet-rs I/O 错误都显式失败。`PhysicalType::Int96` 可进入 schema 构造，但 `write_parquet_group` 没有 Int96 写入分支，会落入物理 writer 类型不匹配错误。
- Parquet 在取得 iterator 后，`parquet_columns`、schema/writer 创建或 row-group 写入若经 `?` 提前返回，并非所有路径都会显式调用 `iter.Close()`；当前只有空 iterator、迭代期 `iter.Error()` 和正常末尾明确关闭。这是扩展错误路径时必须保留关注的现状，而不是已验证的完整清理保证。
- `WriteMeta` 在第一条失败写入处停止，不主动关闭传入 writer；资源所有权属于调用者。`LazyStringWriter::Close` 先 `take` 后关闭，若底层关闭失败，实例中也不会保留 writer 供重试。

## 并发与资源生命周期

本文件不创建线程或异步任务；每次格式写入都在调用线程中顺序消费一个 `&mut dyn SQLRowIter` 和一个 `&mut dyn ObjectWriter`。traits 带 `Send`/`Sync` 约束，使对象能被外层 writer 工作线程持有，但 `LazyStringWriter` 自身通过 `&mut self` 串行改变 `w`，没有内部锁，也没有承诺多线程共享同一实例。

`uploadConcurrency = 4` 与 `uploadPartSize = 5 MiB` 由 `writer.rs` 写入 `WriterOption`，交给对象存储实现决定 multipart 并发；这不是本文件启动四个线程。Parquet 的 `Arc<AtomicU64>` 是为了让实现 `Send` 的输出适配器与调用方共享写入计数，采用 `Relaxed` 足以支持这里只关心数值、不建立跨线程内存顺序的用途。

SQL/CSV 的 iterator 生命周期是“Rows → 单文件函数一次或多次 → Close”；`TableDataIR::Close` 由外层 `Writer::WriteTableData` 负责。对象生命周期是“构造 lazy writer → 首次 Write 时 CreateWithOptions → 格式 writer close/flush footer → LazyStringWriter::Close 提交底层对象”。格式 writer 的 `flush` 适配为 no-op，真正的提交点是格式 close 与外层对象 close，调用者不能省略后者。

## 与 Go 版本的对应关系

主要接口与 `dumpling/export/writer_util.go` 对齐：格式枚举和字符串、SQL/CSV/Parquet 写出、列种类判断、multipart 上限注解、标识符工具及元数据写出均有同名或等价实现。`writer_util_test.rs` 与 `writer_serial_test.rs` 复核了 Go 的特殊注释顺序、全生成列输出、CSV dialect、statement splitting、指标回滚和 Parquet 可读性；Go 的对照测试是 `writer_serial_test.go` 与 `writer_test.go`。

需要明确以下当前差异：

- Go 三个数据入口返回 `(uint64, error)`，Rust 公开入口返回 `Result<()>`；行数仅写 metrics/日志，调用者不能取得返回计数。
- Go SQL 入口名为 `WriteInsert`，Rust 将它命名为 `WriteInsertSQL`，把 `FileFormat::WriteInsert` 留作统一分派器。
- Go `wrapBackTicks` 对已有首/尾反引号的输入不再包裹；Rust 总是先转义内部反引号并包一层。Rust parity test 明确期待 `a\`b → \`a\`\`b\``，因此文档以 Rust 当前行为为准。
- Go 使用 `newSink(context, writer)`、压缩对象 writer 和完整日志/summary；Rust 当前 `CSVObjectWriter` 直接适配 crate stub `ObjectWriter`，没有 context-aware write、成功 summary 或失败日志。
- Go Parquet 由 `parquetfile.NewWriter` 封装；Rust 显式建立 parquet-rs schema 与列 writer。Rust Cargo 为 parquet-rs 固定了上游 tag，并在当前文件自行实现 row-group 组装。
- Go 的 `LazyStringWriter` 包装 `io.StringWriter` 并用 `sync.Once`；Rust 版本包装 `Storage`，惰性创建 `ObjectWriter`，靠独占可变借用与 `Option` 保证单次初始化/关闭。
- Go 文件还包含 compression/intercept writer 等构建函数；Rust 对应资源接线已移到 `writer.rs` 或由较轻的 stub 抽象承担。本文件不能被视为 Go 文件的逐符号完整复刻。

## 扩展指南

- 新增文件格式：扩充 `FileFormat`、三个字符串/扩展名相关 match、`writer.rs::NewWriter` 解析与 `Writer::WriteTableData` 的扩展名/分片策略；同时在独立测试文件增加分派、空输入、错误传播、文件命名和真实可读性用例。不要把测试写入本源文件。
- 调整 SQL/CSV 编码：优先修改相应 dumpformat crate；本文件只负责将 `Config`、列种类和 iterator 正确桥接。同步 `writer_util_test.rs`、`writer_serial_test.rs` 以及 Go 对照测试所表达的行为，重点覆盖 NULL、binary、反斜杠、表头、全生成列和超大单行。
- 调整 Parquet 类型：同时更新 `astersql-dumpformat-parquetfile` 的列类型解析、`parquet_schema` 和 `write_parquet_group` 的物理 writer 分支，并用标准 reader 验证 footer、schema、nullable、decimal/time/timestamp 及压缩兼容性。新增 Int96 等 schema 类型时，必须补齐写入分支，不能只让 schema 构造通过。
- 改变分片：保持 iterator 的“当前文件消费后停在下一行”不变量，并区分 encoded size、实际写入 size 与 raw buffered bytes。涉及 `FileSize` 时应同步检查 `writer.rs` 循环和 `outputFileNamer`，防止空文件、丢行或重复行。
- 改变指标：保留每 1000 行批量更新与失败回滚的对称性；若 Parquet 增加中途指标，也要定义错误时如何撤销。`metrics: None` 必须继续安全。
- 改变 lazy writer：所有 option 必须在首写之前冻结；需要 close 重试时不能沿用当前 `take`-before-close 语义。若引入共享并发写，必须重新设计同步，而不能仅依赖 traits 的 `Send`/`Sync`。
- 改变标识符转义：这是跨 `sql.rs`、`dump.rs`、`ir.rs`、`column_filter.rs`、`schema_projection.rs` 的公共契约，应先补 `parity_test.rs` 和相关 SQL 测试，再修改实现。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter dumpling/export/writer_util.rs` 找到目标文件及 48 个符号；`node --file ... --offset 1/401` 阅读了完整 781 行源码。针对 `WriteInsert` 的 `explore` 给出 Rust 调用边：`FileFormat::WriteInsert → WriteInsertSQL / WriteInsertInCsv / WriteInsertInParquet`，并列出 `writer_serial_test.rs`、`dump_test.rs`、`writer_util_test.rs` 的直接调用证据。未限定文件的 `callers/callees` 查询曾超时，因此调用主链另由下述源码调用点交叉核实。
- 源码与入口：`dumpling/export/writer_util.rs`；`dumpling/export/lib.rs` 的 `include!("writer_util.rs")` 及独立测试模块声明；`dumpling/export/writer.rs` 的 `NewWriter`、`WriteTableData`、`writeSplitSql`、`writeMetaToFile`；`dumpling/export/ir.rs` 的 `TableDataIR`、`TableMeta`、`SQLRowIter`、`MetaIR` traits；`dumpling/export/stubs.rs` 的 `ObjectWriter`、`Storage` 和 `ColumnInfo`。
- crate 边界：`dumpling/export/Cargo.toml`，确认 library 根、Go package 元数据、三个 dumpformat crate、objstore storeapi 与固定 tag 的 parquet 依赖。
- Go 对照：`dumpling/export/writer_util.go`，核对 `WriteMeta`、SQL/CSV/Parquet 流程、`FileFormat`、part-limit 注解和 lazy/intercept writer；`dumpling/export/writer_serial_test.go` 与 `writer_test.go` 核对输出、错误、指标和分片意图。
- Rust 测试证据：`dumpling/export/writer_util_test.rs` 覆盖 unknown format 字符串、反引号转义、selected field、special comments、全生成列、CSV header、Parquet 标准 reader 可读性、指标回滚、multipart 错误身份和 statement split；`dumpling/export/writer_serial_test.rs` 覆盖元数据、SQL/CSV 编码、类型和 iterator error；`dumpling/export/writer_test.rs` 覆盖生产 writer 的文件创建与分片接线；`dumpling/export/dump_test.rs` 覆盖 CSV 列投影调用。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有十一个固定二级标题，并人工复核没有把未接线符号、未使用常量或 Go 行为写成 Rust 已实现事实。
