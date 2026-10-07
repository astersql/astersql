# `pkg/lightning/mydump/region.rs`

## 文件定位

本文件属于 `astersql-lightning-mydump` crate；crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，模块由 [`lib.rs`](./lib.rs) 的 `mod region; pub use region::*;` 接入并公开。这里的 `Region` 是 Lightning 导入计划中的文件分片：它把一个源文件的字节范围、估算行号范围、表身份和目标 engine 绑定在一起，并不是 TiKV 的键范围 Region。

它处在“dump 元数据 → 可执行导入 chunk”的中间层。上游把 [`loader.rs`](./loader.rs) 的 `MDTableMeta`、目标列数和切分策略交给 `MakeTableRegions`；本文件按文件类型决定整文件处理或 CSV 切分，连续化跨文件 row ID，再通过 `AllocateEngineIDs` 分组。下游会把结果转成 importer chunk：`pkg/session/runtime/import_file.rs` 直接按 `engine_id` 分组，`pkg/executor/importer/production_regions.rs` 则把本文件的 `TableRegion` 转成 importer 自己的 `TableRegion`，最终由 `LoadDataController::PopulateChunks` 展开为执行 chunk。

该文件是已有 Go 实现 [`region.go`](./region.go) 的 Rust 移植，但并非完整等价：当前 Rust 版本采用同步串行处理，CSV 打开时把整个输入读入内存，Parquet 行数仅使用已有元数据或文件大小回退；Go 版本还具有 context 取消、文件/切分点并行、I/O worker 限流、Parquet 行数读取和大文件告警等机制。

## 核心职责

1. `TableRegion` 统一描述一个可导入分片，并提供行号、偏移和大小访问器。
2. `MakeTableRegions` 按原始 `data_files` 顺序创建分片：严格格式的未压缩大 CSV 可细分，Parquet 和其他格式各生成一个整文件 Region。
3. `SplitLargeCSV` 先处理可选表头，再把理想切分点向后对齐到真实记录终止符，避免正常配置下从一条记录中间开始。
4. `MakeSourceFileRegion` 与 `makeParquetFileRegion` 为不可细分或无需细分的文件建立读到文件末尾的范围，并估算 row ID 上界。
5. `CalculateBatchSize` 与 `AllocateEngineIDs` 根据数据真实大小、行序、批大小、导入速度比和并发上限，把 Region 分配给一个或多个 engine，以形成非均匀流水线批次。
6. `openCSVParser` 和 `getHeaderColumn` 负责为切分流程建立字符集感知的 CSV parser，并精确确定表头后的首个数据偏移。

本文件不负责扫描目录、识别文件名、解析每一行数据、编码 KV、写入或 ingest SST；这些职责分别位于 loader/router、parser/csv_parser 和 importer/backend 链路。

## 主要符号

