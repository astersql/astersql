# `lightning/pkg/importer/get_pre_info.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` crate；crate 根在 `lightning/pkg/importer/lib.rs` 中以 `mod get_pre_info` 装入它，再通过 `pub use get_pre_info::*` 暴露其公开符号。它位于 Lightning 正式导入之前的“预信息采集”边界：一侧读取 mydumper 发现的源数据库、表、schema 文件和数据文件，另一侧通过 SQL/PD 抽象探测目标集群，向预检规则提供表结构、样例行、容量估算和集群状态。

生产接线可从 `lightning/pkg/importer/precheck.rs:104-127` 看到：`NewPrecheckItemBuilder` 先创建 `TargetInfoGetterImpl`，再把 loader 取得的 `dbMetas`、`srcStorage` 交给 `NewPreImportInfoGetter`。其结果作为 `Arc<dyn PreImportInfoGetter>` 被 `lightning/pkg/importer/precheck_impl.rs` 的容量、版本、存储、schema 等 checker 消费。该文件不是导入写入器，也不修改源数据或目标表；它负责在导入前构造决策依据。

`lightning/pkg/importer/Cargo.toml` 将本 crate 标成对应 Go 包 `lightning/pkg/importer` 的 library port，并声明本文件直接使用的 `flate2`、`parquet`、`bytes`、mydump、importer-opts 等依赖。Cargo 注释还说明当前 Rust crate 以已移植组件和本地边界实现覆盖 Go importer，因此阅读时必须把“当前可运行的瘦实现”和 Go 完整能力分开。

## 核心职责

1. 用 `TargetInfoGetter` 抽象目标端信息：数据库/表模型、版本前置条件、表是否为空、导入所需系统变量、最大副本数、store 容量及空 region。
2. 用 `PreImportInfoGetter` 在目标端能力之上增加源端预览和估算：合并远端/源端表结构、按表名或文件读取前 N 行、估算源数据占用。
3. 提供默认目标端实现 `TargetInfoGetterImpl`：通过 `DB` 执行 `SHOW DATABASES`、`SHOW TABLES` 和空表查询，通过可选 `pdhttp::Client` 返回副本、store、region 信息。
4. 提供组合实现 `PreImportInfoGetterImpl`：持有配置、源元数据、存储、目标 getter，并缓存表结构与估算结果；同时把 `TargetInfoGetter` 方法透明委托给内部 `target`。
5. 把源 schema SQL 转换为 `model::TableInfo`，并为 CSV、SQL、Parquet 和 gzip 输入提供有限行预览。

当前 Rust 实现的估算和初始化明显小于 Go：`Init` 为空，`EstimateSourceDataSize` 只累计 `TotalSize` 并在 local backend 上乘约 `1/3`，不会执行 Go 的抽样、索引比率、行序检查或 TiFlash 计算。`ioWorkers`、`encBuilder` 目前只被保存，未参与本文件的运行路径；`parse_csv_records` 也没有调用者。它们是迁移边界，不应被描述成已经接线的能力。

## 主要符号

