# `pkg/executor/importer/kv_encode.rs`

## 文件定位

本文件属于 `astersql-executor-importer` crate；crate 根 `pkg/executor/importer/lib.rs` 以 `mod kv_encode` 装入它，并通过 `pub use kv_encode::*` 对外导出公共接口。它位于导入链路中“解析后的逻辑行”与 Lightning/TiKV “记录 KV、索引 KV”之间：上游准备字段映射、会话参数和行号，本文件补齐并转换一行，再把最终记录交给 `BaseKVEncoder::Record2KV`。它不负责解析 CSV/Parquet、不写 SST，也不附加 MVCC 版本。

常规文件导入从 `TableImporter::getKVEncoder`（`table_import.rs`）进入 `NewTableKVEncoder`，随后 `ChunkEncoder::encodeLoop`（`chunk_process.rs`）逐行调用 `TableKVEncoder::Encode`。查询导入、大小采样和冲突处理还可从持久化 `TableInfo` 经 `NewTableDefinitionFromMeta`、`NewTableKVEncoderFromMeta` 构造同一编码流水线。

## 核心职责

- 构造表级编码器：`NewTableKVEncoder` 保留控制器给出的字段顺序；`NewTableKVEncoderForDupResolve` 改为全部可见列映射；`NewTableKVEncoderFromMeta` 为没有完整 SQL `table::Table` 对象的运行时建立可见列映射。
- 将 parser datum 按 `FieldMapping` 分派到目标列或大小写不敏感的用户变量，求值 `SET` 列赋值，并处理缺失输入。
- 调用 `ImportDatumConverter` 做目标列类型转换，再由 `BaseKVEncoder::ProcessColDatum` 补默认值、自动值等列语义，并求值生成列。
- 对非聚簇表追加隐式 `_tidb_rowid`，维护 RowID allocator 水位，并将实际生成的 handle 传给 `Record2KV`。
- 将 `TableInfo` 转成 Lightning `TableDefinition`，保留列类型、主键/handle 形态、Public 索引、分片行号和 auto-random 配置。
- 统计真正产生独立索引 KV 的索引：仅 Public 索引，且排除已经由记录键承载的聚簇主键。

## 主要符号

- `TableKVEncoder`：有状态、面向单表的编码器。公共字段 `BaseKVEncoder` 提供会话、表列、分配器、生成列表达式及 `Record2KV`；其余字段保存赋值表达式、字段映射、写入列、datum 转换器和三个可复用行缓冲。
- `NewTableKVEncoder(config, controller)`：常规 `LOAD DATA` / `IMPORT INTO` 构造入口，使用 `controller.FieldMappings` 与 `controller.InsertColumns`。
- `NewTableKVEncoderForDupResolve(config, controller)`：冲突/重复键解析入口，借助 `tableVisCols2FieldMappings` 覆盖全部可见列。
- `NewTableKVEncoderFromMeta(config, meta, datum_converter)`：仅依赖持久化元数据的构造入口；隐藏列不进入输入映射，且会先验证 `BaseKVEncoder.Columns.len() == meta.Columns.len()`。
- `CanonicalImportDatumConverter(types::Flags)`：`ImportDatumConverter` 的 canonical 实现；`CastColumnValue` 先转换为 canonical datum，再按 `FieldType` 转换；`CurrentTime` 生成 UTC 微秒格式时间字符串后复用同一转换逻辑。
- `NewTableDefinitionFromMeta(meta)`：把 `TableInfo` 映射为 `TableDefinition`。`encodingColumnType` 完成 MySQL 类型到 canonical `ColumnType` 的分类。
- `TableKVEncoder::Encode`：唯一公共逐行入口；无论成功失败，调用结束前都会 `TruncateWarns`。
- `parserData2TableData`、`getRow`、`fillRow`：分别负责字段映射/SET 求值、显式值类型转换、缺省/生成列与隐藏 row ID 补全。
- `GetIndicesGenKV`、`GetNumOfIndexGenKV` 与 `GenKVIndex`：向调度、引擎规划和进度估算暴露独立索引 KV 的 ID、名称及唯一性。
- `Close`：关闭底层 `SessionCtx`；调用方必须在编码器生命周期结束时执行。