- `TableFileSizeINF: i64`：10 TiB 哨兵。压缩流无法预知解压后的可 seek 终点时，`MakeSourceFileRegion` 用它作为 `end_offset`，让下游以 EOF 结束读取。
- `CompressSizeFactor: i64`：压缩文件 row ID 估算的额外放大系数 5。
- `DEFAULT_BATCH_SIZE`：未显式配置 engine 大小时使用的 100 GiB Go 默认值。
- `LARGE_CSV_LOWER_THRESHOLD_RATIO`：值为 10；只有文件大小大于 `region_size + region_size / 10` 才切分，避免只略超阈值的 CSV 被拆开。
- `pub struct TableRegion`：含 `engine_id`、`db`、`table`、`SourceFileMeta`、`ExtendColumnData` 和 [`parser.rs`](./parser.rs) 的 `Chunk`。`RowIDMin` 返回 `prev_row_id_max + 1`，`Rows` 返回两个行号边界之差，`Offset`/`Size` 暴露字节范围。
- `AllocateEngineIDs(&mut [TableRegion], &[f64], f64, f64, f64)`：按与 Region 同序的大小数组分配 engine。总量不超过 batch 时保留初始 engine 0；否则使用 `libm::lgamma` 计算 Beta 归一化项，估计 engine 数和递增批容量。
- `pub struct DataDivideConfig`：切分策略集合，包括列数、engine/region 大小、批比例、engine 并发、严格格式开关、CSV/字符集配置，以及当前本文件未使用的可选 `io_workers`。
- `NewDataDivideConfig()`：建立 Rust 默认配置：100 GiB engine、0.75 比例、4 个 engine、256 MiB Region、非 strict、UTF-8MB4 和 U+FFFD 替换串。
- `MakeTableRegions(&MDTableMeta, &DataDivideConfig, &dyn Storage)`：表级主入口，保持文件次序，创建和重基分片，最后分配 engine ID。
- `CalculateBatchSize(f64, bool, f64)`：正配置值原样返回；非正值且行有序时返回 100 GiB，非行有序时返回 `max(100 GiB, total)`。
- `MakeSourceFileRegion`：为 CSV/SQL 等建立一个整文件 Region。CSV 的行号估算除数为列数，其他格式为列数加 2；压缩文件改用 `real_size * 5` 估算并把结束偏移设为 10 TiB。
- `makeParquetFileRegion`：Parquet 不拆分，结束偏移固定为 `i64::MAX`；行数优先采用 `meta.rows`，否则回退到 `file_size`。
- `openCSVParser`：经 `Storage::open` 打开输入并 `read_to_end`，用 `NewCharsetConvertor` 和 `NewCSVParser` 建立内存 parser。
- `getHeaderColumn`：读取表头列名和数据起点；对 CRLF 表头修正 parser 报告在 LF 后的位置，使返回边界与 Go 行为一致。
- `SplitLargeCSV`：均匀计算粗切分点，逐点重新打开 parser 并调用 `ReadUntilTerminator` 对齐，然后构造连续、非空的 `TableRegion`。

源码没有 trait、异步函数、条件编译项或模块级可变状态。公开 API 保留 Go 风格大写命名，crate 根通过 `pub use` 将其暴露给外部 crate。

## 执行流程

`MakeTableRegions` 的表级流程是：

1. 顺序遍历 `table.data_files`，把 `FileInfo` 复制为包含 `real_size`、`rows` 和扩展列的 `SourceFileMeta`。
2. 对 `Csv + strict_format + 未压缩 + 大于 110% region_size` 调用 `SplitLargeCSV`，每个分片大小取其字节 `Size()`；对 Parquet 调用 `makeParquetFileRegion`，用于 engine 分配的大小取 `real_size`；其他文件调用 `MakeSourceFileRegion`，大小同样取 `real_size`。
3. 以之前所有文件的最终 `row_id_max` 为基数，同时平移当前文件所有分片的 `prev_row_id_max` 和 `row_id_max`。因此返回 Region 的行号范围按源文件顺序单调推进，而每个文件内部的范围连续。
4. 累加所有 Region 和对应大小，调用 `CalculateBatchSize` 选取 batch。
5. 调用 `AllocateEngineIDs` 原地写入 `engine_id`，返回结果。

`SplitLargeCSV` 的切分流程是：

1. `openCSVParser` 打开并完整读取文件，创建字符集转换器和 CSV parser。
2. 若 `cfg.csv.header` 为真，`getHeaderColumn` 通过 `ReadColumns` 取得列名与数据起点；否则从偏移 0 开始且列名为空。
3. 用剩余字节数和 `region_size` 向上取整得到 Region 数，再以 quotient/remainder 把粗切分点尽量均匀分布。
4. 对每个非末尾粗切分点重新打开 parser，`SetPos` 后读取到下一个配置终止符；正常时使用 parser 的新位置，遇到 `MydumpError::Eof` 时使用文件末尾，其他错误立即返回。
5. 追加文件末尾，跳过因对齐产生的重复点；每个 Region 从上一个结束偏移开始。估算行数为 `字节长度 / column_count`，并把表头列名复制到每个 chunk。