- `EstimateSourceDataSizeResult`：四个公开结果字段分别表示含索引估算、源文件原始总量、是否存在无序大表、TiFlash 占用。当前 Rust 只实际计算前两个，后两个固定为 `false` 和 `0`。
- `TargetInfoGetter: Send + Sync`：目标集群只读查询契约。`Send + Sync` 使 trait object 可放进 `Arc` 并由预检组件共享。
- `PreImportInfoGetter: TargetInfoGetter`：在目标查询契约上增加 `Init`、`GetAllTableStructures`、两个首行读取入口和 `EstimateSourceDataSize`。
- `StoresInfo` / `StoreInfo` / `StoreMeta` / `StoreStatus`：把 PD client 的扁平 store 元组转换为预检需要的容量、可用空间、region 计数结构。
- `RegionsInfo` / `RegionInfo`：承载空 region 总数及 region 到 store 的映射。
- `TargetInfoGetterImpl`：保存 `Config`、`DB`、可选 PD HTTP client，以及受 `Mutex` 保护的系统变量快照。
- `NewTargetInfoGetterImpl`：只接受空 backend、`tidb` 和 `local`；其他 backend 返回 `unknown backend`。空 backend 被构造器接受，但后续行为取决于各方法，不能等同于一个正式 backend。
- `PreImportInfoGetterImpl`：保存源元数据/存储、目标 getter、两个尚未消费的 worker/encoder 句柄，以及 `tableStructs`、`estimated` 两个缓存。
- `NewPreImportInfoGetter`：克隆配置并组装 trait object；当前忽略构造时 `_opts`，不创建默认 worker/encoder，也不主动调用 `Init`。
- `read_parquet_rows`：对已整体读入内存的 Parquet 字节建立 reader，提取顶层列名，最多转换 `n` 行到 Lightning `Datum`。
- `parse_csv_records`：支持双引号、双引号转义、逗号和 CRLF 的局部 CSV 分割器；当前文件内无调用者，属于未接线辅助函数。
- `newTableInfo`：调用本 crate 的 `Parser::Parse` 构造表模型，然后覆盖 ID 并把状态设为 `StatePublic`。
- `cfg_helpers::is_local_backend_cfg`：供 `tidb/import` 在不形成循环依赖的情况下判断 local backend。

## 执行流程

目标信息路径如下：

1. `precheck.rs` 用配置、目标 `DB` 和可选 PD client 调用 `NewTargetInfoGetterImpl`。
2. `FetchRemoteDBModels` 执行 `SHOW DATABASES`，将每行第一列变成只有名称的 `DBInfo`；`FetchRemoteTableModels` 对转义后的 schema 执行 `SHOW TABLES FROM ...`，生成 `StatePublic` 的基础 `TableInfo`。
3. `IsTableEmpty` 生成 ``SELECT 1 FROM <quoted table> USE INDEX() LIMIT 1``；查到行返回 `Some(false)`，`not_found`/`ErrNoRows` 返回 `Some(true)`，其他 SQL 错误上抛。
4. `GetTargetSysVariablesForImport` 调用 `ObtainImportantVariables`，再用 `cfg.TiDB.Vars` 覆盖同名项，把结果写入 `sysVars` 并返回克隆。
5. PD 相关方法在没有 client 时分别返回默认副本数 `3` 或空集合；client 含 `request_error` 时返回错误，否则复制 client 中的内存状态。

表结构路径如下：

1. `GetAllTableStructures` 用 `ApplyGetPreInfoOptions` 合并本次 options；未要求强制刷新且 `tableStructs` 非空时直接返回缓存克隆。
2. 对每个 `MDDatabaseMeta` 调用 `getTableStructuresByFileMeta`。该方法先一次获取目标 schema 下所有远端表，并按小写表名建 map。
3. 每个源表若在远端存在，优先复用远端 `TableInfo`；否则读取 `SchemaFile`，支持无压缩或 gzip，按 UTF-8 解码并交给 `newTableInfo`。没有 schema 文件时，无论 `IgnoreDBNotExist` 取值为何，当前 Rust 都生成一个只有 ID、名称和公开状态的基础模型。
4. 返回的表模型按源元数据下标重新与源表名配对，包装为 `importdef::TableInfo { Core, Desired }`，最后写入缓存。

首行预览路径如下：

1. `ReadFirstNRowsByTableName` 线性查找 schema/table；找不到分别返回 `cannot find the schema` 或 `cannot find the table`，无数据文件则返回两个空向量，只预览第一个数据文件。
2. `ReadFirstNRowsByFileMeta` 先检查 `Context::Err` 和 `n <= 0`。未压缩 Parquet 走流式 `chunk_process::parquet_source`/`ImportParser` 路径，并在每行前再次检查取消。
3. 其他输入先由 `Storage::Read` 整体读入；compression `1` 用 gzip 解压，其他非零值报不支持。CSV 由 `NewCSVParser` 解析，SQL 由 `NewChunkParser` 解析，Parquet 交给 `read_parquet_rows`。
4. 循环最多读取 `n` 行；EOF 正常结束，其他 parser 错误先尝试关闭再返回。普通路径结束时取列名并显式关闭 parser。数据映射中整数保留为 `Datum::Int`，文本/二进制保留字节或字符串，NULL 表示为字节 `\\N`。

