# `pkg/executor/importer/import.rs`

## 文件定位

本文件（[源文件](./import.rs)）是 `astersql-executor-importer` crate 的导入计划与数据源控制层。crate 根 `pkg/executor/importer/lib.rs` 以私有模块 `mod import` 装配本文件，再通过 `pub use import::*` 暴露公共项；同 crate 的 `kv_encode.rs`、`precheck.rs`、`production_regions.rs`、`table_import.rs` 等在其上继续完成编码、前置检查、分区和单表导入。`pkg/executor/importer/Cargo.toml` 的 `[package.metadata.porting]` 将整个 crate 对应到 Go 包 `pkg/executor/importer`，本文件的直接 Go 对照是 `pkg/executor/importer/import.go`。

它不直接执行完整的 IMPORT INTO 分布式任务，而是负责把 SQL/AST、会话状态和可注入服务固化为 `Plan` 与 `LoadDataController`，再完成数据源打开、文件发现、格式/大小估算、解析器创建和资源参数计算。生产主链可见于 `pkg/dxf/importinto/scheduler.rs`：从任务元数据恢复 `ASTArgs`，构造控制器，依次调用 `InitDataFiles`、数据量检查、`CalResourceParams`，最后把准备结果写回 job；`task_executor.rs` 和 `encode_and_sort_operator.rs` 则在执行阶段重新构造控制器并初始化存储。

## 核心职责

- 表达导入配置：`Plan` 聚合目标表、数据源、CSV 规则、会话 SQL mode、并发/配额、冲突处理、全局排序、前置检查和持久化诊断参数；`Summary`/`StepSummary` 表达各执行阶段统计。
- 构造计划：`NewPlanFromLoadDataPlan` 处理 LOAD DATA 语义；`NewImportPlan` 处理 IMPORT INTO 默认值、选项、SEM/nextgen 限制、查询数据源差异和脱敏参数。
- 构造控制器：`NewLoadDataController` 建立输入字段映射、插入列与 SET 表达式构建器，并注入存储、解析、大小估算、TiKV 探测和资源计算服务。
- 准备数据源：`InitDataStore`、`CheckDataSourceAccess`、`InitDataFiles` 校验 URI/glob/服务器路径，打开对象存储，枚举文件，检测格式与压缩类型，并累计物理大小和估算真实大小。
- 为后续编码提供输入：`GetLoadDataReaderInfos`、`GetParser`、`OpenParquetParser`、`GenerateCSVConfig` 和 `CreateColAssignSimpleExprs` 分别提供 reader、mydump parser、CSV 规则与 SET 表达式。
- 计算执行参数和本地 backend 配置：`CalResourceParams` 根据真实数据量、CPU、索引占比与调度系数回写线程数/节点数/DistSQL 并发；`getLocalBackendCfg` 产生 Lightning local backend 参数。

## 主要符号