`AllocateEngineIDs` 先求总大小。如果需要分批，它以 `total * (1-ratio) / batch_size` 为目标比例，从其上取整值开始搜索 engine 数；`lgamma` 计算的 Beta 项用于求非均匀初始 target。遍历 Region 时先归入当前 engine，再累加大小；达到 target 后递增 engine，搜索范围内按比例放大下一 target，超过并发估计后恢复固定 `batch_size`。分组边界因此落在整个 Region 之后，不会拆开一个已有 Region。

## 数据与状态

`TableRegion.chunk` 同时保存物理字节边界和逻辑行号边界。核心不变量是 `Rows() = row_id_max - prev_row_id_max`、`RowIDMin() = prev_row_id_max + 1`、`Size() = end_offset - offset`。CSV 切分正常返回时，相邻非空分片满足前一项 `end_offset ==` 后一项 `offset`；最后一项结束于 `file_size`。行号是按字节数和列数得到的容量估计，不是预扫描所得的真实行数；它主要为分片之间预留互不重叠的 handle 区间。

`SourceFileMeta.file_size` 是存储对象的字节大小，`real_size` 是用于 engine 容量规划的估算解压/导入大小。代码有意区分二者：未压缩普通文件的读取终点用 `file_size`，压缩文件的 engine 大小与 row ID 估算用 `real_size`，Parquet 的 engine 分配也用 `real_size`。`extend_data` 从文件元数据复制到 Region，不在本文件中解释或变更。

`MakeTableRegions` 的 `prev_row`、Region/size 向量以及 `SplitLargeCSV` 的 split point、offset 和 row ID 累加器都是调用内局部状态。`AllocateEngineIDs` 唯一修改调用方状态的操作是覆盖 `regions` 中的 `engine_id`；它通过 `zip` 使用与 Region 同序的 `sizes`，调用方必须保证二者长度及顺序一致。

配置存在若干隐含前置条件：进入 `MakeSourceFileRegion` 或 `SplitLargeCSV` 时 `column_count` 必须大于 0；进入 `SplitLargeCSV` 时 `region_size` 必须大于 0，且文件数据区不能导致 Region 数为 0。当前代码没有把这些条件转换成 `MydumpError`，违反时可能整数除零 panic。生产调用点会从目标表列数与正的切分配置填充这些值，但扩展者不能依赖构造器的 `column_count = 0` 默认值直接执行切分。

## 依赖与调用关系

crate 内直接依赖包括：

- [`loader.rs`](./loader.rs)：`MDTableMeta`、`SourceFileMeta`；[`common.rs`](./common.rs)：`FileInfo`、`ExtendColumnData`、`MydumpError`。
- [`router.rs`](./router.rs)：`SourceType` 与外层 `Compression` 分类。
- [`parser.rs`](./parser.rs) 和 [`csv_parser.rs`](./csv_parser.rs)：`Chunk`、`CsvParser`、`NewCSVParser`、位置控制、读表头和终止符对齐。
- [`reader.rs`](./reader.rs)：`Storage`、`StringReader`、`WorkerPool`。当前 `openCSVParser` 使用 `Storage::open` 和 `StringReader`；`DataDivideConfig.io_workers` 在本文件中仅保存，未传给 parser。
- [`charset_convertor.rs`](./charset_convertor.rs)：`NewCharsetConvertor`，使切分边界按配置字符集解释。
- `libm`：[`Cargo.toml`](./Cargo.toml) 声明 `libm = "0.2"`，`AllocateEngineIDs` 使用其 `lgamma`；其余所需类型通过 crate 根的 `use crate::*` 引入。

已核对的生产上游为：