## 执行流程

1. 构造阶段调用 `NewBaseKVEncoder`。控制器路径还通过 `CreateColAssignSimpleExprs` 编译 `SET` 表达式，并验证 Lightning 列数和 SQL 表列数一致；元数据路径建立所有非隐藏列的 `FieldMapping`，不带 `SET` 表达式。
2. `Encode(parser_data, row_id)` 调用 `encodeRow`，结束后截断会话 warning，避免 warning 长期持有 parser 输入缓冲。
3. `parserData2TableData` 清空并复用三组缓存，先把所有 `insert_columns` 标记为“有显式值”。随后逐个处理 `field_mappings`：用户变量写入/清除 `SessionCtx`；列输入缺失时，NOT NULL 时间列取当前时间，其他列写 `NULL` 并清除显式值标记；正常输入按原 datum 暂存。
4. 所有 `column_assignments` 在同一 `SessionCtx` 中求值并追加到 insert-row 缓冲，因此表达式可读取此前设置的用户变量。
5. `getRow` 按 `insert_columns` 顺序取值并调用 `datum_converter.CastColumnValue`，再依列 `Offset` 放入表顺序的 `row_cache`。
6. `fillRow` 逐列调用 `BaseKVEncoder::ProcessColDatum`。显式值传入且不要求再次 cast；缺值列传 `None` 并允许底层应用默认值、自动值或缺值规则。
7. 若 `table_has_auto_row_id`（既非整数主键 handle，也非 common handle），将 `AutoIDFn(row_id)` 产生的 datum 追加到 record，并把 RowID allocator rebase 到原始 `row_id`。若存在生成列，再调用 `EvalGeneratedColumns` 更新 record。
8. `encodeRow` 对隐式 row ID 表从 record 尾部取得实际生成的 `Int/UInt` handle；否则沿用调用者的 `row_id`，最后调用 `Record2KV(record, parser_data, encoded_row_id)` 生成记录和索引 `Pairs`。

## 数据与状态

`field_mappings` 描述每个输入字段落到表列还是用户变量；`insert_columns` 是需要从输入或 `SET` 得到值的列集合，两者并不必然等长。`column_assignments` 的结果按控制器建立的 insert 列顺序追加。`has_value_cache` 以表列 offset 为索引，区分“显式 NULL”与“没有输入”；这是默认值处理是否启动的关键不变量。

`insert_column_row_cache` 按 insert 列顺序保存原始/表达式结果，`row_cache` 按完整表列 offset 保存转换后的值，`BaseKVEncoder` 的 record 缓冲则按最终编码列顺序保存补全后的行。三个层次不可混用。每次 `Encode` 都先 `clear`/`resize`，所以缓存跨行复用但不会保留上一行的逻辑值。

`NewTableDefinitionFromMeta` 只纳入 `StatePublic` 索引。索引列优先使用非负 `Offset`，否则以小写列名回查；不存在或越界都会失败。整数主键列既可由 `PriKeyFlag` 识别，也可由主索引列识别，以覆盖 Go 中整数聚簇主键没有独立 `IndexInfo` 的情况。`ShardRowIDBits` 和 `AutoRandomBits` 被夹到 `u8` 范围。

## 依赖与调用关系

上游直接证据包括：

- `TableImporter::getKVEncoder` / `GetKVEncoderForDupResolve`（`pkg/executor/importer/table_import.rs`）分别创建常规与恒等 AutoRowID 的去重编码器。
- `ChunkEncoder::encodeLoop`（`pkg/executor/importer/chunk_process.rs`）读取一行后调用 `encoder.Encode`，把错误补充源 chunk 与偏移，再把 `Pairs` 分批发送到后端引擎。
- `QueryRuntime::GetKVEncoder`（`pkg/session/runtime/import_query.rs`）用真实 allocator、SQL mode 和 chunk 时间戳构造元数据编码器。
- `SampleFileImportKVSizeWithTableInfo` / 采样循环（`pkg/executor/importer/sampler.rs`）复用相同表定义和编码结果，分别累计 data/index KV 大小。
- `pkg/dxf/importinto/conflict_resolution.rs` 为并发冲突处理 worker 构造元数据编码器，并在 `ImporterConflictCodec::EncodeRow` 中从 canonical datum 转回 backend datum 后调用 `Encode`。
- `GetIndicesGenKV` 被 DXF task executor、encode-and-sort operator 使用；`GetNumOfIndexGenKV` 还用于 scheduler 告警阈值和 importer 计划估算。