- 常量与枚举：`DataFormatCSV`、`DataFormatDelimitedData`、`DataFormatSQL`、`DataFormatParquet`、`DataFormatAuto` 定义格式名；`PostOpLevel` 定义后置校验的 off/optional/required；`OnDupKeyMode` 定义 capture/error；`DataSourceType` 区分 File/Query；`ByteSize` 是字节数包装。
- `Plan`：核心计划快照。`Default` 只建立安全的结构默认值，例如 checksum required、一个线程、错误上限 100、格式 auto；磁盘配额保持 0，等待后续容量调整。`initDefaultOptions`、`adjustOptions`、`CheckNonCSVFormatOptions` 分别填入运行默认值、按 CPU 裁剪并发并处理全局排序约束、拒绝非 CSV 格式上的 CSV 专属选项。
- `ASTArgs`、`ImportParameters`：前者保留列/用户变量、SET、FIELDS/LINES 与重复键 AST；后者保存可持久化且 URL 已脱敏的文本参数。
- `ImportSessionContext`、`ResolvedLoadDataPlan`、`ResolvedImportIntoPlan`：把宿主的会话和 planner 类型隔离成窄接口。`ImportOptionValue` 是选项求值后的 string/integer/boolean 联合。
- 服务 trait：`ColumnAssignmentFactory`/`ColAssignExpressionBuilder`/`ColAssignExpression` 负责 SET 表达式；`ImportDatumConverter` 负责 Datum 转换；`ImportParserFactory`、`ImportSizeEstimator`、`ImportStorageFactory`、`TiKVConfigProbe`、`ImportResourceCalculator` 分别隔离解析、估算、存储、TiKV 配置与资源计算。
- `LoadDataController`：持有 `Plan`、表、字段映射、数据文件、共享存储和服务。`LoadDataControllerServices` 是构造时依赖集合；`WithDataStore`、`WithGlobalSortStore` 是可选注入点。
- 文件与解析辅助：`parse_data_source_path`、`check_data_source_glob`、`glob_matches`、`parseFileType`、`compression_from_path`、`open_parquet_file`；选项辅助包括 `initOptions`、`parseByteSize`、`getImportantSysVars`。
- `compressionEstimator`：按压缩类型采样膨胀比；达到 `maxSampledCompressedFiles`（512）后用 `getHarmonicMean` 固化缓存。

本文件没有条件编译项。可见性上，跨 crate API 多为 `pub`；`is_option_allowed_for_query`、`storage_path`、`OpenParquetFile`/`open_parquet_file` 为 crate 内可见；选项解析、glob、压缩后缀等细节保持私有。

## 执行流程

1. IMPORT INTO 入口调用 `NewImportPlan`：复制表/库/路径/格式与会话快照，用 `getDataSourceType` 区分文件和 SELECT，调用 `initDefaultOptions`，再由 `initOptions` 校验选项名、值存在性、重复项、SEM/nextgen/查询数据源限制并写入字段；`adjustOptions` 裁剪线程并在全局排序时禁用 TiKV import mode；`initParameters` 格式化列和 SET，且对文件位置、`cloud_storage_uri` 脱敏。
2. LOAD DATA 路径使用 `NewPlanFromLoadDataPlan`：采用 delimited data 格式，按 strict SQL mode 与 IGNORE 决定 `Restrictive`，从 FIELDS/LINES AST 生成分隔符和 NULL 定义，并保存重要系统变量与 DistSQL 并发。
3. `NewLoadDataController` 通过 `buildFieldMappings` 把输入位置映射到可见列或用户变量，通过 `buildInsertColumns` 合并 SET 目标列且拒绝未知/重复列，再把每个赋值编译为延迟构建器；应用 `ControllerOption` 后运行 `checkFieldParams`。
4. 文件导入准备先由 `InitDataStore` 解析存储根和对象 key，必要时打开数据源存储；全局排序还通过 `GetSortStore` 打开 cloud storage。查询导入在这里直接返回，不创建文件存储。
5. `InitDataFiles` 校验服务器绝对路径策略和 glob，枚举匹配对象或打开单文件，按路径排序；逐文件执行自动格式识别、压缩后缀识别、Parquet 格式膨胀估算与解压真实大小估算，并以饱和加法累计 `Plan.TotalFileSize` 和 `TotalRealSize`。查询导入则清空文件及大小。
6. scheduler 调用 `CalResourceParams`：获取目标 CPU 与调度系数；仅当表含二级索引时采样索引比例，采样失败回落到 0；计算结果回写 `ThreadCnt`、`MaxNodeCnt`、`DistSQLScanConcurrency`。
7. 执行阶段通过 `GetParser` 取得对应文件 reader。Parquet 走 `open_parquet_file` 的范围读取器和 location-aware parser；其余格式由 `ImportParserFactory::NewParser` 创建，再由 `HandleSkipNRows` 消费需要跳过的行。`production_regions.rs` 使用 `GenerateCSVConfig` 生成切分配置，`kv_encode.rs` 使用 `CreateColAssignSimpleExprs` 构建编码阶段的 SET 表达式。
8. 所有权持有者结束时显式调用 `Close` 关闭数据源和全局排序存储；例如 `pkg/dxf/importinto/planner.rs` 在 chunk 规划闭包结束后调用它。

