# `lightning/pkg/importer/table_import.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` library crate。crate 根 `lightning/pkg/importer/lib.rs` 通过 `#[path = "table_import.rs"] mod table_import; pub use table_import::*;` 将其公开；`lightning/pkg/importer/Cargo.toml` 的 `package.metadata.porting.go-package = "lightning/pkg/importer"` 表明它直接移植自同目录 Go 包。任务级上游是 `lightning/pkg/importer/import.rs::Controller::importTables`：它为每张 dump 表构造 `TableImporter`，创建默认 `TableCheckpoint`，调用 `TableImporter::importTable`，再广播最终 checkpoint。

它是单表导入边界，保存表结构、dump 元数据、编码表、auto-ID/storage/meta-manager 句柄，并提供 checkpoint 状态推进、列置换、校验和、DDL/统计辅助及重复检测入口。不过当前 Rust 文件明确是 slim port：`populateChunks` 不读取真实文件区域，`preprocessEngine`/`importEngine` 只推进状态，`importKV` 直接成功，`getDDLStatus` 固定返回 `synced`。因此不能把 `lightning/pkg/importer/table_import.go` 的完整引擎、TiKV、DDL 轮询和后处理能力视为 Rust 已实现能力。

## 核心职责

1. 以 `NewTableImporter` 构造一张表的导入上下文，并拒绝完全缺失表名的 `TableInfo`。
2. 以 `importTable` 串联 `populateChunks → importEngines → postProcess`，在当前瘦实现中建立 data/index engine checkpoint 并推进到 `CheckpointStatusAnalyzed`。
3. 维护列映射：`initializeColumns`、`createColumnPermutation`、`parseColumnPermutations` 把输入列顺序映射到目标表列顺序，以 `-1` 表示忽略列、生成列、缺失列或未提供的 `_tidb_rowid`。
4. 提供恢复辅助：`RebaseChunkRowIDs` 平移所有 chunk 的 row-ID 边界；`estimateCompactionThreshold` 按唯一源文件大小、Parquet 解压估算和索引因子计算阈值。
5. 提供后处理辅助：比较本地/远端 checksum，发出 `ANALYZE TABLE`，更新 `mysql.stats_meta`，以及生成 drop/add index DDL。
6. 为重复检测接线：`preDeduplicate` 打开磁盘 sorter，构造 `dupDetector` 并把 `TableImporter`、`Controller` 与 checkpoint 交给其并发扫描逻辑。

## 主要符号

- `INDEX_ENGINE_ID: i32 = -1`：本文件内部的索引 engine checkpoint 键；非负 ID 被视为 data engine。
- `TableImporter`：单表状态聚合体。`dbInfo`/`tableInfo`/`tableMeta` 是结构和 dump 输入；`tableName` 是经 `common::UniqueTable` 生成的限定名；`encTable`、`alloc`、`store` 是编码、auto-ID 和 KV 边界；`metaMgr` 是可选表元数据管理器；`closed` 仅记录显式关闭状态。
- `NewTableImporter(&DBInfo, &TableInfo, Option<MDTableMeta>, Logger) -> Result<TableImporter>`：构造公开入口。当前只校验表名并填入默认编码表、allocator、storage 和空 meta manager，没有复刻 Go 构造器的 allocator base、etcd、ignore-columns、checkpoint 与真实 table metadata 初始化。
- `TableImporter::{importTable, populateChunks, importEngines, preprocessEngine, importEngine, postProcess, importKV}`：当前单表主链及阶段入口。
- `TableImporter::{Store, AutoIDClient}` 与 `impl autoid::Requirement`：向 auto-ID 公共逻辑暴露 storage/client。
- `TableImporter::{initializeColumns, RebaseChunkRowIDs, compareChecksum, analyzeTable, dropIndexes, addIndexes, executeDDL, preDeduplicate}`：列、row-ID、校验和、SQL、索引和去重辅助入口；`clone_light` 为去重 detector 复制所需句柄。
- `createColumnPermutation` / `parseColumnPermutations`：列映射核心。后者验证未知列、记录被忽略或缺失列，并始终在结果尾部追加 `_tidb_rowid` 槽位。
- `estimateCompactionThreshold`：按 checkpoint 中 chunk 顺序去重相邻同路径文件，优先采用 `FileInfo.RealSize`，Parquet 大小乘二，再调用 `ingestctrl::EstimateCompactionThreshold2`。
- `updateStatsMeta`：开启事务更新 `mysql.stats_meta`；只有 SQL 成功、取得 affected rows 且大于零时提交，否则回滚。函数不向调用者返回错误。
- `isDeterminedError`：仅按 `crate::Error.class` 识别四类确定性 DDL 错误：`ErrDupKeyName`、`ErrMultiplePriKey`、`ErrDupUnique`、`ErrDupEntry`。
- `ddlStatus`、`getDDLStatus`、`getDDLJobIDByQuery`、`ddlStateSynced`、`ddlStateCancelled`：DDL 状态兼容边界。只有 job-query 精确匹配查询是真实实现；状态查询本身仍是固定值占位。