估算路径中，`EstimateSourceDataSize` 先应用 options 和检查缓存，然后累加所有表的 `TotalSize`。local backend 的 `SizeWithIndex` 为总量除以 `3.0` 后截断为 `i64`，其他 backend 等于原始总量；结果写入缓存。`precheck_impl.rs` 的 `clusterResourceCheckItem::Check` 会把该值乘以副本数，与所有 store 的可用空间之和比较。

## 数据与状态

`PreImportInfoGetterImpl` 的稳定输入是配置快照 `cfg`、源端 `dbMetas`、源存储 `srcStorage` 和目标查询接口 `target`。`dbMetas` 决定结构合并顺序、表名查找、首个数据文件选择及 `TotalSize` 累计；`srcStorage` 是 schema 和数据预览的唯一读取边界。

两个缓存都由标准库 `Mutex` 保护：

- `tableStructs: Mutex<HashMap<...>>` 以“map 非空”表示已有缓存。因此合法的空结果不会成为可复用缓存，下次调用仍会重算。
- `estimated: Mutex<Option<...>>` 用 `Option` 区分未计算和已经计算，即使总量为零也可以缓存。

`ForceReloadCache(true)` 只跳过相应读取缓存的分支，计算成功后仍覆盖缓存。缓存返回和写入都使用 clone，调用者不能直接修改内部值。两段代码都只在读取/写入瞬间持锁，昂贵的存储、SQL 和解析工作在锁外执行；因此并发首次调用可能重复计算，后完成者覆盖先完成者，但不会持锁等待 I/O。

`TargetInfoGetterImpl::sysVars` 保存最近一次系统变量结果，但本文件没有读取该缓存的路径；该字段当前主要是状态快照。`ioWorkers` 和 `encBuilder` 也是保留状态，当前预览/估算实现没有使用它们。

## 依赖与调用关系

RustCodeGraph 将本文件识别为 936 行、73 个符号，并报告它被 `import.rs`、`precheck.rs`、`precheck_impl.rs` 以及若干独立测试使用。由于 trait object 动态分派没有产出精确 callers/callees 边，补充文本搜索确认了以下生产调用：

- `lightning/pkg/importer/precheck.rs`：构造 `TargetInfoGetterImpl` 和 `PreImportInfoGetterImpl`，是完整预检接线入口。
- `lightning/pkg/importer/precheck_impl.rs`：`clusterResourceCheckItem` 调 `EstimateSourceDataSize`、`GetStorageInfo`、`GetMaxReplica`；版本 checker 调 `CheckVersionRequirements`；schema checker 调 `GetAllTableStructures`；其他 checker 还复用 store 信息。
- `lightning/pkg/importer/import.rs`：通过 crate 再导出的预信息类型参与导入编排（RustCodeGraph 文件使用边）；具体预检实例仍由 `precheck.rs` 创建。

主要下游依赖为：`sql::DB`（目标 SQL）、`pdhttp::Client`（PD 状态）、`storeapi::Storage`（源文件）、`mydump`/`astersql_lightning_mydump`（元数据与 CSV/SQL parser）、`astersql_dumpformat_parquetfile` 与 `parquet`（Parquet 两条读取路径）、`flate2`（gzip）、`parser::Parser`（CREATE TABLE 建模）、`importer::opts`（缓存/容错选项）。`Context` 的取消只在预览入口和流式 Parquet 行循环中显式检查；多数同步 SQL/存储辅助接口并未消费传入 context。

## 错误处理与边界