- `pkg/session/runtime/import_file.rs`：构造 CSV `MDTableMeta` 和 `DataDivideConfig`，调用 `dump::MakeTableRegions`，再按 `engine_id` 分组。
- `pkg/executor/importer/production_regions.rs::HostTableImporterService::MakeTableRegions`：服务器本地绝对路径走 Rust mydump 切分，并把结果映射为 executor importer 的 Region。
- `pkg/executor/importer/table_import.rs::LoadDataController::PopulateChunks`：消费上述服务结果，以 engine ID 分组为执行 `Chunk`。
- `pkg/session/runtime/import_sst.rs::Runtime::GetParser`：执行阶段再次调用 `dump::openCSVParser`，再将 parser 定位到 chunk 的 offset/row ID；这说明本文件产出的边界直接控制实际读取起点。

根 workspace 和 `pkg/session/Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/executor/importer/Cargo.toml` 都以路径依赖引用该 crate。`lib.rs` 同时把 [`region_test.rs`](./region_test.rs) 作为独立测试模块挂载，测试逻辑没有内嵌进生产文件。

## 错误处理与边界

`MakeTableRegions`、`openCSVParser`、`getHeaderColumn` 和 `SplitLargeCSV` 以 `Result<_, MydumpError>` 传播错误。`Storage::open` 的失败、`read_to_end` I/O 错误、未知字符集、CSV parser 构造/定位/读取错误都会经 `?` 原样上送。对齐切分点时只有 `MydumpError::Eof` 被视为可恢复边界，并转换为 `file_size`；其他错误终止整个表的 Region 生成。

当前边界行为包括：

- 只有严格格式、未压缩且超过 110% 阈值的 CSV 才切分；非 strict、压缩 CSV、SQL 和轻微超阈值文件都保持一个 Region。
- 表头模式下首个 Region 从数据起点而非文件 0 开始，列名写入每个 chunk。CRLF 表头会做一字节兼容修正。
- 粗切分点会向后对齐到终止符，因此 Region 大小可能超过 `region_size`；该配置是目标值，不是硬上限。
- 文件末尾无终止符时仍以 EOF 建立最后边界；多个粗切分点对齐到同一位置时会跳过零长度 Region。
- 压缩普通文件以 10 TiB、Parquet 以 `i64::MAX` 表示“读到 EOF”，调用方不能把这两个值当作真实文件长度。
- `AllocateEngineIDs` 在总量不超过 batch 时不重置 ID，只是提前返回；其正常调用约定是新建 Region 初始 ID 均为 0。
- `AllocateEngineIDs` 用 `regions.zip(sizes)`，长度不一致不会报错，较长一侧的尾项不会参与循环；`MakeTableRegions` 自身保持一一对应，但独立调用者必须维护契约。
- `column_count == 0`、`region_size <= 0`、不合理的 ratio/concurrency 或空数据区没有统一配置校验。尤其前两类可触发除零；修改公共入口时应优先把这些前置条件变为明确的 `MydumpError::Configuration`。

## 并发与资源生命周期

当前 Rust 实现不创建线程、异步任务、通道、锁或事务。`MakeTableRegions` 按文件顺序同步执行；`SplitLargeCSV` 也按切分点顺序同步重新打开文件。`DataDivideConfig.io_workers: Option<Arc<WorkerPool>>` 虽可共享一个限流池，但本文件没有读取该字段，因此它不限制这里的 I/O 并发。

每次 `openCSVParser` 都从 `Storage` 取得一个 reader，将整个内容读入局部 `Vec<u8>`，随后把所有权交给 `StringReader`/`CsvParser`。reader 在读完后随局部值释放；parser 及其内存缓冲在函数或切分点迭代结束时释放。对 N 个候选切分点，文件会被完整打开和读入约 N+1 次（另含表头 parser），峰值主要由单次完整文件缓冲决定，累计 I/O 与文件大小和切分数相乘；这是扩展超大 CSV 时最重要的性能边界。

`TableRegion`、`DataDivideConfig` 和返回向量均由调用者拥有；`Storage` 只在调用期间借用。配置里的 `Arc<WorkerPool>` 可被克隆，但本文件不获取令牌。没有后台工作跨越函数返回，也没有需要显式 close/回滚的资源生命周期。

## 与 Go 版本的对应关系