文件没有自定义 trait、泛型类型或条件编译项；唯一 trait 实现是 `autoid::Requirement for TableImporter`。

## 执行流程

生产主链从 `Controller::importTables` 进入：取得/补齐 `DBInfo` 与 `TableInfo`，调用 `NewTableImporter`，随后以新的空 checkpoint 调用 `importTable`。

`importTable` 当前严格顺序执行：

1. `populateChunks` 只在 `cp.Engines` 为空时插入 ID `-1` 的索引 engine 和 ID `0` 的 data engine，两者均为 `CheckpointStatusLoaded`、空 chunk；已有 engine 完全保留。
2. `importEngines` 先要求索引 engine 存在，否则返回含表名的错误。它收集、排序全部非负 engine ID；已达到 `Imported` 的 engine 跳过，其他 engine 先由 `preprocessEngine` 至少推进到 `AllWritten`，再由 `importEngine` 置为 `Imported`。最后把索引 engine 及表 checkpoint 分别推进到 `Imported` 和 `IndexImported`。
3. `postProcess` 若存在 `metaMgr`，先写入 `metaStatusLocalChecksumUpdated`；配置未关闭 checksum 时，根据 `cp.Checksum` 构造本地 checksum，但当前以默认的远端 checksum 调用 `compareChecksum` 并显式丢弃结果。之后可选调用 `FinishTable`，无条件将表状态置为 `Analyzed`。

独立辅助流程包括：`preDeduplicate` 从 `cfg.TikvImporter.SortedKVDir` 打开 sorter、按 `RegionConcurrency` 配置并调用 `dupDetector::run`；`addIndexes` 为每个非主键索引拼接列名与 `UNIQUE` 修饰后调用 `executeDDL`；`dropIndexes` 同样遍历非主键索引，但当前丢弃每次 `DB::Exec` 的结果。

## 数据与状态

- checkpoint 是主状态机。`populateChunks`、`preprocessEngine`、`importEngine`、`importEngines` 和 `postProcess` 依次可能写入 `Loaded`、`AllWritten`、`Imported`、`IndexImported`、`Analyzed`。阶段判断使用状态值的顺序比较，支持跳过已完成 engine；但当前没有 checkpoint 持久化调用。
- `RebaseChunkRowIDs` 对每个 engine 的每个 chunk 同时增加 `PrevRowIDMax` 与 `RowIDMax`。与 Go 版不同，Rust 没有对 `rowIDBase == 0` 的提前返回，但结果等价。
- 列置换结果按目标表列顺序排列；显式 header 时最后总有 `_tidb_rowid` 槽。未知且未忽略的源列立即失败；源中缺失的普通列只记录 warning 并填 `-1`，生成列不产生该 warning。
- `estimateCompactionThreshold` 的去重仅依赖遍历过程中 `lastFile`，所以只消除相邻重复路径；它没有全局 `HashSet` 去重。文件 map 同路径后写覆盖先写，checkpoint 没有对应 `FileInfo` 时回退 `ChunkCheckpoint.FileMeta.FileSize`。
- `closed` 只是布尔标记；`Close` 不关闭 `encTable`、sorter 或 storage。`clone_light` 复制所有字段，并通过 `Arc` 克隆 `metaMgr`。
- `ddlStatus` 只有 `jobID` 和字符串 `state`；它没有 Go 版的 `rowCount`，也没有创建时间过滤。

