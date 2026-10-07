# `pkg/executor/importer/sampler.rs`

## 文件定位

本文件属于 `astersql-executor-importer` crate，由 `pkg/executor/importer/lib.rs` 的 `mod sampler; pub use sampler::*;` 挂载并向 crate 外重导出。它位于 IMPORT INTO/LOAD DATA 的资源估算链路，不执行整个导入，而是从最多 3 个源文件中总计抽样约 30 行，把行编码成 TiKV KV，返回源字节、Data KV 字节和 Index KV 字节。

主应用链路中，`LoadDataController::CalResourceParams` 仅在表会生成索引 KV 时通过 `ImportResourceCalculator::SampleIndexSizeRatio` 请求采样；`HandleImportResourceCalculator::SampleIndexSizeRatio` 再调用本文件的 `LoadDataController::sampleIndexSizeRatio`（`pkg/executor/importer/import.rs:1116`、`pkg/executor/importer/production_resource.rs:94`）。另一条 SDK 链路由 `pkg/importsdk/file_scanner.rs::estimateOneTableSize` 调用 `SampleFileImportKVSizeWithTableInfo`，用抽样的 KV/源字节比例外推全表体量。

## 核心职责

- 约束抽样成本：`maxSampleFileCount = 3`、`totalSampleRowCount = 30`，非 Parquet 还受 `maxSampleFileSize = 10 MiB` 的起始偏移上限约束。
- 从 `KVSizeSampleConfig` 验证格式和字段分隔规则，并建立列/用户变量到表列的映射及插入列集。
- 通过注入的 `KVSizeSamplerService` 创建解析器和表编码器，逐行累计实际消费的源字节，再借助 `EncodedKVGroupBatch` 的 checksum 区分 Data/Index KV 字节。
- 为 importsdk 提供不依赖完整 `LoadDataController` 的 `TableInfo` 入口：它构造 `TableDefinition` 和 `BaseKVEncoder`，并按可见、Public、非生成列的原始 offset 放置数据。
- 向资源计算器提供 `IndexKVSize / DataKVSize`；Data KV 为 0 时显式返回 0，避免除零。

## 主要符号

- `SampledKVSizeResult { SourceSize, DataKVSize, IndexKVSize }`：一次聚合结果。`TotalKVSize` 使用 `u64::wrapping_add` 后转 `i64`，保留 Go `uint64` 加法的溢出语义。
- `KVSizeSampleConfig`：解析与编码的值对象，含格式、SQL mode、字符集、系统变量、NULL 定义、行/字段规则、忽略行、列/用户变量及赋值表达式。
- `KVSizeSamplerService`：完整路径的工厂边界，提供 `NewParser` 和 `NewEncoder`；返回的 parser 要求 `Send`。
- `KVSizeParserService`：SDK 路径只注入 `NewParser`，编码器由 `SampleFileImportKVSizeWithTableInfo` 内部直接构造。
- `SampleFileImportKVSizeWithTableInfo` / `sampleTableInfoFile`：基于持久化 `TableInfo` 的公开入口和单文件实现。
- `SampleFileImportKVSize` / `newKVSizeSampler` / `KVSizeSampler`：完整导入路径的公开入口、构造器和状态容器。
- `KVSizeSampler::{sample, sampleOneFile, sampleRows, sampledRowSourceSize}`：分别负责选文件/聚合、资源建立与清理、读行编码循环、单行源大小估算。
- `LoadDataController::{buildKVSizeSampleConfig, sampleKVSize, sampleIndexSizeRatio}`：把导入计划与 AST 参数桥接到采样器。
- `sample_file_indices`：以系统时间为种子的 xorshift/Fisher–Yates 洗牌，返回不重复且不越界的最多 3 个下标。
- `parser_datum_to_encoder_datum` / `parser_datum_to_encoder_datum_for_column`：将 mydump datum 转为 Lightning encoder datum；后者额外根据 Int/UInt/Float 列类型解析 CSV 字节串。

## 执行流程

1. `SampleFileImportKVSize` 调用 `newKVSizeSampler`；后者先运行 `validateKVSizeSampleConfig`，再通过 `buildFieldMappings` 和 `buildInsertColumns` 固化字段映射。
2. `KVSizeSampler::sample` 用 `sample_file_indices` 选最多 3 个文件，将 30 行按选中文件数整数均分，顺序调用 `sampleOneFile`。
3. `sampleOneFile` 先构造 parser，跳过 `IgnoreLines` 行并把 row ID 重置为 0，再构造 encoder。跳过期间到 EOF 视为空结果，其他解析错误则返回。
4. `sampleRows` 每轮记录 parser 起始位置；非 Parquet 起始位置达 10 MiB 即停，否则读一行，按位置差（Parquet 按 `Row.length`）累计源字节。
5. 读到的 datum 转为 encoder datum，`TableKVEncoder::Encode` 生成 KV；原行在检查编码结果之前就交还 parser 的回收机制，KV 再加入 `EncodedKVGroupBatch`。
6. 循环因行数、字节上限或 EOF 结束后，从 `group_checksum.DataAndIndexSumSize` 取得两类 KV 字节；多文件结果使用饱和加法聚合。
7. `SampleFileImportKVSizeWithTableInfo` 使用同样的选文件、跳表头、截断和聚合策略，但会先筛出可见 Public 非生成列，把源行值放入完整表行的原 offset，再调用 `BaseKVEncoder::Record2KV`。