## 数据与状态

`Plan` 是可克隆的任务级配置快照，其中 `TableInfo`/`DesiredTableInfo` 使用 `Arc` 共享元数据，`ImportantSysVars`、`SpecifiedOptionNames` 和 `Parameters` 保存编码与诊断所需状态。`Format` 可能在构造时是 `auto`，随后由首批文件的 `detectAndUpdateFormat` 改写，并同步 `Parameters.Format`；因此调用者若需要检查“用户是否选择 auto”，必须像 `scheduler.rs` 一样在 `InitDataFiles` 前保存该事实。

`LoadDataController` 的 `data_files`、`data_store`、`global_sort_store` 是准备后的可变运行状态；`Plan.TotalFileSize` 记录存储对象字节，`TotalRealSize` 记录解压并考虑格式膨胀后的估计量，二者用途不同。`ExecuteNodesCnt` 初始为 1，其他模块可在调度结果已知后设置。查询数据源不维护文件列表，大小归零。

字段状态分成 `FieldMappings`（每个输入字段去向）、`InsertColumns`（最终写入/SET 列）和 `ColumnAssignments`（延迟表达式构建器）。`SharedStorage = Arc<Mutex<Box<dyn Storage + Send>>>` 允许 opener 与控制器共享单个后端句柄；reader opener 当前会在锁内把对象完整读入 `Vec<u8>`，再返回内存 `Cursor`，而 Parquet 使用按 byte range 打开的流。

## 依赖与调用关系

上游生产调用以 `pkg/dxf/importinto` 为主：

- `scheduler.rs` 构造控制器并串联 `InitDataFiles`、`CalResourceParams` 和任务准备状态更新。
- `task_executor.rs`、`encode_and_sort_operator.rs` 为 encode/sort 子任务构造控制器并调用 `InitDataStore`。
- `planner.rs` 构造控制器、发现文件、生成 chunks，并在结束时 `Close`。
- `conflict_resolution.rs` 也构造控制器以服务冲突处理。

同 crate 下游包括：`precheck.rs::CheckRequirementsBeforeInitDataFiles` 在文件发现前调用 `CheckDataSourceAccess`；`kv_encode.rs::newTableKVEncoderInner` 调用 `CreateColAssignSimpleExprs`；`production_regions.rs` 调用 `GenerateCSVConfig`；`table_import.rs`、`engine_process.rs` 消费 parser、文件和本地 backend 配置。

外部依赖从源码与 Cargo 声明可核对：parser AST/MySQL SQL mode 提供语法和模式；`astersql-table`/`astersql-meta-model` 提供表与列；Lightning mydump/kv/encode 提供源文件、parser、Datum 和编码上下文；objstore 的 parse/storeapi/objectio 提供 URI、存储、范围 reader；dumpformat parquetfile 提供 Parquet source reader/parser。所有具体环境能力经 trait 注入，便于生产实现与独立测试替换。

RustCodeGraph 索引显示 `import.rs` 被 37 个文件使用，但当前精确 `callers`/`callees` 对本文件 Rust 符号未产出边；上述调用关系因此以 RustCodeGraph 文件/符号结果加精确 `rg` 命中和相邻源码读取共同确认，不把缺失的图边解释为“没有调用者”。

## 错误处理与边界

本文件统一用 `Result<_, String>` 穿过宿主边界，并在存储打开处补充目标上下文。主要拒绝条件包括：空文件路径；IMPORT INTO 不支持的格式；LOAD DATA 的空分隔符或非法 NULL optional enclosed 组合；字段包围符与终止符互为前缀；未知、重复或值存在性错误的选项；查询导入/SEM/nextgen 禁止的选项；本地排序上的 `on_duplicate_key`；split file 与 skip rows/空行分隔符冲突；未知或重复列；无效/溢出的字节大小；不支持的服务器文件后缀；无效 URI/glob；未初始化存储或未发现目标文件。