## 依赖与调用关系

上游调用者：

- `lightning/pkg/importer/import.rs::Controller::importTables` 是当前 Rust 生产入口，调用 `NewTableImporter` 与 `TableImporter::importTable`。
- `lightning/pkg/importer/chunk_process.rs::chunkProcessor` 持有 `Arc<TableImporter>`，为 chunk 编码/投递保留单表上下文；当前 Rust 文件中未发现其调用 `initializeColumns`，这与 Go `encodeLoop` 的接线不同。
- `lightning/pkg/importer/dup_detect.rs::dupDetector` 持有 `Arc<TableImporter>`，并在逐 chunk 重建列信息时调用 `createColumnPermutation`。
- `lightning/pkg/importer/meta_manager.rs` 的 builder trait 接受 `Arc<TableImporter>` 以创建表级 meta manager；当前 `NewTableImporter` 默认把 `metaMgr` 留空，上层生产构造路径也未在本文件字段中注入它。

主要下游：

- 独立 crate `astersql-lightning-pkg-checkpoints` 定义 table/engine/chunk checkpoint 和状态值。
- crate 内 `common`/`common_ext`/`model` 提供限定表名、identifier escaping、auto-row-ID 判断、未知列错误和表结构；`verify`/`ingestctrl` 提供 checksum 与压缩阈值。
- `extsort` 与 `dup_detect` 承担真实重复键扫描；`sql::DB` 是 ANALYZE、DDL、统计更新和 job 查询边界；`meta_manager::tableMetaMgr` 承担表状态更新与结束通知。
- Cargo 清单还声明 checkpoints、errormanager、precheck、progress、mydump 等 workspace path 依赖，以及带 tag 的 `astersql/arrow-rs` Parquet Git 依赖；本文件通过 crate 内兼容模块使用其中一部分。

RustCodeGraph 对 Go/Rust 同名符号存在交叉语言误配：例如 Rust `importTable` 的 callees 同时返回 Go 方法，部分通用方法名还指向无关 crate。因此调用边以 `--file lightning/pkg/importer/table_import.rs` 查询后，再用模块引用与已索引源码交叉确认；不能把图中所有同名结果都当作 Rust 边。

## 错误处理与边界

- `NewTableImporter` 仅在 `tableInfo.Core.Name.L` 与 `tableInfo.Name` 同时为空时报错。二者只有一个为空时仍可构造，而 `tableName` 使用外层 `tableInfo.Name`。
- 主链使用 `?` 在 `populateChunks`、engine 状态推进或 meta manager 调用失败时立即停止；但 checksum 比较结果在 `postProcess` 被丢弃，默认远端 checksum 不匹配也不会阻止状态变为 `Analyzed`。
- `importEngines` 先显式验证索引 engine，随后对该键使用 `expect`；只要函数中间没有删除该键就不会 panic。data engine 缺失由 `preprocessEngine`/`importEngine` 返回普通错误。
- `compareChecksum` 同时比较 sum、KV 数和字节数，任何一项不符都失败；错误文本只打印 remote/local sum，未展示另外两项差异。
- `dropIndexes` 忽略所有执行错误；`addIndexes`/`executeDDL` 则传播错误。两者均没有 Go 版重试、特定 MySQL 错误降级、逐索引回退与进度查询。
- `updateStatsMeta` 将 Begin、Exec、RowsAffected、Commit/Rollback 的所有错误吞掉；调用者只能通过 DB 侧日志/状态观测。affected rows 为零时回滚。
- `getDDLJobIDByQuery` 要求每行至少两列、job ID 可解析，并采用 SQL 文本完全相等；部分匹配或找不到返回 `0`， malformed row/ID 返回错误。
- `preDeduplicate` 打开了一个名为 `ignore` 的 sorter 交给 detector；打开失败经 `errors::Trace` 传播。文件本身没有显式关闭该 sorter，生命周期取决于 `dupDetector::run` 及 sorter 的 `Arc`/实现。