直接对照是 [`region.go`](./region.go)，测试对照是 [`region_test.go`](./region_test.go)。Rust 保留了 `TableRegion` 访问器、10 TiB 压缩终点、压缩行号放大、110% 大 CSV 阈值、Beta/lgamma engine 分桶、默认 batch 选择、表头列名、终止符对齐、row ID 重基和 `real_size` 容量规划等核心语义。Rust [`region_test.rs`](./region_test.rs) 用与 Go 相同的 engine 桶计数和主要切分 offset 证明这些行为。

已确认的差异如下：

- Go `DataDivideConfig` 持有 `Store`、`TableMeta`、文件切分并发、read block size 和 `SkipParquetRowCount`；Rust 把表与 store 作为函数参数，只保留 `io_workers`，且没有对应的文件并发/read block/skip 标志。
- Go `MakeTableRegions` 用 `errgroup` 并行处理文件、保留 context 取消，超大 CSV 还能并行校正切分点；Rust 完全同步且没有取消输入。
- Go `openCSVParser` 流式使用 store reader并显式 close；Rust `read_to_end` 后在内存中解析，也没有显式关闭接口需求。
- Go 只在 `Header && HeaderSchemaMatch` 时读取表头；当前 Rust `getHeaderColumn` 的调用条件只看 `csv.header`。扩展 header schema 行为时必须核对 [`csv_parser.rs`](./csv_parser.rs) 的 `CsvConfig` 字段与 Go 契约。
- Go 可调用 parquet reader 获取实际行数，并可由 `SkipParquetRowCount`/failpoint 回退；Rust 只读 `meta.rows`，否则使用 `file_size`。两者都用 `real_size` 做 engine 分配，并用最大结束偏移读到 EOF。
- Go 对大于 1 GiB 的整文件 Region记录效率告警；Rust没有日志或该告警阈值。
- Go Region 直接从 `cfg.TableMeta` 填充 DB/Table；Rust先构造空名称，再由 `MakeTableRegions` 统一回填。独立调用 `MakeSourceFileRegion`、`makeParquetFileRegion` 或 `SplitLargeCSV` 会得到空 DB/Table，这是当前 API 的真实行为。
- Go 的错误路径包括 context 取消、并行 worker 和 parser close；Rust 的错误集合较窄，但增加了由 Rust `Storage`/字符集转换器产生的 `MydumpError` 传播。

因此不能仅凭函数同名推断功能完全对等。若要补齐并行、流式或 Parquet 读取，必须保持现有分片顺序、row ID 范围、engine 大小输入和错误传播语义，而不是只让测试样例通过。

## 扩展指南

新增或修改文件类型分支时，主要接入点是 `MakeTableRegions` 的 `SourceType` match；需要同步决定三件事：实际读取终点、row ID 上界来源、传给 `AllocateEngineIDs` 的大小度量。若类型不可高效 seek，应参考 Parquet 的整文件策略，但不能未经证据假设 `file_size` 等于行数或 `real_size`。

调整 CSV 切分时，应集中修改 `getHeaderColumn`/`SplitLargeCSV`，并保持：表头只属于元数据、不落入首数据分片；分片连续且不重叠；终点对齐配置终止符；EOF 无换行可恢复；CRLF 和自定义多字节终止符不从中间切开。应在独立的 [`region_test.rs`](./region_test.rs) 扩展单元测试，不得把测试写进 `region.rs`；用户可见链路还应按影响补充 `tests/realtikvtest/importintotest4/split_file_test.rs` 或对应 importer/session 测试。

修改 engine 算法时，应同时验证 `AllocateEngineIDs` 的既有桶计数、ratio 为 0、并发上限和总量小于 batch 的情况，并确认 `sizes` 与 Region 的一一对应。该算法依赖浮点 `lgamma/exp`，风险包括跨平台舍入导致分桶边界变化、极端 ratio/concurrency 产生非有限值，以及单个超大 Region 必然使 engine 超目标；不要把 batch target 描述为硬限制。