## 数据与状态

`KVSizeSampler` 仅在构造时持有不变的配置、`Arc<dyn Table>`、文件元数据、字段映射和插入列。采样的可变状态（parser 位置、row ID、encoder、batch checksum、当前计数和聚合结果）都限于方法栈和单文件循环，没有持久化副作用。

`SourceSize` 表示实际被抽样行消费的源字节，不是整个文件大小：CSV/SQL 优先取 `Parser::Pos` 的前后差，若位置未前进则回退到 `Row.length`；Parquet 的位置是行计数而非字节，因此始终用 `Row.length`。`DataKVSize`/`IndexKVSize` 是 batch checksum 统计，`keyspace_codec` 会影响编码后字节数。

## 依赖与调用关系

上游调用边包括：

- `pkg/executor/importer/import.rs::LoadDataController::CalResourceParams` 通过 `ImportResourceCalculator` 计算索引占比；`pkg/executor/importer/production_resource.rs::HandleImportResourceCalculator` 将调用接到本文件。采样错误在 `CalResourceParams` 处被 `unwrap_or(0.0)` 降级为零占比，不会中止资源计算。
- `pkg/importsdk/file_scanner.rs::estimateOneTableSize` 调用 `SampleFileImportKVSizeWithTableInfo`；若采样结果全为零则返回 0，若仅一边非正则回退到源总大小，否则按 `TotalKVSize / SourceSize` 外推。
- `pkg/executor/importer/sampler_test.rs` 直接覆盖配置验证、选文件契约、元数据采样与 parser 关闭语义。

下游直接依赖是 `astersql-lightning-mydump::{Parser, SourceFileMeta}`、`astersql-lightning-backend-encode::{Datum, EncodingConfig, SessionOptions}`、`astersql-lightning-backend-kv::{NewBaseKVEncoder, TableDefinition}`、`astersql-table::{Table, Column}`、`astersql-meta-model::TableInfo` 及本 crate 的映射、插入列、编码器和 batch 辅助符号。`pkg/executor/importer/Cargo.toml` 将这些都声明为 workspace 内 path 依赖，crate 本身无 sampler 专用 feature gate。

## 错误处理与边界

- `validateKVSizeSampleConfig` 仅接受 CSV、SQL、Parquet；当非空 `FIELDS ENCLOSED BY` 与 `FIELDS TERMINATED BY` 任一为另一个的前缀时拒绝配置。
- `SampleFileImportKVSizeWithTableInfo` 明确不支持 `ColumnsAndUserVars` 或 `ColumnAssignments`；完整 `KVSizeSampler` 路径则会构建对应映射和插入列。
- 元数据路径若源行字段多于可接收的可见列会报错；整数、无符号整数和浮点列的字节 datum 必须是 UTF-8 且可解析，否则传播字符串错误。
- EOF 是正常终止；跳过表头时提前 EOF 返回空采样。parser 读取、encoder 构造/编码和 batch 加入错误均向上传播。
- 多文件循环会继续采样后续文件并保留第一个错误；如果任一文件失败，最终返回该错误而非部分聚合结果。因此上游需自行决定是否降级。
- 空文件列表立即返回全零。`rows_per_file` 只在已选中至少一个文件后计算，不会除零。聚合字段使用饱和加法，但 `TotalKVSize` 特意保留 Go 的环绕加法。

## 并发与资源生命周期

采样本身是单线程、逐文件、逐行执行，没有在文件内启动任务、通道、锁或事务。`Arc<dyn Table>` 只用于共享表对象；每个文件都新建 parser 和 encoder，batch 仅活跃于当次 `sampleRows`。

`sampleOneFile` 在 parser 成功创建后对所有路径执行 `Parser::Close`，包括跳过行失败或 encoder 构造失败；encoder 成功创建后也会在采样成功/失败后调用 `Close`。`sampleTableInfoFile` 同样始终关闭 parser。两类 `Close` 错误都被忽略，不替换原采样结果，对齐 Go defer 中“仅记录关闭失败”的语义。该文件本身不记录警告，因为 Rust 服务边界未携带 logger。