大小累加使用 `saturating_add`，避免大文件集合整数溢出；`parseByteSize` 使用 `checked_mul` 并明确报溢出。Parquet 膨胀比最低为 1.0，空文件回落 2.0；压缩采样失败或非正值回落 1.0。索引比例采样失败在 `CalResourceParams` 中回落 0.0，这是显式的可用性策略，而 CPU、调度系数或最终计算服务错误仍向上传播。

`CheckDataSourceAccess` 对 glob 只需成功打开首个对象即终止遍历；使用内部 sentinel 错误停止后，根据 `stopped_after_first_object` 将结果视为成功。非 glob 路径的错误会附带检查文件位置的提示。互斥锁中毒：表达式锁和压缩估算锁恢复 poisoned inner；存储锁则返回错误。`Close` 是 best-effort，锁失败或底层关闭结果不会向上传播。

## 并发与资源生命周期

控制器及 opener 共享存储使用 `Arc<Mutex<...>>`；存储 API 的 Open/Walk/Close 由互斥锁串行化。`CreateColAssignSimpleExprs` 用 `col_assign_mu` 保护表达式构建，避免同一控制器被并发编码器重复编译时进入非线程安全构建路径。`compressionEstimator` 分别锁住样本与固化比例；它先查缓存，再采样并追加，达到 512 个样本后计算调和平均。

`ReaderOpener`、服务 trait 和表达式 trait 要求 `Send + Sync`（opener 还要求 `'static`），以适应分布式执行器持有和跨任务调用。普通数据文件 opener 会在持锁期间完整读取文件，锁释放后 parser 只操作内存 cursor；这降低 reader 生命周期复杂度，但大文件会产生内存峰值并限制同一 store 的并行打开。Parquet 的 `RangeOpener` 每次范围读取都检查 `StorageContext` 取消状态；`ClosingObjectReader::Drop` 确保 object reader 被关闭，元数据探测路径也显式关闭 reader。

控制器没有 `Drop` 实现，资源释放依赖上层调用 `Close`。扩展调用链时必须保持“构造/初始化成功后无论后续成功或失败都关闭”的结构；不要假设离开作用域会自动调用后端 `Storage::Close`。

## 与 Go 版本的对应关系

Rust 符号总体沿用 `pkg/executor/importer/import.go` 的职责与 Go 风格命名：`Plan`、`ASTArgs`、`LoadDataController`、`NewImportPlan`、`NewLoadDataController`、`InitDataFiles`、`CalResourceParams`、`GenerateCSVConfig`、`parseFileType`、`compressionEstimator` 等均有直接对应。共同语义包括：query/file 默认并发差异、global sort 禁用 import mode、非 CSV 选项校验、参数 URL 脱敏、格式自动识别、压缩采样调和平均、Parquet location 和真实大小估算。

移植形态存在必要差异：Go 直接依赖 `sessionctx`、planner core、全局配置和具体存储实现；Rust 通过 `ImportSessionContext`、resolved-plan traits 与 `LoadDataControllerServices` 注入适配，且错误被归一成字符串。Go 的 controller 还包含 logger 等宿主对象；Rust 的本文件只保留此 crate 当前执行路径所需状态。Rust 的 `GetLoadDataReaderInfos` 会完整缓冲对象，Parquet 则额外实现范围读取桥接；这两点应按 Rust 当前源码理解，不能从 Go 实现推断资源行为。