若补齐 Go 的并行/流式能力，应先设计资源所有权与确定性结果合并：允许并行发现边界，但最终仍需按 `table.data_files` 与文件内 offset 排序后重基 row ID。还应真正接入或移除 `io_workers`，避免配置看似生效但实际无效。若增加输入校验，最安全的入口是 `MakeTableRegions` 与 `SplitLargeCSV` 开始处，并使用 `MydumpError::Configuration` 覆盖零列、非正 region、非法比例和空数据区。

兼容性上应重点关注 Go/Rust 表头条件、空 DB/Table 的独立构造 API、压缩/Parquet 哨兵；正确性上关注 row ID 不重叠和记录边界；性能上关注重复整文件读取及 Region 数量。任何公开字段或 Go 风格函数名变更还需同步 session、executor importer 和 crate 的公开再导出调用者。

## 验证依据

- 目标源码：[`region.rs`](./region.rs) 全部 387 行；逐项核对常量、`TableRegion`/`DataDivideConfig`、`AllocateEngineIDs`、`MakeTableRegions`、`CalculateBatchSize`、两个整文件构造器以及 CSV parser/切分函数。
- crate/模块：[`Cargo.toml`](./Cargo.toml) 确认 crate 名、`lib.rs` 入口、`libm = "0.2"` 和 Go 包映射；[`lib.rs`](./lib.rs) 确认 `region` 的公开再导出及独立 `region_test.rs` 挂载。根 `Cargo.toml` 与 session/executor/importer manifests 确认路径依赖关系。
- 数据与错误定义：[`common.rs`](./common.rs) 的 `MydumpError`、`FileInfo`、`ExtendColumnData`；[`loader.rs`](./loader.rs) 的 `MDTableMeta`/`SourceFileMeta`；[`parser.rs`](./parser.rs) 的 `Chunk`；[`router.rs`](./router.rs) 的 `SourceType`/`Compression`；[`reader.rs`](./reader.rs) 的 `Storage`/`WorkerPool`。
- 生产调用链：`pkg/session/runtime/import_file.rs` 的 `dump::MakeTableRegions`；`pkg/executor/importer/production_regions.rs::HostTableImporterService::MakeTableRegions`；`pkg/executor/importer/table_import.rs::LoadDataController::PopulateChunks`；`pkg/session/runtime/import_sst.rs::Runtime::GetParser` 对 `openCSVParser` 和 chunk offset 的消费。
- Rust 单元测试：[`region_test.rs`](./region_test.rs) 的 15 个测试覆盖访问器、engine 分桶/default batch、strict/110% 阈值、多文件 row ID 重基、`real_size`、压缩/Parquet、非法字符集、表头、EOF、自定义终止符、单分片和 CRLF seek。
- Rust 集成证据：`tests/realtikvtest/importintotest4/split_file_test.rs` 验证 `IMPORT INTO ... WITH split_file` 的 CRLF 边界、row ID 起点及缺失对象错误传播；该文件仅作为行为证据，本纯文档任务未运行 RealTiKV。
- Go 对照：[`region.go`](./region.go) 的同名类型和函数，以及 [`region_test.go`](./region_test.go) 的 engine、CSV、压缩和 Parquet 场景；用于区分已移植核心语义与尚未移植的 context、并行、流式、Parquet 行数读取和告警。
- RustCodeGraph：运行 `status` 成功，索引报告 11,467 个文件、307,296 个节点和 1,848,419 条边；随后运行 `files --filter pkg/lightning/mydump/region` 与 `explore "pkg/lightning/mydump/region.rs Region EngineRegion get_region contains intersect"`，目标文件未出现在索引查询结果中。依照导航技能规则，调用边改由精确 `rg` 搜索上述唯一符号并读取直接调用点建立；没有把缺失的图结果伪装成调用证据。
- 结构验证按任务给定命令执行；同时人工核对本文回答了文件存在原因、表级/CSV/engine 流程、状态不变量、Go 差异和安全扩展位置，且未声称运行 Cargo 或代码测试。