## 并发与资源生命周期

本文件主导入链本身同步串行：data engine ID 排序后逐个处理，`importKV` 不启动任务，DDL 执行也同步调用内存/兼容 DB。`TableImporter` 没有 `Drop`；调用方必须显式调用 `Close`，而当前 `Controller::importTables` 没有调用它，且 `Close` 只置位，不释放外部资源。

共享所有权主要出现在 `metaMgr: Option<Arc<dyn tableMetaMgr>>` 和 `preDeduplicate`。后者接收 `Arc<Controller>`，用 `clone_light` 创建 `Arc<TableImporter>`，再由 `dupDetector::run` 使用 `RegionConcurrency`；真正的 scoped worker、adder flush/close 和 sorter cleanup 位于 `dup_detect.rs`，不在本文件。`Context` 在阶段间和 meta manager 调用间克隆，但本文件当前不检查取消状态；Go `importTable`、engine 处理和 DDL 轮询的 cancel/goroutine 生命周期尚未移植。

`updateStatsMeta` 的事务生命周期是 Begin 后恰好选择 Commit 或 Rollback；提交/回滚错误均不外传。`preDeduplicate` 创建的 sorter 通过 `Arc` 传递，新增提前返回分支时必须确认 sorter 清理仍由下游保证。

## 与 Go 版本的对应关系

直接对照文件是 `lightning/pkg/importer/table_import.go`。Rust 保留了同名类型、函数和大致阶段次序，列置换、row-ID 平移、checksum 三元组比较、Parquet 大小乘二、`stats_meta` SQL、确定性重复键分类及 DDL query 精确匹配等局部语义与 Go 基本一致。

重要差异如下：

- Go `NewTableImporter` 接收 tableName、checkpoint、ignore columns、KV store、etcd client，构造真实 allocator/encoding table；Rust 参数和初始化明显更少，`initializeColumns` 也固定使用空 ignore set。
- Go `importTable` 处理取消、checkpoint 复用、文件分区、row-ID 分配、meta manager、真实 engine 导入、重复检测和延迟 post-process；Rust 只执行三个简化阶段。
- Go `populateChunks` 调用 `mydump.MakeTableRegions` 并生成真实 chunk；Rust只创建两个空 engine。Go engine 流程管理 worker、opened/closed engine、chunk goroutine、flush、checkpoint 持久化和 TiKV import；Rust只改变状态。
- Go `postProcess` 处理 allocator rebase、checksum、重复键、统计和 analyze 的多级状态与配置；Rust可调用两次 meta manager 并最终直接标记 `Analyzed`，且忽略 checksum mismatch。
- Go `Close` 释放 encoding table 和 duplicate sorter；Rust只设置 `closed`。Go `importKV` 导入、保存 checkpoint、成功后 cleanup；Rust为空成功。
- Go drop/add index 会筛选、重建 encoding table、重试、逐索引回退并轮询 DDL；Rust是直接 SQL 循环。Rust `getDDLStatus` 是固定 `synced` 占位。
- Go `preDeduplicate` 对重复键错误生成更友好消息并接收 working directory/既有 sorter；Rust直接使用配置目录新建 sorter并返回 detector 结果。

独立 Rust 测试明确使用 “slim” 描述这些限制，故这些差异是当前代码事实，而非文档推测。

## 扩展指南