## 与 Go 版本的对应关系

`pkg/executor/importer/sampler.go` 是直接语义对照：常量、结果字段、配置、校验、最多 3 文件/30 行、10 MiB 截断、源字节计算、checksum 分类、首错误和关闭错误不覆盖主结果均保持一致。`pkg/executor/importer/sampler_test.go` 还提供真实 CSV/SQL、多索引、keyspace codec、短文件与超长行的预期比例和资源关闭证据。

可见差异有：

- Go sampler 直接持有 storage/logger/context，并内部创建 parser/encoder；Rust 完整路径把两者抽象为 `KVSizeSamplerService`，便于由 host 注入真实 I/O 边界。
- Go 用 `rand.Perm`；Rust `sample_file_indices` 用时间种子 xorshift 执行 Fisher–Yates。两者保证随机不重复子集，但不保证相同随机序列。
- Go 返回类型化/带上下文的 TiDB 错误并记录 close warning；Rust 当前统一为 `String`，且静默忽略 close error。
- Go `kvSizeSampler` 保留列赋值表达式所需的 mutex；Rust 文件不在内部构建表达式，因而没有该锁。SDK 的 Rust `TableInfo` 路径更明确拒绝列/用户变量和赋值。
- Rust 额外提供 `SampleFileImportKVSizeWithTableInfo`，用于 `pkg/importsdk/file_scanner.rs`无完整导入控制器的路径；Go importsdk 直接复用原 `SampleFileImportKVSize`。

## 扩展指南

- 新增输入格式时，同步修改 `validateKVSizeSampleConfig`、host `NewParser` 实现、`sampledRowSourceSize` 的位置语义，以及 `pkg/executor/importer/sampler_test.rs`和 Go 对照测试；不应只把新字符串加入白名单。
- 改变抽样量、文件选择或大小上限时，重点复核 `sample_file_indices`、`KVSizeSampler::sample`、`SampleFileImportKVSizeWithTableInfo` 与 Go 的 `rand.Perm`/超长行契约；这会影响估算方差和 I/O 成本。
- 扩展列类型转换时修改 `parser_datum_to_encoder_datum_for_column`，必须用独立测试覆盖非 UTF-8、空白、溢出、无符号负值和小数边界，并核对正式导入 `CastColumnValue` 语义。
- 若允许 SDK 路径支持列列表或赋值，不能只移除入口拒绝；还需复制完整路径的字段映射、表达式上下文、同步和 encoder 契约。
- 改变错误策略时保持“继续采样后续文件、最终返回首错误”及“close 失败不覆盖主结果”的 Go 契约，并在 `sampler_test.rs` 中增加失败注入。
- Rust 单元测试应继续放在独立的 `pkg/executor/importer/sampler_test.rs`，由 `lib.rs` 的 `#[cfg(test)] mod sampler_test;` 挂载，不要嵌入本源文件。

## 验证依据

- RustCodeGraph 索引检查：`rustcodegraph status` 显示已索引 11,467 个文件；`files --filter pkg/executor/importer` 确认 `sampler.rs`、`sampler_test.rs`、Go 对照文件和测试均在图中。
- RustCodeGraph 符号/源码查询：`node --file pkg/executor/importer/sampler.rs --offset 1 --limit 500` 及 `--offset 489 --limit 100` 覆盖全文；`query SampleFileImportKVSize --kind function --json`、`query sampleIndexSizeRatio --kind function --json`、`query sampleRows --kind function --json` 确认入口、签名和定位。精确 `callers/callees` 在本地索引上两次超过 90 秒无输出后中止，调用边因此用下列直接源码与 `rg` 结果交叉验证。
- 已读 Rust 路径：`pkg/executor/importer/sampler.rs`、`lib.rs`、`Cargo.toml`、`sampler_test.rs`、`import.rs`、`production_resource.rs`，以及直接 SDK 调用者 `pkg/importsdk/file_scanner.rs`。目标目录没有 `doc.go`。
- 已读 Go 对照：`pkg/executor/importer/sampler.go`、`sampler_test.go`，并检查 `pkg/executor/importer/import.go::CalResourceParams` 与 `pkg/importsdk/file_scanner.go` 的直接调用和降级行为。
- Rust 独立测试确认：`TotalKVSize` 环绕加法；选中下标数量、唯一性和边界；格式/前缀验证；4 文件时仅关闭 3 个 parser、总计采样 300 源字节且生成两类 KV；parser close 错误不导致失败。
- 本任务仅生成文档，按计划不运行 Cargo；交付前使用任务指定的 `test -f` + `rg -c` 命令验证固定的 11 个二级标题。