- 构造目标 getter 时未知 backend 立即失败；local backend 的 `CheckVersionRequirements` 要求存在 PD client，但当前只验证 client 是否存在，没有执行 Go 的完整组件版本比较。
- SQL 标识符通过 `EscapeIdentifier`/`UniqueTable` 生成，避免直接拼接未转义的 schema/table。空表判断只把 `not_found` 或 `ErrNoRows` 归为空，其他错误保留。
- PD client 缺失不是 store/region 查询错误，而是空信息；`GetMaxReplica` 缺失 client 时采用 `3`。调用方必须理解这是 fallback，而非真实集群探测结果。
- schema 文件只支持无压缩和 compression 值 `1` 的 gzip；无效 UTF-8、读文件、解压或解析失败都附带阶段性错误文本。`IgnoreDBNotExist` 在当前 `getTableStructuresByFileMeta` 中没有改变远端查询失败的处理，也没有改变缺 schema 文件分支，和 Go 语义存在差距。
- 数据预览对 `n <= 0` 直接返回空列、空行；未知文件类型和未知压缩返回普通错误，而 Go 的未知文件类型路径会 panic。普通 CSV/SQL/内存 Parquet 路径在整体 `Storage::Read` 前只检查一次取消，读取循环不再检查；流式 Parquet 每行检查。
- Parquet 映射把无符号 `ULong` 转为十进制字节串，以避免溢出 `i64`；未专门匹配的 Parquet field 用 `to_string()` 转成 `Datum::String`。NULL 统一用 `\\N` 字节约定表达。
- `Mutex::lock().unwrap()` 意味着一旦某次持锁线程 panic 导致 poisoning，后续调用也会 panic；当前 API 没有把 poisoned lock 转成 `Result`。
- `parse_csv_records` 会拒绝未闭合引号，但因当前无调用者，它不影响实际 CSV 预览行为。

## 并发与资源生命周期

两个实现都通过 `Arc` 共享：`NewTargetInfoGetterImpl` 返回具体类型的 `Arc`，`NewPreImportInfoGetter` 返回 `Arc<dyn PreImportInfoGetter>`；trait 的 `Send + Sync` 约束允许多个 checker 保存同一实例。内部可变状态只存在于三个 `Mutex` 字段，源元数据和配置在构造时按值克隆/移动后只读。

缓存采用短临界区，不把 SQL、文件读取、解压或 parser 执行包在锁里。这避免长时间阻塞，但不提供 single-flight：并发 miss 或强制刷新可以重复访问外部资源。若未来缓存计算有副作用或成本显著，应明确选择双重检查、一次性单元还是允许重复计算。

parser 生命周期分两类。流式 Parquet 的局部 parser 随函数退出析构；普通 CSV/SQL parser 在成功路径显式 `Close`，解析错误路径也尽力 `Close` 后再返回原解析错误。内存 Parquet reader 由局部所有权自动释放。gzip decoder 和完整文件字节均驻留当前调用内存，意味着大压缩文件可能产生明显峰值内存。

`ioWorkers`、`encBuilder` 虽以 `Arc`/trait object 保存，但当前没有消费点，因此构造传入的资源仅由 getter 延长生命周期。`Context` 是按值 clone 的轻量取消状态；没有后台任务、通道、事务或显式 join 行为。

## 与 Go 版本的对应关系

直接对照文件是 `lightning/pkg/importer/get_pre_info.go`，独立 Go 测试是 `lightning/pkg/importer/get_pre_info_test.go`。公开 trait/结构名、表结构合并、首行预览、估算结果字段和 `newTableInfo` 均沿用 Go 命名与总体意图，但当前 Rust 并非逐行为完整移植：

- Go `NewTargetInfoGetterImpl` 根据 backend 创建 TiDB/local 专用 backend getter；Rust 直接在本类型中执行基础 SQL/PD 内存 client 查询。
- Go 通过 context 注入 dbMetas 并执行真实版本检查；Rust local 分支只要求 PD client 存在，TiDB/其他已接受分支直接成功。
- Go `NewPreImportInfoGetter` 会补建默认 worker/encoder、保存默认 options、调用 `Init` 建立按库/表索引；Rust保存可选句柄、忽略构造 options，`Init` 为空，并在线性扫描中查表。
- Go `GetAllTableStructures` 使用 `LoadSchemaInfo`，向远端只请求相关表，并在 `IgnoreDBNotExist` 时仅容忍特定坏库错误；Rust请求 schema 下全部表，任意远端错误都返回，且缺 schema 时生成基础模型。
- Go 预览经 `NewReaderOpener` 支持更完整的压缩/字符集/SQL mode/worker 配置并 defer 关闭；Rust当前仅明确支持无压缩和 gzip，使用精简 parser 配置。Rust额外存在一条未压缩 Parquet 流式路径，并有大 page/多 row-group 测试。
- Go 估算会取得表结构和系统变量，对大表抽样，计算索引比率、行有序性、无序大表和 TiFlash 副本占用，最后对 local 结果应用压缩比；Rust不抽样，`SizeWithIndex` 对 local 简化为源总量的三分之一，其他 backend 等于源总量。
- Go `newTableInfo` 验证 AST 确实是 `CREATE TABLE` 并通过 DDL builder 建模；Rust委托本地 `Parser::Parse`，其精确接受范围由该 parser 决定。