- 补齐单表主链时，以 `importTable` 为总序，分阶段扩展 `populateChunks`、`importEngines`、`preprocessEngine`、`importEngine`、`importKV`、`postProcess`；每项必须逐分支核对同名 Go 方法，避免一次递归补建整个 backend 子系统。
- 引入真实 checkpoint/engine 操作时，保留“已完成状态可跳过”和“索引 engine 与表状态非原子持久化”的恢复语义；同步修改独立测试 `lightning/pkg/importer/table_import_test.rs`，不要把测试写进生产文件。
- 修改列映射时，同时检查 `initializeColumns`、`createColumnPermutation`、`parseColumnPermutations` 和 `dup_detect.rs`；覆盖空 header、显式 `_tidb_rowid`、生成列、ignore、缺失列、未知列及大小写已规范化前提。
- 修改压缩估算时，先决定是否继续保留 Go 的“仅相邻路径去重”行为；全局去重会改变阈值。同步更新 `test_estimate` 与 `parity_test.rs`。
- 恢复 checksum/post-process 时，不能继续丢弃 `compareChecksum` 错误；需要同时定义 meta status、checkpoint status、skip/required 配置和 analyze 失败策略，并覆盖重启恢复路径。
- 恢复 DDL 流程时，围绕 `executeDDL`、`isDeterminedError`、`getDDLStatus`、`getDDLJobIDByQuery` 补齐取消、轮询间隔、create-time 过滤、subjob row count、确定/不确定错误及逐索引降级；兼容风险是误判已完成或重复执行 DDL。
- 新增持有资源的字段时，同时更新构造器、`clone_light`、`Close` 和所有测试夹具；若资源需可靠释放，应优先使用 RAII/`Drop`，不要只扩展 `closed` 标记。
- 性能敏感点是 engine/chunk 并发、sorter 磁盘空间、Parquet 估算、DDL 长轮询和 checkpoint flush；补实现时应保持 Go 的并发上限与取消/清理顺序。

## 验证依据

- RustCodeGraph 索引可用：`status` 报告 11467 个文件、307296 个节点、1848419 条边；`files --filter lightning/pkg/importer/table_import.rs` 确认目标文件 658 行、57 个符号，并列出 `import.rs`、`chunk_process.rs`、测试等使用者。
- 通过 RustCodeGraph `node --file` 完整读取 `lightning/pkg/importer/table_import.rs` 第 1–658 行，并读取 `import.rs::Controller::importTables`、`chunk_process.rs` 与 `dup_detect.rs` 的直接接线段落。
- 对 `TableImporter`、`NewTableImporter`、`importTable`、`createColumnPermutation`、`estimateCompactionThreshold`、`updateStatsMeta`、`parseColumnPermutations`、`isDeterminedError`、`getDDLJobIDByQuery` 执行 `query`；对主要入口执行带目标文件限定的 `callers/callees`。因同名跨语言边有噪声，又以 `rg` 核对 `import.rs`、`chunk_process.rs`、`dup_detect.rs`、`meta_manager.rs` 的真实引用。
- 读取 `lightning/pkg/importer/Cargo.toml` 与 `lib.rs`，确认 crate、Go package 映射、模块再导出、独立测试模块和依赖边界；目标包不存在 `doc.go`。
- Go 对照证据来自 `lightning/pkg/importer/table_import.go` 的 `TableImporter`、构造器、主链、engine、post-process、列映射、统计、KV 导入、checksum、索引 DDL、DDL 状态和预去重实现。
- Rust 独立测试证据来自 `lightning/pkg/importer/table_import_test.rs`：覆盖 engine 状态、缺失索引 engine、列映射/生成列/未知列、checksum 三元组、ANALYZE、stats SQL、空 `importKV`、完整 slim 主链、row-ID、阈值、DDL query 与错误类别；`parity_test.rs` 和 `chunk_process_test.rs` 另覆盖构造失败、未知列及 ignore-columns 接线。Go 测试 `table_import_test.go` 用于核对原测试意图。
- 本任务只新增文档，按计划未运行 Cargo。交付时运行任务指定的 shell 结构检查，确认文件存在且恰含 11 个固定二级标题，并人工复核所有能力描述都区分 Rust 当前实现与 Go 对照。