主要下游是 `astersql-lightning-backend-kv` 的 `NewBaseKVEncoder`、`ProcessColDatum`、`EvalGeneratedColumns`、`Record2KV` 和 allocator API；类型转换依赖 `astersql-types`、`astersql-table` 与 `astersql-lightning-backend-encode`；元数据及 MySQL flag/type 来自 `astersql-meta-model`、`astersql-parser-mysql`。这些均由 `pkg/executor/importer/Cargo.toml` 声明，crate 本身没有控制本文件行为的 feature 条件。

## 错误处理与边界

- 所有公开构造与编码 API 统一返回 `Result<_, String>`；底层错误通常转为字符串传播。本文件没有重试，重试/取消和源位置增强由上游负责。
- 两条构造路径都拒绝编码列数与表元数据列数不一致。`NewTableDefinitionFromMeta` 还拒绝索引引用未知列或越界 offset。
- `FieldMapping` 若既无 `Column` 又无 `UserVar` 会失败；insert 缓冲短于 `insert_columns` 时，`getRow` 报告缺失列名。
- cast 失败会调用 `LogKVConvertFailed`，并构造含列名、目标类型、输入值和根因的 `[Import:ErrCastValue]` 信息。`datum_for_cast_error` 对字节采用有损 UTF-8 仅用于日志展示，不改变参与编码的原字节。
- `ProcessColDatum` 与生成列表达式错误也经底层日志辅助函数补充行/列上下文；生成列错误使用底层返回的列索引定位列名。
- 隐式 row ID 只接受 record 尾部的 `Int`/`UInt` 作为最终 handle；异常形态保守回退到调用者 `row_id`。`UInt` 到 `i64` 使用 Rust 转换语义，扩展超大无符号 handle 时需专门核对边界。
- `CanonicalImportDatumConverter::CurrentTime` 使用 `chrono::Utc::now()`，与配置中的固定 session timestamp 并非同一来源；涉及可复现导入时间语义的改动必须先补测试确认。

## 并发与资源生命周期

`TableKVEncoder` 持有可变缓存、可变 `SessionCtx` 和 allocator 状态，`Encode` 需要 `&mut self`，设计上是单 worker 独占而非多线程共享。冲突处理若需要并发，会在 `conflict_resolution.rs` 中为每个 scoped worker 单独创建 encoder；chunk 编码同样由各自 `ChunkEncoder` 持有实例。

每行缓存被复用以减少分配；`Encode` 每次都截断 warning，避免错误/warning 引用输入内存造成 parser 缓冲无法释放。编码器用完必须调用 `Close` 关闭 `BaseKVEncoder.SessionCtx`；测试和冲突 codec 都显式执行该操作。文件中没有锁、异步任务或通道；背压、线程退出和批次传输属于上游 chunk/DXF worker。

## 与 Go 版本的对应关系

Rust 主流程直接对应 `pkg/executor/importer/kv_encode.go`：`TableKVEncoder`、两个控制器构造入口、字段映射、用户变量、缺失 NOT NULL 时间列、SET 求值、cast、`ProcessColDatum`、隐式 row ID、生成列、`Close` 以及索引过滤均保留。

已确认的 Rust 扩展/差异如下：