`pkg/executor/importer/import_test.rs` 前半部的 `GO_REFERENCE` 是原始字符串中的 Go 测试参考和仿真文本，不会作为 Rust 用例执行。实际编译的文件末尾测试验证：参考文本仍存在、LOAD DATA/IMPORT INTO 的 CSV 默认差异、未知后缀回落 CSV、默认磁盘配额为 0、查询导入选项白名单、字节大小解析、零 CPU 裁剪、checksum backoff 默认值以及服务器绝对路径映射。更广的 Go 行为证据来自 `pkg/executor/importer/import_test.go`；不能把原始字符串中的断言当成 Rust 已获得同等覆盖。

## 扩展指南

- 新增 IMPORT INTO 选项时，至少同步 `option_expects_value`、`initOptions` 的类型/范围处理、`Plan` 字段、`SpecifiedOptionNames` 相关格式限制和 `Plan::initParameters` 持久化/脱敏策略；若查询导入可用，还要更新 `is_option_allowed_for_query`。同时对照 Go `Plan.initOptions`，在独立的 `import_test.rs` 增加真实 Rust 测试，不把测试嵌入生产文件。
- 新增格式时，同步格式常量、`checkFieldParams` 支持集合、`CheckNonCSVFormatOptions`、`parseFileType`、`getSourceType`、parser factory 和大小估算；检查压缩后缀组合、auto 格式多文件一致性、`production_regions.rs` 的切分配置，以及 Go `parseFileType`/`newLoadDataParser`。
- 改动字段/SET 映射时，从 `buildFieldMappings`、`buildInsertColumns` 与 `CreateColAssignSimpleExprs` 接入，并验证未知列、大小写、用户变量、重复列、生成列/不可见列等边界；同步 `kv_encode.rs` 消费契约和独立测试。
- 改动对象存储路径时，同时检查 `parse_data_source_path`、`check_data_source_glob`、`InitDataStore`、`CheckDataSourceAccess`、`InitDataFiles` 与 `storage_path`；特别保护带凭证 URI 的 storage root、对象 key、服务器本地 basename 和关闭语义。
- 改动并发/资源估算时，从 `Plan::initDefaultOptions`、`adjustOptions`、`CalResourceParams`、`getLocalBackendCfg` 接入；评估 query/file、global/local sort、零 CPU、无索引、采样失败和 nextgen 情况，并同步 scheduler 对结果的使用。
- 改动 reader 生命周期时要评估 `SharedStorage` 锁粒度和整文件缓冲的内存/吞吐风险；Parquet 改动还需保持取消检查、范围边界、文件大小探测与 reader close。公共行为的回归测试继续放在同目录 `import_test.rs` 或相关独立测试文件中。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 文件、307,296 节点和 1,848,419 条边；`files --filter pkg/executor/importer` 确认目标 Rust/Go/测试/模块文件均在索引；`node --file pkg/executor/importer/import.rs --offset ...` 分四段核对了 2,346 行源码和文件被 37 个文件使用的信息；`query NewImportPlan`、`query NewLoadDataController` 区分了 Go/Rust 同名符号。精确 `callers/callees` 对这些 Rust 符号没有返回边，故没有据此虚构调用关系。
- Rust 源与装配：`pkg/executor/importer/import.rs`、`pkg/executor/importer/lib.rs`、`pkg/executor/importer/Cargo.toml`。
- 直接生产调用：`pkg/dxf/importinto/task_executor.rs`、`encode_and_sort_operator.rs`、`scheduler.rs`、`planner.rs`、`conflict_resolution.rs`；同 crate 消费者 `pkg/executor/importer/precheck.rs`、`kv_encode.rs`、`production_regions.rs`、`table_import.rs`、`engine_process.rs`。
- Go 对照与测试：`pkg/executor/importer/import.go`、`pkg/executor/importer/import_test.go`；Rust 独立测试 `pkg/executor/importer/import_test.rs`，并参考 `precheck_test.rs`、`importer_testkit_test.rs` 中的控制器调用。目标目录不存在 `doc.go`，因此没有额外的包级 Go contract 可读。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰好包含上述 11 个固定二级标题，并人工复核所有现状陈述均可回指到这里列出的符号或文件。