这些差异是当前代码事实。后续对齐 Go 时应逐项移植并扩展独立 Rust 测试，不能仅通过调整测试期望掩盖缺失逻辑。

## 扩展指南

- 扩展目标端探测：优先在 `TargetInfoGetter` 增加/调整契约，并同步 `TargetInfoGetterImpl`、`PreImportInfoGetterImpl` 的委托实现、测试 fake 及 `get_pre_info_test.rs`。要评估缺 PD client 的 fallback 是否仍安全。
- 补齐结构合并：修改 `getTableStructuresByFileMeta` 时保持源表顺序、大小写查找和远端优先不变量；重点补测远端坏库 + `IgnoreDBNotExist`、缺 schema、压缩 schema、解析错误。测试必须继续放在独立的 `get_pre_info_test.rs`，不要内嵌进生产文件。
- 扩展预览格式/压缩：在 `ReadFirstNRowsByFileMeta` 接入，并确保所有成功/错误/取消路径释放 reader/parser；同时测试列名、NULL、EOF、行数上限、损坏输入和大文件内存行为。
- 补齐 Go 估算：最可能修改 `EstimateSourceDataSize`、构造器/`Init` 以及 sampling 辅助逻辑；需要同时接入 `ioWorkers`、`encBuilder`、表结构和系统变量，并验证索引比率、无序大表、TiFlash、缓存和 local 压缩。性能风险主要是对每张大表采样及重复并发刷新。
- 调整缓存：必须明确空 map 是否算有效缓存、并发 miss 是否允许重复 I/O、失败是否缓存。若更换锁策略，要保留 trait 的 `Send + Sync` 与 clone 隔离语义。
- 对公开结果或字段做兼容变更时，同时检查 `precheck_impl.rs` 的容量计算和 Cargo crate 的再导出使用者；Go 命名风格虽不符合 Rust 惯例，却是移植 API 的兼容面，不宜单独重命名。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter lightning/pkg/importer/get_pre_info.rs` 命中目标；`query get_pre_info` 定位 Rust/Go 文件、trait、构造器和独立测试；`node --file ... --offset/--limit` 覆盖目标 1-936 行；文件使用边列出 `import.rs`、`precheck.rs`、`precheck_impl.rs` 和测试。精确 trait callers/callees 查询无输出，因此动态分派调用用 `rg` 补证。
- 已读实现与装配：`lightning/pkg/importer/get_pre_info.rs`、`lightning/pkg/importer/lib.rs`、`lightning/pkg/importer/Cargo.toml`、`lightning/pkg/importer/opts/get_pre_info_opts.rs`、`lightning/pkg/importer/precheck.rs`、`lightning/pkg/importer/precheck_impl.rs`。
- Go 对照：`lightning/pkg/importer/get_pre_info.go`、`lightning/pkg/importer/get_pre_info_test.go`；重点核对构造/初始化、结构合并、文件预览、采样估算和目标端查询。
- Rust 独立测试：`lightning/pkg/importer/get_pre_info_test.rs` 覆盖 `newTableInfo`、auto-random/default、结构缓存与强制刷新、从存储加载 schema、CSV/SQL/Parquet/gzip 首行、估算缓存、空表 SQL 形状、backend 校验、PD store/region/副本，以及大 Parquet page/多 row group/NULL/行上限。
- 调用搜索：`rg` 确认生产调用集中在 `precheck.rs` 和 `precheck_impl.rs`，并确认 `parse_csv_records`、`ioWorkers`、`encBuilder` 当前无消费点。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核本文没有把 Go 的完整实现写成 Rust 当前能力。