- Rust 新增 `NewTableKVEncoderFromMeta`、`CanonicalImportDatumConverter`、`NewTableDefinitionFromMeta` 和 `encodingColumnType`，服务查询导入、采样及不具备 Go `table.Table` mutation 接口的运行时；Go 同路径文件没有这些入口。
- Rust 构造阶段显式校验 Base encoder 与目标表的列数；Go 的 `newTableKVEncoderInner` 不做此检查。
- Go `Encode` 最终把原始 `rowID` 传给 `Record2KV`；Rust 对非聚簇表从 record 末尾取 `AutoIDFn` 生成的实际 handle，保证分片/非恒等 row ID 的记录键与行内容一致。Rust 独立测试覆盖 identity 与 sharded 两种情况。
- Go 的 RowID allocator `Rebase` 可返回错误并传播；当前 Rust allocator 调用没有可传播返回值。不能据此假定两边故障模型完全相同。
- Go 的缺失时间列直接用 `types.CurrentTime`；Rust 委托 `ImportDatumConverter::CurrentTime`，使控制器实现与 canonical 实现可分别决定转换细节。

## 扩展指南

- 新增字段映射或 `SET` 语义时，优先修改 `parserData2TableData`，同时维护 `insert_columns` 与追加结果的顺序不变量；测试放在独立的 `kv_encode_test.rs`，不要嵌入生产文件。
- 新增 MySQL 字段类型支持时修改 `encodingColumnType`，并同时验证 `CanonicalImportDatumConverter` 和 `TableDefinition` 的列元数据足够让 `Record2KV` 正确编码；特别关注 unsigned、binary/collation、enum/set、decimal、时间与 geometry。
- 修改默认值、自动值、生成列或 handle 逻辑时，应从 `fillRow` 与 `encodeRow` 接入，并核对 `BaseKVEncoder` 的契约。必须同时验证整数聚簇主键、common handle、非聚簇隐式 row ID、sharded row ID 和 auto-random。
- 修改索引筛选时同步 `GetIndicesGenKV` 与 `NewTableDefinitionFromMeta`：前者影响引擎/调度分组，后者决定实际 KV 编码；两者对 Public、聚簇主键的判断若漂移，会造成计数与输出不一致。
- 保持 Go 对照逻辑；若 Rust 为真实运行时增加必要能力，应在本文列明差异，并在 `kv_encode_test.rs` 或同目录独立测试文件增加回归。性能上需维持缓存复用，避免逐行重复构造列元数据或不必要 clone。
- 新调用方必须明确 `row_id` 的来源、session SQL mode/时间戳、allocator 所有权，并保证所有退出路径调用 `Close`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/importer` 确认目标、Go 对照及独立测试均已索引。
- RustCodeGraph `node --file pkg/executor/importer/kv_encode.rs`：逐行读取完整 522 行，确认构造入口、类型映射、三段行处理流水线、资源关闭和索引过滤实现；文件节点报告被 11 个文件使用。
- RustCodeGraph 精确源码节点：读取 `table_import.rs` 的构造调用、`chunk_process.rs` 的逐行调用、`sampler.rs` 的元数据/采样调用、`session/runtime/import_query.rs` 的查询导入调用，以及 `dxf/importinto/conflict_resolution.rs` 的 worker 构造与编码调用。
- crate/模块边界：`pkg/executor/importer/Cargo.toml` 与 `pkg/executor/importer/lib.rs`。
- Go 对照：`pkg/executor/importer/kv_encode.go`；Go 回归：`pkg/executor/importer/kv_encode_test.go`（identity/sharded row ID、tinyint/enum cast 错误）。
- Rust 独立测试：`pkg/executor/importer/kv_encode_test.rs`（整数主键 handle、分片 row ID、cast 错误、VARCHAR 索引恢复）；`pkg/executor/importer/kv_encode_aster_unit_test.rs`（元数据 cast/隐藏 handle、Public 非聚簇索引过滤）。
- 调用搜索：精确 `rg` 核对 `NewTableKVEncoder*`、`NewTableDefinitionFromMeta`、`GetIndicesGenKV`、`GetNumOfIndexGenKV` 及 `.Encode(` 的 Rust 调用点。RustCodeGraph 的通用符号 `callers/callees` 查询未产生可用输出，故按索引图覆盖不足规则用该精确搜索补齐证据。
- 本任务只新增说明文档；按计划不运行 Cargo。交付前以任务指定命令验证恰有 11 个固定二级章节，并人工复核文档未把未证实设计写成现状。
